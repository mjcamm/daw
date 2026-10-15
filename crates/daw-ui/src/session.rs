//! The running engine and audio output behind the UI.
//!
//! Views never touch engine state directly: they read the latest
//! [`Snapshot`] and send [`Command`]s through here.

use crate::layout::METER_FALL_PER_FRAME;
use daw_engine::{Command, Engine, EngineConfig, Meter, Snapshot};
use daw_host::{AudioOutput, OutputDevice, demo};

pub struct Session {
    engine: Engine,
    /// Kept alive so audio keeps playing; `None` if no device opened.
    _output: Option<AudioOutput>,
    snapshot: Snapshot,
    /// Meter levels as displayed: peaks that fall back smoothly.
    meter: Meter,
    pub song_name: String,
    /// Why audio is unavailable, if it is.
    pub audio_error: Option<String>,
}

impl Session {
    /// Opens the default audio device and loads the demo song.
    pub fn start() -> Self {
        let device = OutputDevice::open_default();
        let config = device
            .as_ref()
            .map_or_else(|_| EngineConfig::default(), OutputDevice::engine_config);
        let (mut engine, processor) = Engine::new(config);
        if let Err(e) = demo::load(&mut engine) {
            eprintln!("could not load the demo song: {e}");
        }
        let (output, audio_error) = match device.and_then(|d| d.start(processor)) {
            Ok(output) => (Some(output), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let snapshot = engine.snapshot();
        Self {
            engine,
            _output: output,
            snapshot,
            meter: Meter::default(),
            song_name: "Demo".to_owned(),
            audio_error,
        }
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
