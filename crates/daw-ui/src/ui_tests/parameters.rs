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
