//! Live playback through the default audio device.

use crate::offline::{TAIL_SECONDS, render_demo};
use crate::{Result, demo, frames_for_bars};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use daw_engine::{Command, Engine, EngineConfig};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How often the control thread services the engine.
const CONTROL_INTERVAL: Duration = Duration::from_millis(10);
/// Most bars `--compare` records; one pass through the demo loop is enough.
const COMPARE_BARS: u32 = demo::BARS as u32;
/// Allowed per-sample difference between live and offline output. They run
/// identical code on identical input, so this should be exact.
const COMPARE_TOLERANCE: f32 = 0.0;

pub fn play(bars: u32, compare: bool) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("no audio output device found")?;
    let stream_config = f32_config(&device)?;
    let config = EngineConfig {
        sample_rate: stream_config.sample_rate,
        channels: stream_config.channels,
    };
    let channels = usize::from(config.channels);
    let name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "default device".into());
    println!(
        "Playing {bars} bars at {} BPM on {name} ({} Hz, {} channels). Ctrl+C to quit.",
        demo::TEMPO_BPM,
        config.sample_rate,
        config.channels
    );
    println!("{}", demo::DESCRIPTION);

    let (mut engine, mut processor) = Engine::new(config);
    demo::load(&mut engine)?;
    // Queued now, so the very first callback starts playing from the top,
    // exactly like an offline render.
    engine.apply(Command::Play)?;

    // Recording for --compare: allocated up front, filled on the audio thread
    // without allocating, then handed back through a queue.
    let compare_frames = frames_for_bars(bars.min(COMPARE_BARS), config.sample_rate);
    let mut capture = compare.then(|| Vec::<f32>::with_capacity(compare_frames * channels));
    let (mut capture_tx, mut capture_rx) = rtrb::RingBuffer::new(1);
    let stats = Arc::new(CallbackStats::default());
    stats
        .sample_rate
        .store(u64::from(config.sample_rate), Ordering::Relaxed);
    let callback_stats = Arc::clone(&stats);
    let mut previous_call: Option<Instant> = None;

    let stream = device.build_output_stream(
        stream_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let started = Instant::now();
            processor.process(data);
            callback_stats.record(
                started,
                started.elapsed(),
                previous_call.replace(started),
                data.len() / channels,
            );
            daw_rt::realtime(|| {
                if let Some(buffer) = capture.as_mut() {
                    let room = buffer.capacity() - buffer.len();
                    buffer.extend_from_slice(&data[..room.min(data.len())]);
                    if buffer.len() == buffer.capacity()
                        && let Some(full) = capture.take()
                    {
                        // Capacity 1 and pushed once, so this cannot fail.
                        let _ = capture_tx.push(full);
                    }
                }
            });
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;
    stream.play()?;

    let song = Duration::from_secs_f64(f64::from(bars) * 4.0 * 60.0 / demo::TEMPO_BPM);
    let tail = Duration::from_secs_f64(TAIL_SECONDS);
    let started = Instant::now();
    let mut stopped = false;
    let mut captured = None;
    let mut last_beat = None;
    while started.elapsed() < song + tail {
        engine.maintain();
        if !stopped && started.elapsed() >= song {
            engine.apply(Command::Stop)?;
            stopped = true;
        }
        if let Ok(buffer) = capture_rx.pop() {
            captured = Some(buffer);
        }
        let beat = engine.snapshot().position_beats.floor() as i64;
        if !stopped && last_beat != Some(beat) {
            print!("\r  bar {}  beat {}  ", beat / 4 + 1, beat % 4 + 1);
            std::io::stdout().flush()?;
            last_beat = Some(beat);
        }
        wait(CONTROL_INTERVAL);
    }
    drop(stream);
    println!("\rDone.                ");
    stats.report(config.sample_rate);

    if compare {
        let live = captured.ok_or("live recording did not finish")?;
        compare_with_offline(&live, config, compare_frames)?;
    }
    Ok(())
}

#[allow(
    clippy::disallowed_methods,
    reason = "control thread pacing, not the audio thread"
)]
fn wait(duration: Duration) {
    std::thread::sleep(duration);
}

fn compare_with_offline(live: &[f32], config: EngineConfig, frames: usize) -> Result<()> {
    let offline = render_demo(config, frames)?;
    let channels = usize::from(config.channels);
    let worst = live
        .iter()
        .zip(&offline)
        .map(|(a, b)| (a - b).abs())
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b));
    match worst {
        Some((i, diff)) if diff > COMPARE_TOLERANCE => Err(format!(
            "live output differs from offline render: {diff} at frame {}",
            i / channels
        )
        .into()),
        _ => {
            println!("Live output matches the offline render exactly ({frames} frames compared).");
            Ok(())
        }
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

/// Timing of live audio callbacks, gathered on the audio thread with atomics
/// only (no allocation or locking).
#[derive(Debug, Default)]
struct CallbackStats {
    sample_rate: AtomicU64,
    callbacks: AtomicU64,
    total_process_ns: AtomicU64,
    max_process_ns: AtomicU64,
    max_gap_ns: AtomicU64,
    /// Callbacks that arrived much later than their buffer length implies:
    /// the device likely ran dry (an underrun).
    late: AtomicU64,
    min_frames: AtomicU64,
    max_frames: AtomicU64,
}

/// Seconds of audio a buffer of `frames` frames holds.
fn buffer_seconds(frames: u64, sample_rate: u32) -> f64 {
    frames as f64 / f64::from(sample_rate)
}

impl CallbackStats {
    fn record(&self, now: Instant, took: Duration, previous: Option<Instant>, frames: usize) {
        let frames = frames as u64;
        let count = self.callbacks.fetch_add(1, Ordering::Relaxed);
        let took = took.as_nanos() as u64;
        self.total_process_ns.fetch_add(took, Ordering::Relaxed);
        self.max_process_ns.fetch_max(took, Ordering::Relaxed);
        if count == 0 {
            self.min_frames.store(frames, Ordering::Relaxed);
        }
        self.min_frames.fetch_min(frames, Ordering::Relaxed);
        self.max_frames.fetch_max(frames, Ordering::Relaxed);
        // Skip the first few callbacks while the stream settles.
        if let Some(previous) = previous.filter(|_| count > 4) {
            let gap = now.duration_since(previous).as_nanos() as u64;
            self.max_gap_ns.fetch_max(gap, Ordering::Relaxed);
            // The smallest buffer is the device period: the normal spacing.
            let sample_rate = self.sample_rate.load(Ordering::Relaxed).max(1);
            let period = self.min_frames.load(Ordering::Relaxed) * 1_000_000_000 / sample_rate;
            if gap > period * 2 + 1_000_000 {
                self.late.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn report(&self, sample_rate: u32) {
        let callbacks = self.callbacks.load(Ordering::Relaxed).max(1);
        let ms = |ns: u64| ns as f64 / 1e6;
        let min_frames = self.min_frames.load(Ordering::Relaxed);
        let max_frames = self.max_frames.load(Ordering::Relaxed);
        let budget_ms = buffer_seconds(min_frames, sample_rate) * 1e3;
        let max_process = ms(self.max_process_ns.load(Ordering::Relaxed));
        println!(
            "Audio callbacks: {callbacks}, buffers of {min_frames}..={max_frames} frames \
             ({budget_ms:.1} ms or more each)."
        );
        println!(
            "  Processing: {:.3} ms average, {max_process:.3} ms worst ({:.0}% of the budget).",
            ms(self.total_process_ns.load(Ordering::Relaxed)) / callbacks as f64,
            max_process / budget_ms * 100.0
        );
        println!(
            "  Longest gap between callbacks: {:.1} ms. Late callbacks (likely dropouts): {}.",
            ms(self.max_gap_ns.load(Ordering::Relaxed)),
            self.late.load(Ordering::Relaxed)
        );
    }
}
