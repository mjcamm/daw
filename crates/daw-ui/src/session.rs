//! An open project: the running engine and audio output behind the UI.
//!
//! Views never touch engine state directly: they read the latest
//! [`Snapshot`] and send [`Command`]s through here.

use crate::layout::METER_FALL_PER_FRAME;
use daw_engine::{Command, Edit, Engine, EngineConfig, Meter, Snapshot};
use daw_host::{AudioOutput, OutputDevice, projects};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long unsaved changes wait before a crash-recovery backup is written.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

pub struct Session {
    engine: Engine,
    /// Kept alive so audio keeps playing; `None` if no device opened.
    _output: Option<AudioOutput>,
    snapshot: Snapshot,
    /// Meter levels as displayed: peaks that fall back smoothly.
    meter: Meter,
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
    /// Creates a new, empty project called `name` in `folder` and saves it.
    pub fn create(folder: PathBuf, name: &str) -> Result<Self, String> {
        let mut session = Self::boot(folder, |engine| {
            engine
                .apply(Command::Edit(Edit::RenameSong {
                    name: name.to_owned(),
                }))
                .map_err(|e| e.to_string())
        })?;
        session.save().map_err(|e| format!("could not save: {e}"))?;
        Ok(session)
    }

    /// Opens the project saved in `folder`.
    pub fn open(folder: PathBuf) -> Result<Self, String> {
        let file = projects::open(&folder).map_err(|e| e.to_string())?;
        Self::boot(folder, |engine| {
            engine.load(file).map_err(|e| e.to_string())
        })
    }

    /// Opens the audio device, creates the engine, lets `setup` fill it, then
    /// starts playback through the device.
    fn boot(
        folder: PathBuf,
        setup: impl FnOnce(&mut Engine) -> Result<(), String>,
    ) -> Result<Self, String> {
        let device = OutputDevice::open_default();
        let config = device
            .as_ref()
            .map_or_else(|_| EngineConfig::default(), OutputDevice::engine_config);
        let (mut engine, processor) = Engine::new(config);
        setup(&mut engine)?;
        let (output, audio_error) = match device.and_then(|d| d.start(processor)) {
            Ok(output) => (Some(output), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let revision = engine.content_revision();
        Ok(Self {
            snapshot: engine.snapshot(),
            engine,
            _output: output,
            meter: Meter::default(),
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
            self.snapshot = self.engine.snapshot();
        }
        let peak = self.engine.take_meter();
        self.meter = Meter {
            left: peak.left.max(self.meter.left * METER_FALL_PER_FRAME),
            right: peak.right.max(self.meter.right * METER_FALL_PER_FRAME),
        };
        self.autosave_if_due();
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

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn position_beats(&self) -> f64 {
        self.engine.position_beats()
    }

    pub fn meter(&self) -> Meter {
        self.meter
    }

    /// Sends a command. Rejected commands change nothing, so the UI just
    /// reports them.
    pub fn apply(&mut self, command: Command) {
        if let Err(e) = self.engine.apply(command) {
            eprintln!("command rejected: {e}");
        }
    }
}
