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

/// docs/behaviours.md: "Drag the top of a clip's right end (or its loop
/// icon) to loop it".
#[test]
fn dragging_the_top_of_a_clips_end_loops_it() {
    let mut h = Harness::new("arrange-loop");
    let lane = h.arrange_lane(FIRST_NODE);
    let (start, length) = h.view.read_with(h.cx, |v, _| {
        let node = v.session().snapshot().node(FIRST_NODE).unwrap().clone();
        let clip = &node.clips[0].1;
        (clip.start, clip.length)
    });
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    let end_x = lane.left() + px((start + length).as_beats() as f32 * ppb);
    // The top quarter of the clip's right edge, just inside it.
    let grab = point(
        end_x - px(2.),
        lane.top() + px(crate::clip_paint::CLIP_INSET + 2.),
    );
    // Drag out one more cycle.
    let to = point(grab.x + px(length.as_beats() as f32 * ppb), grab.y);
    h.drag(grab, to, 6);

    let snapshot = h.snapshot();
    let clip = &snapshot.node(FIRST_NODE).unwrap().clips[0].1;
    assert_eq!(clip.length, length + length, "two cycles");
    assert_eq!(
        clip.looping,
        Some(daw_engine::ClipLoop::new(length)),
        "looped on its own length"
    );
}
