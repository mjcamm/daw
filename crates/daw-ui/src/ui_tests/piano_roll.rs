//! The piano roll: editing notes.

use super::Harness;
use daw_engine::{Command, Edit, FIRST_NODE, MidiClip, MidiNote, Ticks};
use gpui::{Pixels, Point, point, px};

/// The editor open on the synth, its one clip holding a single note C4
/// over the first two beats. Returns where (beat, pitch) is on screen.
fn editor_with_one_note(name: &str) -> (Harness, impl Fn(&mut Harness, f64) -> Point<Pixels>) {
    let mut h = Harness::new(name);
    h.view.update(h.cx, |view, _| {
        let (id, clip) = view
            .session()
            .snapshot()
            .node(FIRST_NODE)
            .expect("the synth")
            .clips[0]
            .clone();
        view.session.apply_or_panic(Command::Edit(Edit::SetClip {
            id,
            node: FIRST_NODE,
            clip: MidiClip {
                notes: vec![MidiNote {
                    start: Ticks::ZERO,
                    length: Ticks::from_beats(2),
                    pitch: 60,
                    velocity: 100,
                }],
                ..clip
            },
        }));
        view.open_node_editor(FIRST_NODE, 0.);
        // Already slid up: the grid stays put while the test points at it.
        if let Some(editor) = &mut view.note_editor {
            editor.opened = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(1))
                .unwrap_or(editor.opened);
        }
    });
    // The editor fits itself to its notes over its first frames; then the
    // note goes in the middle of the grid, clear of the edges where drags
    // scroll it.
    for _ in 0..4 {
        h.redraw();
    }
    h.view.update(h.cx, |v, _| {
        let editor = v.note_editor.as_ref().expect("the editor");
        let row = editor.rows.row_of(60).expect("C4 shows") as f32;
        let handle = &v.notes_view.x;
        let view = handle.bounds().size.height / px(1.);
        let middle = (row + 0.5) * v.notes_view.zoom.row_height - view / 2.;
        crate::scroll::scroll_to(handle, crate::slider::Axis::Vertical, middle.max(0.));
    });
    h.redraw();
    h.redraw();
    let on_screen = |h: &mut Harness, beat: f64| {
        h.view.read_with(h.cx, |v, _| {
            let editor = v.note_editor.as_ref().expect("the editor");
            let zoom = v.notes_view.zoom;
            let grid = editor.grid.get();
            let row = editor.rows.row_of(60).expect("C4 shows") as f32;
            point(
                grid.origin.x + px(beat as f32 * zoom.pixels_per_beat),
                grid.origin.y + px((row + 0.5) * zoom.row_height),
            )
        })
    };
    (h, on_screen)
}

fn on_edge(h: &mut Harness) -> bool {
    h.view.read_with(h.cx, |v, _| {
        v.note_editor
            .as_ref()
            .and_then(|e| e.hover_key.get())
            .is_some_and(|(_, _, edge)| edge)
    })
}

fn note_length(h: &mut Harness) -> Ticks {
    let snapshot = h.snapshot();
    snapshot.node(FIRST_NODE).expect("the synth").clips[0]
        .1
        .notes[0]
        .length
}

/// docs/behaviours.md: "The piano roll's resize cursor follows the notes as
/// they are: after a drag, moving away and back".
#[test]
fn after_a_note_resize_moving_away_and_back_shows_the_right_cursor() {
    let (mut h, at) = editor_with_one_note("roll-cursor-away-back");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.notes_view.zoom.pixels_per_beat);
    // The note's end, a little inside; shortened by a beat.
    let grab = at(&mut h, 2.) - point(px(2.), px(0.));
    let to = grab - point(px(ppb), px(0.));
    h.hover(grab);
    assert!(on_edge(&mut h), "on the end");
    h.press(grab);
    h.move_held(grab - point(px(ppb / 2.), px(0.)));
    h.move_held(to);
    h.release(to);
    assert_eq!(note_length(&mut h), Ticks::from_beats(1));
    assert!(on_edge(&mut h), "on the new end");
    let body = at(&mut h, 0.5);
    h.hover(body);
    assert!(!on_edge(&mut h), "the body");
    h.hover(to);
    assert!(on_edge(&mut h), "back on the end");
    let past = at(&mut h, 1.75);
    h.hover(past);
    assert!(!on_edge(&mut h), "empty row");
    h.hover(to);
    assert!(on_edge(&mut h), "and back again");
}
