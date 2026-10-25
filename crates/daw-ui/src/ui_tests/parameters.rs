//! Parameters: editing blocks on layer lanes, as crisp as clips on Arrange.

use super::Harness;
use crate::parameters::CursorKind;
use crate::project::Page;
use daw_engine::{
    BlendMode, Block, BlockId, Command, DeviceRef, Edit, FIRST_NODE, Layer, LayerId, ParamId,
    Shape, Ticks, devices::channel,
};
use gpui::{Bounds, Pixels, point, px};

const VOLUME: ParamId = ParamId {
    node: FIRST_NODE,
    device: DeviceRef::Channel,
    index: channel::VOLUME,
};

fn beats(n: f64) -> Ticks {
    Ticks::from_beats_f64(n)
}

/// A flat block `length` beats long from the song start, looped every
/// `cycle` beats if given.
fn block(length: f64, cycle: Option<f64>) -> Block {
    let mut block = Block::new(Ticks::ZERO, beats(length), Shape::constant(0.25));
    block.cycle = cycle.map(beats);
    block
}

/// The Parameters page with the synth's volume layered by `block`; returns
/// the lane and pixels per beat.
fn page_with(name: &str, block: Block) -> (Harness, Bounds<Pixels>, f32) {
    let mut h = Harness::new(name);
    h.view.update(h.cx, |view, _| {
        let mut layer = Layer::named("Layer 1", BlendMode::Add);
        layer.blocks.push((BlockId(1), block));
        view.session.apply_or_panic(Command::Edit(Edit::AddLayer {
            param: VOLUME,
            id: LayerId(1),
            index: 0,
            layer,
        }));
        view.selected_node = Some(FIRST_NODE);
        view.page = Page::Parameters;
    });
    h.redraw_all();
    let lane = h.bounds("layer-lane-1");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.parameters_view.zoom.pixels_per_beat);
    (h, lane, ppb)
}

/// The block as the song has it now.
fn the_block(h: &mut Harness) -> Block {
    let snapshot = h.snapshot();
    let node = snapshot.node(FIRST_NODE).expect("the synth");
    let device = node
        .devices
        .iter()
        .find(|d| d.device == DeviceRef::Channel)
        .expect("the channel");
    let param = &device.params[channel::VOLUME as usize];
    param.layers[0].1.blocks[0].1.clone()
}

fn shown(h: &mut Harness) -> Option<CursorKind> {
    h.view
        .read_with(h.cx, |v, _| v.lane_hover.get().and_then(|hover| hover.kind))
}

/// docs/behaviours.md: "Block edges grab from their outline and just past
/// it".
#[test]
fn dragging_from_just_past_a_blocks_end_resizes_it() {
    let (mut h, lane, ppb) = page_with("params-resize-outside", block(4., None));
    let end_x = lane.left() + px(4. * ppb);
    let grab = point(end_x + px(2.), lane.bottom() - px(10.));
    h.drag(grab, point(grab.x + px(4. * ppb), grab.y), 6);
    assert_eq!(the_block(&mut h).length, beats(8.));
}

/// docs/behaviours.md: "A looped block's right end only drags repeats".
#[test]
fn a_looped_blocks_end_drags_repeats_never_resizes() {
    let (mut h, lane, ppb) = page_with("params-looped-end", block(4., Some(2.)));
    // Low on the end, where an unlooped block resizes.
    let grab = point(lane.left() + px(4. * ppb - 2.), lane.bottom() - px(10.));
    h.hover(grab);
    assert_eq!(shown(&mut h), Some(CursorKind::Loop));
    // Back past the first cycle: the loop tool only takes repeats away,
    // stopping at the block itself (a resize would cut into it).
    h.drag(grab, point(grab.x - px(3.4 * ppb), grab.y), 6);
    let block = the_block(&mut h);
    assert_eq!(block.length, beats(2.), "the block itself");
    assert_eq!(block.cycle, None, "plain again");
}

/// docs/behaviours.md: "The cursor on Parameters and Signals lanes follows
/// the blocks as they are ...".
#[test]
fn after_a_drag_moving_away_and_back_shows_the_right_cursor() {
    let (mut h, lane, ppb) = page_with("params-cursor-away-back", block(8., None));
    let y = lane.bottom() - px(10.);
    // Shorten by two beats, straying off the lane on the way.
    let grab = point(lane.left() + px(8. * ppb - 2.), y);
    let to = point(grab.x - px(2. * ppb), y);
    h.press(grab);
    h.move_held(point(to.x, y + px(60.)));
    assert_eq!(
        h.view.read_with(h.cx, |v, _| v.drag_cursor()),
        Some(gpui::CursorStyle::ResizeLeftRight),
        "the resize cursor holds off the lane"
    );
    h.move_held(to);
    h.release(to);
    assert_eq!(the_block(&mut h).length, beats(6.));
    assert_eq!(shown(&mut h), Some(CursorKind::Resize), "on the new end");
    h.hover(point(lane.left() + px(3. * ppb), y));
    assert_eq!(shown(&mut h), None, "the body");
    h.hover(to);
    assert_eq!(shown(&mut h), Some(CursorKind::Resize), "back on the end");
    h.hover(point(to.x + px(2. * ppb), y));
    assert_eq!(shown(&mut h), None, "empty lane");
    h.hover(to);
    assert_eq!(shown(&mut h), Some(CursorKind::Resize), "and back again");
}

/// Clicks the block (a beat in, low on the lane) and returns its settings'
/// controls on screen once the inspector shows them.
fn select_block(h: &mut Harness, lane: Bounds<Pixels>, ppb: f32) {
    h.click(point(lane.left() + px(ppb), lane.bottom() - px(10.)), 1);
    h.redraw_all();
}

/// docs/behaviours.md: "Selecting a block opens the inspector with its
/// settings: depth and fades on sliders, repeat and nudge dragged
/// sideways, invert".
#[test]
fn a_selected_blocks_settings_are_in_the_inspector() {
    let (mut h, lane, ppb) = page_with("block-inspector", block(4., None));
    h.view.update(h.cx, |v, _| v.inspector_open = false);
    select_block(&mut h, lane, ppb);
    assert!(h.view.read_with(h.cx, |v, _| v.inspector_open), "opens");

    // Depth: from 100% in the middle, a quarter of the slider's travel
    // left is 50% (the slider runs from 0% to 200%).
    let depth = h.bounds("block-depth");
    let travel = depth.size.width / px(1.) - 9.;
    let from = depth.center();
    let to = point(from.x - px(travel / 4.), from.y);
    h.press(from);
    h.move_held(to);
    let heard = h.view.read_with(h.cx, |v, _| {
        let param = v
            .session()
            .engine()
            .project()
            .param(VOLUME)
            .expect("volume");
        param.layers[0].1.blocks[0].1.depth
    });
    assert!((heard - 0.5).abs() < 1e-3, "heard as it moves: {heard}");
    h.release(to);
    assert!((the_block(&mut h).depth - 0.5).abs() < 1e-3);
    h.keys("ctrl-z");
    assert_eq!(the_block(&mut h).depth, 1., "one undo step");

    // Fade in: halfway along is half the block, on the beat grid.
    let fade = h.bounds("block-fade-in");
    let travel = fade.size.width / px(1.) - 9.;
    let from = point(fade.left() + px(4.5), fade.center().y);
    h.drag(from, point(from.x + px(travel / 2.), from.y), 4);
    assert_eq!(the_block(&mut h).fade_in, beats(2.));

    // Repeat, dragged sideways to the left: the block repeats.
    let repeat = h.bounds("block-repeat").center();
    h.drag(repeat, point(repeat.x - px(40.), repeat.y), 4);
    assert!(the_block(&mut h).cycle.is_some(), "repeats");

    let invert = h.bounds("block-invert").center();
    h.click(invert, 1);
    assert!(the_block(&mut h).invert);
}

/// docs/behaviours.md: "... a window on a listening layer has no repeat".
#[test]
fn a_windows_settings_have_no_repeat() {
    use daw_engine::{Signal, SignalId, SignalSource};
    let mut h = Harness::new("window-inspector");
    h.view.update(h.cx, |view, _| {
        view.session.apply_or_panic(Command::Edit(Edit::AddSignal {
            id: SignalId(1),
            signal: Signal {
                name: "Wobble".into(),
                color: 1,
                source: SignalSource::Drawn(Vec::new()),
            },
            listeners: Vec::new(),
        }));
        let mut layer = Layer::new(BlendMode::Add).listening(SignalId(1));
        layer.blocks.push((BlockId(1), block(4., None)));
        view.session.apply_or_panic(Command::Edit(Edit::AddLayer {
            param: VOLUME,
            id: LayerId(1),
            index: 0,
            layer,
        }));
        view.selected_node = Some(FIRST_NODE);
        view.page = Page::Parameters;
    });
    h.redraw_all();
    let lane = h.bounds("layer-lane-1");
    let ppb = h
        .view
        .read_with(h.cx, |v, _| v.parameters_view.zoom.pixels_per_beat);
    select_block(&mut h, lane, ppb);
    assert!(
        h.cx.debug_bounds("block-depth").is_some(),
        "its settings show"
    );
    assert!(h.cx.debug_bounds("block-nudge").is_some());
    assert!(
        h.cx.debug_bounds("block-repeat").is_none(),
        "the signal decides what plays through a window"
    );
}

/// docs/behaviours.md: "Layer rows: ... blend mode (Add, Subtract,
/// Multiply)".
#[test]
fn a_layers_blend_mode_is_picked_from_its_menu() {
    let (mut h, _, _) = page_with("blend-mode", block(4., None));
    let at = h.bounds("layer-mode-1").center();
    h.click(at, 1);
    h.redraw_all();
    let at = h.bounds("blend-mode-Subtract").center();
    h.click(at, 1);
    let mode = h.view.read_with(h.cx, |v, _| {
        let param = v
            .session()
            .engine()
            .project()
            .param(VOLUME)
            .expect("volume");
        param.layers[0].1.mode
    });
    assert_eq!(mode, BlendMode::Subtract);
}
