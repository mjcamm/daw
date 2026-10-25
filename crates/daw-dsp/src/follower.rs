//! Following a sound's loudness.

/// An envelope follower: tracks the peak of a stereo sound, rising over
/// `attack` and falling over `release` (one-pole, each time constant the
/// time to cover about 63% of a step), and reads it as a level in `0..=1`
/// between a floor and a ceiling in dB.
#[derive(Debug, Clone, Copy)]
pub struct Follower {
    /// The followed peak, as a linear amplitude.
    envelope: f32,
    attack: f32,
    release: f32,
    floor_db: f32,
    ceiling_db: f32,
}

impl Follower {
    /// Starts from silence. Times are in milliseconds.
    pub fn new(
        attack_ms: f32,
        release_ms: f32,
        floor_db: f32,
        ceiling_db: f32,
        sample_rate: f32,
    ) -> Self {
        let mut follower = Self {
            envelope: 0.0,
            attack: 0.0,
            release: 0.0,
            floor_db,
            ceiling_db,
        };
        follower.set(attack_ms, release_ms, floor_db, ceiling_db, sample_rate);
        follower
    }

    /// New settings, keeping where the envelope is.
    pub fn set(
        &mut self,
        attack_ms: f32,
        release_ms: f32,
        floor_db: f32,
        ceiling_db: f32,
        sample_rate: f32,
    ) {
        self.attack = coefficient(attack_ms, sample_rate);
        self.release = coefficient(release_ms, sample_rate);
        self.floor_db = floor_db;
        self.ceiling_db = ceiling_db;
    }

    /// Follows the sound through `left` and `right`.
    #[inline]
    pub fn process(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            let peak = l.abs().max(r.abs());
            // Broken numbers never reach the envelope.
            let peak = if peak.is_finite() { peak } else { 0.0 };
            let coefficient = if peak > self.envelope {
                self.attack
            } else {
                self.release
            };
            self.envelope = peak + (self.envelope - peak) * coefficient;
        }
    }

    /// Follows `frames` frames that all peak at `peak`: exactly what
    /// [`Self::process`] would do with them.
    pub fn process_held(&mut self, peak: f32, frames: u32) {
        let peak = if peak.is_finite() { peak.abs() } else { 0.0 };
        let coefficient = if peak > self.envelope {
            self.attack
        } else {
            self.release
        };
        self.envelope = peak + (self.envelope - peak) * coefficient.powi(frames as i32);
    }

    /// The loudness so far: 0 at or below the floor, 1 at or above the
    /// ceiling, even in dB between.
    pub fn level(&self) -> f32 {
        if self.envelope <= 0.0 {
            return 0.0;
        }
        let db = 20.0 * self.envelope.log10();
        let range = self.ceiling_db - self.floor_db;
        if range <= 0.0 {
            return if db >= self.ceiling_db { 1.0 } else { 0.0 };
        }
        ((db - self.floor_db) / range).clamp(0.0, 1.0)
    }
}

/// The one-pole coefficient for a time constant; 0 (instant) for no time.
fn coefficient(ms: f32, sample_rate: f32) -> f32 {
    let samples = ms * 0.001 * sample_rate;
    if samples > 0.0 {
        (-1.0 / samples).exp()
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn steady(follower: &mut Follower, amplitude: f32, frames: usize) {
        let samples = vec![amplitude; frames];
        follower.process(&samples, &samples);
    }

    #[test]
    fn silence_is_level_zero() {
        let mut f = Follower::new(5.0, 150.0, -48.0, 0.0, RATE);
        assert_eq!(f.level(), 0.0);
        steady(&mut f, 0.0, 1000);
        assert_eq!(f.level(), 0.0);
    }

    #[test]
    fn full_scale_reaches_the_ceiling_and_the_floor_is_zero() {
        let mut f = Follower::new(0.0, 0.0, -48.0, 0.0, RATE);
        steady(&mut f, 1.0, 1);
        assert_eq!(f.level(), 1.0);
        // Halfway in dB: -24 dB.
        steady(&mut f, 10f32.powf(-24.0 / 20.0), 1);
        assert!((f.level() - 0.5).abs() < 1e-4, "{}", f.level());
        steady(&mut f, 10f32.powf(-60.0 / 20.0), 1);
        assert_eq!(f.level(), 0.0, "below the floor");
    }

    #[test]
    fn attack_rises_about_63_percent_in_its_time() {
        let mut f = Follower::new(10.0, 1000.0, -200.0, 0.0, RATE);
        steady(&mut f, 1.0, (0.010 * RATE) as usize);
        assert!((f.envelope - 0.632).abs() < 0.01, "{}", f.envelope);
    }

    #[test]
    fn release_falls_about_63_percent_in_its_time() {
        let mut f = Follower::new(0.0, 100.0, -200.0, 0.0, RATE);
        steady(&mut f, 1.0, 1);
        steady(&mut f, 0.0, (0.100 * RATE) as usize);
        assert!((f.envelope - 0.368).abs() < 0.01, "{}", f.envelope);
    }

    #[test]
    fn follows_the_louder_side_and_ignores_broken_numbers() {
        let mut f = Follower::new(0.0, 0.0, -48.0, 0.0, RATE);
        f.process(&[0.0], &[-1.0]);
        assert_eq!(f.level(), 1.0);
        f.process(&[f32::NAN], &[f32::INFINITY]);
        assert_eq!(f.level(), 0.0);
    }

    #[test]
    fn a_held_level_follows_like_that_many_frames_of_it() {
        let mut held = Follower::new(10.0, 50.0, -48.0, 0.0, RATE);
        let mut framed = held;
        for (peak, frames) in [(0.8, 32), (0.8, 480), (0.1, 32), (0.0, 2000)] {
            held.process_held(peak, frames);
            steady(&mut framed, peak, frames as usize);
            assert!((held.envelope - framed.envelope).abs() < 1e-5);
        }
    }

    #[test]
    fn the_same_sound_in_any_pieces_gives_the_same_level() {
        let sound: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut whole = Follower::new(5.0, 150.0, -48.0, 0.0, RATE);
        whole.process(&sound, &sound);
        let mut pieces = Follower::new(5.0, 150.0, -48.0, 0.0, RATE);
        for piece in sound.chunks(37) {
            pieces.process(piece, piece);
        }
        assert_eq!(whole.level(), pieces.level());
    }
}
