//! The metronome's click: one short generated sound per beat, a choice of
//! tones, brighter and louder on the first beat of a bar. No samples, no
//! allocation; every click starts from the same state, so it sounds the
//! same wherever it lands.

use std::f32::consts::TAU;

/// The click's sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickTone {
    /// A clean sine beep.
    Beep,
    /// A short, woody knock: a sine dropping in pitch with a hollow overtone.
    Wood,
    /// A bright tick, like a closed hi-hat.
    Tick,
    /// Two clanging partials, like a cowbell.
    Cowbell,
}

impl ClickTone {
    /// How long a click of this tone lasts, in seconds.
    fn length(self) -> f32 {
        match self {
            Self::Beep => 0.06,
            Self::Wood => 0.05,
            Self::Tick => 0.04,
            Self::Cowbell => 0.12,
        }
    }

    /// How quickly it dies away: the time for it to fall to about a third.
    fn decay(self) -> f32 {
        match self {
            Self::Beep => 0.014,
            Self::Wood => 0.009,
            Self::Tick => 0.007,
            Self::Cowbell => 0.03,
        }
    }

    /// Its main pitch in Hz, before the pitch setting.
    fn hz(self) -> f32 {
        match self {
            Self::Beep => 1000.,
            Self::Wood => 900.,
            Self::Tick => 7000.,
            Self::Cowbell => 560.,
        }
    }
}

/// The accent (a bar's first beat) sounds this much higher.
const ACCENT_PITCH: f32 = 1.5;
/// Beats other than the accent sound this much quieter.
const BEAT_LEVEL: f32 = 0.6;
/// How long a click takes to rise, in seconds: no snap at its start.
const ATTACK: f32 = 0.0005;

/// One click at a time; a new one starts over the last.
#[derive(Debug, Clone)]
pub struct ClickVoice {
    sample_rate: f32,
    tone: ClickTone,
    /// Frames since the click began, while it sounds.
    frame: u32,
    length: u32,
    sounding: bool,
    hz: f32,
    gain: f32,
    phase: f32,
    phase2: f32,
    noise: u32,
    last_noise: f32,
}

impl ClickVoice {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            tone: ClickTone::Beep,
            frame: 0,
            length: 0,
            sounding: false,
            hz: 0.,
            gain: 0.,
            phase: 0.,
            phase2: 0.,
            noise: 0,
            last_noise: 0.,
        }
    }

    /// Starts a click: `accent` for a bar's first beat (higher and louder),
    /// `pitch` as a frequency ratio (1 is the tone's own), `gain` its level.
    pub fn trigger(&mut self, tone: ClickTone, accent: bool, pitch: f32, gain: f32) {
        self.tone = tone;
        self.frame = 0;
        self.length = (tone.length() * self.sample_rate) as u32;
        self.sounding = true;
        self.hz = tone.hz() * pitch * if accent { ACCENT_PITCH } else { 1. };
        self.gain = gain * if accent { 1. } else { BEAT_LEVEL };
        self.phase = 0.;
        self.phase2 = 0.;
        // The same noise every click, so ticks sound alike.
        self.noise = 0x9E37_79B9;
        self.last_noise = 0.;
    }

    pub fn is_sounding(&self) -> bool {
        self.sounding
    }

    /// The next sample: silence once the click has died away.
    pub fn next_sample(&mut self) -> f32 {
        if !self.sounding {
            return 0.;
        }
        let t = self.frame as f32 / self.sample_rate;
        let attack = (t / ATTACK).min(1.);
        // Falls to nothing at its end, so it never stops with a click.
        let tail = 1. - self.frame as f32 / self.length.max(1) as f32;
        let envelope = attack * (-t / self.tone.decay()).exp() * tail;
        let step = self.hz / self.sample_rate;
        let raw = match self.tone {
            ClickTone::Beep => (TAU * self.phase).sin(),
            ClickTone::Wood => {
                // Drops an octave in its first few milliseconds.
                let drop = 1. + (-t / 0.003).exp();
                self.phase2 = (self.phase2 + step * 2.76 * drop).fract();
                (TAU * self.phase).sin() * 0.8 + (TAU * self.phase2).sin() * 0.3
            }
            ClickTone::Tick => {
                // White noise, differenced to keep only its bright part,
                // with a little of the tone's pitch on top.
                self.noise ^= self.noise << 13;
                self.noise ^= self.noise >> 17;
                self.noise ^= self.noise << 5;
                let white = (self.noise as f32 / u32::MAX as f32) * 2. - 1.;
                let bright = white - self.last_noise;
                self.last_noise = white;
                bright * 0.45 + (TAU * self.phase).sin() * 0.25
            }
            ClickTone::Cowbell => {
                self.phase2 = (self.phase2 + step * 1.508).fract();
                let a = (TAU * self.phase).sin();
                let b = (TAU * self.phase2).sin();
                // A touch of drive for the clang.
                ((a + b) * 1.4).tanh() * 0.7
            }
        };
        let step = match self.tone {
            ClickTone::Wood => step * (1. + (-t / 0.003).exp()),
            _ => step,
        };
        self.phase = (self.phase + step).fract();
        self.frame += 1;
        if self.frame >= self.length {
            self.sounding = false;
        }
        raw * envelope * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.;
    const TONES: [ClickTone; 4] = [
        ClickTone::Beep,
        ClickTone::Wood,
        ClickTone::Tick,
        ClickTone::Cowbell,
    ];

    fn render(tone: ClickTone, accent: bool, pitch: f32, frames: usize) -> Vec<f32> {
        let mut voice = ClickVoice::new(RATE);
        voice.trigger(tone, accent, pitch, 1.);
        (0..frames).map(|_| voice.next_sample()).collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0., |m, s| m.max(s.abs()))
    }

    /// Sign changes in the first `frames`: twice the pitch, near enough.
    fn crossings(samples: &[f32], frames: usize) -> usize {
        samples[..frames]
            .windows(2)
            .filter(|w| (w[0] < 0.) != (w[1] < 0.))
            .count()
    }

    #[test]
    fn silent_until_triggered() {
        let mut voice = ClickVoice::new(RATE);
        assert!((0..100).all(|_| voice.next_sample() == 0.));
        assert!(!voice.is_sounding());
    }

    #[test]
    fn every_tone_clicks_then_falls_silent() {
        for tone in TONES {
            let samples = render(tone, false, 1., 9_600);
            let length = (tone.length() * RATE) as usize;
            assert!(peak(&samples[..length]) > 0.05, "{tone:?} is heard");
            assert!(peak(&samples) <= 1., "{tone:?} stays in range");
            assert!(
                samples[length..].iter().all(|s| *s == 0.),
                "{tone:?} ends silent"
            );
            // It never starts with a jump.
            assert!(samples[0].abs() < 0.01, "{tone:?} starts soft");
        }
    }

    #[test]
    fn the_accent_is_louder_and_higher() {
        for tone in [ClickTone::Beep, ClickTone::Cowbell] {
            let beat = render(tone, false, 1., 2_000);
            let accent = render(tone, true, 1., 2_000);
            assert!(peak(&accent) > peak(&beat), "{tone:?} louder");
            assert!(
                crossings(&accent, 1_000) > crossings(&beat, 1_000),
                "{tone:?} higher"
            );
        }
    }

    #[test]
    fn the_pitch_setting_moves_the_tone() {
        let low = render(ClickTone::Beep, false, 1., 1_000);
        let high = render(ClickTone::Beep, false, 2., 1_000);
        let (low, high) = (crossings(&low, 960), crossings(&high, 960));
        // 1 kHz over 20 ms: about 40 crossings; an octave up, about 80.
        assert!((38..=42).contains(&low), "{low}");
        assert!((78..=82).contains(&high), "{high}");
    }

    #[test]
    fn every_click_of_a_tone_sounds_the_same() {
        for tone in TONES {
            let mut voice = ClickVoice::new(RATE);
            voice.trigger(tone, false, 1., 1.);
            let first: Vec<f32> = (0..500).map(|_| voice.next_sample()).collect();
            voice.trigger(tone, false, 1., 1.);
            let second: Vec<f32> = (0..500).map(|_| voice.next_sample()).collect();
            assert_eq!(first, second, "{tone:?}");
        }
    }

    #[test]
    fn gain_scales_the_click() {
        let mut voice = ClickVoice::new(RATE);
        voice.trigger(ClickTone::Beep, true, 1., 0.5);
        let half: Vec<f32> = (0..500).map(|_| voice.next_sample()).collect();
        let full = render(ClickTone::Beep, true, 1., 500);
        assert!((peak(&half) * 2. - peak(&full)).abs() < 1e-4);
    }
}
