//! An open project: the running engine and audio output behind the UI.
//!
//! Views never touch engine state directly: they read the latest
//! [`Snapshot`] and send [`Command`]s through here.

use crate::layout::METER_FALL_PER_FRAME;
use daw_engine::{Command, Edit, Engine, EngineConfig, Meter, NodeId, Snapshot};
use daw_host::preferences::AudioSettings;
use daw_host::{AudioOutput, OutputDevice, demo, projects};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long unsaved changes wait before a crash-recovery backup is written.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

/// The audio setup running now, as the settings show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioInfo {
    pub driver: String,
    pub device: String,
    pub sample_rate: u32,
    /// The buffer asked for; `None` leaves it to the device.
    pub buffer_frames: Option<u32>,
    pub threads: usize,
}

/// Where imported audio goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    /// Each file on a new audio node, `parent`'s children from `index` on.
    NewNodes { parent: NodeId, index: usize },
    /// One after another on this audio node.
    Into(NodeId),
}

pub struct Session {
    engine: Engine,
    /// Kept alive so audio keeps playing; `None` if no device opened.
    output: Option<AudioOutput>,
    audio: Option<AudioInfo>,
    /// The audio thread's load, smoothed for display (1.0 is the limit),
    /// and whether a dropout happened lately.
    load: f32,
    dropout_frames: u32,
    snapshot: Snapshot,
    /// The snapshot before the current one, so a view can still draw what
    /// an edit just removed while it animates away.
    previous: Option<Snapshot>,
    /// Each node's meter as displayed: peaks that fall back smoothly.
    meters: BTreeMap<NodeId, Meter>,
    folder: PathBuf,
    /// Content revision at the last save.
    saved_revision: u64,
    /// Content revision at the last autosave, and when that was.
    autosaved_revision: u64,
    last_autosave: Instant,
    /// Why audio is unavailable, if it is.
    pub audio_error: Option<String>,
}

impl Session {
    /// Creates a new project called `name` in `folder`, starting with the
    /// demo notes, and saves it.
    pub fn create(folder: PathBuf, name: &str, audio: &AudioSettings) -> Result<Self, String> {
        let mut session = Self::boot(folder, open_device(audio), |engine| {
            engine
                .apply(Command::Edit(Edit::RenameSong {
                    name: name.to_owned(),
                }))
                .and_then(|()| demo::add_starter_notes(engine))
                .map_err(|e| e.to_string())?;
            // The starting content is not something to undo.
            engine.clear_history();
            Ok(())
        })?;
        session.save().map_err(|e| format!("could not save: {e}"))?;
        Ok(session)
    }

    /// Opens the project saved in `folder`.
    pub fn open(folder: PathBuf, audio: &AudioSettings) -> Result<Self, String> {
        let file = projects::open(&folder).map_err(|e| e.to_string())?;
        Self::boot(folder, open_device(audio), |engine| {
            engine.load(file).map_err(|e| e.to_string())
        })
    }

    /// A new project with the starter song and no audio device, for the
    /// headless UI tests. Nothing is saved until asked.
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn headless(folder: PathBuf) -> Result<Self, String> {
        Self::boot(folder, Err("no audio in tests".into()), |engine| {
            demo::add_starter_notes(engine).map_err(|e| e.to_string())?;
            engine.clear_history();
            Ok(())
        })
    }

    /// Creates the engine on `device`, lets `setup` fill it, then starts
    /// playback through the device.
    fn boot(
        folder: PathBuf,
        device: daw_host::Result<OutputDevice>,
        setup: impl FnOnce(&mut Engine) -> Result<(), String>,
    ) -> Result<Self, String> {
        let config = device
            .as_ref()
            .map_or_else(|_| EngineConfig::default(), OutputDevice::engine_config);
        let (mut engine, processor) = Engine::new(config);
        // Tests run no plugins (their binary can't be a plugin host).
        if let Some(command) = daw_host::plugins::host_command()
            && !cfg!(test)
        {
            engine.set_plugin_host(command);
        }
        engine.set_sample_dir(Some(folder.clone()));
        setup(&mut engine)?;
        let info = device.as_ref().ok().map(audio_info);
        let (output, audio_error) = match device.and_then(|d| d.start(processor)) {
            Ok(output) => (Some(output), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let revision = engine.content_revision();
        Ok(Self {
            snapshot: engine.snapshot(),
            previous: None,
            engine,
            audio: info.filter(|_| output.is_some()),
            output,
            load: 0.0,
            dropout_frames: 0,
            meters: BTreeMap::new(),
            folder,
            saved_revision: revision,
            autosaved_revision: revision,
            last_autosave: Instant::now(),
            audio_error,
        })
    }

    /// Services the engine once per UI frame.
    pub fn tick(&mut self) {
        self.engine.maintain();
        if self.engine.revision() != self.snapshot.revision {
            let current = std::mem::replace(&mut self.snapshot, self.engine.snapshot());
            self.previous = Some(current);
        }
        for (node, peak) in self.engine.take_meters() {
            let shown = self.meters.entry(node).or_default();
            *shown = Meter {
                left: peak.left.max(shown.left * METER_FALL_PER_FRAME),
                right: peak.right.max(shown.right * METER_FALL_PER_FRAME),
            };
        }
        if let Some(output) = &self.output {
            let load = output.stats().take_load();
            // Rises at once, falls back slowly, so peaks can be read.
            self.load = load.fraction.max(self.load * 0.95);
            if load.late > 0 {
                // Shown for about a second.
                self.dropout_frames = 60;
            } else {
                self.dropout_frames = self.dropout_frames.saturating_sub(1);
            }
        }
        self.autosave_if_due();
    }

    /// The audio setup running now; `None` without a device.
    pub fn audio(&self) -> Option<&AudioInfo> {
        self.audio.as_ref()
    }

    /// The buffer size the device is actually using, once it has played.
    pub fn device_buffer(&self) -> Option<u32> {
        let frames = self.output.as_ref()?.stats().buffer_frames();
        (frames > 0).then_some(frames as u32)
    }

    /// The audio thread's recent load (1.0 is the limit), and whether a
    /// dropout happened lately.
    pub fn load(&self) -> (f32, bool) {
        (self.load, self.dropout_frames > 0)
    }

    /// Switches to another audio setup, keeping the song, undo history and
    /// playback. If the setup cannot start, the system default is tried.
    pub fn apply_audio(&mut self, settings: &AudioSettings) {
        // Free the device first: some drivers allow one stream at a time.
        self.output = None;
        self.audio = None;
        let device = open_device(settings);
        let result = device.and_then(|device| {
            let info = audio_info(&device);
            let processor = self.engine.reconnect(device.engine_config());
            device.start(processor).map(|output| (output, info))
        });
        match result {
            Ok((output, info)) => {
                self.output = Some(output);
                self.audio = Some(info);
                self.audio_error = None;
            }
            Err(e) => self.audio_error = Some(e.to_string()),
        }
    }

    /// A short tone through the output, to check the device.
    pub fn test_tone(&mut self) {
        self.apply(Command::TestTone);
    }

    fn autosave_if_due(&mut self) {
        let revision = self.engine.content_revision();
        if self.is_edited()
            && revision != self.autosaved_revision
            && self.last_autosave.elapsed() >= AUTOSAVE_INTERVAL
        {
            self.last_autosave = Instant::now();
            match projects::autosave(&self.folder, &self.engine.to_file()) {
                Ok(()) => self.autosaved_revision = revision,
                Err(e) => eprintln!("autosave failed: {e}"),
            }
        }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        projects::save(&self.folder, &self.engine.to_file())?;
        self.saved_revision = self.engine.content_revision();
        self.autosaved_revision = self.saved_revision;
        Ok(())
    }

    /// Whether there are changes since the last save.
    pub fn is_edited(&self) -> bool {
        self.engine.content_revision() != self.saved_revision
    }

    /// Copies an audio file into the project and makes it the sample a
    /// SingleSampler plays, as one undoable step.
    pub fn import_sample(
        &mut self,
        instrument: daw_engine::DeviceId,
        source: &Path,
    ) -> Result<(), String> {
        let sample = projects::import_sample(&self.folder, source).map_err(|e| e.to_string())?;
        let id = daw_engine::SampleId(
            self.engine
                .snapshot()
                .samples
                .iter()
                .map(|s| s.id.0 + 1)
                .max()
                .unwrap_or(1),
        );
        self.engine
            .apply(Command::Edit(Edit::Batch(vec![
                Edit::AddSample { id, sample },
                Edit::SetInstrumentSample {
                    id: instrument,
                    sample: Some(id),
                },
            ])))
            .map_err(|e| e.to_string())
    }

    /// Copies audio files into the project and adds them to the song at the
    /// playhead, as one undoable step: each on a new audio node in
    /// `parent` from `index` on, or (with `into`) one after another on that
    /// audio node. Files that cannot be read are skipped and named in the
    /// error; the rest still go in.
    pub fn import_audio(&mut self, files: &[PathBuf], target: ImportTarget) -> Result<(), String> {
        let mut imported = Vec::new();
        let mut failed = Vec::new();
        for path in files {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            // Read the length first, so an unreadable file is never copied.
            let result = projects::wav_seconds(path).and_then(|seconds| {
                projects::import_sample(&self.folder, path).map(|sample| (sample, seconds))
            });
            match result {
                Ok((sample, seconds)) => {
                    imported.push(crate::audio_import::Imported { sample, seconds });
                }
                Err(e) => failed.push(format!("{name}: {e}")),
            }
        }
        if !imported.is_empty() {
            let snapshot = self.engine.snapshot();
            let at = daw_engine::Ticks::from_beats_f64(self.engine.position_beats().max(0.));
            let edit = match target {
                ImportTarget::NewNodes { parent, index } => {
                    crate::audio_import::onto_new_nodes(&snapshot, &imported, parent, index, at)
                }
                ImportTarget::Into(node) => {
                    crate::audio_import::into_node(&snapshot, &imported, node, at)
                }
            };
            self.engine
                .apply(Command::Edit(edit))
                .map_err(|e| e.to_string())?;
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(format!("Could not import {}", failed.join("; ")))
        }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// The snapshot before the current one, if any.
    pub fn previous_snapshot(&self) -> Option<&Snapshot> {
        self.previous.as_ref()
    }

    pub fn position_beats(&self) -> f64 {
        self.engine.position_beats()
    }

    /// Every node's meter as displayed.
    pub fn meters(&self) -> &BTreeMap<NodeId, Meter> {
        &self.meters
    }

    /// Applies a command, failing loudly: for building test songs.
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn apply_or_panic(&mut self, command: Command) {
        if let Err(e) = self.engine.apply(command) {
            panic!("command rejected: {e}");
        }
    }

    /// What a plugin effect lists, as if it ran (tests run no plugin host).
    #[cfg(all(test, feature = "ui-tests"))]
    pub fn show_plugin_params(
        &mut self,
        effect: daw_engine::DeviceId,
        params: Vec<daw_engine::PluginParamInfo>,
    ) {
        self.engine.show_plugin_params(effect, params);
        self.tick();
    }

    /// Sends a command. Rejected commands change nothing, so the UI just
    /// reports them.
    pub fn apply(&mut self, command: Command) {
        if let Err(e) = self.engine.apply(command) {
            eprintln!("command rejected: {e}");
        }
    }
}

/// The device in `settings`, else (if that fails) the system default.
fn open_device(settings: &AudioSettings) -> daw_host::Result<OutputDevice> {
    OutputDevice::open(settings).or_else(|e| {
        if *settings == AudioSettings::default() {
            Err(e)
        } else {
            OutputDevice::open(&AudioSettings {
                threads: settings.threads,
                ..AudioSettings::default()
            })
        }
    })
}

fn audio_info(device: &OutputDevice) -> AudioInfo {
    AudioInfo {
        driver: device.driver().to_owned(),
        device: device.name().to_owned(),
        sample_rate: device.sample_rate(),
        buffer_frames: device.buffer_frames(),
        threads: device.engine_config().threads,
    }
}
