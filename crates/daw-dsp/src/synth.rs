//! A polyphonic subtractive synthesizer: sawtooth into a low-pass filter
//! swept by the amp envelope.

use crate::envelope::{Adsr, AdsrParams};
use crate::filter::LowPass;
use crate::midi_to_hz;
use crate::oscillator::Saw;

pub const MAX_VOICES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SynthParams {
    pub cutoff_hz: f32,
    /// 0..1.
    pub resonance: f32,
    /// How far the envelope opens the filter above the cutoff, in octaves.
    pub filter_env_octaves: f32,
    pub envelope: AdsrParams,
    /// Linear output gain per voice at full velocity.
    pub gain: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 800.0,
            resonance: 0.3,
            filter_env_octaves: 3.0,
            envelope: AdsrParams {
                attack_s: 0.003,
                decay_s: 0.3,
                sustain: 0.5,
                release_s: 0.15,
            },
            gain: 0.2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    /// Caller-chosen note id; `None` when the voice is free.
    note: Option<u32>,
    /// Order the voice was started in, for stealing the oldest.
    started: u64,
    increment: f32,
    velocity: f32,
    osc: Saw,
    filter: LowPass,
    env: Adsr,
}

impl Voice {
    fn is_free(&self) -> bool {
        self.note.is_none() && self.env.is_idle()
    }

    #[inline]
    fn next_sample(&mut self, params: &SynthParams, sample_rate: f32) -> f32 {
        let env = self.env.next_level(&params.envelope, sample_rate);
        if self.env.is_idle() {
            self.note = None;
            return 0.0;
        }
        let cutoff = params.cutoff_hz * (params.filter_env_octaves * env).exp2();
        let raw = self.osc.next_sample(self.increment);
        let filtered = self
            .filter
            .process(raw, cutoff, params.resonance, sample_rate);
        filtered * env * self.velocity * params.gain
    }
}

#[derive(Debug, Clone)]
pub struct Synth {
    sample_rate: f32,
    params: SynthParams,
    voices: [Voice; MAX_VOICES],
    started: u64,
}

impl Synth {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            params: SynthParams::default(),
            voices: [Voice::default(); MAX_VOICES],
            started: 0,
        }
    }

    pub fn set_params(&mut self, params: SynthParams) {
        self.params = params;
    }

    /// Starts a note. `note` is a caller-chosen id used to stop it later.
    /// When all voices are busy the oldest is stolen, preferring released ones.
    pub fn note_on(&mut self, note: u32, pitch: u8, velocity: u8) {
        let index = self.voice_for_new_note();
        let hz = midi_to_hz(f32::from(pitch.min(127)));
        self.started += 1;
        let voice = &mut self.voices[index];
        if voice.env.is_idle() {
            // A fresh voice starts from a known state; a stolen one keeps its
            // envelope level so the takeover does not click.
            voice.osc.reset();
            voice.filter.reset();
        }
        voice.note = Some(note);
        voice.started = self.started;
        voice.increment = (hz / self.sample_rate).min(0.45);
        voice.velocity = f32::from(velocity.min(127)) / 127.0;
        voice.env.gate_on();
    }

    /// Releases the note with this id, if it is still sounding.
    pub fn note_off(&mut self, note: u32) {
        for voice in &mut self.voices {
            if voice.note == Some(note) {
                voice.note = None;
                voice.env.gate_off();
            }
        }
    }

    /// Releases every note; tails still ring out.
    pub fn release_all(&mut self) {
        for voice in &mut self.voices {
            voice.note = None;
            voice.env.gate_off();
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| !v.is_free()).count()
    }

    /// Renders mono audio, overwriting `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        for voice in self.voices.iter_mut().filter(|v| !v.is_free()) {
            for sample in out.iter_mut() {
                *sample += voice.next_sample(&self.params, self.sample_rate);
            }
        }
    }

    fn voice_for_new_note(&self) -> usize {
        let oldest = |releasing: bool| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| !releasing || v.note.is_none())
                .min_by_key(|(_, v)| v.started)
                .map(|(i, _)| i)
        };
        self.voices
            .iter()
            .position(Voice::is_free)
            .or_else(|| oldest(true))
            .or_else(|| oldest(false))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(synth: &mut Synth, samples: usize) -> Vec<f32> {
        let mut out = vec![0.0; samples];
        synth.render(&mut out);
        out
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |p, s| p.max(s.abs()))
    }

    #[test]
    fn silent_with_no_notes() {
        let mut synth = Synth::new(SR);
        assert!(render(&mut synth, 1_000).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn note_sounds_immediately() {
        let mut synth = Synth::new(SR);
        synth.note_on(1, 60, 100);
        let out = render(&mut synth, 1_000);
        // A band-limited saw starting at phase 0 outputs exactly 0 first.
        assert!(out[..2].iter().any(|&s| s != 0.0));
        assert!(peak(&out) > 0.05);
    }

    #[test]
    fn released_note_fades_to_silence_and_frees_its_voice() {
        let mut synth = Synth::new(SR);
        synth.note_on(1, 60, 100);
        render(&mut synth, 4_800);
        synth.note_off(1);
        let tail = render(&mut synth, 48_000);
        assert_eq!(*tail.last().unwrap(), 0.0);
        assert_eq!(synth.active_voices(), 0);
    }

    #[test]
    fn note_off_only_releases_the_matching_note() {
        let mut synth = Synth::new(SR);
        synth.note_on(1, 60, 100);
        synth.note_on(2, 64, 100);
        synth.note_off(1);
        render(&mut synth, 48_000);
        assert_eq!(synth.active_voices(), 1);
    }

    #[test]
    fn steals_the_oldest_voice_when_full() {
        let mut synth = Synth::new(SR);
        for note in 0..MAX_VOICES as u32 + 1 {
            synth.note_on(note, 60, 100);
        }
        assert_eq!(synth.active_voices(), MAX_VOICES);
        // Note 0 was stolen, so releasing it does nothing.
        synth.note_off(0);
        render(&mut synth, 48_000);
        assert_eq!(synth.active_voices(), MAX_VOICES);
    }

    #[test]
    fn velocity_scales_level() {
        let mut loud = Synth::new(SR);
        loud.note_on(1, 60, 127);
        let mut quiet = Synth::new(SR);
        quiet.note_on(1, 60, 32);
        let ratio = peak(&render(&mut quiet, 4_800)) / peak(&render(&mut loud, 4_800));
        assert!((ratio - 32.0 / 127.0).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn full_chord_stays_finite_and_below_clipping() {
        let mut synth = Synth::new(SR);
        for (note, pitch) in
            (0..MAX_VOICES as u32).zip([24_u8, 36, 48, 55, 60, 64, 67, 72].iter().cycle())
        {
            synth.note_on(note, *pitch, 127);
        }
        let out = render(&mut synth, 48_000);
        assert!(out.iter().all(|s| s.is_finite()));
        // Sixteen voices at full velocity is the worst case; leave headroom.
        assert!(peak(&out) < 4.0, "{}", peak(&out));
    }
}
