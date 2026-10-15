//! Offline rendering.

use crate::{Result, frames_for_bars};
use daw_engine::{Command, Engine, EngineConfig};
use daw_host::demo;
use std::path::Path;

/// Block size for offline renders. Any size gives identical output.
const BLOCK_FRAMES: usize = 512;
/// Release tails ring out for this long after the song stops.
pub const TAIL_SECONDS: f64 = 1.0;

/// Renders `frames` frames of the demo from the top, as interleaved samples.
pub fn render_demo(config: EngineConfig, frames: usize) -> Result<Vec<f32>> {
    let (mut engine, mut processor) = Engine::new(config);
    demo::load(&mut engine)?;
    engine.apply(Command::Play)?;
    let mut samples = vec![0.0; frames * usize::from(config.channels)];
    render_into(&mut engine, &mut processor, &mut samples);
    Ok(samples)
}

fn render_into(engine: &mut Engine, processor: &mut daw_engine::AudioProcessor, out: &mut [f32]) {
    let channels = processor.channels();
    for block in out.chunks_mut(BLOCK_FRAMES * channels) {
        engine.maintain();
        processor.process(block);
    }
}

pub fn render_to_file(path: &Path, bars: u32) -> Result<()> {
    let config = EngineConfig::default();
    let (mut engine, mut processor) = Engine::new(config);
    demo::load(&mut engine)?;
    engine.apply(Command::Play)?;

    let channels = usize::from(config.channels);
    let song = frames_for_bars(bars, config.sample_rate);
    let tail = (TAIL_SECONDS * f64::from(config.sample_rate)) as usize;
    let mut samples = vec![0.0; (song + tail) * channels];
    let (song_part, tail_part) = samples.split_at_mut(song * channels);
    render_into(&mut engine, &mut processor, song_part);
    engine.apply(Command::Stop)?;
    render_into(&mut engine, &mut processor, tail_part);

    let spec = hound::WavSpec {
        channels: config.channels,
        sample_rate: config.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in &samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;

    let peak = samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
    println!(
        "Rendered {bars} bars ({:.1} s) to {} — peak {:.1} dBFS",
        samples.len() as f64 / channels as f64 / f64::from(config.sample_rate),
        path.display(),
        20.0 * peak.log10()
    );
    Ok(())
}
