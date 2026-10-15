//! Reading a shape at a point in its cycle.

use daw_protocol::{Segment, ShapePoint};

/// The shape's value at `phase` (`0..=1` through its cycle).
///
/// Before the first point the shape holds the first value; after the last it
/// holds the last. An empty shape is 0.
pub fn shape_value(points: &[ShapePoint], phase: f64) -> f64 {
    let after = points.partition_point(|p| p.at <= phase);
    let (Some(a), Some(b)) = (after.checked_sub(1).map(|i| &points[i]), points.get(after)) else {
        // Before the first point or after the last.
        return points
            .get(after.min(points.len().saturating_sub(1)))
            .map_or(0.0, |p| p.value);
    };
    // `a.at <= phase < b.at`, so the span is positive.
    let u = (phase - a.at) / (b.at - a.at);
    match a.segment {
        Segment::Linear => lerp(a.value, b.value, u),
        Segment::Hold => a.value,
        Segment::Curve(tension) => lerp(a.value, b.value, bend(u, f64::from(tension))),
    }
}

fn lerp(a: f64, b: f64, u: f64) -> f64 {
    a + (b - a) * u
}

/// Bends `u` in `0..=1` keeping both ends fixed. Positive tension starts
/// slow, negative starts fast; zero is a straight line.
fn bend(u: f64, tension: f64) -> f64 {
    let t = tension.clamp(-1.0, 1.0);
    if t >= 0.0 {
        u.powf(1.0 + 4.0 * t)
    } else {
        1.0 - (1.0 - u).powf(1.0 - 4.0 * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(at: f64, value: f64) -> ShapePoint {
        ShapePoint::new(at, value)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn empty_shape_is_zero() {
        assert_eq!(shape_value(&[], 0.5), 0.0);
    }

    #[test]
    fn single_point_is_constant() {
        for phase in [0.0, 0.3, 1.0] {
            assert_eq!(shape_value(&[p(0.5, 0.7)], phase), 0.7);
        }
    }

    #[test]
    fn holds_end_values_outside_the_points() {
        let points = [p(0.25, 0.2), p(0.75, 0.8)];
        assert_eq!(shape_value(&points, 0.0), 0.2);
        assert_eq!(shape_value(&points, 1.0), 0.8);
    }

    #[test]
    fn linear_segments_interpolate() {
        let points = [p(0.0, 0.0), p(0.5, 1.0), p(1.0, 0.0)];
        assert!(close(shape_value(&points, 0.25), 0.5));
        assert!(close(shape_value(&points, 0.5), 1.0));
        assert!(close(shape_value(&points, 0.75), 0.5));
    }

    #[test]
    fn hold_segments_step() {
        let points = [p(0.0, 0.1).with_segment(Segment::Hold), p(0.5, 0.9)];
        assert_eq!(shape_value(&points, 0.0), 0.1);
        assert_eq!(shape_value(&points, 0.49), 0.1);
        assert_eq!(shape_value(&points, 0.5), 0.9);
    }

    #[test]
    fn curves_keep_their_ends_and_bend_the_middle() {
        let shape = |t| [p(0.0, 0.0).with_segment(Segment::Curve(t)), p(1.0, 1.0)];
        let slow = shape(1.0);
        let fast = shape(-1.0);
        let straight = shape(0.0);
        assert!(close(shape_value(&straight, 0.5), 0.5));
        assert!(shape_value(&slow, 0.5) < 0.1);
        assert!(shape_value(&fast, 0.5) > 0.9);
        for points in [slow, fast] {
            assert_eq!(shape_value(&points, 0.0), 0.0);
            assert_eq!(shape_value(&points, 1.0), 1.0);
        }
    }

    #[test]
    fn curves_are_monotonic() {
        for t in [-1.0, -0.3, 0.4, 1.0] {
            let points = [p(0.0, 0.0).with_segment(Segment::Curve(t)), p(1.0, 1.0)];
            let mut previous = 0.0;
            for i in 0..=100 {
                let v = shape_value(&points, f64::from(i) / 100.0);
                assert!(v >= previous, "tension {t} at {i}");
                previous = v;
            }
        }
    }

    #[test]
    fn coincident_points_make_a_jump() {
        let points = [p(0.0, 0.0), p(0.5, 0.2), p(0.5, 0.8), p(1.0, 1.0)];
        assert!((shape_value(&points, 0.499_999) - 0.2).abs() < 1e-5);
        assert!(close(shape_value(&points, 0.5), 0.8));
    }
}
