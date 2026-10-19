//! Real-time, pitch-preserving time-stretching.
//!
//! The `stem` stretcher: phase-gradient heap integration with group delay
//! scaled by the stretch ratio and a transient lock (see
//! `docs/design/stretcher.md`). [`StemStream`] runs it in real time from a
//! clip held in memory: no latency, no allocation in `process`, output that
//! does not depend on how the host slices it into blocks, and bit-exact
//! pass-through at ratio 1.
//!
//! Brought over from the user's `audioperformerv2` project, where it was
//! built and scored against a test gym; the gym and its baseline stretchers
//! stayed there.

mod fastmath;
mod stem;
mod stream;

pub use stem::Stem;
pub use stream::{Cue, StemStream};

/// Output length for an input of `input_len` samples at `ratio` (output
/// length / input length).
pub fn out_len(input_len: usize, ratio: f64) -> usize {
    (input_len as f64 * ratio).round() as usize
}

/// Periodic Hann window.
pub(crate) fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .map(|w| w as f32)
        .collect()
}
