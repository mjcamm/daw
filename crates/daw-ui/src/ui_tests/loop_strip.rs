//! The loop strip at the top of every ruler (`docs/design/loop-strip.md`).

use super::Harness;
use crate::block_edit::{Grid, Snap};
use daw_engine::{FIRST_NODE, LoopRange, Ticks};
use gpui::{Pixels, Point, point, px};

fn beats(b: f64) -> Ticks {
    Ticks::from_beats_f64(b)
}

fn range(start: f64, end: f64) -> Option<LoopRange> {
    Some(LoopRange {
        start: beats(start),
        end: beats(end),
    })
}

/// A point on Arrange's loop strip at `beat`.
fn on_main(h: &mut Harness, beat: f64) -> Point<Pixels> {
    let strip = h.bounds("loop-strip-main");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    point(strip.left() + px(beat as f32 * ppb), strip.center().y)
}

fn drag_main(h: &mut Harness, from: f64, to: f64) {
    let (from, to) = (on_main(h, from), on_main(h, to));
    h.drag(from, to, 6);
}

/// docs/behaviours.md: "Drag on a ruler's loop strip draws a loop".
#[test]
fn dragging_on_the_loop_strip_draws_a_loop_that_plays() {
    let mut h = Harness::new("loop-draw");
    drag_main(&mut h, 4.1, 11.9);
    let snapshot = h.snapshot();
    assert_eq!(snapshot.loop_range, range(4., 12.), "snapped to the grid");
    assert!(snapshot.loop_enabled, "a new loop plays");
    assert_eq!(snapshot.position_beats, 0., "the playhead stays put");
}

/// docs/behaviours.md: "Drag a loop's start or end handle".
#[test]
fn dragging_a_handle_moves_only_that_end() {
    let mut h = Harness::new("loop-handles");
    drag_main(&mut h, 4., 12.);
    drag_main(&mut h, 12., 16.);
    assert_eq!(h.snapshot().loop_range, range(4., 16.), "the end");
    drag_main(&mut h, 4., 0.);
    assert_eq!(h.snapshot().loop_range, range(0., 16.), "the start");
}

/// docs/behaviours.md: "any other drag on the strip draws a fresh loop".
#[test]
fn a_drag_inside_the_loop_draws_a_fresh_one() {
    let mut h = Harness::new("loop-fresh");
    drag_main(&mut h, 4., 16.);
    drag_main(&mut h, 8., 10.);
    assert_eq!(h.snapshot().loop_range, range(8., 10.));
}

/// docs/behaviours.md: "Snap or Slide and the grid apply".
#[test]
fn sliding_leaves_the_loop_off_the_grid() {
    let mut h = Harness::new("loop-slide");
    h.view.update(h.cx, |v, _| v.snap.enabled = false);
    drag_main(&mut h, 4.3, 11.6);
    let made = h.snapshot().loop_range.unwrap();
    let beat = Ticks::from_beats(1).0;
    assert_ne!(made.start.0 % beat, 0, "{made:?}");
    assert_ne!(made.end.0 % beat, 0, "{made:?}");
}

/// docs/behaviours.md: "The transport's loop button switches looping".
#[test]
fn the_loop_button_switches_looping_and_keeps_the_region() {
    let mut h = Harness::new("loop-button");
    drag_main(&mut h, 4., 12.);
    let button = h.bounds("transport-loop").center();
    h.click(button, 1);
    let snapshot = h.snapshot();
    assert!(!snapshot.loop_enabled);
    assert_eq!(snapshot.loop_range, range(4., 12.), "the region stays");
    h.click(button, 1);
    assert!(h.snapshot().loop_enabled);
}

/// docs/behaviours.md: "every ruler has it (piano roll: its own grid)".
#[test]
fn the_piano_roll_strip_uses_the_rolls_grid() {
    let mut h = Harness::new("loop-roll");
    // Open the editor on the starter clip.
    let lane = h.arrange_lane(FIRST_NODE);
    h.click(point(lane.left() + px(20.), lane.center().y), 2);
    assert!(h.view.read_with(h.cx, |v, _| v.note_editor.is_some()));
    h.view.update(h.cx, |v, _| {
        v.note_grid = Snap {
            enabled: true,
            grid: Grid::Bar,
        };
    });
    h.redraw();

    let strip = h.bounds("loop-strip-notes");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.notes_view.zoom.pixels_per_beat);
    let at = |beat: f32| point(strip.left() + px(beat * ppb), strip.center().y);
    h.drag(at(1.), at(7.), 6);
    assert_eq!(h.snapshot().loop_range, range(0., 8.), "whole bars");
}
