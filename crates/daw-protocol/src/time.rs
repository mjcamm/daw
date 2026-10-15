//! Musical time.

use std::ops::{Add, Sub};

/// Resolution of musical time: ticks in one beat (a quarter note).
pub const TICKS_PER_BEAT: i64 = 960;

/// A musical position or duration, in ticks. Song positions start at zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(pub i64);

impl Ticks {
    pub const ZERO: Self = Self(0);

    pub const fn from_beats(beats: i64) -> Self {
        Self(beats * TICKS_PER_BEAT)
    }

    /// A whole number of bars in 4/4.
    pub const fn from_bars(bars: i64) -> Self {
        Self::from_beats(bars * 4)
    }

    /// The nearest tick to a fractional number of beats.
    pub fn from_beats_f64(beats: f64) -> Self {
        Self((beats * TICKS_PER_BEAT as f64).round() as i64)
    }

    pub fn as_beats(self) -> f64 {
        self.0 as f64 / TICKS_PER_BEAT as f64
    }
}

impl Add for Ticks {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Ticks {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_and_bars_convert_to_ticks() {
        assert_eq!(Ticks::from_beats(1), Ticks(960));
        assert_eq!(Ticks::from_bars(2), Ticks(7680));
        assert_eq!(Ticks(480).as_beats(), 0.5);
    }

    #[test]
    fn fractional_beats_round_to_nearest_tick() {
        assert_eq!(Ticks::from_beats_f64(0.25), Ticks(240));
        assert_eq!(Ticks::from_beats_f64(1.0 / 3.0), Ticks(320));
    }

    #[test]
    fn arithmetic() {
        assert_eq!(Ticks(10) + Ticks(5), Ticks(15));
        assert_eq!(Ticks(10) - Ticks(15), Ticks(-5));
    }
}
