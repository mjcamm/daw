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

/// docs/behaviours.md: "Drag a clip's edge to resize ... a press on its
/// outline or just past it takes the end".
#[test]
fn dragging_from_just_past_a_clips_end_resizes_it() {
    let mut h = Harness::new("arrange-resize-outside");
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
    // Two pixels past the outline, low on the clip (clear of the loop zone).
    let grab = point(end_x + px(2.), lane.bottom() - px(12.));
    h.drag(grab, point(grab.x + px(4. * ppb), grab.y), 6);
    let snapshot = h.snapshot();
    let clip = &snapshot.node(FIRST_NODE).unwrap().clips[0].1;
    assert_eq!(clip.start, start, "the start stays");
    assert_eq!(clip.length, length + Ticks::from_beats(4), "a bar longer");
}

/// docs/behaviours.md: "The cursor on Arrange's lanes follows the clips as
/// they are: right after a drag lands, and through the whole drag".
#[test]
fn the_resize_cursor_follows_the_clip_after_a_drag() {
    use crate::clip_edit::Grab;
    let mut h = Harness::new("arrange-cursor-after-drag");
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
    let grab = point(end_x - px(2.), lane.bottom() - px(12.));
    let shown = |h: &mut Harness| {
        h.view.read_with(h.cx, |v, _| {
            v.arrange_hover.get().map(|(n, _, grab)| (n, grab))
        })
    };
    h.move_to(grab);
    assert_eq!(
        shown(&mut h),
        Some((FIRST_NODE, Grab::End)),
        "hovering the end"
    );

    // Drag the end a bar on, straying down off the row as hands do.
    let to = point(grab.x + px(4. * ppb), grab.y + px(40.));
    h.press(grab);
    h.move_held(to);
    assert_eq!(
        h.view.read_with(h.cx, |v, _| v.drag_cursor()),
        Some(gpui::CursorStyle::ResizeLeftRight),
        "the resize cursor holds off the row"
    );
    // Back onto the row and let go, without moving after.
    let back = point(to.x, grab.y);
    h.move_held(back);
    h.release(back);
    assert_eq!(
        shown(&mut h),
        Some((FIRST_NODE, Grab::End)),
        "the pointer is on the clip's new end"
    );

    // Undo puts the end back under a still pointer: no resize cursor left.
    h.keys("ctrl-z");
    assert_eq!(shown(&mut h), None, "nothing under the pointer now");
}

/// docs/behaviours.md: "The cursor on the lanes follows the clips as they
/// are ...": after a resize either way, moving into the body shows the
/// plain cursor, back to the end the resize one, past it none.
#[test]
fn after_a_drag_moving_away_and_back_shows_the_right_cursor() {
    for beats in [-4., 4.] {
        away_and_back(beats);
    }
}

/// Drags the starter clip's end `beats` on (back if negative), lets go,
/// then hovers the body, the end and past it with real (cached) moves.
fn away_and_back(beats: f32) {
    use crate::clip_edit::Grab;
    let mut h = Harness::new(&format!("arrange-cursor-away-back-{beats}"));
    let lane = h.arrange_lane(FIRST_NODE);
    let (start, length) = h.view.read_with(h.cx, |v, _| {
        let node = v
            .session()
            .snapshot()
            .node(FIRST_NODE)
            .expect("the synth")
            .clone();
        let clip = &node.clips[0].1;
        (clip.start, clip.length)
    });
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    let end_x = lane.left() + px((start + length).as_beats() as f32 * ppb);
    let y = lane.bottom() - px(12.);
    let shown = |h: &mut Harness| {
        h.view
            .read_with(h.cx, |v, _| v.arrange_hover.get().map(|(_, _, grab)| grab))
    };
    let grab = point(end_x - px(2.), y);
    let to = point(grab.x + px(beats * ppb), y);
    h.press(grab);
    h.move_held(point(grab.x + px(beats / 2. * ppb), y));
    h.move_held(to);
    h.release(to);
    assert_eq!(shown(&mut h), Some(Grab::End), "{beats}: on the new end");
    h.hover(point(to.x - px(2. * ppb), y));
    assert_eq!(shown(&mut h), Some(Grab::Body), "{beats}: the body moves");
    h.hover(to);
    assert_eq!(shown(&mut h), Some(Grab::End), "{beats}: back on the end");
    h.hover(point(to.x + px(2. * ppb), y));
    assert_eq!(shown(&mut h), None, "{beats}: empty lane");
    h.hover(to);
    assert_eq!(shown(&mut h), Some(Grab::End), "{beats}: and back again");
}

/// Seeds the starter clip looped on its own length, two cycles; returns
/// its end on screen (x), the lane, its cycle and pixels per beat.
fn looped_starter(h: &mut Harness) -> (gpui::Pixels, gpui::Bounds<gpui::Pixels>, Ticks, f32) {
    let (id, clip) = h.view.read_with(h.cx, |v, _| {
        let node = v
            .session()
            .snapshot()
            .node(FIRST_NODE)
            .expect("the synth")
            .clone();
        node.clips[0].clone()
    });
    let cycle = clip.length;
    h.view.update(h.cx, |v, _| {
        v.session
            .apply(daw_engine::Command::Edit(daw_engine::Edit::SetClip {
                id,
                node: FIRST_NODE,
                clip: daw_engine::MidiClip {
                    length: cycle + cycle,
                    looping: Some(daw_engine::ClipLoop::new(cycle)),
                    ..clip.clone()
                },
            }));
    });
    h.redraw();
    let lane = h.arrange_lane(FIRST_NODE);
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    let end_x = lane.left() + px((clip.start + cycle + cycle).as_beats() as f32 * ppb);
    (end_x, lane, cycle, ppb)
}

/// docs/behaviours.md: "A looped clip's right end only drags repeats".
#[test]
fn a_looped_clips_end_drags_repeats_never_resizes() {
    use crate::clip_edit::Grab;
    let mut h = Harness::new("arrange-looped-end");
    let (end_x, lane, cycle, ppb) = looped_starter(&mut h);
    // Low on the end, where an unlooped clip resizes, and just past it.
    let low = point(end_x - px(2.), lane.bottom() - px(12.));
    for at in [low, point(end_x + px(2.), low.y)] {
        h.hover(at);
        assert_eq!(
            h.view
                .read_with(h.cx, |v, _| v.arrange_hover.get().map(|(_, _, g)| g)),
            Some(Grab::Loop)
        );
    }
    // Half a cycle on: a whole repeat more, never a part of one.
    h.drag(
        low,
        point(low.x + px(cycle.as_beats() as f32 * ppb * 0.6), low.y),
        6,
    );
    let snapshot = h.snapshot();
    let clip = &snapshot.node(FIRST_NODE).unwrap().clips[0].1;
    assert_eq!(clip.length, cycle + cycle + cycle, "three cycles");
    assert_eq!(clip.looping, Some(daw_engine::ClipLoop::new(cycle)));
}

/// docs/behaviours.md: "Drag the top of a clip's right end ... to loop it":
/// while dragging, the clip shows its repeats as they will look.
#[test]
fn dragging_out_a_loop_shows_its_repeats() {
    let mut h = Harness::new("arrange-loop-ghost");
    let (end_x, lane, cycle, ppb) = looped_starter(&mut h);
    let grab = point(end_x - px(2.), lane.bottom() - px(12.));
    h.press(grab);
    h.move_held(point(grab.x + px(cycle.as_beats() as f32 * ppb), grab.y));
    let seams = h.view.read_with(h.cx, |v, cx| {
        let (drag, plan) = v.clip_plan().expect("dragging");
        let theme = crate::theme::Theme::get(cx);
        v.drag_ghosts(drag.held(), &plan, &theme)[0].seams.len()
    });
    assert_eq!(seams, 2, "three cycles: a line at the end of the first two");
    h.release(grab);
}
