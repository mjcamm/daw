//! Oscillators.

/// A band-limited sawtooth using polyBLEP to suppress aliasing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Saw {
    phase: f32,
}

impl Saw {
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Next sample in -1..=1 for a frequency given as cycles per sample
    /// (`hz / sample_rate`), which must be below 0.5.
    #[inline]
    pub fn next_sample(&mut self, increment: f32) -> f32 {
        let value = 2.0 * self.phase - 1.0 - poly_blep(self.phase, increment);
        self.phase += increment;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value
    }
}

/// Polynomial correction around the waveform discontinuity at phase 0.
#[inline]
fn poly_blep(phase: f32, increment: f32) -> f32 {
    if phase < increment {
        let t = phase / increment;
        t + t - t * t - 1.0
    } else if phase > 1.0 - increment {
        let t = (phase - 1.0) / increment;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_in_range() {
        let mut saw = Saw::default();
        for _ in 0..48_000 {
            let s = saw.next_sample(440.0 / 48_000.0);
            assert!((-1.0..=1.0).contains(&s), "{s}");
        }
    }

    #[test]
    fn completes_one_cycle_per_period() {
        // The rising ramp crosses zero upwards once per cycle.
        let mut saw = Saw::default();
        let mut previous = saw.next_sample(440.0 / 48_000.0);
        let mut cycles = 0;
        for _ in 1..48_000 {
            let s = saw.next_sample(440.0 / 48_000.0);
            if previous < 0.0 && s >= 0.0 {
                cycles += 1;
            }
            previous = s;
        }
        assert!((439..=441).contains(&cycles), "{cycles}");
    }

    #[test]
    fn has_no_dc_offset() {
        let mut saw = Saw::default();
        let mean = (0..48_000)
            .map(|_| saw.next_sample(100.0 / 48_000.0))
            .sum::<f32>()
            / 48_000.0;
        assert!(mean.abs() < 0.01, "{mean}");
    }
}
