//! Click-free parameter changes.

/// A value that ramps linearly to each new target over a fixed number of
/// samples, so sudden parameter changes do not click.
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_samples: u32,
}

impl Smoothed {
    /// Starts settled at `value`. A `ramp_samples` of 0 makes changes instant.
    pub fn new(value: f32, ramp_samples: u32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
            ramp_samples,
        }
    }

    /// Ramp length for a duration in seconds at a sample rate.
    pub fn ramp_for(seconds: f32, sample_rate: f32) -> u32 {
        (seconds * sample_rate).round().max(0.0) as u32
    }

    pub fn set_target(&mut self, target: f32) {
        if target == self.target {
            return;
        }
        self.target = target;
        if self.ramp_samples == 0 || !target.is_finite() || !self.current.is_finite() {
            self.current = target;
            self.remaining = 0;
        } else {
            self.remaining = self.ramp_samples;
            self.step = (target - self.current) / self.ramp_samples as f32;
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }

    /// Advances one sample and returns the value for it.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.step
            };
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_settled() {
        let mut s = Smoothed::new(0.5, 10);
        assert!(s.is_settled());
        assert_eq!(s.next_value(), 0.5);
    }

    #[test]
    fn reaches_target_exactly_after_the_ramp() {
        let mut s = Smoothed::new(0.0, 4);
        s.set_target(1.0);
        let values: Vec<f32> = (0..6).map(|_| s.next_value()).collect();
        assert_eq!(values, [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]);
        assert!(s.is_settled());
    }

    #[test]
    fn retargeting_mid_ramp_never_overshoots() {
        let mut s = Smoothed::new(0.0, 4);
        s.set_target(1.0);
        s.next_value();
        s.set_target(0.0);
        for _ in 0..10 {
            let v = s.next_value();
            assert!((0.0..=0.25).contains(&v), "{v}");
        }
        assert_eq!(s.next_value(), 0.0);
    }

    #[test]
    fn zero_ramp_is_instant() {
        let mut s = Smoothed::new(0.0, 0);
        s.set_target(1.0);
        assert_eq!(s.next_value(), 1.0);
    }

    #[test]
    fn ramp_length_from_seconds() {
        assert_eq!(Smoothed::ramp_for(0.01, 48_000.0), 480);
    }
}
