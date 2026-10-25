//! Reading a curve at a point in its cycle, and cutting a piece out of one
//! (`docs/design/curve-editor.md`).

use crate::{Curve, CurvePoint, Handle, PointKind};

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
    if !a.has_handles() && !b.has_handles() {
        return a.value + (b.value - a.value) * (phase - a.at) / (b.at - a.at);
    }
    let c = controls(a, b);
    let t = solve(c[0].0, c[1].0, c[2].0, c[3].0, phase);
    cubic(c[0].1, c[1].1, c[2].1, c[3].1, t)
}

/// The segment's Bézier control points as `(time, value)`, with each
/// handle's time held inside the segment so its time runs one way only. A
/// Corner end's control point is the point itself.
fn controls(a: &CurvePoint, b: &CurvePoint) -> [(f64, f64); 4] {
    let span = b.at - a.at;
    let p1 = if a.has_handles() {
        (
            a.at + a.handle_out.at.clamp(0.0, span),
            a.value + a.handle_out.value,
        )
    } else {
        (a.at, a.value)
    };
    let p2 = if b.has_handles() {
        (
            b.at + b.handle_in.at.clamp(-span, 0.0),
            b.value + b.handle_in.value,
        )
    } else {
        (b.at, b.value)
    };
    [(a.at, a.value), p1, p2, (b.at, b.value)]
}

impl Curve {
    /// The piece of the curve from `from` to `to` (`0 <= from < to <= 1`),
    /// stretched across the whole cycle: it reads at `u` what this reads at
    /// `from + u * (to - from)`. Points inside the piece are kept; its new
    /// ends are Corners on straight lines, else Broken with the handle that
    /// keeps the shape. Used where a bent note is cut short.
    pub fn slice(&self, from: f64, to: f64) -> Curve {
        let points = &self.points;
        if points.len() < 2 || to <= from {
            let value = curve_value(points, from);
            return Curve::line(value, value);
        }
        // The segment `from` reads in (as `curve_value` reads it) and the one
        // `to` ends in, approached from before: a jump at `to` is left out.
        let last_index = points.len() - 1;
        let first = points
            .partition_point(|p| p.at <= from)
            .clamp(1, last_index)
            - 1;
        let last = points.partition_point(|p| p.at < to).clamp(1, last_index);
        let mut out;
        if first + 1 == last {
            out = piece(&points[first], &points[last], from, to);
        } else {
            let head = piece(
                &points[first],
                &points[first + 1],
                from,
                points[first + 1].at,
            );
            let tail = piece(&points[last - 1], &points[last], points[last - 1].at, to);
            out = head[..head.len() - 1].to_vec();
            for (i, point) in points.iter().enumerate().take(last).skip(first + 1) {
                let mut point = *point;
                if point.has_handles() {
                    if i == first + 1 {
                        point.handle_in = head[head.len() - 1].handle_in;
                    }
                    if i == last - 1 {
                        point.handle_out = tail[0].handle_out;
                    }
                }
                out.push(point);
            }
            out.extend_from_slice(&tail[1..]);
        }
        let span = to - from;
        for point in &mut out {
            point.at = ((point.at - from) / span).clamp(0.0, 1.0);
            point.handle_in.at /= span;
            point.handle_out.at /= span;
        }
        let end = out.len() - 1;
        out[0].at = 0.0;
        out[end].at = 1.0;
        Curve { points: out }
    }
}

/// The points drawing the part of the segment `a` to `b` from time `lo` to
/// `hi` (inside it, `lo < hi`): its two ends, and between them any points
/// needed to keep every handle inside its own segment.
fn piece(a: &CurvePoint, b: &CurvePoint, lo: f64, hi: f64) -> Vec<CurvePoint> {
    if !a.has_handles() && !b.has_handles() {
        let value = |at: f64| a.value + (b.value - a.value) * (at - a.at) / (b.at - a.at);
        return vec![
            CurvePoint::corner(lo, value(lo)),
            CurvePoint::corner(hi, value(hi)),
        ];
    }
    let c = controls(a, b);
    let t = |x: f64| solve(c[0].0, c[1].0, c[2].0, c[3].0, x);
    let t0 = if lo <= a.at { 0.0 } else { t(lo) };
    let t1 = if hi >= b.at { 1.0 } else { t(hi) };
    let (before, _) = split(c, t1);
    // A part of a curve can reach its handles past its own ends in time,
    // which reading would hold back and so bend; halves reach less far.
    let mut parts = Vec::new();
    split_inside(split(before, t0 / t1).1, 8, &mut parts);
    let handle = |p: (f64, f64), from: (f64, f64)| Handle::new(p.0 - from.0, p.1 - from.1);
    let point = |p: (f64, f64), handle_in, handle_out| {
        CurvePoint::corner(p.0, p.1.clamp(0.0, 1.0)).with_handles(
            PointKind::Broken,
            handle_in,
            handle_out,
        )
    };
    let mut out = Vec::with_capacity(parts.len() + 1);
    let mut handle_in = Handle::NONE;
    for part in &parts {
        out.push(point(part[0], handle_in, handle(part[1], part[0])));
        handle_in = handle(part[2], part[3]);
    }
    out.push(point(parts[parts.len() - 1][3], handle_in, Handle::NONE));
    let end = out.len() - 1;
    out[0].at = lo;
    out[end].at = hi;
    out
}

/// Splits a cubic Bézier in halves until each part's handles lie inside it
/// in time, at most `depth` times deep.
fn split_inside(c: [(f64, f64); 4], depth: u32, out: &mut Vec<[(f64, f64); 4]>) {
    let range = (c[0].0 - 1e-12)..=(c[3].0 + 1e-12);
    if depth == 0 || (range.contains(&c[1].0) && range.contains(&c[2].0)) {
        out.push(c);
    } else {
        let (left, right) = split(c, 0.5);
        split_inside(left, depth - 1, out);
        split_inside(right, depth - 1, out);
    }
}

/// A cubic Bézier split at `t`, by de Casteljau.
#[allow(clippy::type_complexity)]
fn split(c: [(f64, f64); 4], t: f64) -> ([(f64, f64); 4], [(f64, f64); 4]) {
    let lerp = |p: (f64, f64), q: (f64, f64)| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
    let (a, b, d) = (lerp(c[0], c[1]), lerp(c[1], c[2]), lerp(c[2], c[3]));
    let (e, f) = (lerp(a, b), lerp(b, d));
    let g = lerp(e, f);
    ([c[0], a, e, g], [g, f, d, c[3]])
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

    /// Checks that the slice reads what the curve reads over `from..to`.
    fn reads_the_same(points: Vec<CurvePoint>, from: f64, to: f64) {
        let curve = Curve { points };
        let slice = curve.slice(from, to);
        assert_eq!(slice.points.first().map(|p| p.at), Some(0.0));
        assert_eq!(slice.points.last().map(|p| p.at), Some(1.0));
        assert!(slice.points.windows(2).all(|w| w[0].at <= w[1].at));
        for i in 0..=400 {
            // Just off each step, so none sits exactly on a jump.
            let u = (f64::from(i) + 0.37) / 401.0;
            let got = curve_value(&slice.points, u);
            let want = curve_value(&curve.points, from + u * (to - from));
            assert!((got - want).abs() < 1e-9, "at {u}: {got} vs {want}");
        }
    }

    fn eased() -> Vec<CurvePoint> {
        vec![
            smooth(0.0, 0.2, Handle::NONE, Handle::new(0.3, 0.0)),
            corner(0.5, 0.9).with_handles(
                PointKind::Broken,
                Handle::new(-0.1, -0.3),
                Handle::new(0.2, 0.1),
            ),
            corner(0.7, 0.4),
            smooth(1.0, 0.6, Handle::new(-0.2, -0.2), Handle::NONE),
        ]
    }

    #[test]
    fn a_slice_of_a_line_is_the_same_line() {
        reads_the_same(vec![corner(0.0, 0.0), corner(1.0, 1.0)], 0.25, 0.75);
    }

    #[test]
    fn a_slice_inside_one_curved_segment_keeps_its_shape() {
        reads_the_same(eased(), 0.1, 0.4);
        reads_the_same(eased(), 0.55, 0.65);
    }

    #[test]
    fn a_slice_across_points_keeps_them_and_their_shape() {
        reads_the_same(eased(), 0.2, 0.9);
        reads_the_same(eased(), 0.0, 1.0);
        assert_eq!(Curve { points: eased() }.slice(0.2, 0.9).points.len(), 4);
    }

    #[test]
    fn a_slice_can_start_or_end_on_a_point() {
        reads_the_same(eased(), 0.5, 1.0);
        reads_the_same(eased(), 0.0, 0.5);
        reads_the_same(eased(), 0.5, 0.7);
    }

    #[test]
    fn a_slice_keeps_jumps_inside_it_and_leaves_out_one_at_its_end() {
        let jump = vec![
            corner(0.0, 0.2),
            corner(0.5, 0.2),
            corner(0.5, 0.9),
            corner(1.0, 0.9),
        ];
        reads_the_same(jump.clone(), 0.25, 0.75);
        let before = Curve { points: jump }.slice(0.0, 0.5);
        assert_eq!(
            curve_value(&before.points, 1.0),
            0.2,
            "ends before the jump"
        );
    }

    #[test]
    fn a_slice_of_handles_reaching_past_their_segment() {
        let points = vec![
            smooth(0.0, 0.0, Handle::NONE, Handle::new(5.0, 0.0)),
            smooth(1.0, 1.0, Handle::new(-5.0, 0.0), Handle::NONE),
        ];
        reads_the_same(points, 0.3, 0.8);
    }

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
