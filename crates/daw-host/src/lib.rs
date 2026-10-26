//! The DAW's host: runs the engine on an audio device (`daw-device`),
//! project folders, preferences, export and the demo song.
//!
//! Open a device with [`OutputDevice::open`], create the engine with
//! [`engine_config`], then start the processor:
//! `device.start(move |out| processor.process(out))`.

pub mod analysis;
pub mod demo;
pub mod export;
pub mod plugins;
pub mod preferences;
pub mod projects;
pub mod stamps;

pub use daw_device::{
    AudioOutput, BUFFER_SIZES, CallbackStats, DeviceChoice, DeviceChoices, Load, OutputDevice,
    SAMPLE_RATES, device_choices, drivers,
};

use daw_engine::EngineConfig;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The configuration to create the engine with for `device`.
pub fn engine_config(device: &OutputDevice) -> EngineConfig {
    EngineConfig {
        sample_rate: device.sample_rate(),
        channels: device.channels(),
        threads: device.threads().unwrap_or_else(daw_engine::auto_threads),
    }
}
