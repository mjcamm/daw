//! Gain and decibel conversions.

/// Converts decibels to a linear gain factor. `-inf` dB is silence.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Converts a linear gain factor to decibels. Zero or negative gain is `-inf` dB.
#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * gain.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn zero_db_is_unity_gain() {
        assert_eq!(db_to_gain(0.0), 1.0);
    }

    #[test]
    fn six_db_roughly_doubles() {
        assert!(close(db_to_gain(6.0206), 2.0));
        assert!(close(db_to_gain(-6.0206), 0.5));
    }

    #[test]
    fn negative_infinity_db_is_silence() {
        assert_eq!(db_to_gain(f32::NEG_INFINITY), 0.0);
        assert_eq!(gain_to_db(0.0), f32::NEG_INFINITY);
        assert_eq!(gain_to_db(-1.0), f32::NEG_INFINITY);
    }

    #[test]
    fn conversions_round_trip() {
        for db in [-60.0, -12.5, 0.0, 3.0, 24.0] {
            assert!(close(gain_to_db(db_to_gain(db)), db), "{db} dB");
        }
    }
}
