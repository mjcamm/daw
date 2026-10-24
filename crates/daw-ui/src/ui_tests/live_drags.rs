//! Drags are heard as they move: sliders follow the pointer, and whatever a
//! drag changes plays at once, landing as one undo step on release.

use super::Harness;
use crate::mixer::{pan_id, volume_id};
use crate::project::Page;
use daw_engine::{FIRST_NODE, ParamId, Ticks};
use gpui::{point, px};

/// The slider's thumb size: the travel is the slider's length less this.
const THUMB: f32 = 9.;

fn mixer(name: &str) -> Harness {
    let mut h = Harness::new(name);
    h.view.update(h.cx, |view, _| view.page = Page::Mixer);
    h.redraw_all();
    h
}

/// A parameter's resting value as the engine plays it now.
fn heard(h: &mut Harness, param: ParamId) -> f64 {
    h.view.read_with(h.cx, |v, _| {
        v.session()
            .engine()
            .project()
            .param(param)
            .expect("param in the song")
            .base
    })
}

/// A parameter's resting value in the song (what the views show).
fn landed(h: &mut Harness, param: ParamId) -> f64 {
    let snapshot = h.snapshot();
    let node = snapshot.node(param.node).expect("node in the song");
    node.devices
        .iter()
        .find(|d| d.device == param.device)
        .expect("device on the node")
        .params[param.index as usize]
        .base
}

/// docs/behaviours.md: "Fader and pan follow the pointer and are heard
/// while dragged; each drag is one undo step".
#[test]
fn a_fader_follows_the_pointer_and_is_heard_while_dragged() {
    let mut h = mixer("live-fader");
    let param = volume_id(FIRST_NODE);
    let before = landed(&mut h, param);
    let fader = h.bounds("fader-1");
    let travel = fader.size.height / px(1.) - THUMB;

    // Down a quarter of the fader's travel: a quarter of the range.
    let from = fader.center();
    h.press(from);
    h.move_held(point(from.x, from.y + px(travel / 4.)));
    let expected = before - 0.25;
    assert!(
        (heard(&mut h, param) - expected).abs() < 1e-3,
        "heard before release"
    );
    assert_eq!(landed(&mut h, param), before, "not an edit yet");

    h.release(point(from.x, from.y + px(travel / 4.)));
    assert!((landed(&mut h, param) - expected).abs() < 1e-3);
    h.keys("ctrl-z");
    assert_eq!(landed(&mut h, param), before, "one undo step");
    assert_eq!(heard(&mut h, param), before);
}

/// docs/behaviours.md: "Fader and pan follow the pointer and are heard
/// while dragged; each drag is one undo step".
#[test]
fn a_pan_follows_the_pointer_and_is_heard_while_dragged() {
    let mut h = mixer("live-pan");
    let param = pan_id(FIRST_NODE);
    let pan = h.bounds("pan-1");
    let travel = pan.size.width / px(1.) - THUMB;
    // From the centre, right a third of the way: past the centre detent.
    let from = pan.center();
    h.press(from);
    h.move_held(point(from.x + px(travel / 3.), from.y));
    let expected = 0.5 + 1. / 3.;
    assert!((heard(&mut h, param) - expected).abs() < 1e-3);
    h.release(point(from.x + px(travel / 3.), from.y));
    assert!((landed(&mut h, param) - expected).abs() < 1e-3);
}

/// docs/behaviours.md: "A clip drag is heard as it moves; Escape puts it
/// back".
#[test]
fn a_clip_drag_is_heard_as_it_moves_and_escape_puts_it_back() {
    let mut h = Harness::new("live-clip");
    let (id, start) = {
        let snapshot = h.snapshot();
        let (id, clip) = snapshot.node(FIRST_NODE).unwrap().clips[0].clone();
        (id, clip.start)
    };
    let heard_start = |h: &mut Harness| {
        h.view.read_with(h.cx, |v, _| {
            let project = v.session().engine().project();
            project.node(FIRST_NODE).unwrap().clips[&id].start
        })
    };
    let lane = h.arrange_lane(FIRST_NODE);
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.arrange_view.zoom.pixels_per_beat);
    // The clip's body, a beat in, then four beats on.
    let grab = point(
        lane.left() + px((start.as_beats() as f32 + 1.) * ppb),
        lane.center().y,
    );
    h.press(grab);
    h.move_held(point(grab.x + px(2. * ppb), grab.y));
    h.move_held(point(grab.x + px(4. * ppb), grab.y));
    assert_eq!(heard_start(&mut h), start + Ticks::from_beats(4));

    h.keys("escape");
    assert_eq!(heard_start(&mut h), start, "back where it was");
    h.release(point(grab.x + px(4. * ppb), grab.y));
    assert_eq!(heard_start(&mut h), start);
    assert!(!h.snapshot().can_undo, "nothing changed");
}
