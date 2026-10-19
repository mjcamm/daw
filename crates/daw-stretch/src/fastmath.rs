//! Cheap replacements for libm calls in the per-bin hot loops. Errors are far below anything
//! audible (see tests); all are branch-light so loops around them can vectorise.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Adding then subtracting 1.5·2²³ rounds an f32 to the nearest integer without a libm call
/// (baseline x86-64 has no round instruction). Valid for |x| < 2²².
#[inline(always)]
fn round_fast(x: f32) -> f32 {
    const MAGIC: f32 = 12_582_912.0;
    (x + MAGIC) - MAGIC
}

/// Wraps to [−π, π].
#[inline(always)]
pub fn princarg(x: f32) -> f32 {
    x - TAU * round_fast(x * (1.0 / TAU))
}

/// atan2 with max error ~1e-5 rad: octant reduction plus Abramowitz & Stegun 4.4.49.
#[inline(always)]
pub fn atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (x.abs(), y.abs());
    let a = ax.min(ay) / ax.max(ay).max(1e-30);
    let s = a * a;
    let r = a * (0.999_866 + s * (-0.330_299_5 + s * (0.180_141 + s * (-0.085_133 + s * 0.020_835_1))));
    // Octant fix-up as selects so loops around this vectorise.
    let r = if ay > ax { FRAC_PI_2 - r } else { r };
    let r = if x < 0.0 { PI - r } else { r };
    if y < 0.0 { -r } else { r }
}

/// (sin, cos) with max error ~2e-6 for |x| < 20 (f32 reduction error grows with |x|): reduce to [−π/4, π/4] by quadrant,
/// then Taylor polynomials.
#[inline(always)]
pub fn sincos(x: f32) -> (f32, f32) {
    let q = round_fast(x * (2.0 / PI));
    let r = x - q * FRAC_PI_2;
    let r2 = r * r;
    let s = r * (1.0 + r2 * (-1.0 / 6.0 + r2 * (1.0 / 120.0 + r2 * (-1.0 / 5040.0))));
    let c = 1.0 + r2 * (-0.5 + r2 * (1.0 / 24.0 + r2 * (-1.0 / 720.0 + r2 * (1.0 / 40320.0))));
    // Quadrant fix-up as selects rather than a match, so loops around this vectorise.
    let qi = q as i32;
    let (s1, c1) = if qi & 1 != 0 { (c, s) } else { (s, c) };
    let sign_s = if qi & 2 != 0 { -1.0 } else { 1.0 };
    let sign_c = if (qi + 1) & 2 != 0 { -1.0 } else { 1.0 };
    (s1 * sign_s, c1 * sign_c)
}

/// 10·log10(power), within ~0.03 dB: exponent bits plus a quadratic for the mantissa's log2.
#[inline(always)]
pub fn power_db(p: f32) -> f32 {
    let bits = p.max(1e-30).to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let log2 = e as f32 + (-0.344_848_43 * m + 2.024_665_8) * m - 1.674_877_6;
    log2 * 3.010_3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accuracy() {
        let mut worst = [0f32; 4];
        for i in 0..200_000 {
            let t = (i as f32 / 200_000.0 - 0.5) * 40.0;
            let (y, x) = (t.sin() * (1.0 + i as f32 % 7.0), (t * 1.7).cos());
            worst[0] = worst[0].max((atan2(y, x) - y.atan2(x)).abs());
            let (s, c) = sincos(t);
            worst[1] = worst[1].max((s - t.sin()).abs().max((c - t.cos()).abs()));
            worst[2] = worst[2].max((princarg(t) - (t - TAU * (t / TAU).round())).abs());
            let p = 10f32.powf(t * 0.4);
            worst[3] = worst[3].max((power_db(p) - 10.0 * p.log10()).abs());
        }
        assert!(worst[0] < 2e-5, "atan2 {}", worst[0]);
        assert!(worst[1] < 3e-6, "sincos {}", worst[1]);
        assert!(worst[2] < 1e-5, "princarg {}", worst[2]);
        assert!(worst[3] < 0.05, "power_db {}", worst[3]);
    }
}
