//! Headless rendering and audio regression testing.
//!
//! Linking this crate installs [`daw_rt::GuardedAllocator`], so every test that
//! uses it fails loudly if the audio side allocates or frees memory.
//!
//! ```ignore
//! let (mut engine, mut processor) = Engine::new(EngineConfig::default());
//! let rendered = render(&mut engine, &mut processor, 4800, VARIED_BLOCKS);
//! daw_testkit::assert_reference!("my-test", &rendered);
//! ```

mod reference;

pub use reference::{BLESS_ENV, DEFAULT_TOLERANCE, check_reference};

use daw_engine::{AudioProcessor, Engine};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

/// A deliberately awkward mix of block sizes, including tiny and odd ones, to
/// catch bugs at block boundaries.
pub const VARIED_BLOCKS: &[usize] = &[1, 7, 64, 128, 333, 512, 2, 1024];

/// Rendered interleaved audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Rendered {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels)
    }

    /// Largest absolute sample value.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0, |peak, s| peak.max(s.abs()))
    }

    pub fn is_silent(&self) -> bool {
        self.samples.iter().all(|&s| s == 0.0)
    }
}

/// Renders `frames` frames offline, cycling through `block_frames` for block
/// sizes and calling [`Engine::maintain`] between blocks as a host would.
///
/// # Panics
///
/// If `block_frames` is empty or contains a zero, or if the realtime guard is
/// not installed.
pub fn render(
    engine: &mut Engine,
    processor: &mut AudioProcessor,
    frames: usize,
    block_frames: &[usize],
) -> Rendered {
    assert!(
        daw_rt::is_installed(),
        "the realtime allocation guard is not installed"
    );
    assert!(
        !block_frames.is_empty() && !block_frames.contains(&0),
        "block sizes must be non-empty and non-zero"
    );
    let config = engine.config();
    let channels = usize::from(config.channels);
    let mut samples = vec![0.0; frames * channels];
    let mut start = 0;
    for &block in block_frames.iter().cycle() {
        if start == frames {
            break;
        }
        let end = (start + block).min(frames);
        engine.maintain();
        processor.process(&mut samples[start * channels..end * channels]);
        start = end;
    }
    engine.maintain();
    Rendered {
        sample_rate: config.sample_rate,
        channels: config.channels,
        samples,
    }
}

/// Asserts that `rendered` matches the reference recording `name`, stored in
/// the calling crate's `tests/references/` directory.
///
/// Set `DAW_BLESS=1` to write or update the reference instead.
#[macro_export]
macro_rules! assert_reference {
    ($name:expr, $rendered:expr) => {
        $crate::assert_reference!($name, $rendered, $crate::DEFAULT_TOLERANCE)
    };
    ($name:expr, $rendered:expr, $tolerance:expr) => {{
        let path = ::std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("references")
            .join(format!("{}.wav", $name));
        if let Err(message) = $crate::check_reference(&path, $rendered, $tolerance) {
            panic!("{message}");
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Command, EngineConfig};

    #[test]
    fn renders_exactly_the_requested_frames() {
        let (mut engine, mut processor) = Engine::new(EngineConfig::default());
        engine.apply(Command::Play).unwrap();
        let rendered = render(&mut engine, &mut processor, 1000, VARIED_BLOCKS);
        assert_eq!(rendered.frames(), 1000);
        assert_eq!(rendered.channels, 2);
        assert_eq!(engine.snapshot().position_beats, 1000.0 / 24_000.0);
    }

    #[test]
    #[should_panic(expected = "block sizes")]
    fn zero_block_size_is_rejected() {
        let (mut engine, mut processor) = Engine::new(EngineConfig::default());
        render(&mut engine, &mut processor, 10, &[0]);
    }
}
