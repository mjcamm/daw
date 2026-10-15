//! A stereo multi-mode resonant filter effect.

use crate::filter::{Svf, SvfCoefficients};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    LowPass,
    HighPass,
    BandPass,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterEffectParams {
    pub cutoff_hz: f32,
    /// 0..1.
    pub resonance: f32,
    pub mode: FilterMode,
}

impl Default for FilterEffectParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FilterEffect {
    sample_rate: f32,
    params: FilterEffectParams,
    coefficients: SvfCoefficients,
    left: Svf,
    right: Svf,
}

impl FilterEffect {
    pub fn new(sample_rate: f32) -> Self {
        let params = FilterEffectParams::default();
        Self {
            sample_rate,
            params,
            coefficients: coefficients(&params, sample_rate),
            left: Svf::default(),
            right: Svf::default(),
        }
    }

    pub fn set_params(&mut self, params: FilterEffectParams) {
        if params != self.params {
            self.params = params;
            self.coefficients = coefficients(&params, self.sample_rate);
        }
    }

    /// Filters a stereo block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let mode = self.params.mode;
        let c = self.coefficients;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = pick(self.left.tick(*l, &c), mode);
            *r = pick(self.right.tick(*r, &c), mode);
        }
    }
}

fn coefficients(params: &FilterEffectParams, sample_rate: f32) -> SvfCoefficients {
    SvfCoefficients::new(params.cutoff_hz, params.resonance, sample_rate)
}

#[inline]
fn pick(out: crate::filter::SvfOutput, mode: FilterMode) -> f32 {
    match mode {
        FilterMode::LowPass => out.low,
        FilterMode::HighPass => out.high,
        FilterMode::BandPass => out.band,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    /// Steady-state amplitude of a sine through the effect, on both channels.
    /// Measured from RMS: sample peaks miss the true peak at high frequencies.
    fn response(freq: f32, params: FilterEffectParams) -> (f32, f32) {
        let mut effect = FilterEffect::new(SR);
        effect.set_params(params);
        let input: Vec<f32> = (0..48_000)
            .map(|n| (2.0 * PI * freq * n as f32 / SR).sin())
            .collect();
        let (mut left, mut right) = (input.clone(), input);
        effect.process(&mut left, &mut right);
        let amplitude = |s: &[f32]| {
            let tail = &s[24_000..];
            (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt()
                * std::f32::consts::SQRT_2
        };
        (amplitude(&left), amplitude(&right))
    }

    fn with(cutoff_hz: f32, mode: FilterMode) -> FilterEffectParams {
        FilterEffectParams {
            cutoff_hz,
            resonance: 0.0,
            mode,
        }
    }

    #[test]
    fn default_is_nearly_transparent_for_audible_content() {
        let (l, r) = response(1_000.0, FilterEffectParams::default());
        assert!((l - 1.0).abs() < 0.01 && (r - 1.0).abs() < 0.01, "{l} {r}");
    }

    #[test]
    fn low_pass_keeps_lows_and_cuts_highs() {
        assert!(response(100.0, with(2_000.0, FilterMode::LowPass)).0 > 0.98);
        assert!(response(8_000.0, with(2_000.0, FilterMode::LowPass)).0 < 0.1);
    }

    #[test]
    fn high_pass_keeps_highs_and_cuts_lows() {
        assert!(response(8_000.0, with(500.0, FilterMode::HighPass)).0 > 0.98);
        assert!(response(100.0, with(2_000.0, FilterMode::HighPass)).0 < 0.01);
    }

    #[test]
    fn band_pass_keeps_only_around_the_cutoff() {
        let params = with(1_000.0, FilterMode::BandPass);
        let at = response(1_000.0, params).0;
        assert!(at > 0.9, "{at}");
        assert!(response(100.0, params).0 < 0.25);
        assert!(response(10_000.0, params).0 < 0.25);
    }

    #[test]
    fn channels_are_filtered_independently_and_identically() {
        let (l, r) = response(3_000.0, with(1_000.0, FilterMode::LowPass));
        assert_eq!(l, r);
    }
}
