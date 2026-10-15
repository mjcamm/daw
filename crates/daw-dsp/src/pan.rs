//! Stereo panning.

use std::f32::consts::FRAC_PI_4;

/// Equal-power pan law: left and right gains for `pan` from -1 (left) to 1
/// (right). The centre is -3 dB on each side, keeping perceived loudness
/// constant across the stereo field.
#[inline]
pub fn equal_power(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn centre_is_minus_three_db_each_side() {
        let (l, r) = equal_power(0.0);
        assert!(close(l, std::f32::consts::FRAC_1_SQRT_2));
        assert!(close(r, std::f32::consts::FRAC_1_SQRT_2));
    }

    #[test]
    fn hard_pans_silence_the_other_side() {
        let (l, r) = equal_power(-1.0);
        assert!(close(l, 1.0) && close(r, 0.0));
        let (l, r) = equal_power(1.0);
        assert!(close(l, 0.0) && close(r, 1.0));
    }

    #[test]
    fn power_is_constant() {
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let (l, r) = equal_power(pan);
            assert!(close(l * l + r * r, 1.0), "{pan}");
        }
    }
}
