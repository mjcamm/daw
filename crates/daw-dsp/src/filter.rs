//! Filters.

use std::f32::consts::PI;

/// Highest usable cutoff as a fraction of the sample rate.
const MAX_CUTOFF_RATIO: f32 = 0.49;
/// Highest resonance; 1.0 would self-oscillate without bound.
const MAX_RESONANCE: f32 = 0.95;

/// Coefficients for [`Svf`], for a given cutoff and resonance. Compute once
/// and reuse while the settings are steady.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
}

impl SvfCoefficients {
    /// `resonance` runs from 0 (none) towards 1 (ringing).
    #[inline]
    pub fn new(cutoff_hz: f32, resonance: f32, sample_rate: f32) -> Self {
        let cutoff = cutoff_hz.clamp(10.0, sample_rate * MAX_CUTOFF_RATIO);
        let g = (PI * cutoff / sample_rate).tan();
        let k = 2.0 - 2.0 * resonance.clamp(0.0, MAX_RESONANCE);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

/// All three outputs of one [`Svf`] step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvfOutput {
    pub low: f32,
    /// Normalized to unity gain at the cutoff.
    pub band: f32,
    pub high: f32,
}

/// A state-variable filter (topology-preserving transform) with low-, band-
/// and high-pass outputs, stable under fast cutoff modulation.
#[derive(Debug, Clone, Copy, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Filters one sample, producing every output.
    #[inline]
    pub fn tick(&mut self, input: f32, c: &SvfCoefficients) -> SvfOutput {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOutput {
            low: v2,
            band: c.k * v1,
            high: input - c.k * v1 - v2,
        }
    }

    /// Low-passes one sample, computing coefficients on the spot (for
    /// cutoffs that change every sample).
    #[inline]
    pub fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32, sample_rate: f32) -> f32 {
        self.tick(
            input,
            &SvfCoefficients::new(cutoff_hz, resonance, sample_rate),
        )
        .low
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Steady-state peak amplitude of a sine through the filter.
    fn response(freq: f32, cutoff: f32) -> f32 {
        let mut filter = Svf::default();
        let mut peak = 0.0_f32;
        for n in 0..48_000 {
            let x = (2.0 * PI * freq * n as f32 / SR).sin();
            let y = filter.process(x, cutoff, 0.0, SR);
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn passes_low_frequencies() {
        assert!(response(100.0, 5_000.0) > 0.98);
    }

    #[test]
    fn attenuates_high_frequencies() {
        // Two octaves above cutoff, 12 dB/octave: roughly -24 dB.
        assert!(response(8_000.0, 2_000.0) < 0.1);
    }

    #[test]
    fn passes_dc_unchanged() {
        let mut filter = Svf::default();
        let mut y = 0.0;
        for _ in 0..10_000 {
            y = filter.process(1.0, 1_000.0, 0.5, SR);
        }
        assert!((y - 1.0).abs() < 1e-4, "{y}");
    }

    #[test]
    fn stays_stable_at_extreme_settings() {
        let mut filter = Svf::default();
        for n in 0..48_000 {
            let x = if n % 100 < 50 { 1.0 } else { -1.0 };
            let cutoff = if n % 2 == 0 { 20.0 } else { 30_000.0 };
            let y = filter.process(x, cutoff, 1.0, SR);
            assert!(y.is_finite() && y.abs() < 100.0, "{y}");
        }
    }
}
