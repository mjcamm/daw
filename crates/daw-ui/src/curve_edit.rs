//! Editing curves (`docs/design/curve-editor.md`): adding, removing and
//! moving points, bending handles, switching a point's kind and snapping.
//! Pure functions on [`Curve`]s; the editor (`curve_editor.rs`)
//! turns gestures into these.
//!
//! Handle lengths and the line a Smooth point keeps are measured on screen,
//! so a handle's partner swings opposite as it looks. `aspect` is the
//! editor's width over its height: one unit of time is `aspect` units of
//! value on screen.

use daw_engine::{Curve, CurvePoint, Handle, PointKind};

/// Which of a point's two handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

/// Value steps points snap to: eighths of the height.
const VALUE_STEPS: f64 = 8.;

/// `curve` with a new Corner point at `at`, `value`, and the new point's
/// index. It goes after the points at or before `at`, never outside the
/// two ends.
pub fn add_point(curve: &Curve, at: f64, value: f64) -> (Curve, usize) {
    let mut curve = curve.clone();
    let last = curve.points.len().saturating_sub(1);
    let index = curve
        .points
        .partition_point(|p| p.at <= at)
        .clamp(1, last.max(1));
    curve.points.insert(
        index,
        CurvePoint::corner(at.clamp(0., 1.), value.clamp(0., 1.)),
    );
    (curve, index)
}

/// `curve` without point `index`; never the two ends.
pub fn remove_point(curve: &Curve, index: usize) -> Option<Curve> {
    let last = curve.points.len().checked_sub(1)?;
    if index == 0 || index >= last {
        return None;
    }
    let mut curve = curve.clone();
    curve.points.remove(index);
    Some(curve)
}

/// How far each handle of point `index` may reach in time: back to the
/// previous point, on to the next.
fn reach(curve: &Curve, index: usize) -> (f64, f64) {
    let p = &curve.points[index];
    let back = index
        .checked_sub(1)
        .map_or(0., |i| p.at - curve.points[i].at);
    let on = curve.points.get(index + 1).map_or(0., |n| n.at - p.at);
    (back, on)
}

/// `handle` shrunk towards its point, keeping its direction, until its
/// time reaches no further than `room`.
fn fit(handle: Handle, room: f64) -> Handle {
    if handle.at.abs() <= room || handle.at == 0. {
        return handle;
    }
    let scale = room / handle.at.abs();
    Handle::new(handle.at * scale, handle.value * scale)
}

/// Fits the handles of point `index` and the ones facing it on its
/// neighbours inside their segments.
fn fit_around(curve: &mut Curve, index: usize) {
    let last = curve.points.len().saturating_sub(1);
    for i in index.saturating_sub(1)..=(index + 1).min(last) {
        let (back, on) = reach(curve, i);
        let p = &mut curve.points[i];
        p.handle_in = fit(p.handle_in, back);
        p.handle_out = fit(p.handle_out, on);
    }
}

/// `curve` with point `index` moved to `at`, `value`: kept between its
/// neighbours in time, the ends pinned in time, values within `0..=1`. Its
/// handles move with it, shrinking if their segments do.
pub fn move_point(curve: &Curve, index: usize, at: f64, value: f64) -> Curve {
    let mut curve = curve.clone();
    let last = curve.points.len().saturating_sub(1);
    let at = match index {
        0 => 0.,
        i if i == last => 1.,
        i => at.clamp(curve.points[i - 1].at, curve.points[i + 1].at),
    };
    if let Some(point) = curve.points.get_mut(index) {
        point.at = at;
        point.value = value.clamp(0., 1.);
        fit_around(&mut curve, index);
    }
    curve
}

/// The on-screen length of a handle.
fn length(handle: Handle, aspect: f64) -> f64 {
    (handle.at * aspect).hypot(handle.value)
}

/// `handle` turned to point opposite `towards`, keeping its own on-screen
/// length. A zero `towards` leaves it as it is.
fn opposite(handle: Handle, towards: Handle, aspect: f64) -> Handle {
    let reach = length(towards, aspect);
    if reach == 0. {
        return handle;
    }
    let scale = length(handle, aspect) / reach;
    Handle::new(-towards.at * scale, -towards.value * scale)
}

/// `curve` with handle `side` of point `index` at `offset` from its point.
/// An in-handle never points later than its point, nor an out-handle
/// earlier, and neither reaches past the next point in its direction. On a
/// Smooth point the other handle swings opposite, keeping its own length
/// (or less, to stay inside its segment); on a Broken point it stays.
pub fn move_handle(curve: &Curve, index: usize, side: Side, offset: Handle, aspect: f64) -> Curve {
    let mut curve = curve.clone();
    if index >= curve.points.len() {
        return curve;
    }
    let (back, on) = reach(&curve, index);
    let point = &mut curve.points[index];
    let offset = match side {
        Side::In => fit(Handle::new(offset.at.min(0.), offset.value), back),
        Side::Out => fit(Handle::new(offset.at.max(0.), offset.value), on),
    };
    let smooth = point.kind == PointKind::Smooth;
    match side {
        Side::In => {
            point.handle_in = offset;
            if smooth {
                point.handle_out = fit(opposite(point.handle_out, offset, aspect), on);
            }
        }
        Side::Out => {
            point.handle_out = offset;
            if smooth {
                point.handle_in = fit(opposite(point.handle_in, offset, aspect), back);
            }
        }
    }
    curve
}

/// `curve` with point `index` switched to `kind`.
///
/// - A point gaining handles for the first time gets them a third of the way
///   to each neighbour, along the line between its neighbours (flat at the
///   ends).
/// - To Smooth, the in-handle swings opposite the out-handle.
/// - To Corner, the handles are kept for switching back.
pub fn set_kind(curve: &Curve, index: usize, kind: PointKind, aspect: f64) -> Curve {
    let mut curve = curve.clone();
    let points = curve.points.clone();
    let Some(point) = curve.points.get_mut(index) else {
        return curve;
    };
    point.kind = kind;
    if kind == PointKind::Corner {
        return curve;
    }
    if point.handle_in == Handle::NONE && point.handle_out == Handle::NONE {
        let previous = index.checked_sub(1).map(|i| points[i]);
        let next = points.get(index + 1).copied();
        let slope = match (previous, next) {
            (Some(p), Some(n)) if n.at > p.at => (n.value - p.value) / (n.at - p.at),
            _ => 0.,
        };
        if let Some(p) = previous {
            let reach = (point.at - p.at) / 3.;
            point.handle_in = Handle::new(-reach, -slope * reach);
        }
        if let Some(n) = next {
            let reach = (n.at - point.at) / 3.;
            point.handle_out = Handle::new(reach, slope * reach);
        }
    }
    if kind == PointKind::Smooth {
        point.handle_in = opposite(point.handle_in, point.handle_out, aspect);
    }
    fit_around(&mut curve, index);
    curve
}

/// `at` on the nearest of `cells` grid lines across the window.
pub fn snap_at(at: f64, cells: usize) -> f64 {
    let cells = cells.max(1) as f64;
    (at * cells).round() / cells
}

/// `value` on the nearest eighth of the height.
pub fn snap_value(value: f64) -> f64 {
    (value * VALUE_STEPS).round() / VALUE_STEPS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn corners(points: &[(f64, f64)]) -> Curve {
        Curve {
            points: points
                .iter()
                .map(|&(at, value)| CurvePoint::corner(at, value))
                .collect(),
        }
    }

    fn triangle() -> Curve {
        corners(&[(0., 0.), (0.5, 1.), (1., 0.)])
    }

    #[test]
    fn added_points_land_in_time_order_between_the_ends() {
        let (curve, index) = add_point(&triangle(), 0.25, 0.4);
        assert_eq!(index, 1);
        assert_eq!(curve.points[1], CurvePoint::corner(0.25, 0.4));
        let (curve, index) = add_point(&triangle(), 1., 0.4);
        assert_eq!(index, 2, "before the end, never after it");
        assert_eq!(curve.points.last().unwrap().at, 1.);
        let (_, index) = add_point(&triangle(), 0., 0.4);
        assert_eq!(index, 1, "after the start");
        let (curve, _) = add_point(&triangle(), 0.7, 2.);
        assert_eq!(curve.points[2].value, 1., "values within the height");
    }

    #[test]
    fn the_ends_cannot_be_removed() {
        let curve = triangle();
        assert_eq!(remove_point(&curve, 0), None);
        assert_eq!(remove_point(&curve, 2), None);
        assert_eq!(
            remove_point(&curve, 1),
            Some(corners(&[(0., 0.), (1., 0.)]))
        );
    }

    #[test]
    fn points_move_between_their_neighbours_and_the_ends_only_up_and_down() {
        let curve = corners(&[(0., 0.), (0.3, 1.), (0.6, 0.5), (1., 0.)]);
        let moved = move_point(&curve, 1, 0.9, 0.2);
        assert_eq!((moved.points[1].at, moved.points[1].value), (0.6, 0.2));
        let moved = move_point(&curve, 1, -1., 1.5);
        assert_eq!((moved.points[1].at, moved.points[1].value), (0., 1.));
        let moved = move_point(&curve, 0, 0.5, 0.7);
        assert_eq!((moved.points[0].at, moved.points[0].value), (0., 0.7));
        let moved = move_point(&curve, 3, 0.5, 0.7);
        assert_eq!((moved.points[3].at, moved.points[3].value), (1., 0.7));
    }

    #[test]
    fn a_corner_gets_handles_along_its_neighbours_when_it_curves() {
        // Neighbours rise one in value over the whole time: slope 1.
        let curve = corners(&[(0., 0.), (0.5, 0.5), (1., 1.)]);
        let smooth = set_kind(&curve, 1, PointKind::Smooth, 1.);
        let p = smooth.points[1];
        assert_eq!(p.kind, PointKind::Smooth);
        assert!(close(p.handle_in.at, -0.5 / 3.) && close(p.handle_in.value, -0.5 / 3.));
        assert!(close(p.handle_out.at, 0.5 / 3.) && close(p.handle_out.value, 0.5 / 3.));
        // The ends curve flat.
        let end = set_kind(&curve, 0, PointKind::Broken, 1.).points[0];
        assert_eq!(end.handle_in, Handle::NONE);
        assert!(close(end.handle_out.at, 0.5 / 3.) && end.handle_out.value == 0.);
    }

    #[test]
    fn switching_to_corner_keeps_the_handles_for_switching_back() {
        let curve = set_kind(&triangle(), 1, PointKind::Broken, 1.);
        let bent = move_handle(&curve, 1, Side::Out, Handle::new(0.1, 0.3), 1.);
        let corner = set_kind(&bent, 1, PointKind::Corner, 1.);
        assert_eq!(corner.points[1].kind, PointKind::Corner);
        let back = set_kind(&corner, 1, PointKind::Broken, 1.);
        assert_eq!(back.points[1], bent.points[1]);
    }

    #[test]
    fn a_smooth_points_handles_stay_opposite_keeping_their_own_lengths() {
        let curve = set_kind(&triangle(), 1, PointKind::Smooth, 1.);
        let before = length(curve.points[1].handle_in, 1.);
        let moved = move_handle(&curve, 1, Side::Out, Handle::new(0.3, 0.4), 1.);
        let p = moved.points[1];
        assert_eq!(p.handle_out, Handle::new(0.3, 0.4));
        assert!(close(length(p.handle_in, 1.), before), "own length kept");
        // Opposite: in = -k · out.
        let k = p.handle_in.at / -0.3;
        assert!(close(p.handle_in.value, -0.4 * k));
    }

    #[test]
    fn opposite_is_judged_on_screen() {
        // A wide editor (time stretched 4×): lengths count time 4× more.
        let curve = set_kind(&triangle(), 1, PointKind::Smooth, 4.);
        let before = length(curve.points[1].handle_in, 4.);
        let moved = move_handle(&curve, 1, Side::Out, Handle::new(0.1, 0.3), 4.);
        assert!(close(length(moved.points[1].handle_in, 4.), before));
    }

    #[test]
    fn a_broken_points_handles_move_on_their_own() {
        let curve = set_kind(&triangle(), 1, PointKind::Broken, 1.);
        let before = curve.points[1].handle_in;
        let moved = move_handle(&curve, 1, Side::Out, Handle::new(0.2, -0.4), 1.);
        assert_eq!(moved.points[1].handle_in, before);
        assert_eq!(moved.points[1].handle_out, Handle::new(0.2, -0.4));
    }

    #[test]
    fn handles_never_point_back_past_their_point() {
        let curve = set_kind(&triangle(), 1, PointKind::Broken, 1.);
        let moved = move_handle(&curve, 1, Side::Out, Handle::new(-0.2, 0.1), 1.);
        assert_eq!(moved.points[1].handle_out, Handle::new(0., 0.1));
        let moved = move_handle(&curve, 1, Side::In, Handle::new(0.2, 0.1), 1.);
        assert_eq!(moved.points[1].handle_in, Handle::new(0., 0.1));
    }

    #[test]
    fn handles_reach_no_further_than_the_next_point() {
        let curve = set_kind(&triangle(), 1, PointKind::Smooth, 1.);
        // Out past the end (0.5 away): shrunk along its line to reach it.
        let moved = move_handle(&curve, 1, Side::Out, Handle::new(1.0, 0.4), 1.);
        let p = moved.points[1];
        assert!(close(p.handle_out.at, 0.5) && close(p.handle_out.value, 0.2));
        assert!(p.handle_in.at >= -0.5, "its partner too");
    }

    #[test]
    fn moving_a_point_shrinks_handles_to_their_new_segments() {
        let curve = corners(&[(0., 0.), (0.5, 1.), (1., 0.)]);
        let curve = set_kind(&curve, 1, PointKind::Broken, 1.);
        let curve = move_handle(&curve, 1, Side::Out, Handle::new(0.4, 0.), 1.);
        let moved = move_point(&curve, 1, 0.9, 1.);
        assert!(
            close(moved.points[1].handle_out.at, 0.1),
            "0.1 left to the end"
        );
    }

    #[test]
    fn switching_to_smooth_lines_the_handles_up() {
        let curve = set_kind(&triangle(), 1, PointKind::Broken, 1.);
        let kinked = move_handle(&curve, 1, Side::Out, Handle::new(0.1, -0.2), 1.);
        let smooth = set_kind(&kinked, 1, PointKind::Smooth, 1.).points[1];
        assert_eq!(smooth.handle_out, Handle::new(0.1, -0.2));
        let k = smooth.handle_in.at / -0.1;
        assert!(k > 0. && close(smooth.handle_in.value, 0.2 * k));
    }

    #[test]
    fn snapping_finds_the_nearest_grid_line_and_eighth() {
        assert_eq!(snap_at(0.3, 4), 0.25);
        assert_eq!(snap_at(0.4, 4), 0.5);
        assert_eq!(snap_at(0.4, 0), 0., "at least one cell");
        assert_eq!(snap_value(0.3), 0.25);
        assert_eq!(snap_value(0.9), 0.875);
    }
}
