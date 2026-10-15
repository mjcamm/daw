//! Runs the engine's audio side on a real output device.
//!
//! Open a device with [`OutputDevice::open_default`], create the engine with
//! its [`OutputDevice::engine_config`], then [`OutputDevice::start`] the
//! processor. The returned [`AudioOutput`] plays until dropped.

pub mod demo;
pub mod preferences;
pub mod projects;
mod stats;

pub use stats::CallbackStats;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use daw_engine::{AudioProcessor, EngineConfig};
use std::sync::Arc;
use std::time::Instant;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// An output device and the stream format chosen for it.
pub struct OutputDevice {
    device: cpal::Device,
    config: cpal::StreamConfig,
    name: String,
}

impl std::fmt::Debug for OutputDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputDevice")
            .field("name", &self.name)
            .field("config", &self.config)
            .finish()
    }
}

impl OutputDevice {
    /// The system's default output device, in 32-bit float.
    pub fn open_default() -> Result<Self> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no audio output device found")?;
        let config = f32_config(&device)?;
        let name = device
            .description()
            .map(|d| d.to_string())
            .unwrap_or_else(|_| "default device".into());
        Ok(Self {
            device,
            config,
            name,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The configuration to create the engine with for this device.
    pub fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
        }
    }

    /// Starts playing `processor` on the device.
    pub fn start(self, processor: AudioProcessor) -> Result<AudioOutput> {
        self.start_with_tap(processor, |_| {})
    }

    /// Like [`Self::start`], but calls `tap` with every rendered block on the
    /// audio thread, inside a realtime section: it must not allocate.
    pub fn start_with_tap(
        self,
        mut processor: AudioProcessor,
        mut tap: impl FnMut(&[f32]) + Send + 'static,
    ) -> Result<AudioOutput> {
        let channels = usize::from(self.config.channels);
        let stats = Arc::new(CallbackStats::new(self.config.sample_rate));
        let callback_stats = Arc::clone(&stats);
        let mut previous_call: Option<Instant> = None;
        let stream = self.device.build_output_stream(
            self.config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let started = Instant::now();
                processor.process(data);
                callback_stats.record(
                    started,
                    started.elapsed(),
                    previous_call.replace(started),
                    data.len() / channels,
                );
                daw_rt::realtime(|| tap(data));
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )?;
        stream.play()?;
        Ok(AudioOutput {
            _stream: stream,
            stats,
        })
    }
}

/// A running audio stream. Audio stops when this is dropped.
pub struct AudioOutput {
    _stream: cpal::Stream,
    stats: Arc<CallbackStats>,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput").finish_non_exhaustive()
    }
}

impl AudioOutput {
    pub fn stats(&self) -> &CallbackStats {
        &self.stats
    }
}

/// The device's default output configuration, in 32-bit float.
fn f32_config(device: &cpal::Device) -> Result<cpal::StreamConfig> {
    let default = device.default_output_config()?;
    if default.sample_format() == cpal::SampleFormat::F32 {
        return Ok(default.config());
    }
    let rate = default.sample_rate();
    device
        .supported_output_configs()?
        .find(|c| {
            c.sample_format() == cpal::SampleFormat::F32
                && (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate)
        })
        .map(|c| c.with_sample_rate(rate).config())
        .ok_or_else(|| "the output device does not support 32-bit float audio".into())
}
