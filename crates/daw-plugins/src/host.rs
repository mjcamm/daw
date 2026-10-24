//! The plugin host process: control messages on stdin, replies on stdout,
//! one realtime audio thread per active instance. Run by `daw-plugin-host`
//! and by the app itself in plugin-host mode.

use crate::block::{HostBlock, HostEnd, Served};
use crate::control::{
    FromHost, HostEvent, PROTOCOL, PluginParamInfo, PluginSpec, Reply, Request, TestPlugin, ToHost,
    read_message, write_message,
};
#[cfg(windows)]
use crate::editor::Editor;
use crate::sys::Event;
use crate::vst3::Module;
use crate::vst3_host::{Events, Vst3Instance, Vst3State};
use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Serves the DAW until it says stop or goes away.
pub fn run() -> ExitCode {
    crate::sys::init_com();
    crate::sys::dpi_aware();
    crate::vst3::quiet_errors();
    let mut output = BufWriter::new(io::stdout().lock());
    match serve(io::stdin(), &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("plugin host: {e}");
            ExitCode::FAILURE
        }
    }
}

/// How long the main thread waits between rounds when nothing happens: how
/// often open windows show the song's latest values.
const ROUND: Duration = Duration::from_millis(30);

/// The main thread: messages from the DAW (read on their own thread, so
/// waiting for them never stops plugin windows), plugin windows, and what
/// they report.
#[allow(
    clippy::disallowed_types,
    reason = "the host's main thread, never an audio thread"
)]
fn serve(input: impl io::Read + Send + 'static, output: &mut impl Write) -> io::Result<()> {
    let mut input = BufReader::new(input);
    let daw_pid = match read_message::<ToHost>(&mut input)? {
        Some(ToHost::Hello { protocol, daw_pid }) if protocol == PROTOCOL => daw_pid,
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected hello with protocol {PROTOCOL}, got {other:?}"),
            ));
        }
    };
    write_message(
        output,
        &FromHost::Ready {
            protocol: PROTOCOL,
            pid: std::process::id(),
        },
    )?;
    #[cfg(windows)]
    crate::editor::stacking::set_daw(daw_pid);
    #[cfg(not(windows))]
    let _ = daw_pid;

    let wake = Arc::new(Event::local()?);
    let (sender, messages) = std::sync::mpsc::channel::<Option<ToHost>>();
    let waker = wake.clone();
    std::thread::Builder::new()
        .name("plugin host reader".into())
        .spawn(move || {
            loop {
                // A closed or broken stdin means the DAW is gone.
                let message = read_message::<ToHost>(&mut input).ok().flatten();
                let done = message.is_none();
                if sender.send(message).is_err() {
                    break;
                }
                waker.set();
                if done {
                    break;
                }
            }
        })?;

    let mut host = Host::default();
    'serve: loop {
        while let Ok(message) = messages.try_recv() {
            match message {
                None | Some(ToHost::Shutdown) => break 'serve,
                Some(ToHost::Hello { .. }) => {}
                Some(ToHost::Call { id, request }) => {
                    let result = host.handle(request);
                    write_message(output, &FromHost::Reply { id, result })?;
                }
            }
        }
        for event in host.round() {
            write_message(output, &FromHost::Event(event))?;
        }
        crate::sys::pump_until(&wake, ROUND);
    }
    for (_, mut instance) in host.instances.drain() {
        instance.editor = None;
        instance.stop();
    }
    Ok(())
}

#[derive(Debug, Default)]
struct Host {
    instances: HashMap<u64, Instance>,
    /// Plugin modules, loaded once per bundle and kept while used.
    modules: HashMap<PathBuf, Arc<Module>>,
    /// What plugins reported from their windows since the last round.
    events: Events,
}

impl Host {
    /// After each round of messages: closes windows the user closed, shows
    /// the song's latest values in open ones, and hands over what plugins
    /// reported.
    fn round(&mut self) -> Vec<HostEvent> {
        let mut events = std::mem::take(&mut *self.events.borrow_mut());
        for (&id, instance) in &mut self.instances {
            if let Some(at) = instance.moved_to() {
                events.push(HostEvent::EditorMoved { instance: id, at });
            }
            if instance.close_if_closed() {
                events.push(HostEvent::EditorClosed { instance: id });
            }
            if let Some(state) = &instance.state {
                state.follow();
            }
        }
        events
    }

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
                let (processor, handle) = self.processor(&device, instance)?;
                let test_params = match &device {
                    PluginSpec::Test(TestPlugin::Gain) => vec![PluginParamInfo {
                        id: 0,
                        name: "Gain".into(),
                        group: String::new(),
                        unit: String::new(),
                        steps: 0,
                        value: 0.5,
                        bypass: false,
                    }],
                    _ => Vec::new(),
                };
                if let (Some(handle), Some(state)) = (&handle, &state) {
                    handle.set(state);
                }
                self.instances.insert(
                    instance,
                    Instance {
                        state: handle,
                        test_params,
                        test: match &device {
                            PluginSpec::Test(test) => Some(*test),
                            PluginSpec::Vst3 { .. } => None,
                        },
                        #[cfg(windows)]
                        editor: None,
                        position: None,
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
            Request::GetParams { instance } => {
                let entry = self
                    .instances
                    .get(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                Ok(Reply::Params(match &entry.state {
                    Some(handle) => handle.params(),
                    None => entry.test_params.clone(),
                }))
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
            Request::OpenEditor {
                instance,
                title,
                at,
            } => {
                let entry = self
                    .instances
                    .get_mut(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                entry.open_editor(&title, at)
            }
            Request::GetLatency { instance } => {
                let entry = self
                    .instances
                    .get(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                Ok(Reply::Latency(match (&entry.state, &entry.test) {
                    (Some(state), _) => state.latency(),
                    (None, Some(TestPlugin::Delay { frames })) => *frames,
                    _ => 0,
                }))
            }
            Request::SetEditorTitle { instance, title } => {
                #[cfg(windows)]
                if let Some(editor) = self
                    .instances
                    .get(&instance)
                    .and_then(|i| i.editor.as_ref())
                {
                    editor.set_title(&title);
                }
                #[cfg(not(windows))]
                let _ = (instance, title);
                Ok(Reply::Done)
            }
            Request::CloseEditor { instance } => {
                let entry = self
                    .instances
                    .get_mut(&instance)
                    .ok_or_else(|| format!("no instance {instance}"))?;
                entry.close_editor();
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
                entry.close_editor();
                entry.stop();
                drop(entry);
                // Unload modules no instance uses any more.
                self.modules
                    .retain(|_, module| Arc::strong_count(module) > 1);
                Ok(Reply::Done)
            }
        }
    }

    /// The device's processor, and for a VST3 its main-thread side.
    fn processor(
        &mut self,
        device: &PluginSpec,
        instance: u64,
    ) -> Result<(Processor, Option<Vst3State>), String> {
        Ok(match device {
            PluginSpec::Test(test) => (Processor::Test(TestProcessor::new(*test)), None),
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
                let (plugin, state) =
                    Vst3Instance::create(module, cid, instance, self.events.clone())?;
                (Processor::Vst3(Box::new(plugin)), Some(state))
            }
        })
    }
}

#[derive(Debug)]
struct Instance {
    /// Reads the plugin's state while it processes (none for test devices).
    state: Option<Vst3State>,
    /// A built-in test device's parameters.
    test_params: Vec<PluginParamInfo>,
    /// Which built-in test device, if it is one.
    test: Option<TestPlugin>,
    /// The plugin's window, while open.
    #[cfg(windows)]
    editor: Option<Editor>,
    /// Where its window was last, to open it there again.
    position: Option<(i32, i32)>,
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
    /// Opens the plugin's window, or brings it to the front.
    fn open_editor(&mut self, title: &str, at: Option<(i32, i32)>) -> Result<Reply, String> {
        #[cfg(windows)]
        {
            if let Some(editor) = &self.editor {
                editor.raise(title);
            } else {
                let controller = self
                    .state
                    .as_ref()
                    .and_then(Vst3State::controller)
                    .ok_or("this plugin has no window")?;
                self.editor = Some(Editor::open(controller, title, at.or(self.position))?);
            }
            let (width, height) = self.editor.as_ref().map_or((0, 0), Editor::size);
            Ok(Reply::Editor { width, height })
        }
        #[cfg(not(windows))]
        {
            let _ = (title, at);
            Err("plugin windows are Windows-only for now".into())
        }
    }

    fn close_editor(&mut self) {
        #[cfg(windows)]
        if let Some(editor) = self.editor.take() {
            self.position = Some(editor.position());
        }
    }

    /// Where its window went, if it was placed or moved since last asked.
    fn moved_to(&mut self) -> Option<(i32, i32)> {
        #[cfg(windows)]
        if let Some(editor) = self.editor.as_ref().filter(|e| e.take_moved()) {
            let at = editor.position();
            self.position = Some(at);
            return Some(at);
        }
        None
    }

    /// Takes down a window the user closed; true if there was one.
    fn close_if_closed(&mut self) -> bool {
        #[cfg(windows)]
        if self.editor.as_ref().is_some_and(Editor::closed) {
            self.close_editor();
            return true;
        }
        false
    }

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
    Gain {
        gain: f32,
    },
    CrashAt {
        at: u64,
        blocks: u64,
    },
    HangAt {
        at: u64,
        blocks: u64,
    },
    Slow {
        millis: u64,
    },
    /// A ring of `frames` per channel (made on the main thread).
    Delay {
        lines: Vec<Vec<f32>>,
        at: usize,
    },
}

impl TestProcessor {
    fn new(test: TestPlugin) -> Self {
        match test {
            TestPlugin::Gain => Self::Gain { gain: 1. },
            TestPlugin::CrashAt(at) => Self::CrashAt { at, blocks: 0 },
            TestPlugin::HangAt(at) => Self::HangAt { at, blocks: 0 },
            TestPlugin::Slow { millis } => Self::Slow { millis },
            TestPlugin::Delay { frames } => Self::Delay {
                lines: vec![vec![0.; frames as usize]; crate::block::MAX_CHANNELS],
                at: 0,
            },
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "test devices stand in for slow and hung plugins"
    )]
    fn process(&mut self, block: &mut HostBlock<'_>) {
        if let Self::Delay { lines, at } = self {
            let len = lines[0].len();
            let start = *at;
            for (ch, line) in lines.iter_mut().enumerate().take(block.outputs()) {
                let mut pos = start;
                for i in 0..block.frames() {
                    let input = if ch < block.inputs() {
                        block.input(ch)[i]
                    } else {
                        0.
                    };
                    if len == 0 {
                        block.output(ch)[i] = input;
                        continue;
                    }
                    block.output(ch)[i] = line[pos];
                    line[pos] = input;
                    pos = (pos + 1) % len;
                }
            }
            if len > 0 {
                *at = (start + block.frames()) % len;
            }
            return;
        }
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
            Self::Delay { .. } => 1.,
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
