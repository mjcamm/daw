//! The DAW's side of a host process: starting it, calls over the control
//! channel, and noticing when it is gone (its stdout closes).

#![allow(
    clippy::disallowed_types,
    reason = "control side only: the reader thread hands replies to the control thread"
)]

use crate::block::{DawBlock, Layout, Names};
use crate::control::{
    FromHost, PROTOCOL, PluginData, PluginSpec, Reply, Request, ToHost, read_message, write_message,
};
use crate::sys::Job;
use std::fmt;
use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// How to start a host process: the app runs itself in plugin-host mode;
/// tests run the `daw-plugin-host` binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// How long a control call may take before the host counts as hung.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum HostError {
    Spawn(io::Error),
    /// The host is gone (crashed, killed or exited).
    Gone,
    /// No reply in time.
    TimedOut,
    /// The host spoke a different protocol.
    Protocol(String),
    /// The host refused the call.
    Refused(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start the plugin host: {e}"),
            Self::Gone => f.write_str("the plugin host is gone"),
            Self::TimedOut => f.write_str("the plugin host did not answer in time"),
            Self::Protocol(m) => write!(f, "the plugin host speaks another protocol: {m}"),
            Self::Refused(m) => write!(f, "the plugin host refused: {m}"),
        }
    }
}

impl std::error::Error for HostError {}

/// A fresh instance id, unique in this DAW process (it names shared memory).
pub fn next_instance() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A running host process.
#[derive(Debug)]
pub struct HostProcess {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    replies: Receiver<FromHost>,
    alive: Arc<AtomicBool>,
    next_call: u64,
    pid: u32,
    // Ends the process with the DAW, even if the DAW crashes.
    _job: Job,
}

impl HostProcess {
    /// Starts a host and waits until it is ready.
    pub fn spawn(command: &HostCommand) -> Result<Self, HostError> {
        let job = Job::new().map_err(HostError::Spawn)?;
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(HostError::Spawn)?;
        if let Err(e) = job.assign(&child) {
            let _ = child.kill();
            return Err(HostError::Spawn(e));
        }
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(HostError::Gone);
        };
        let alive = Arc::new(AtomicBool::new(true));
        let (sender, replies) = channel();
        let flag = alive.clone();
        std::thread::Builder::new()
            .name("plugin host reader".into())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                while let Ok(Some(message)) = read_message::<FromHost>(&mut stdout) {
                    if sender.send(message).is_err() {
                        break;
                    }
                }
                flag.store(false, Ordering::Release);
            })
            .map_err(HostError::Spawn)?;

        let mut host = Self {
            child,
            stdin: BufWriter::new(stdin),
            replies,
            alive,
            next_call: 0,
            pid: 0,
            _job: job,
        };
        host.send(&ToHost::Hello { protocol: PROTOCOL })?;
        match host.replies.recv_timeout(CALL_TIMEOUT) {
            Ok(FromHost::Ready { protocol, pid }) if protocol == PROTOCOL => host.pid = pid,
            Ok(other) => return Err(HostError::Protocol(format!("{other:?}"))),
            Err(RecvTimeoutError::Timeout) => return Err(HostError::TimedOut),
            Err(RecvTimeoutError::Disconnected) => return Err(HostError::Gone),
        }
        Ok(host)
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// False once the process is gone. Shared with each instance's
    /// [`DawBlock`], so the audio thread stops waiting on a dead host.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Starts an instance, from saved state if there is some.
    pub fn create(
        &mut self,
        instance: u64,
        device: PluginSpec,
        state: Option<PluginData>,
    ) -> Result<(), HostError> {
        self.call(Request::Create {
            instance,
            device,
            state,
        })
        .map(drop)
    }

    /// The instance's state, to save with the song; `None` for a device
    /// without any.
    pub fn state(&mut self, instance: u64) -> Result<Option<PluginData>, HostError> {
        match self.call(Request::GetState { instance })? {
            Reply::State(state) => Ok(state),
            Reply::Done => Ok(None),
        }
    }

    /// Makes the instance's audio channel and starts its audio thread in
    /// the host. The block moves to the DAW's audio thread.
    pub fn activate(
        &mut self,
        instance: u64,
        layout: Layout,
        sample_rate: f64,
    ) -> Result<DawBlock, HostError> {
        let names = Names::unique();
        let block = DawBlock::create(instance, &names, layout, self.alive.clone())
            .map_err(HostError::Spawn)?;
        self.call(Request::Activate {
            instance,
            names,
            layout,
            sample_rate,
        })?;
        Ok(block)
    }

    pub fn deactivate(&mut self, instance: u64) -> Result<(), HostError> {
        self.call(Request::Deactivate { instance }).map(drop)
    }

    pub fn destroy(&mut self, instance: u64) -> Result<(), HostError> {
        self.call(Request::Destroy { instance }).map(drop)
    }

    /// Ends the process at once (a hung host, or Reload).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.alive.store(false, Ordering::Release);
    }

    fn send(&mut self, message: &ToHost) -> Result<(), HostError> {
        if !self.is_alive() {
            return Err(HostError::Gone);
        }
        write_message(&mut self.stdin, message).map_err(|_| HostError::Gone)
    }

    fn call(&mut self, request: Request) -> Result<Reply, HostError> {
        self.next_call += 1;
        let id = self.next_call;
        self.send(&ToHost::Call { id, request })?;
        let until = Instant::now() + CALL_TIMEOUT;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.replies.recv_timeout(left) {
                Ok(FromHost::Reply { id: got, result }) if got == id => {
                    return result.map_err(HostError::Refused);
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err(HostError::TimedOut),
                Err(RecvTimeoutError::Disconnected) => return Err(HostError::Gone),
            }
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if self.is_alive() && self.send(&ToHost::Shutdown).is_ok() {
            // The reader ends when the host closes its stdout on exit.
            while !matches!(
                self.replies.recv_timeout(Duration::from_secs(1)),
                Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout)
            ) {}
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
