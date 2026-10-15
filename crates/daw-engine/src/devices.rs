//! Built-in devices and their parameters.
//!
//! Every device describes its parameters the same way, so layers work
//! identically on all of them. Parameter indices are positions in these lists.

use daw_dsp::envelope::AdsrParams;
use daw_dsp::filter_effect::{FilterEffectParams, FilterMode};
use daw_dsp::gain::db_to_gain;
use daw_dsp::synth::SynthParams;
use daw_protocol::{DeviceRef, EffectKind, Scale};

/// Most parameters any built-in device has.
pub const MAX_DEVICE_PARAMS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamInfo {
    pub name: &'static str,
    /// Display unit: "Hz", "s", "dB", "oct", "pan", or "" for none.
    pub unit: &'static str,
    pub scale: Scale,
    /// Default in real units.
    pub default: f64,
    /// Option names for [`Scale::Choice`] parameters.
    pub choices: &'static [&'static str],
}

impl ParamInfo {
    const fn new(name: &'static str, unit: &'static str, scale: Scale, default: f64) -> Self {
        Self {
            name,
            unit,
            scale,
            default,
            choices: &[],
        }
    }

    pub fn default_normalized(&self) -> f64 {
        self.scale.to_normalized(self.default)
    }

    /// A real-unit value formatted for display.
    pub fn format(&self, plain: f64) -> String {
        daw_protocol::format_param_value(self.unit, self.choices, plain)
    }
}

const FREQUENCY: Scale = Scale::Logarithmic {
    min: 20.0,
    max: 20_000.0,
};
const UNIT: Scale = Scale::Linear { min: 0.0, max: 1.0 };
const TIME: Scale = Scale::Logarithmic {
    min: 0.001,
    max: 10.0,
};

pub mod instrument {
    pub const CUTOFF: u32 = 0;
    pub const RESONANCE: u32 = 1;
    pub const ENV_AMOUNT: u32 = 2;
    pub const ATTACK: u32 = 3;
    pub const DECAY: u32 = 4;
    pub const SUSTAIN: u32 = 5;
    pub const RELEASE: u32 = 6;
    pub const LEVEL: u32 = 7;
}

/// The built-in synth. Defaults match [`SynthParams::default`].
pub static INSTRUMENT_PARAMS: [ParamInfo; 8] = [
    ParamInfo::new("Cutoff", "Hz", FREQUENCY, 800.0),
    ParamInfo::new("Resonance", "", UNIT, 0.3),
    ParamInfo::new(
        "Filter envelope",
        "oct",
        Scale::Linear { min: 0.0, max: 8.0 },
        3.0,
    ),
    ParamInfo::new("Attack", "s", TIME, 0.003),
    ParamInfo::new("Decay", "s", TIME, 0.3),
    ParamInfo::new("Sustain", "", UNIT, 0.5),
    ParamInfo::new("Release", "s", TIME, 0.15),
    // 20·log10(0.2): a linear voice gain of 0.2.
    ParamInfo::new(
        "Level",
        "dB",
        Scale::Decibels {
            min_db: -60.0,
            max_db: 0.0,
        },
        -13.979_400_086_720_376,
    ),
];

pub mod channel {
    pub const VOLUME: u32 = 0;
    pub const PAN: u32 = 1;
}

pub static CHANNEL_PARAMS: [ParamInfo; 2] = [
    ParamInfo::new(
        "Volume",
        "dB",
        Scale::Decibels {
            min_db: -60.0,
            max_db: 12.0,
        },
        0.0,
    ),
    ParamInfo::new(
        "Pan",
        "pan",
        Scale::Linear {
            min: -1.0,
            max: 1.0,
        },
        0.0,
    ),
];

pub mod filter {
    pub const CUTOFF: u32 = 0;
    pub const RESONANCE: u32 = 1;
    pub const MODE: u32 = 2;
}

pub static FILTER_PARAMS: [ParamInfo; 3] = [
    ParamInfo::new("Cutoff", "Hz", FREQUENCY, 20_000.0),
    ParamInfo::new("Resonance", "", UNIT, 0.0),
    ParamInfo {
        choices: &["Low-pass", "High-pass", "Band-pass"],
        ..ParamInfo::new("Mode", "", Scale::Choice { count: 3 }, 0.0)
    },
];

pub fn effect_params(kind: EffectKind) -> &'static [ParamInfo] {
    match kind {
        EffectKind::Filter => &FILTER_PARAMS,
    }
}

pub fn effect_name(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Filter => "Filter",
    }
}

/// Parameters of a device that is not an effect.
pub fn fixed_device_params(device: DeviceRef) -> Option<&'static [ParamInfo]> {
    match device {
        DeviceRef::Instrument => Some(&INSTRUMENT_PARAMS),
        DeviceRef::Channel => Some(&CHANNEL_PARAMS),
        DeviceRef::Effect(_) => None,
    }
}

/// Synth settings from real-unit values in [`INSTRUMENT_PARAMS`] order.
pub fn synth_params(plain: &[f64]) -> SynthParams {
    let get = |i: u32| plain[i as usize] as f32;
    SynthParams {
        cutoff_hz: get(instrument::CUTOFF),
        resonance: get(instrument::RESONANCE),
        filter_env_octaves: get(instrument::ENV_AMOUNT),
        envelope: AdsrParams {
            attack_s: get(instrument::ATTACK),
            decay_s: get(instrument::DECAY),
            sustain: get(instrument::SUSTAIN),
            release_s: get(instrument::RELEASE),
        },
        gain: db_to_gain(get(instrument::LEVEL)),
    }
}

/// Filter settings from real-unit values in [`FILTER_PARAMS`] order.
pub fn filter_params(plain: &[f64]) -> FilterEffectParams {
    FilterEffectParams {
        cutoff_hz: plain[filter::CUTOFF as usize] as f32,
        resonance: plain[filter::RESONANCE as usize] as f32,
        mode: match plain[filter::MODE as usize] as u32 {
            1 => FilterMode::HighPass,
            2 => FilterMode::BandPass,
            _ => FilterMode::LowPass,
        },
    }
}

/// Linear gain and pan position from real-unit values in [`CHANNEL_PARAMS`]
/// order.
pub fn channel_settings(plain: &[f64]) -> (f32, f32) {
    (
        db_to_gain(plain[channel::VOLUME as usize] as f32),
        plain[channel::PAN as usize] as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(params: &[ParamInfo]) -> Vec<f64> {
        params
            .iter()
            .map(|p| p.scale.to_plain(p.default_normalized()))
            .collect()
    }

    #[test]
    fn instrument_defaults_match_the_synth_defaults() {
        let from_params = synth_params(&defaults(&INSTRUMENT_PARAMS));
        let expected = SynthParams::default();
        let close = |a: f32, b: f32| (a - b).abs() <= b.abs() * 1e-5;
        assert!(close(from_params.cutoff_hz, expected.cutoff_hz));
        assert!(close(from_params.resonance, expected.resonance));
        assert!(close(
            from_params.filter_env_octaves,
            expected.filter_env_octaves
        ));
        assert!(close(
            from_params.envelope.attack_s,
            expected.envelope.attack_s
        ));
        assert!(close(
            from_params.envelope.decay_s,
            expected.envelope.decay_s
        ));
        assert!(close(
            from_params.envelope.sustain,
            expected.envelope.sustain
        ));
        assert!(close(
            from_params.envelope.release_s,
            expected.envelope.release_s
        ));
        assert!(close(from_params.gain, expected.gain));
    }

    #[test]
    fn filter_defaults_are_an_open_low_pass() {
        let params = filter_params(&defaults(&FILTER_PARAMS));
        assert_eq!(params.mode, FilterMode::LowPass);
        assert!((params.cutoff_hz - 20_000.0).abs() < 0.1);
    }

    #[test]
    fn channel_defaults_are_unity_and_centre() {
        let (gain, pan) = channel_settings(&defaults(&CHANNEL_PARAMS));
        assert!((gain - 1.0).abs() < 1e-6);
        assert!(pan.abs() < 1e-6);
    }

    #[test]
    fn devices_fit_the_parameter_scratch_space() {
        for params in [&INSTRUMENT_PARAMS[..], &CHANNEL_PARAMS, &FILTER_PARAMS] {
            assert!(params.len() <= MAX_DEVICE_PARAMS);
        }
    }

    #[test]
    fn values_format_for_display() {
        let cutoff = &INSTRUMENT_PARAMS[instrument::CUTOFF as usize];
        assert_eq!(cutoff.format(800.0), "800 Hz");
        assert_eq!(cutoff.format(2_500.0), "2.50 kHz");
        let attack = &INSTRUMENT_PARAMS[instrument::ATTACK as usize];
        assert_eq!(attack.format(0.003), "3 ms");
        let volume = &CHANNEL_PARAMS[channel::VOLUME as usize];
        assert_eq!(volume.format(f64::NEG_INFINITY), "-inf dB");
        assert_eq!(volume.format(-6.0), "-6.0 dB");
        let pan = &CHANNEL_PARAMS[channel::PAN as usize];
        assert_eq!(pan.format(0.0), "C");
        assert_eq!(pan.format(-0.5), "L50");
        let mode = &FILTER_PARAMS[filter::MODE as usize];
        assert_eq!(mode.format(1.0), "High-pass");
    }
}
