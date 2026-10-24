//! Rulers: a click below the loop strip places the playhead, on every page
//! and panel.

use super::Harness;
use daw_engine::{Command, Edit, Node, NodeId, NodeKind, Ticks};
use gpui::{point, px};

/// Clicks the ruler under `strip` (its loop strip's selector) `beats` in,
/// at `ppb` pixels per beat; returns where the playhead was sent.
fn click_ruler(h: &mut Harness, strip: &'static str, beats: f32, ppb: f32) -> Option<Ticks> {
    let strip = h.bounds(strip);
    // Below the loop strip, inside the ruler.
    let at = point(strip.left() + px(beats * ppb + 1.), strip.bottom() + px(4.));
    h.click(at, 1);
    h.view.read_with(h.cx, |v, _| v.session().located)
}

fn settled(h: &mut Harness) {
    // Bottom panels slide up in real time: start them up already.
    h.view.update(h.cx, |v, _| {
        let past = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(1));
        if let (Some(editor), Some(past)) = (&mut v.note_editor, past) {
            editor.opened = past;
        }
        if let (Some(editor), Some(past)) = (&mut v.audio_editor, past) {
            editor.opened = past;
        }
    });
    h.redraw_all();
    h.redraw_all();
}

/// docs/behaviours.md: "Click a ruler below its loop strip to place the
/// playhead (Arrange, Parameters, Signals, piano roll, audio panel)".
#[test]
fn clicking_the_piano_rolls_ruler_places_the_playhead() {
    let mut h = Harness::new("ruler-notes");
    h.view
        .update(h.cx, |v, _| v.open_node_editor(daw_engine::FIRST_NODE, 0.));
    settled(&mut h);
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.notes_view.zoom.pixels_per_beat);
    let looped = h.snapshot().loop_range;
    let at = click_ruler(&mut h, "loop-strip-notes", 8., ppb);
    assert_eq!(at, Some(Ticks::from_beats(8)), "bar 3");
    assert_eq!(h.snapshot().loop_range, looped, "the loop stays");
}

/// docs/behaviours.md: "Click a ruler below its loop strip ...".
#[test]
fn clicking_the_audio_panels_ruler_places_the_playhead() {
    let mut h = Harness::new("ruler-audio");
    let audio = NodeId(500);
    h.view.update(h.cx, |v, _| {
        v.session.apply_or_panic(Command::Edit(Edit::AddNode {
            id: audio,
            parent: NodeId::ROOT,
            index: 1,
            node: Node::new("Audio", 2).with_kind(NodeKind::Audio),
        }));
        v.open_audio_editor(audio, 0.);
    });
    settled(&mut h);
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.notes_view.zoom.pixels_per_beat);
    let at = click_ruler(&mut h, "loop-strip-audio", 6., ppb);
    assert_eq!(at, Some(Ticks::from_beats(6)));
}

/// docs/behaviours.md: "Click a ruler below its loop strip ...": Arrange,
/// through the same code.
#[test]
fn clicking_arranges_ruler_places_the_playhead() {
    let mut h = Harness::new("ruler-main");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    let at = click_ruler(&mut h, "loop-strip-main", 4., ppb);
    assert_eq!(at, Some(Ticks::from_beats(4)));
}
