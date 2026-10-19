//! Arrange: behaviours on the lanes.

use super::Harness;
use daw_engine::{FIRST_NODE, Ticks};
use gpui::{point, px};

/// docs/behaviours.md: "Double-click an empty MIDI lane".
#[test]
fn double_clicking_an_empty_midi_lane_adds_a_one_bar_clip_and_opens_the_editor() {
    let mut h = Harness::new("arrange-new-clip");
    let before = h.snapshot().node(FIRST_NODE).unwrap().clips.len();
    let lane = h.arrange_lane(FIRST_NODE);
    // The starter clip covers bars 1 to 4; bar 6 is empty.
    let zoom = h.view.read_with(h.cx, |v, _| v.arrange_view.zoom);
    let x = lane.left() + px(zoom.beat_to_x(20.) + 2.);
    h.click(point(x, lane.center().y), 2);

    let snapshot = h.snapshot();
    let clips = &snapshot.node(FIRST_NODE).unwrap().clips;
    assert_eq!(clips.len(), before + 1, "a new clip");
    let (id, clip) = clips.iter().max_by_key(|(id, _)| id.0).unwrap();
    assert_eq!(
        clip.start,
        Ticks::from_beats(20),
        "on the grid line under the pointer"
    );
    assert_eq!(clip.length, Ticks::from_beats(4), "a bar long");
    assert!(clip.notes.is_empty());
    h.view.read_with(h.cx, |v, _| {
        assert!(v.selected_clips.contains(id), "selected");
        assert!(v.note_editor.is_some(), "the editor opened");
    });
}
