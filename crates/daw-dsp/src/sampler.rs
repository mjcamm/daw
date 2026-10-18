//! A one-shot sample player: each note plays the sample from the top to its
//! end, whatever the note's length, at a level set by its velocity. Hits
//! overlap, up to [`MAX_SAMPLER_VOICES`].
//!
//! The sample plays at its own rate: a voice steps through it by
//! `sample rate / output rate` each frame, with linear interpolation.

pub const MAX_SAMPLER_VOICES: usize = 16;
/// Voices cut short (stopping, stealing) fade out over this long, so they
/// end without a click.
const FADE_SECONDS: f32 = 0.005;

/// The audio a sampler plays: one or two channels at the file's own rate.
/// For a mono sample, `right` is the same slice as `left`.
#[derive(Debug, Clone, Copy)]
pub struct SampleView<'a> {
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub sample_rate: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    active: bool,
    /// Order the voice was started in, for stealing the oldest.
    started: u64,
    /// Position in the sample, in sample frames.
    position: f64,
    gain: f32,
    /// Fading out: the level falls from 1 to 0, then the voice stops.
    fading: bool,
    fade: f32,
}

#[derive(Debug, Clone)]
pub struct Sampler {
    sample_rate: f32,
    /// Linear output level.
    level: f32,
    voices: [Voice; MAX_SAMPLER_VOICES],
    started: u64,
}

impl Sampler {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            level: 1.0,
            voices: [Voice::default(); MAX_SAMPLER_VOICES],
            started: 0,
        }
    }

    /// Linear output level.
    pub fn set_level(&mut self, level: f32) {
        self.level = level;
    }

    /// Starts the sample from the top. `_note` and `_pitch` are ignored: a
    /// one-shot plays the same for every note. When every voice is busy,
    /// the oldest is taken.
    pub fn note_on(&mut self, _note: u32, _pitch: u8, velocity: u8) {
        let index = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, v)| v.started)
                    .map_or(0, |(i, _)| i)
            });
        self.started += 1;
        self.voices[index] = Voice {
            active: true,
            started: self.started,
            position: 0.0,
            gain: f32::from(velocity.min(127)) / 127.0,
            fading: false,
            fade: 1.0,
        };
    }

    /// One-shots ignore note ends.
    pub fn note_off(&mut self, _note: u32) {}

    /// Fades out everything sounding (on stop and locate).
    pub fn release_all(&mut self) {
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            voice.fading = true;
        }
    }

    /// Whether any voice is sounding.
    pub fn is_sounding(&self) -> bool {
        self.voices.iter().any(|v| v.active)
    }

    /// Renders into `left` and `right` (overwriting them). With no sample,
    /// it is silent and every voice stops.
    pub fn render(&mut self, sample: Option<SampleView<'_>>, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
        let Some(sample) = sample else {
            for voice in &mut self.voices {
                voice.active = false;
            }
            return;
        };
        let step = f64::from(sample.sample_rate / self.sample_rate);
        let fade_step = 1.0 / (FADE_SECONDS * self.sample_rate).max(1.0);
        let length = sample.left.len().min(sample.right.len());
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let index = voice.position as usize;
                if index >= length {
                    voice.active = false;
                    break;
                }
                if voice.fading {
                    voice.fade -= fade_step;
                    if voice.fade <= 0.0 {
                        voice.active = false;
                        break;
                    }
                }
                let frac = (voice.position - index as f64) as f32;
                let at = |channel: &[f32]| {
                    let a = channel[index];
                    let b = channel.get(index + 1).copied().unwrap_or(0.0);
                    a + (b - a) * frac
                };
                let gain = voice.gain * voice.fade * self.level;
                *l += at(sample.left) * gain;
                *r += at(sample.right) * gain;
                voice.position += step;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view<'a>(left: &'a [f32], right: &'a [f32], rate: f32) -> Option<SampleView<'a>> {
        Some(SampleView {
            left,
            right,
            sample_rate: rate,
        })
    }

    fn render(
        sampler: &mut Sampler,
        sample: Option<SampleView<'_>>,
        frames: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        sampler.render(sample, &mut left, &mut right);
        (left, right)
    }

    #[test]
    fn a_note_plays_the_sample_exactly_at_its_own_rate() {
        let data = [0.5, -0.25, 1.0, 0.75];
        let mut sampler = Sampler::new(48_000.0);
        sampler.note_on(0, 60, 127);
        let (left, right) = render(&mut sampler, view(&data, &data, 48_000.0), 6);
        assert_eq!(left, [0.5, -0.25, 1.0, 0.75, 0.0, 0.0]);
        assert_eq!(left, right, "mono plays in both sides");
        assert!(!sampler.is_sounding(), "it stops at the end");
    }

    #[test]
    fn stereo_samples_keep_their_sides() {
        let (l, r) = ([1.0, 1.0], [-1.0, 0.0]);
        let mut sampler = Sampler::new(48_000.0);
        sampler.note_on(0, 60, 127);
        let (left, right) = render(&mut sampler, view(&l, &r, 48_000.0), 2);
        assert_eq!(left, [1.0, 1.0]);
        assert_eq!(right, [-1.0, 0.0]);
    }

    #[test]
    fn a_sample_at_another_rate_plays_at_the_right_speed() {
        // Recorded at twice the output rate: every other frame, in half
        // the time.
        let data = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7];
        let mut sampler = Sampler::new(24_000.0);
        sampler.note_on(0, 60, 127);
        let (left, _) = render(&mut sampler, view(&data, &data, 48_000.0), 5);
        for (got, want) in left.iter().zip([0.0, 0.2, 0.4, 0.6, 0.0]) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
        // Half the output rate's speed interpolates between frames.
        let mut slow = Sampler::new(96_000.0);
        slow.note_on(0, 60, 127);
        let (left, _) = render(&mut slow, view(&data, &data, 48_000.0), 3);
        for (got, want) in left.iter().zip([0.0, 0.05, 0.1]) {
            assert!((got - want).abs() < 1e-6, "{left:?}");
        }
    }

    #[test]
    fn velocity_and_level_set_the_gain_and_hits_overlap() {
        let data = [1.0, 1.0, 1.0, 1.0];
        let mut sampler = Sampler::new(48_000.0);
        sampler.set_level(0.5);
        sampler.note_on(0, 60, 127);
        let (first, _) = render(&mut sampler, view(&data, &data, 48_000.0), 2);
        assert_eq!(first, [0.5, 0.5]);
        sampler.note_off(0);
        sampler.note_on(1, 60, 127);
        let (both, _) = render(&mut sampler, view(&data, &data, 48_000.0), 2);
        assert_eq!(both, [1.0, 1.0], "the first hit plays on past its note off");
    }

    #[test]
    fn releasing_fades_out_instead_of_cutting() {
        let data = vec![1.0; 48_000];
        let mut sampler = Sampler::new(48_000.0);
        sampler.note_on(0, 60, 127);
        sampler.release_all();
        let (left, _) = render(&mut sampler, view(&data, &data, 48_000.0), 480);
        assert!(left[0] > 0.9 && left[0] < 1.0, "starts near full");
        assert!(left.windows(2).all(|w| w[1] <= w[0]), "falls steadily");
        assert_eq!(left[479], 0.0, "silent within 10 ms");
        assert!(!sampler.is_sounding());
    }

    #[test]
    fn without_a_sample_it_is_silent() {
        let mut sampler = Sampler::new(48_000.0);
        sampler.note_on(0, 60, 127);
        let (left, right) = render(&mut sampler, None, 4);
        assert_eq!(left, [0.0; 4]);
        assert_eq!(right, [0.0; 4]);
        assert!(!sampler.is_sounding());
    }
}
