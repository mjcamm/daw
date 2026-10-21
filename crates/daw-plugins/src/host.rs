//! The plugin host process: control messages on stdin, replies on stdout,
//! one realtime audio thread per active instance. Run by `daw-plugin-host`
//! and by the app itself in plugin-host mode.

use crate::block::{HostBlock, HostEnd, Served};
use crate::control::{
    FromHost, PROTOCOL, PluginSpec, Request, TestPlugin, ToHost, read_message, write_message,
};
use crate::sys::Event;
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Serves the DAW until it says stop or goes away.
pub fn run() -> ExitCode {
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    match serve(&mut input, &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("plugin host: {e}");
            ExitCode::FAILURE
        }
    }
}

fn serve(input: &mut impl io::Read, output: &mut impl Write) -> io::Result<()> {
    match read_message::<ToHost>(input)? {
        Some(ToHost::Hello { protocol }) if protocol == PROTOCOL => {}
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected hello with protocol {PROTOCOL}, got {other:?}"),
            ));
        }
    }
    write_message(
        output,
        &FromHost::Ready {
            protocol: PROTOCOL,
            pid: std::process::id(),
        },
    )?;
    let mut instances: HashMap<u64, Instance> = HashMap::new();
    // A closed stdin means the DAW is gone: stop too.
    while let Some(message) = read_message::<ToHost>(input)? {
        match message {
            ToHost::Hello { .. } => {}
            ToHost::Shutdown => break,
            ToHost::Call { id, request } => {
                let result = handle(&mut instances, request);
                write_message(output, &FromHost::Reply { id, result })?;
            }
        }
    }
    for (_, mut instance) in instances.drain() {
        instance.stop();
    }
    Ok(())
}

fn handle(instances: &mut HashMap<u64, Instance>, request: Request) -> Result<(), String> {
    match request {
        Request::Create { instance, device } => {
            if instances.contains_key(&instance) {
                return Err(format!("instance {instance} exists"));
            }
            instances.insert(
                instance,
                Instance {
                    device,
                    running: None,
                },
            );
            Ok(())
        }
        Request::Activate {
            instance,
            names,
            layout,
            sample_rate,
        } => {
            let entry = instances
                .get_mut(&instance)
                .ok_or_else(|| format!("no instance {instance}"))?;
            entry.stop();
            let mut end = HostEnd::open(&names, layout).map_err(|e| e.to_string())?;
            let waker = HostEnd::waker(&names).map_err(|e| e.to_string())?;
            let stop = Arc::new(AtomicBool::new(false));
            let mut processor = Processor::new(&entry.device);
            let flag = stop.clone();
            let thread = std::thread::Builder::new()
                .name(format!("plugin audio {instance}"))
                .spawn(move || {
                    // Best effort: without it the plugin just runs at
                    // normal priority.
                    let _realtime = audio_thread_priority::promote_current_thread_to_real_time(
                        layout.max_frames,
                        sample_rate as u32,
                    );
                    while end.serve(&flag, |block| processor.process(block)) == Served::Processed {}
                })
                .map_err(|e| e.to_string())?;
            entry.running = Some(Running {
                stop,
                waker,
                thread,
            });
            Ok(())
        }
        Request::Deactivate { instance } => {
            instances
                .get_mut(&instance)
                .ok_or_else(|| format!("no instance {instance}"))?
                .stop();
            Ok(())
        }
        Request::Destroy { instance } => {
            let mut entry = instances
                .remove(&instance)
                .ok_or_else(|| format!("no instance {instance}"))?;
            entry.stop();
            Ok(())
        }
    }
}

#[derive(Debug)]
struct Instance {
    device: PluginSpec,
    running: Option<Running>,
}

#[derive(Debug)]
struct Running {
    stop: Arc<AtomicBool>,
    waker: Event,
    thread: JoinHandle<()>,
}

impl Instance {
    /// Ends the audio thread, if any. A device hung in `process` never
    /// returns, so neither does this; the DAW then ends the process.
    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop.store(true, Ordering::Release);
            running.waker.set();
            let _ = running.thread.join();
        }
    }
}

/// Runs a device on the audio thread.
#[derive(Debug)]
enum Processor {
    Gain { gain: f32 },
    CrashAt { at: u64, blocks: u64 },
    HangAt { at: u64, blocks: u64 },
    Slow { millis: u64 },
}

impl Processor {
    fn new(device: &PluginSpec) -> Self {
        match device {
            PluginSpec::Test(TestPlugin::Gain) => Self::Gain { gain: 1. },
            PluginSpec::Test(TestPlugin::CrashAt(at)) => Self::CrashAt { at: *at, blocks: 0 },
            PluginSpec::Test(TestPlugin::HangAt(at)) => Self::HangAt { at: *at, blocks: 0 },
            PluginSpec::Test(TestPlugin::Slow { millis }) => Self::Slow { millis: *millis },
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "test devices stand in for slow and hung plugins"
    )]
    fn process(&mut self, block: &mut HostBlock<'_>) {
        let gain = match self {
            Self::Gain { gain } => {
                if let Some(last) = block.params().iter().rev().find(|p| p.id == 0) {
                    *gain = (last.value * 2.) as f32;
                }
                *gain
            }
            Self::CrashAt { at, blocks } => {
                *blocks += 1;
                if *blocks >= *at {
                    std::process::abort();
                }
                1.
            }
            Self::HangAt { at, blocks } => {
                *blocks += 1;
                if *blocks >= *at {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(60));
                    }
                }
                1.
            }
            Self::Slow { millis } => {
                std::thread::sleep(std::time::Duration::from_millis(*millis));
                1.
            }
        };
        for ch in 0..block.outputs() {
            if ch < block.inputs() {
                for i in 0..block.frames() {
                    let s = block.input(ch)[i] * gain;
                    block.output(ch)[i] = s;
                }
            } else {
                block.output(ch).fill(0.);
            }
        }
    }
}
