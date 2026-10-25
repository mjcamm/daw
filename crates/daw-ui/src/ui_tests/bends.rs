//! Bent notes in the piano roll (`docs/design/note-bends.md`).

use super::Harness;
use super::piano_roll::editor_with_one_note_at;
use daw_engine::{
    Command, Curve, CurvePoint, Edit, FIRST_NODE, MidiNote, PointKind, Ticks, bend_value,
};
use gpui::{Pixels, Point, point, px};

/// The one note on the synth's clip.
fn the_note(h: &mut Harness) -> MidiNote {
    let snapshot = h.snapshot();
    snapshot.node(FIRST_NODE).expect("the synth").clips[0]
        .1
        .notes[0]
        .clone()
}

/// Replaces the one note's bend.
fn bend(h: &mut Harness, bend: Option<Curve>) {
    h.view.update(h.cx, |view, _| {
        let (id, mut clip) = view
            .session()
            .snapshot()
            .node(FIRST_NODE)
            .expect("the synth")
            .clips[0]
            .clone();
        clip.notes[0].bend = bend;
        view.session.apply_or_panic(Command::Edit(Edit::SetClip {
            id,
            node: FIRST_NODE,
            clip,
        }));
    });
    h.redraw();
}

/// The editor open on one note, tall enough that dots dragged an octave
/// stay clear of the edges (where a drag scrolls the view).
fn tall(name: &str) -> (Harness, impl Fn(&mut Harness, f64) -> Point<Pixels>) {
    editor_with_one_note_at(name, 640.)
}

fn glide(from: f64, to: f64) -> Option<Curve> {
    Some(Curve::line(bend_value(from), bend_value(to)))
}

/// One row (a semitone) on screen.
fn key(h: &mut Harness) -> Pixels {
    h.view
        .read_with(h.cx, |v, _| px(v.notes_view.zoom.row_height))
}

fn beat_width(h: &mut Harness) -> Pixels {
    h.view
        .read_with(h.cx, |v, _| px(v.notes_view.zoom.pixels_per_beat))
}

fn up(by: Pixels) -> Point<Pixels> {
    point(px(0.), -by)
}

/// A drag that stays in the editor's hands until released, checking
/// mid-way that it would be heard (`ProjectView::drag_edit`).
fn drag_heard(h: &mut Harness, from: Point<Pixels>, to: Point<Pixels>) {
    h.press(from);
    let middle = point((from.x + to.x) / 2., (from.y + to.y) / 2.);
    h.move_held(middle);
    h.move_held(to);
    let heard = h.view.read_with(h.cx, |v, _| v.note_drag_edit().is_some());
    assert!(heard, "the drag is heard as it moves");
    h.release(to);
}

/// docs/behaviours.md: "Bend tool (4): drag a note's end dot up or down to
/// bend it (a straight glide, snapped to semitones); heard as it moves, one
/// undo step".
#[test]
fn dragging_the_end_dot_bends_the_note_and_undoes_in_one_step() {
    let (mut h, at) = tall("bend-end-dot");
    h.keys("4");
    let end = at(&mut h, 2.);
    let five = key(&mut h) * 5.;
    drag_heard(&mut h, end, end + up(five));
    let note = the_note(&mut h);
    assert_eq!(note.bend, glide(0., 5.), "a straight glide up a fourth");
    assert_eq!((note.pitch, note.length), (60, Ticks::from_beats(2)));
    h.keys("ctrl-z");
    assert_eq!(the_note(&mut h).bend, None, "one undo step");
}

/// docs/behaviours.md: "Drag a note's front dot down and it slides up into
/// the note; it stays the same note".
#[test]
fn dragging_the_front_dot_down_slides_up_into_the_note() {
    let (mut h, at) = tall("bend-front-dot");
    h.keys("4");
    let start = at(&mut h, 0.) + point(px(1.), px(0.));
    let three = key(&mut h) * 3.;
    h.drag(start, start - up(three), 4);
    let note = the_note(&mut h);
    assert_eq!(note.bend, glide(-3., 0.));
    assert_eq!(note.pitch, 60, "still a C");
}

/// docs/behaviours.md: "An end dot dragged sideways changes the note's
/// length; its bend stretches with it".
#[test]
fn an_end_dot_dragged_sideways_resizes_the_note() {
    let (mut h, at) = tall("bend-resize");
    h.keys("4");
    bend(&mut h, glide(0., 7.));
    let beat = beat_width(&mut h);
    let seven = key(&mut h) * 7.;
    let end = at(&mut h, 2.) + up(seven);
    h.drag(end, end + point(beat, px(0.)), 4);
    let note = the_note(&mut h);
    assert_eq!(note.length, Ticks::from_beats(3));
    assert_eq!(note.bend, glide(0., 7.), "the same glide, stretched");
}

/// docs/behaviours.md: "Double-click a note's line to add a bend point;
/// double-click a point to remove it (never the ends)".
#[test]
fn double_clicks_add_and_remove_bend_points() {
    let (mut h, at) = tall("bend-points");
    h.keys("4");
    bend(&mut h, glide(0., 4.));
    // Halfway along, the line is two semitones up.
    let two = key(&mut h) * 2.;
    let middle = at(&mut h, 1.) + up(two);
    h.click(middle, 2);
    let points = the_note(&mut h).bend.expect("bent").points;
    assert_eq!(points.len(), 3);
    assert_eq!(points[1].at, 0.5);
    h.click(middle, 2);
    assert_eq!(the_note(&mut h).bend, glide(0., 4.));
    // The end stays.
    let four = key(&mut h) * 4.;
    let end = at(&mut h, 2.) + up(four);
    h.click(end, 2);
    assert_eq!(the_note(&mut h).bend, glide(0., 4.));
}

/// docs/behaviours.md: "With the Bend tool, dragging a note anywhere off its
/// dots (a click on its line too) moves it whole, bend and all".
#[test]
fn dragging_a_bent_note_off_its_dots_moves_it_whole() {
    let (mut h, at) = tall("bend-move");
    h.keys("4");
    bend(&mut h, glide(0., 6.));
    let beat = beat_width(&mut h);
    let row = key(&mut h);
    // A quarter of the way along, the line is one and a half up.
    let on_line = at(&mut h, 0.5) + up(row * 1.5);
    h.drag(on_line, on_line + point(beat, -row * 2.), 4);
    let note = the_note(&mut h);
    assert_eq!(note.start, Ticks::from_beats(1));
    assert_eq!(note.pitch, 62);
    assert_eq!(note.bend, glide(0., 6.));
}

/// docs/behaviours.md: "Corner, Smooth or Broken for the selected bend
/// point; a curved point's handles drag to shape the bend".
#[test]
fn a_selected_bend_point_turns_smooth_and_its_handle_curves_the_bend() {
    let (mut h, at) = tall("bend-kind");
    h.keys("4");
    bend(
        &mut h,
        Some(Curve {
            points: vec![
                CurvePoint::corner(0., bend_value(0.)),
                CurvePoint::corner(0.5, bend_value(4.)),
                CurvePoint::corner(1., bend_value(0.)),
            ],
        }),
    );
    let row = key(&mut h);
    let tip = at(&mut h, 1.) + up(row * 4.);
    h.click(tip, 1);
    // Smooth is the middle of the three.
    let kinds = h.bounds("bend-kind");
    h.click(
        point(kinds.origin.x + kinds.size.width / 2., kinds.center().y),
        1,
    );
    let points = the_note(&mut h).bend.expect("bent").points;
    assert_eq!(points[1].kind, PointKind::Smooth);
    // Smooth turns the tip's handles flat (its neighbours are level), a
    // third of the way out: pull the out-handle up and the line after the
    // tip rises above where it was.
    let before = the_note(&mut h).bend_at(0.6);
    let beat = beat_width(&mut h);
    let handle = tip + point(beat / 3., px(0.));
    h.drag(handle, handle + up(row * 2.), 4);
    let after = the_note(&mut h);
    assert!(
        after.bend_at(0.6) > before,
        "{} > {before}",
        after.bend_at(0.6)
    );
}

/// docs/behaviours.md: "Bent notes draw along their bends in every tool, and
/// the other tools find them there".
#[test]
fn the_select_tool_finds_a_bent_note_along_its_line() {
    let (mut h, at) = tall("bend-select");
    bend(&mut h, glide(0., 12.));
    let row = key(&mut h);
    // Near the end the note is almost an octave up, far from its own row.
    let high = at(&mut h, 1.75) + up(row * 10.5);
    h.click(high, 1);
    let selected = h.view.read_with(h.cx, |v, _| {
        v.note_editor.as_ref().map(|e| e.selection.clone())
    });
    assert_eq!(selected, Some([0].into()));
    // Its own row there is empty now.
    let home = at(&mut h, 1.75);
    h.click(home, 1);
    let selected = h.view.read_with(h.cx, |v, _| {
        v.note_editor.as_ref().map(|e| e.selection.clone())
    });
    assert_eq!(selected, Some([].into()));
}
