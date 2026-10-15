//! Live playback through the default audio device.

use crate::offline::{TAIL_SECONDS, render_demo};
use crate::{Result, frames_for_bars};
use daw_engine::{Command, Engine, EngineConfig};
use daw_host::{OutputDevice, demo};
use std::io::Write;
use std::time::{Duration, Instant};

/// How often the control thread services the engine.
const CONTROL_INTERVAL: Duration = Duration::from_millis(10);
/// Most bars `--compare` records; one pass through the demo loop is enough.
const COMPARE_BARS: u32 = demo::BARS as u32;
/// Allowed per-sample difference between live and offline output. They run
/// identical code on identical input, so this should be exact.
const COMPARE_TOLERANCE: f32 = 0.0;

pub fn play(bars: u32, compare: bool) -> Result<()> {
    let device = OutputDevice::open_default()?;
    let config = device.engine_config();
    let channels = usize::from(config.channels);
    println!(
        "Playing {bars} bars at {} BPM on {} ({} Hz, {} channels). Ctrl+C to quit.",
        demo::TEMPO_BPM,
        device.name(),
        config.sample_rate,
        config.channels
    );
    println!("{}", demo::DESCRIPTION);

    let (mut engine, processor) = Engine::new(config);
    demo::load(&mut engine)?;
    // Queued now, so the very first callback starts playing from the top,
    // exactly like an offline render.
    engine.apply(Command::Play)?;

    // Recording for --compare: allocated up front, filled on the audio thread
    // without allocating, then handed back through a queue.
    let compare_frames = frames_for_bars(bars.min(COMPARE_BARS), config.sample_rate);
    let mut capture = compare.then(|| Vec::<f32>::with_capacity(compare_frames * channels));
    let (mut capture_tx, mut capture_rx) = rtrb::RingBuffer::new(1);
    let output = device.start_with_tap(processor, move |data| {
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
    })?;

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
        let beat = engine.position_beats().floor() as i64;
        if !stopped && last_beat != Some(beat) {
            print!("\r  bar {}  beat {}  ", beat / 4 + 1, beat % 4 + 1);
            std::io::stdout().flush()?;
            last_beat = Some(beat);
        }
        wait(CONTROL_INTERVAL);
    }
    println!("\rDone.                ");
    output.stats().report();
    drop(output);

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
