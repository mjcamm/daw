//! Reading a curve at a point in its cycle (`docs/design/curve-editor.md`).

use daw_protocol::CurvePoint;

/// The curve's value at `phase` (`0..=1` through its cycle), clipped to
/// `0..=1`.
///
/// Before the first point the curve holds the first value; after the last it
/// holds the last. An empty curve is the centre line. Allocation-free.
pub fn curve_value(points: &[CurvePoint], phase: f64) -> f64 {
    let after = points.partition_point(|p| p.at <= phase);
    let (Some(a), Some(b)) = (after.checked_sub(1).map(|i| &points[i]), points.get(after)) else {
        return points
            .get(after.min(points.len().saturating_sub(1)))
            .map_or(0.5, |p| p.value.clamp(0.0, 1.0));
    };
    // `a.at <= phase < b.at`, so the span is positive.
    segment_value(a, b, phase).clamp(0.0, 1.0)
}

/// The value at `phase` between `a` and `b`, where `a.at <= phase < b.at`.
fn segment_value(a: &CurvePoint, b: &CurvePoint, phase: f64) -> f64 {
    let span = b.at - a.at;
    if !a.has_handles() && !b.has_handles() {
        return a.value + (b.value - a.value) * (phase - a.at) / span;
    }
    // Control points, with each handle's time held inside the segment so the
    // segment's time runs one way only.
    let (x1, y1) = if a.has_handles() {
        (
            a.at + a.handle_out.at.clamp(0.0, span),
            a.value + a.handle_out.value,
        )
    } else {
        (a.at, a.value)
    };
    let (x2, y2) = if b.has_handles() {
        (
            b.at + b.handle_in.at.clamp(-span, 0.0),
            b.value + b.handle_in.value,
        )
    } else {
        (b.at, b.value)
    };
    let t = solve(a.at, x1, x2, b.at, phase);
    cubic(a.value, y1, y2, b.value, t)
}

fn cubic(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let s = 1.0 - t;
    s * s * s * p0 + 3.0 * s * s * t * p1 + 3.0 * s * t * t * p2 + t * t * t * p3
}

fn cubic_slope(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let s = 1.0 - t;
    3.0 * (s * s * (p1 - p0) + 2.0 * s * t * (p2 - p1) + t * t * (p3 - p2))
}

/// The Bézier time `t` in `0..=1` at which its time coordinate is `x`. The
/// coordinate never falls as `t` grows (its handles stay inside the
/// segment), so Newton steps kept inside a shrinking bracket find it,
/// halving the bracket when a step would leave it.
fn solve(x0: f64, x1: f64, x2: f64, x3: f64, x: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    let mut t = (x - x0) / (x3 - x0);
    for _ in 0..64 {
        let error = cubic(x0, x1, x2, x3, t) - x;
        if error.abs() < 1e-12 {
            break;
        }
        if error < 0.0 {
            lo = t;
        } else {
            hi = t;
        }
        let slope = cubic_slope(x0, x1, x2, x3, t);
        let next = t - error / slope;
        t = if slope > 0.0 && next > lo && next < hi {
            next
        } else {
            0.5 * (lo + hi)
        };
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_protocol::{Handle, PointKind};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn corner(at: f64, value: f64) -> CurvePoint {
        CurvePoint::corner(at, value)
    }

    fn smooth(at: f64, value: f64, handle_in: Handle, handle_out: Handle) -> CurvePoint {
        corner(at, value).with_handles(PointKind::Smooth, handle_in, handle_out)
    }

    #[test]
    fn an_empty_curve_is_the_centre_line() {
        assert_eq!(curve_value(&[], 0.3), 0.5);
    }

    #[test]
    fn corners_join_with_straight_lines() {
        let points = [corner(0.0, 0.0), corner(0.5, 1.0), corner(1.0, 0.5)];
        assert!(close(curve_value(&points, 0.25), 0.5));
        assert!(close(curve_value(&points, 0.5), 1.0));
        assert!(close(curve_value(&points, 0.75), 0.75));
        assert_eq!(curve_value(&points, 1.0), 0.5, "holds the last value");
    }

    #[test]
    fn two_points_at_one_time_make_a_jump() {
        let points = [
            corner(0.0, 0.2),
            corner(0.5, 0.2),
            corner(0.5, 0.9),
            corner(1.0, 0.9),
        ];
        assert!(close(curve_value(&points, 0.499_999), 0.2));
        assert!(close(curve_value(&points, 0.5), 0.9));
    }

    #[test]
    fn a_corners_handles_are_ignored() {
        let mut a = corner(0.0, 0.0);
        a.handle_out = Handle::new(0.3, 0.8);
        let points = [a, corner(1.0, 1.0)];
        assert!(close(curve_value(&points, 0.5), 0.5), "still straight");
    }

    #[test]
    fn handles_lying_on_the_line_keep_it_straight() {
        // Handles a third of the way along the line itself: a straight line
        // drawn as a curve, evenly timed.
        let points = [
            smooth(0.0, 0.0, Handle::NONE, Handle::new(1.0 / 3.0, 1.0 / 3.0)),
            smooth(1.0, 1.0, Handle::new(-1.0 / 3.0, -1.0 / 3.0), Handle::NONE),
        ];
        for phase in [0.1, 0.25, 0.5, 0.8] {
            assert!(close(curve_value(&points, phase), phase), "at {phase}");
        }
    }

    #[test]
    fn handles_bend_the_curve_and_keep_its_ends() {
        // Both handles pull flat: an ease in and out, slow at the ends.
        let points = [
            smooth(0.0, 0.0, Handle::NONE, Handle::new(0.5, 0.0)),
            smooth(1.0, 1.0, Handle::new(-0.5, 0.0), Handle::NONE),
        ];
        assert_eq!(curve_value(&points, 0.0), 0.0);
        assert!(curve_value(&points, 0.1) < 0.1, "slow start");
        assert!(close(curve_value(&points, 0.5), 0.5), "symmetric");
        assert!(curve_value(&points, 0.9) > 0.9, "slow finish");
        assert_eq!(curve_value(&points, 1.0), 1.0);
    }

    #[test]
    fn one_curved_end_bends_the_segment() {
        // Only the start has a handle, pulled up: the curve rises fast.
        let points = [
            smooth(0.0, 0.0, Handle::NONE, Handle::new(0.0, 1.0)),
            corner(1.0, 1.0),
        ];
        assert!(curve_value(&points, 0.25) > 0.5);
    }

    #[test]
    fn handles_reaching_past_their_segment_are_held_inside_it() {
        // Out-handle reaches beyond the next point, in-handle before the
        // previous one: the curve still gives one value for each time,
        // never going back on itself.
        let points = [
            smooth(0.0, 0.0, Handle::NONE, Handle::new(5.0, 0.0)),
            smooth(1.0, 1.0, Handle::new(-5.0, 0.0), Handle::NONE),
        ];
        let mut previous = 0.0;
        for i in 0..=200 {
            let v = curve_value(&points, f64::from(i) / 200.0);
            assert!(v >= previous - 1e-9, "rises at {i}");
            previous = v;
        }
    }

    #[test]
    fn values_pulled_past_the_edge_are_clipped() {
        let points = [
            smooth(0.0, 0.9, Handle::NONE, Handle::new(0.3, 2.0)),
            corner(1.0, 0.9),
        ];
        for i in 0..=100 {
            let v = curve_value(&points, f64::from(i) / 100.0);
            assert!((0.0..=1.0).contains(&v));
        }
        assert_eq!(curve_value(&points, 0.3), 1.0);
    }

    #[test]
    fn broken_points_curve_both_sides_with_a_sharp_tip() {
        // A tip at the middle, each side's handle pointing its own way.
        let tip = corner(0.5, 1.0).with_handles(
            PointKind::Broken,
            Handle::new(-0.2, -0.1),
            Handle::new(0.2, -0.1),
        );
        let points = [corner(0.0, 0.0), tip, corner(1.0, 0.0)];
        assert!(close(curve_value(&points, 0.5), 1.0));
        let (left, right) = (curve_value(&points, 0.4), curve_value(&points, 0.6));
        assert!(close(left, right), "mirror sides: {left} {right}");
        assert!(left > 0.8, "bulging above the straight line");
    }
}
