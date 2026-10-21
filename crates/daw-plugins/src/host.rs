//! The plugin host process: control messages on stdin, replies on stdout,
//! one realtime audio thread per active instance. Run by `daw-plugin-host`
//! and by the app itself in plugin-host mode.

use crate::block::{HostBlock, HostEnd, Served};
use crate::control::{
    FromHost, PROTOCOL, PluginSpec, Reply, Request, TestPlugin, ToHost, read_message, write_message,
};
use crate::sys::Event;
use crate::vst3::Module;
use crate::vst3_host::{Vst3Instance, Vst3State};
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Serves the DAW until it says stop or goes away.
pub fn run() -> ExitCode {
    crate::sys::init_com();
    crate::vst3::quiet_errors();
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
    let mut host = Host::default();
    // A closed stdin means the DAW is gone: stop too.
    while let Some(message) = read_message::<ToHost>(input)? {
        match message {
            ToHost::Hello { .. } => {}
            ToHost::Shutdown => break,
            ToHost::Call { id, request } => {
                let result = host.handle(request);
                write_message(output, &FromHost::Reply { id, result })?;
            }
        }
    }
    for (_, mut instance) in host.instances.drain() {
        instance.stop();
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Host {
    instances: HashMap<u64, Instance>,
    /// Plugin modules, loaded once per bundle and kept while used.
    modules: HashMap<PathBuf, Arc<Module>>,
}

impl Host {
    fn handle(&mut self, request: Request) -> Result<Reply, String> {
        match request {
            Request::Create {
                instance,
                device,
                state,
            } => {
                if self.instances.contains_key(&instance) {
                    return Err(format!("instance {instance} exists"));
                }
                let processor = self.processor(&device)?;
                let handle = match &processor {
                    Processor::Vst3(plugin) => Some(plugin.state_handle()),
                    Processor::Test(_) => None,
                };
                if let (Some(handle), Some(state)) = (&handle, &state) {
                    handle.set(state);
                }
                self.instances.insert(
                    instance,
                    Instance {
                        state: handle,
                        processor: Some(processor),
                        running: None,
                    },
                );
                Ok(Reply::Done)
            }
            Request::GetState { instance } => {
                let entry = self
                    .instances
                    .get(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                Ok(Reply::State(entry.state.as_ref().map(Vst3State::get)))
            }
            Request::Activate {
                instance,
                names,
                layout,
                sample_rate,
            } => {
                let entry = self
                    .instances
                    .get_mut(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                entry.stop();
                let mut processor = entry.processor.take().ok_or("instance is gone")?;
                let opened = HostEnd::open(&names, layout)
                    .and_then(|end| Ok((end, HostEnd::waker(&names)?)))
                    .map_err(|e| e.to_string())
                    .and_then(|ends| {
                        processor
                            .activate(sample_rate, layout.max_frames)
                            .map(|()| ends)
                    });
                let (mut end, waker) = match opened {
                    Ok(ends) => ends,
                    Err(e) => {
                        entry.processor = Some(processor);
                        return Err(e);
                    }
                };
                let stop = Arc::new(AtomicBool::new(false));
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
                        while end.serve(&flag, |block| processor.process(block))
                            == Served::Processed
                        {}
                        processor.stop_processing();
                        processor
                    })
                    .map_err(|e| e.to_string())?;
                entry.running = Some(Running {
                    stop,
                    waker,
                    thread,
                });
                Ok(Reply::Done)
            }
            Request::Deactivate { instance } => {
                self.instances
                    .get_mut(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?
                    .stop();
                Ok(Reply::Done)
            }
            Request::Destroy { instance } => {
                let mut entry = self
                    .instances
                    .remove(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                entry.stop();
                drop(entry);
                // Unload modules no instance uses any more.
                self.modules
                    .retain(|_, module| Arc::strong_count(module) > 1);
                Ok(Reply::Done)
            }
        }
    }

    fn processor(&mut self, device: &PluginSpec) -> Result<Processor, String> {
        Ok(match device {
            PluginSpec::Test(test) => Processor::Test(TestProcessor::new(*test)),
            PluginSpec::Vst3 { bundle, cid, .. } => {
                let module = match self.modules.get(bundle) {
                    Some(module) => module.clone(),
                    None => {
                        let binary = crate::scan::binary_path(bundle)
                            .ok_or_else(|| format!("{} is not there any more", bundle.display()))?;
                        let module = Arc::new(Module::load(&binary)?);
                        self.modules.insert(bundle.clone(), module.clone());
                        module
                    }
                };
                Processor::Vst3(Box::new(Vst3Instance::create(module, cid)?))
            }
        })
    }
}

#[derive(Debug)]
struct Instance {
    /// Reads the plugin's state while it processes (none for test devices).
    state: Option<Vst3State>,
    /// Here while not processing; with the audio thread while it is.
    processor: Option<Processor>,
    running: Option<Running>,
}

#[derive(Debug)]
struct Running {
    stop: Arc<AtomicBool>,
    waker: Event,
    thread: JoinHandle<Processor>,
}

impl Instance {
    /// Ends the audio thread, if any, and takes the processor back
    /// (deactivated). A device hung in `process` never returns, so neither
    /// does this; the DAW then ends the process.
    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.stop.store(true, Ordering::Release);
            running.waker.set();
            if let Ok(mut processor) = running.thread.join() {
                processor.deactivate();
                self.processor = Some(processor);
            }
        }
    }
}

/// What runs on an instance's audio thread.
#[derive(Debug)]
enum Processor {
    Test(TestProcessor),
    Vst3(Box<Vst3Instance>),
}

impl Processor {
    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<(), String> {
        match self {
            Self::Test(_) => Ok(()),
            Self::Vst3(plugin) => plugin.activate(sample_rate, max_frames, false),
        }
    }

    fn process(&mut self, block: &mut HostBlock<'_>) {
        match self {
            Self::Test(test) => test.process(block),
            Self::Vst3(plugin) => plugin.process(block),
        }
    }

    fn stop_processing(&mut self) {
        if let Self::Vst3(plugin) = self {
            plugin.stop_processing();
        }
    }

    fn deactivate(&mut self) {
        if let Self::Vst3(plugin) = self {
            plugin.deactivate();
        }
    }
}

/// The built-in test devices.
#[derive(Debug)]
enum TestProcessor {
    Gain { gain: f32 },
    CrashAt { at: u64, blocks: u64 },
    HangAt { at: u64, blocks: u64 },
    Slow { millis: u64 },
}

impl TestProcessor {
    fn new(test: TestPlugin) -> Self {
        match test {
            TestPlugin::Gain => Self::Gain { gain: 1. },
            TestPlugin::CrashAt(at) => Self::CrashAt { at, blocks: 0 },
            TestPlugin::HangAt(at) => Self::HangAt { at, blocks: 0 },
            TestPlugin::Slow { millis } => Self::Slow { millis },
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
