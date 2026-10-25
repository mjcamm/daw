//! Runs the engine's audio side on a real output device.
//!
//! Open a device with [`OutputDevice::open`] (or
//! [`OutputDevice::open_default`]), create the engine with its
//! [`OutputDevice::engine_config`], then [`OutputDevice::start`] the
//! processor. The returned [`AudioOutput`] plays until dropped.
//! [`drivers`] and [`device_choices`] list what the settings can choose.

pub mod demo;
pub mod export;
pub mod plugins;
pub mod preferences;
pub mod projects;
pub mod stamps;
mod stats;

pub use stats::{CallbackStats, Load};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use daw_engine::{AudioProcessor, EngineConfig};
use preferences::AudioSettings;
use std::sync::Arc;
use std::time::Instant;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Sample rates offered, where the device supports them.
pub const SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Buffer sizes offered, where the device supports them.
pub const BUFFER_SIZES: [u32; 7] = [32, 64, 128, 256, 512, 1024, 2048];

/// The drivers on this system, the default first.
pub fn drivers() -> Vec<String> {
    let default = cpal::default_host().id();
    let mut hosts = cpal::available_hosts();
    hosts.sort_by_key(|h| *h != default);
    hosts.into_iter().map(|h| h.name().to_owned()).collect()
}

/// An output device as the settings list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    /// Stable system id, as saved in the settings.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// What the settings can choose for a driver and device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceChoices {
    pub driver: String,
    pub devices: Vec<DeviceChoice>,
    /// For the chosen (or default) device.
    pub sample_rates: Vec<u32>,
    pub default_rate: Option<u32>,
    pub buffer_sizes: Vec<u32>,
    /// Set when the driver fixes the buffer size at the chosen rate (as
    /// Windows Audio does in shared mode): that size.
    pub fixed_buffer: Option<u32>,
}

fn host(settings: &AudioSettings) -> cpal::Host {
    settings
        .driver
        .as_deref()
        .and_then(|name| {
            cpal::available_hosts()
                .into_iter()
                .find(|h| h.name() == name)
        })
        .and_then(|id| cpal::host_from_id(id).ok())
        .unwrap_or_else(cpal::default_host)
}

fn device_id(device: &cpal::Device) -> Option<String> {
    device.id().ok().map(|id| id.to_string())
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "Unnamed device".into())
}

/// The chosen device on the chosen driver, else the driver's default.
fn device(host: &cpal::Host, settings: &AudioSettings) -> Option<cpal::Device> {
    let chosen = settings.device.as_deref().and_then(|wanted| {
        host.output_devices()
            .ok()?
            .find(|d| device_id(d).as_deref() == Some(wanted))
    });
    chosen.or_else(|| host.default_output_device())
}

/// The choices for the driver and device in `settings`.
pub fn device_choices(settings: &AudioSettings) -> DeviceChoices {
    let host = host(settings);
    let default_id = host.default_output_device().as_ref().and_then(device_id);
    let devices = host
        .output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| {
                    let id = device_id(&d)?;
                    Some(DeviceChoice {
                        is_default: Some(&id) == default_id.as_ref(),
                        name: device_name(&d),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut choices = DeviceChoices {
        driver: host.id().name().to_owned(),
        devices,
        ..DeviceChoices::default()
    };
    if let Some(device) = device(&host, settings)
        && let Ok(configs) = device.supported_output_configs()
    {
        let configs: Vec<_> = configs
            .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
            .collect();
        choices.sample_rates = SAMPLE_RATES
            .into_iter()
            .filter(|r| {
                configs
                    .iter()
                    .any(|c| (c.min_sample_rate()..=c.max_sample_rate()).contains(r))
            })
            .collect();
        choices.default_rate = device.default_output_config().ok().map(|c| c.sample_rate());
        let rate = settings.sample_rate.or(choices.default_rate);
        // The buffer range at the rate in use.
        let at_rate: Vec<_> = configs
            .iter()
            .filter(|c| {
                rate.is_none_or(|r| (c.min_sample_rate()..=c.max_sample_rate()).contains(&r))
            })
            .collect();
        let ranges: Vec<_> = at_rate.iter().map(|c| *c.buffer_size()).collect();
        if let [cpal::SupportedBufferSize::Range { min, max }, ..] = ranges.as_slice()
            && min == max
            && ranges.iter().all(|r| *r == ranges[0])
        {
            choices.fixed_buffer = Some(*min);
        } else {
            choices.buffer_sizes = BUFFER_SIZES
                .into_iter()
                .filter(|&size| {
                    ranges.iter().any(|r| match *r {
                        cpal::SupportedBufferSize::Range { min, max } => {
                            (min..=max).contains(&size)
                        }
                        // Unknown: offer them all and let the device decide.
                        cpal::SupportedBufferSize::Unknown => true,
                    })
                })
                .collect();
        }
    }
    choices
}

/// An output device and the stream format chosen for it.
pub struct OutputDevice {
    device: cpal::Device,
    config: cpal::StreamConfig,
    name: String,
    driver: String,
    threads: usize,
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
        Self::open(&AudioSettings::default())
    }

    /// The device, sample rate and buffer size in `settings`, in 32-bit
    /// float. A choice the system no longer offers (an unplugged device, a
    /// rate the device lacks) falls back to the default.
    pub fn open(settings: &AudioSettings) -> Result<Self> {
        let host = host(settings);
        let device = device(&host, settings).ok_or("no audio output device found")?;
        let mut config = f32_config(&device, settings.sample_rate)?;
        // Only ask for a size the device offers at this rate.
        let offered = settings.buffer_frames.filter(|&frames| {
            device.supported_output_configs().is_ok_and(|mut configs| {
                configs.any(|c| {
                    (c.min_sample_rate()..=c.max_sample_rate()).contains(&config.sample_rate)
                        && match *c.buffer_size() {
                            cpal::SupportedBufferSize::Range { min, max } => {
                                min != max && (min..=max).contains(&frames)
                            }
                            cpal::SupportedBufferSize::Unknown => true,
                        }
                })
            })
        });
        if let Some(frames) = offered {
            config.buffer_size = cpal::BufferSize::Fixed(frames);
        }
        Ok(Self {
            name: device_name(&device),
            driver: host.id().name().to_owned(),
            device,
            config,
            threads: settings.threads.unwrap_or_else(daw_engine::auto_threads),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn driver(&self) -> &str {
        &self.driver
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// The buffer size asked for; `None` leaves it to the device.
    pub fn buffer_frames(&self) -> Option<u32> {
        match self.config.buffer_size {
            cpal::BufferSize::Fixed(frames) => Some(frames),
            cpal::BufferSize::Default => None,
        }
    }

    /// The configuration to create the engine with for this device.
    pub fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            threads: self.threads,
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

/// An output configuration in 32-bit float at `rate` if the device offers
/// it, else at the device's default rate.
fn f32_config(device: &cpal::Device, rate: Option<u32>) -> Result<cpal::StreamConfig> {
    let default = device.default_output_config()?;
    let offers = |rate: u32| {
        device.supported_output_configs().is_ok_and(|mut configs| {
            configs.any(|c| {
                c.sample_format() == cpal::SampleFormat::F32
                    && (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate)
            })
        })
    };
    let rate = rate
        .filter(|&r| offers(r))
        .unwrap_or_else(|| default.sample_rate());
    if default.sample_format() == cpal::SampleFormat::F32 && rate == default.sample_rate() {
        return Ok(default.config());
    }
    device
        .supported_output_configs()?
        .find(|c| {
            c.sample_format() == cpal::SampleFormat::F32
                && (c.min_sample_rate()..=c.max_sample_rate()).contains(&rate)
        })
        .map(|c| c.with_sample_rate(rate).config())
        .ok_or_else(|| "the output device does not support 32-bit float audio".into())
}
