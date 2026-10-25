//! Bending notes in the piano roll (`docs/design/note-bends.md`): where a
//! bent note sits on the grid, what a press finds on it, and the edits the
//! Bend tool makes. Pure functions, tested without a window.
//!
//! Positions are in pixels from the grid's top-left (song start, top row).
//! A note's bend is a [`Curve`] across it in semitones from its pitch; on
//! screen a semitone is one row, measured from its own row's middle.

use crate::curve_edit::{self, Side};
use crate::note_edit::MIN_LENGTH;
use daw_engine::{
    BEND_RANGE, Curve, CurvePoint, Handle, MidiNote, PointKind, Ticks, bend_semitones, bend_value,
    curve_value,
};

/// How close the pointer must be to grab a dot or a handle.
pub const GRAB: f64 = 7.;

/// Where a note sits on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// Its start and width.
    pub x: f64,
    pub w: f64,
    /// The middle of its own row.
    pub centre: f64,
    /// A row's height: one semitone.
    pub key: f64,
}

impl Place {
    pub fn of(note: &MidiNote, row: usize, pixels_per_beat: f64, key: f64) -> Self {
        Self {
            x: note.start.as_beats() * pixels_per_beat,
            w: note.length.as_beats() * pixels_per_beat,
            centre: (row as f64 + 0.5) * key,
            key,
        }
    }

    /// A curve position (`at` across the note, a bend `value`) on screen.
    pub fn screen(&self, at: f64, value: f64) -> (f64, f64) {
        (
            self.x + at * self.w,
            self.centre - bend_semitones(value) * self.key,
        )
    }

    /// Where a point's handle ends on screen.
    pub fn handle_end(&self, point: &CurvePoint, handle: Handle) -> (f64, f64) {
        self.screen(point.at + handle.at, point.value + handle.value)
    }

    /// A handle reaching from `point` to `x`, `y` on screen.
    pub fn handle_to(&self, point: &CurvePoint, x: f64, y: f64) -> Handle {
        let (px, py) = self.screen(point.at, point.value);
        Handle::new((x - px) / self.w, (py - y) / self.semitone_value())
    }

    /// The height of one bend value unit on screen.
    fn semitone_value(&self) -> f64 {
        2. * BEND_RANGE * self.key
    }

    /// The note's width over its curve's height on screen, for
    /// `curve_edit`'s handle rules.
    pub fn aspect(&self) -> f64 {
        self.w / self.semitone_value()
    }

    /// The middle of the note's line at `x`.
    pub fn line_at(&self, note: &MidiNote, x: f64) -> f64 {
        let phase = ((x - self.x) / self.w).clamp(0., 1.);
        self.centre - note.bend_at(phase) * self.key
    }

    /// Whether `x`, `y` is on the note: within half a row of its line.
    pub fn contains(&self, note: &MidiNote, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && (y - self.line_at(note, x)).abs() <= self.key / 2.
    }
}

/// A note's bend points: its bend, or a straight line's two ends.
pub fn points(note: &MidiNote) -> Curve {
    note.bend
        .clone()
        .unwrap_or_else(|| Curve::line(bend_value(0.), bend_value(0.)))
}

/// The bend to keep: none when every point sits on the note's pitch with
/// no handle reaching up or down, so a note bent back home is plain again.
fn tidy(curve: Curve) -> Option<Curve> {
    let home = bend_value(0.);
    let flat = curve.points.iter().all(|p| {
        p.value == home
            && (!p.has_handles() || (p.handle_in.value == 0. && p.handle_out.value == 0.))
    });
    (!flat).then_some(curve)
}

/// What a press finds with the Bend tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BendHit {
    /// A note's bend point (its ends are its first and last).
    Point { note: usize, point: usize },
    /// A handle of the selected point.
    Handle {
        note: usize,
        point: usize,
        side: Side,
    },
    /// The note anywhere else along its line.
    Line { note: usize },
}

/// What a press at `x`, `y` lands on: the selected point's handles first,
/// then the nearest dot in reach, then a note's line. Later notes are drawn
/// on top, so they win. `place` gives where each note sits (`None` when it
/// has no row on screen).
pub fn hit(
    notes: &[MidiNote],
    place: &dyn Fn(usize) -> Option<Place>,
    selected: Option<(usize, usize)>,
    x: f64,
    y: f64,
) -> Option<BendHit> {
    let near = |(px, py): (f64, f64)| {
        let d = (px - x).hypot(py - y);
        (d <= GRAB).then_some(d)
    };
    if let Some((note, point)) = selected
        && let (Some(n), Some(at)) = (notes.get(note), place(note))
        && let Some(p) = points(n).points.get(point).copied()
        && p.has_handles()
    {
        for (side, handle) in [(Side::In, p.handle_in), (Side::Out, p.handle_out)] {
            if handle != Handle::NONE && near(at.handle_end(&p, handle)).is_some() {
                return Some(BendHit::Handle { note, point, side });
            }
        }
    }
    let mut best: Option<(f64, BendHit)> = None;
    for (i, n) in notes.iter().enumerate().rev() {
        let Some(at) = place(i) else { continue };
        for (k, p) in points(n).points.iter().enumerate() {
            if let Some(d) = near(at.screen(p.at, p.value))
                && best.is_none_or(|(b, _)| d < b)
            {
                best = Some((d, BendHit::Point { note: i, point: k }));
            }
        }
    }
    if let Some((_, found)) = best {
        return Some(found);
    }
    notes.iter().enumerate().rev().find_map(|(i, n)| {
        let at = place(i)?;
        at.contains(n, x, y).then_some(BendHit::Line { note: i })
    })
}

/// The bent note whose line is at `x`, `y`, if any (later notes win): other
/// tools find bent notes along their lines, not on their rows.
pub fn bent_at(
    notes: &[MidiNote],
    place: &dyn Fn(usize) -> Option<Place>,
    x: f64,
    y: f64,
) -> Option<usize> {
    notes.iter().enumerate().rev().find_map(|(i, n)| {
        let at = place(i)?;
        (n.bend.is_some() && at.contains(n, x, y)).then_some(i)
    })
}

/// Whether the note's line, a row thick, passes through the box from
/// `left`, `top` to `right`, `bottom`.
pub fn crosses(
    note: &MidiNote,
    place: Place,
    (left, top, right, bottom): (f64, f64, f64, f64),
) -> bool {
    let half = place.key / 2.;
    let touches =
        |(x, y): (f64, f64)| x >= left && x <= right && y + half >= top && y - half <= bottom;
    // Where the box's sides cross the note too, for boxes narrower than
    // the line's steps.
    let sides = [left, right]
        .into_iter()
        .filter(|&x| x >= place.x && x < place.x + place.w)
        .map(|x| (x, place.line_at(note, x)));
    line(note, place, 2.).into_iter().chain(sides).any(touches)
}

/// How a dot drag moves: `dx` pixels sideways and `dy` down since the
/// press, at `pixels_per_beat`, with `snap` for times and whole semitones
/// unless `free`.
#[derive(Clone, Copy)]
pub struct Drag<'a> {
    pub dx: f64,
    pub dy: f64,
    pub pixels_per_beat: f64,
    pub snap: &'a dyn Fn(Ticks) -> Ticks,
    pub free: bool,
}

impl Drag<'_> {
    fn ticks(&self) -> Ticks {
        Ticks::from_beats_f64(self.dx / self.pixels_per_beat)
    }
}

/// `note` with bend point `point` dragged: up and down bends it (by the
/// semitones the pointer moved, from where it was); sideways moves it in
/// time. The ends move that end of the note, so the note grows or shrinks
/// and its bend stretches with it. A straight note gains a bend: a straight
/// glide between its ends.
pub fn drag_point(note: &MidiNote, point: usize, place: Place, drag: Drag) -> MidiNote {
    let curve = points(note);
    let Some(p) = curve.points.get(point).copied() else {
        return note.clone();
    };
    let mut semitones = bend_semitones(p.value) - drag.dy / place.key;
    if !drag.free {
        semitones = semitones.round();
    }
    let value = bend_value(semitones.clamp(-BEND_RANGE, BEND_RANGE));
    let (start, end) = (note.start, note.start + note.length);
    let last = curve.points.len() - 1;
    let mut out = note.clone();
    let mut curve = curve;
    if point == 0 {
        let at = (drag.snap)(start + drag.ticks());
        out.start = at.max(Ticks::ZERO).min(end - MIN_LENGTH);
        out.length = end - out.start;
        curve = curve_edit::move_point(&curve, 0, 0., value);
    } else if point == last {
        let at = (drag.snap)(end + drag.ticks());
        out.length = at.max(start + MIN_LENGTH) - start;
        curve = curve_edit::move_point(&curve, last, 1., value);
    } else {
        let length = note.length.0 as f64;
        let was = start + Ticks((p.at * length).round() as i64);
        let at = ((drag.snap)(was + drag.ticks()) - start).0 as f64 / length;
        curve = curve_edit::move_point(&curve, point, at, value);
    }
    out.bend = tidy(curve);
    out
}

/// `note` with handle `side` of point `point` reaching to `x`, `y`.
pub fn drag_handle(
    note: &MidiNote,
    point: usize,
    side: Side,
    place: Place,
    x: f64,
    y: f64,
) -> MidiNote {
    let curve = points(note);
    let Some(p) = curve.points.get(point) else {
        return note.clone();
    };
    let handle = place.handle_to(p, x, y);
    let curve = curve_edit::move_handle(&curve, point, side, handle, place.aspect());
    MidiNote {
        bend: tidy(curve),
        ..note.clone()
    }
}

/// `note` with a new point on its line at `x` (time snapped by `snap`),
/// and the new point's index. The line keeps its shape there.
pub fn add_point(
    note: &MidiNote,
    x: f64,
    pixels_per_beat: f64,
    snap: &dyn Fn(Ticks) -> Ticks,
) -> Option<(MidiNote, usize)> {
    let tick = snap(Ticks::from_beats_f64(x / pixels_per_beat));
    let at = (tick - note.start).0 as f64 / note.length.0 as f64;
    if at <= 0. || at >= 1. {
        return None;
    }
    let curve = points(note);
    let value = curve_value(&curve.points, at);
    let (curve, index) = curve_edit::add_point(&curve, at, value);
    Some((
        MidiNote {
            bend: Some(curve),
            ..note.clone()
        },
        index,
    ))
}

/// `note` without bend point `point` (never its ends).
pub fn remove_point(note: &MidiNote, point: usize) -> Option<MidiNote> {
    let curve = curve_edit::remove_point(&points(note), point)?;
    Some(MidiNote {
        bend: tidy(curve),
        ..note.clone()
    })
}

/// `note` with bend point `point` switched to `kind`.
pub fn set_kind(note: &MidiNote, point: usize, kind: PointKind, place: Place) -> MidiNote {
    let curve = curve_edit::set_kind(&points(note), point, kind, place.aspect());
    MidiNote {
        bend: tidy(curve),
        ..note.clone()
    }
}

/// The note's line on screen, left to right, `step` pixels apart, with a
/// sample on each side of every point so corners and jumps stay sharp.
pub fn line(note: &MidiNote, place: Place, step: f64) -> Vec<(f64, f64)> {
    let steps = (place.w / step.max(1.)).ceil().clamp(1., 512.) as usize;
    let mut phases: Vec<f64> = (0..=steps).map(|i| i as f64 / steps as f64).collect();
    if let Some(bend) = &note.bend {
        for p in &bend.points {
            phases.extend([(p.at - 1e-9).max(0.), p.at]);
        }
    }
    phases.sort_by(f64::total_cmp);
    phases.dedup();
    phases
        .into_iter()
        .map(|phase| {
            let (x, _) = place.screen(phase, 0.5);
            (x, place.centre - note.bend_at(phase) * place.key)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PPB: f64 = 48.;
    const KEY: f64 = 12.;

    fn beats(n: f64) -> Ticks {
        Ticks::from_beats_f64(n)
    }

    /// A C (row 67 of a full keyboard) from beat 1 for two beats.
    fn c(bend: Option<Curve>) -> MidiNote {
        MidiNote {
            start: beats(1.),
            length: beats(2.),
            pitch: 60,
            velocity: 100,
            bend,
        }
    }

    fn place(note: &MidiNote) -> Place {
        Place::of(note, 127 - usize::from(note.pitch), PPB, KEY)
    }

    fn glide(from: f64, to: f64) -> Option<Curve> {
        Some(Curve::line(bend_value(from), bend_value(to)))
    }

    fn quarter(t: Ticks) -> Ticks {
        let q = beats(0.25).0;
        Ticks((t.0 as f64 / q as f64).round() as i64 * q)
    }

    fn drag(dx: f64, dy: f64) -> Drag<'static> {
        Drag {
            dx,
            dy,
            pixels_per_beat: PPB,
            snap: &quarter,
            free: false,
        }
    }

    #[test]
    fn a_note_sits_on_its_row_and_a_semitone_is_a_row() {
        let note = c(glide(0., 2.));
        let at = place(&note);
        assert_eq!(at.screen(0., bend_value(0.)), (48., 67.5 * KEY));
        assert_eq!(at.screen(1., bend_value(2.)), (48. + 96., 65.5 * KEY));
        assert!(at.contains(&note, 140., 65.5 * KEY), "on the bent line");
        assert!(!at.contains(&note, 140., 67.5 * KEY), "not its old row");
    }

    #[test]
    fn dragging_the_end_dot_up_glides_the_note_away() {
        let note = c(None);
        let bent = drag_point(&note, 1, place(&note), drag(0., -5. * KEY));
        assert_eq!(bent.bend, glide(0., 5.), "a straight glide up a fourth");
        assert_eq!((bent.start, bent.length), (note.start, note.length));
    }

    #[test]
    fn dragging_the_front_dot_down_slides_up_into_the_note() {
        let note = c(None);
        let bent = drag_point(&note, 0, place(&note), drag(0., 3. * KEY));
        assert_eq!(bent.bend, glide(-3., 0.));
        assert_eq!(bent.pitch, 60, "still a C");
    }

    #[test]
    fn dots_snap_to_semitones_unless_free() {
        let note = c(None);
        let near = drag_point(&note, 1, place(&note), drag(0., -1.4 * KEY));
        assert_eq!(near.bend, glide(0., 1.));
        let free = Drag {
            free: true,
            ..drag(0., -1.4 * KEY)
        };
        let off = drag_point(&note, 1, place(&note), free);
        assert!((off.bend_at(1.) - 1.4).abs() < 1e-9);
    }

    #[test]
    fn dragging_an_end_sideways_resizes_and_the_bend_stretches() {
        let note = c(glide(0., 12.));
        let longer = drag_point(&note, 1, place(&note), drag(PPB, 0.));
        assert_eq!(longer.length, beats(3.));
        assert_eq!(longer.bend, note.bend, "the same glide, over three beats");
        let later = drag_point(&note, 0, place(&note), drag(0.5 * PPB, 0.));
        assert_eq!((later.start, later.length), (beats(1.5), beats(1.5)));
        let never_empty = drag_point(&note, 1, place(&note), drag(-10. * PPB, 0.));
        assert_eq!(never_empty.length, MIN_LENGTH);
    }

    #[test]
    fn a_note_bent_back_home_is_plain_again() {
        let note = c(glide(0., 5.));
        let home = drag_point(&note, 1, place(&note), drag(0., 5. * KEY));
        assert_eq!(home.bend, None);
    }

    #[test]
    fn double_clicking_the_line_adds_a_point_that_keeps_its_shape() {
        let note = c(glide(0., 4.));
        let at = place(&note);
        let (with, index) = add_point(&note, 48. + 48., PPB, &quarter).unwrap();
        assert_eq!(index, 1);
        assert!((with.bend_at(0.5) - 2.).abs() < 1e-9, "on the line");
        assert!((with.bend_at(0.25) - note.bend_at(0.25)).abs() < 1e-9);
        // An inner point moves in time on the grid, between its neighbours.
        let moved = drag_point(&with, 1, at, drag(0.5 * PPB, -2. * KEY));
        let p = moved.bend.as_ref().unwrap().points[1];
        assert!((p.at - 0.75).abs() < 1e-9);
        assert_eq!(p.value, bend_value(4.));
        // Removing it leaves the glide; the ends never go.
        assert_eq!(remove_point(&with, 1).unwrap().bend, note.bend);
        assert_eq!(remove_point(&with, 0), None);
        // Off the note or on its ends adds nothing.
        assert_eq!(add_point(&note, 48., PPB, &quarter), None);
    }

    #[test]
    fn presses_find_handles_then_dots_then_lines() {
        let smooth = Curve {
            points: vec![
                CurvePoint::corner(0., bend_value(0.)),
                CurvePoint::corner(0.5, bend_value(3.)).with_handles(
                    PointKind::Smooth,
                    Handle::new(-0.25, 0.),
                    Handle::new(0.25, 0.),
                ),
                CurvePoint::corner(1., bend_value(0.)),
            ],
        };
        let notes = [c(Some(smooth))];
        let at = place(&notes[0]);
        let place = |i: usize| (i == 0).then_some(at);
        let (mx, my) = at.screen(0.5, bend_value(3.));
        let (hx, hy) = (mx + 24., my);
        assert_eq!(
            hit(&notes, &place, Some((0, 1)), hx + 2., hy),
            Some(BendHit::Handle {
                note: 0,
                point: 1,
                side: Side::Out
            })
        );
        assert!(
            !matches!(
                hit(&notes, &place, None, hx + 2., hy),
                Some(BendHit::Handle { .. })
            ),
            "handles only show on the selected point"
        );
        assert_eq!(
            hit(&notes, &place, None, mx + 3., my - 3.),
            Some(BendHit::Point { note: 0, point: 1 })
        );
        let (ex, ey) = at.screen(1., bend_value(0.));
        assert_eq!(
            hit(&notes, &place, None, ex - 2., ey),
            Some(BendHit::Point { note: 0, point: 2 })
        );
        assert_eq!(hit(&notes, &place, None, mx, my + 3. * KEY), None);
        assert_eq!(bent_at(&notes, &place, mx, my), Some(0));
    }

    #[test]
    fn a_handle_dragged_bends_the_line_between_points() {
        let note = c(glide(0., 6.));
        let at = place(&note);
        let curved = set_kind(&note, 0, PointKind::Broken, at);
        let (sx, sy) = at.screen(0., bend_value(0.));
        // Pull the start's handle flat and out: a slow start.
        let eased = drag_handle(&curved, 0, Side::Out, at, sx + 60., sy);
        assert!(eased.bend_at(0.25) < note.bend_at(0.25));
        assert_eq!(eased.bend_at(1.), 6.);
    }

    #[test]
    fn a_box_catches_a_bent_note_where_its_line_runs() {
        let note = c(glide(0., 12.));
        let at = place(&note);
        // Near its end it is up by the octave, not on its own row.
        let (x, y) = at.screen(0.9, bend_value(10.8));
        assert!(crosses(&note, at, (x - 1., y - 1., x + 1., y + 1.)));
        let home = at.screen(0.9, bend_value(0.));
        assert!(!crosses(
            &note,
            at,
            (home.0 - 1., home.1 - 1., home.0 + 1., home.1 + 1.)
        ));
    }

    #[test]
    fn the_line_follows_the_bend_with_sharp_corners() {
        let note = c(glide(0., 12.));
        let at = place(&note);
        let line = line(&note, at, 4.);
        assert_eq!(line.first(), Some(&(48., 67.5 * KEY)));
        assert_eq!(line.last(), Some(&(144., 55.5 * KEY)));
        assert!(line.windows(2).all(|w| w[0].0 <= w[1].0));
    }
}
