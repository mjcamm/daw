//! Editing blocks on a layer lane: snapping, creating, moving, stretching,
//! looping, reshaping and the per-block settings. Pure functions, free of
//! GPUI, so every gesture is unit tested; the Parameters page turns mouse
//! positions into calls here.
//!
//! Geometry is in pixels local to a block's drawing box: x from the block's
//! left edge, y from the top of the box.

use daw_automation::shape_value;
use daw_engine::{Block, BlockId, Shape, ShapePoint, TICKS_PER_BEAT, Ticks};

/// Space above and below a curve inside its box.
pub const CURVE_PADDING: f32 = 6.;
/// Width of the stretch zones at a block's edges.
pub const EDGE_ZONE: f32 = 6.;
/// Size of the loop handle in a block's top-right corner.
pub const LOOP_HANDLE: f32 = 14.;
/// How close the pointer must be to grab a point.
const POINT_RADIUS: f32 = 6.;
/// Shortest block or cycle when sliding freely: a 64th of a beat.
const MIN_FREE_LENGTH: Ticks = Ticks(TICKS_PER_BEAT / 64);
/// Length of a new block.
const NEW_BLOCK_LENGTH: Ticks = Ticks::from_bars(1);
/// Value of a new block's flat line.
const NEW_BLOCK_VALUE: f64 = 0.5;

/// Grid sizes for snapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    Bar,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    SixtyFourth,
}

impl Grid {
    pub const ALL: [Self; 7] = [
        Self::Bar,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::SixtyFourth,
    ];

    pub fn ticks(self) -> Ticks {
        let beat = TICKS_PER_BEAT;
        Ticks(match self {
            Self::Bar => 4 * beat,
            Self::Half => 2 * beat,
            Self::Quarter => beat,
            Self::Eighth => beat / 2,
            Self::Sixteenth => beat / 4,
            Self::ThirtySecond => beat / 8,
            Self::SixtyFourth => beat / 16,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bar => "1 Bar",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::SixtyFourth => "1/64",
        }
    }
}

/// Whether block edits snap to a grid or slide freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snap {
    pub enabled: bool,
    pub grid: Grid,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            enabled: true,
            grid: Grid::Quarter,
        }
    }
}

impl Snap {
    /// The same grid with snapping flipped, for a held modifier key.
    pub fn inverted(self) -> Self {
        Self {
            enabled: !self.enabled,
            ..self
        }
    }

    /// The nearest grid line, or `t` itself when sliding.
    pub fn round(self, t: Ticks) -> Ticks {
        if !self.enabled {
            return t;
        }
        let g = self.grid.ticks().0;
        Ticks((t.0 + g / 2).div_euclid(g) * g)
    }

    /// The grid line at or before `t`, or `t` itself when sliding.
    pub fn floor(self, t: Ticks) -> Ticks {
        if !self.enabled {
            return t;
        }
        let g = self.grid.ticks().0;
        Ticks(t.0.div_euclid(g) * g)
    }

    /// The shortest length a drag can make.
    pub fn min_length(self) -> Ticks {
        if self.enabled {
            self.grid.ticks()
        } else {
            MIN_FREE_LENGTH
        }
    }

    /// Shown on the header control.
    pub fn label(self) -> &'static str {
        if self.enabled {
            self.grid.label()
        } else {
            "Slide"
        }
    }
}

/// The free space around a position on a lane, between its neighbours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// End of the block before, or the song start.
    pub start: Ticks,
    /// Start of the block after, if there is one.
    pub end: Option<Ticks>,
}

impl Room {
    fn end_or_max(self) -> Ticks {
        self.end.unwrap_or(Ticks(i64::MAX / 2))
    }
}

/// The room around `at` among `blocks`, ignoring the block `exclude`.
pub fn room_around(blocks: &[(BlockId, Block)], exclude: Option<BlockId>, at: Ticks) -> Room {
    let others = || blocks.iter().filter(|(id, _)| Some(*id) != exclude);
    Room {
        start: others()
            .filter(|(_, b)| b.start < at)
            .map(|(_, b)| b.end())
            .max()
            .unwrap_or(Ticks::ZERO),
        end: others()
            .filter(|(_, b)| b.start >= at)
            .map(|(_, b)| b.start)
            .min(),
    }
}

/// A new block for a double-click at `at` on an empty part of a lane: a flat
/// line one bar long, starting on the grid, shortened to fit before the next
/// block. `None` if `at` is inside a block or there is no space.
pub fn new_block(blocks: &[(BlockId, Block)], at: Ticks, snap: Snap) -> Option<Block> {
    if at < Ticks::ZERO || blocks.iter().any(|(_, b)| b.start <= at && at < b.end()) {
        return None;
    }
    let room = room_around(blocks, None, at);
    let start = snap.floor(at).max(room.start);
    let end = (start + NEW_BLOCK_LENGTH).min(room.end_or_max());
    (end > start).then(|| Block::new(start, end - start, Shape::constant(NEW_BLOCK_VALUE)))
}

/// What part of a block the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// A shape point (selected blocks only), by index.
    Point(usize),
    /// The loop handle in the top-right corner: drag to repeat the shape.
    Loop,
    /// The left edge: drag to move where the block starts; its contents stay put.
    Start,
    /// The right edge: drag to move where the block ends; its contents stay put.
    End,
    /// The end of a looped block's first cycle: the edge of the block that
    /// repeats. Drag to widen or narrow it; the repeats follow.
    CycleEnd,
    /// Anywhere else: drag to move.
    Body,
}

/// Half the width of the grab zone on a looped block's first cycle end.
const CYCLE_EDGE: f32 = 4.;
/// How far the pointer moves before an ambiguous press (a point sitting on
/// an edge) decides what it is dragging.
const DECIDE_DISTANCE: f32 = 4.;

/// Which part of `block` is at local `(x, y)`, for a box `height` pixels
/// tall drawn at `pixels_per_beat`.
pub fn hit_test(
    block: &Block,
    x: f32,
    y: f32,
    pixels_per_beat: f32,
    height: f32,
    selected: bool,
) -> Hit {
    let width = beats_to_px(block.length, pixels_per_beat);
    if selected && let Some(index) = point_at(block, x, y, pixels_per_beat, height) {
        return Hit::Point(index);
    }
    // The top quarter of the right edge, and the loop icon beside it, loop;
    // the rest of the right edge resizes.
    let edge = EDGE_ZONE.min(width / 4.);
    let loop_from = width
        - edge
        - if has_loop_handle(width) {
            LOOP_HANDLE
        } else {
            0.
        };
    if x >= loop_from && y <= loop_band(height) {
        return Hit::Loop;
    }
    edge_at(block, x, pixels_per_beat).unwrap_or(Hit::Body)
}

/// What a press at `x` on a window (a block on a layer listening to
/// a signal) would grab: its edges or its body. Windows have no points to
/// drag and nothing to loop: the signal decides what plays through them.
pub fn window_hit_test(block: &Block, x: f32, pixels_per_beat: f32) -> Hit {
    match edge_at(block, x, pixels_per_beat) {
        Some(Hit::CycleEnd) | None => Hit::Body,
        Some(edge) => edge,
    }
}

/// The top part of the right edge that loops rather than resizes.
const LOOP_EDGE_FRACTION: f32 = 0.25;
/// The loop band is never thinner than this, so it stays grabbable.
const MIN_LOOP_BAND: f32 = 6.;

/// Height of the band along the top of a block's right end (the edge and
/// the loop icon beside it) that loops, for a box `height` tall.
pub fn loop_band(height: f32) -> f32 {
    (height * LOOP_EDGE_FRACTION).max(MIN_LOOP_BAND)
}

/// The edge of `block` at local `x`, if any: its start, its end, or the end
/// of a looped block's first cycle. A selected block's point can sit on an
/// edge too; then the first movement decides (see [`decide`]).
pub fn edge_at(block: &Block, x: f32, pixels_per_beat: f32) -> Option<Hit> {
    let width = beats_to_px(block.length, pixels_per_beat);
    // Small blocks keep a middle to grab.
    let edge = EDGE_ZONE.min(width / 4.);
    if x < edge {
        Some(Hit::Start)
    } else if x >= width - edge {
        Some(Hit::End)
    } else if cycle_end_x(block, pixels_per_beat).is_some_and(|end| (x - end).abs() <= CYCLE_EDGE) {
        Some(Hit::CycleEnd)
    } else {
        None
    }
}

/// Where a looped block's first cycle ends, in local pixels: the right edge
/// of the block that repeats. `None` if the block does not loop.
pub fn cycle_end_x(block: &Block, pixels_per_beat: f32) -> Option<f32> {
    let cycle = block.cycle.filter(|c| *c < block.length)?.0;
    let first = block.nudge.0.div_euclid(cycle) + 1;
    let end = Ticks(first * cycle - block.nudge.0);
    (end > Ticks::ZERO && end < block.length).then(|| beats_to_px(end, pixels_per_beat))
}

/// For a press that could grab a point or an edge: `None` until the pointer
/// has moved far enough, then `Some(true)` for the edge (mostly sideways)
/// or `Some(false)` for the point (mostly up or down).
pub fn decide(dx: f32, dy: f32) -> Option<bool> {
    (dx.hypot(dy) >= DECIDE_DISTANCE).then_some(dx.abs() > dy.abs())
}

/// Whether a block `width` pixels wide has room for its loop handle, which
/// sits in the top-right corner just inside the right edge zone.
pub fn has_loop_handle(width: f32) -> bool {
    width >= 2. * LOOP_HANDLE + 2. * EDGE_ZONE
}

fn beats_to_px(t: Ticks, pixels_per_beat: f32) -> f32 {
    t.as_beats() as f32 * pixels_per_beat
}

fn px_to_ticks(x: f32, pixels_per_beat: f32) -> Ticks {
    Ticks::from_beats_f64(f64::from(x / pixels_per_beat))
}

/// A drag of `dx` pixels on part of a block, `original` being the block when
/// the drag began and `room` the space around it. Positions snap to the
/// grid; the result never leaves its room or gets shorter than the minimum.
pub fn drag_block(
    original: &Block,
    hit: Hit,
    dx: f32,
    pixels_per_beat: f32,
    snap: Snap,
    room: Room,
) -> Block {
    let delta = px_to_ticks(dx, pixels_per_beat);
    let min = snap.min_length();
    let room_end = room.end_or_max();
    match hit {
        Hit::Body => {
            let start = snap
                .round(original.start + delta)
                .max(room.start)
                .min(room_end - original.length);
            Block {
                start,
                ..original.clone()
            }
        }
        Hit::Start => {
            let latest = (original.end() - min).max(room.start);
            let start = snap
                .round(original.start + delta)
                .max(room.start)
                .min(latest);
            reframed(original, start, original.end() - start)
        }
        Hit::End => {
            let end = snap
                .round(original.end() + delta)
                .min(room_end)
                .max((original.start + min).min(room_end));
            reframed(original, original.start, end - original.start)
        }
        Hit::Loop => {
            // Only repeats come and go: never shorter than one cycle (the
            // block itself). With every repeat gone it is a plain block.
            let cycle = cycle_of(original).min(original.length);
            let end = snap
                .round(original.end() + delta)
                .min(room_end)
                .max(original.start + cycle);
            let length = end - original.start;
            Block {
                length,
                cycle: (length > cycle).then_some(cycle),
                ..original.clone()
            }
        }
        Hit::CycleEnd => {
            // The repeating block widens or narrows as a window: its drawn
            // shape stays put within each cycle, and every repeat follows.
            let cycle = cycle_of(original);
            let new_cycle = snap.round(cycle + delta).max(min);
            let ratio = new_cycle.0 as f64 / cycle.0 as f64;
            let length = Ticks((original.length.0 as f64 * ratio).round() as i64)
                .min(room_end - original.start)
                .max(new_cycle.min(room_end - original.start));
            Block {
                length,
                cycle: Some(new_cycle),
                shape: reframe_shape(&original.shape, cycle, Ticks::ZERO, new_cycle),
                ..original.clone()
            }
        }
        Hit::Point(_) => original.clone(),
    }
}

/// `original` as a window onto the song from `start` for `length`, with its
/// contents left where they are in song time. Widening shows more: a single
/// shape continues at its edge value (its edge point moving out with the
/// edge), a looped one shows more repeats. Narrowing cuts the shape at the
/// new edge.
fn reframed(original: &Block, start: Ticks, length: Ticks) -> Block {
    let offset = start - original.start;
    let fade = |t: Ticks| t.min(length);
    let base = Block {
        start,
        length,
        fade_in: fade(original.fade_in),
        fade_out: fade(original.fade_out),
        ..original.clone()
    };
    let looped = original.cycle.is_some_and(|c| c < original.length);
    if !looped && original.nudge == Ticks::ZERO {
        // One cycle spans the block, so the points move within it.
        Block {
            cycle: None,
            shape: reframe_shape(&original.shape, original.length, offset, length),
            ..base
        }
    } else {
        // Repeats keep their length; only where they are read from shifts.
        let cycle = cycle_of(original);
        Block {
            cycle: Some(cycle),
            nudge: Ticks((original.nudge + offset).0.rem_euclid(cycle.0)),
            ..base
        }
    }
}

/// A shape drawn across `old_length`, redrawn across `new_length` starting
/// `offset` later, so every point keeps its song position. Points that fall
/// outside are cut, leaving a point at the edge with the value there.
fn reframe_shape(shape: &Shape, old_length: Ticks, offset: Ticks, new_length: Ticks) -> Shape {
    let (old, offset, new) = (old_length.0 as f64, offset.0 as f64, new_length.0 as f64);
    let to_new = |at: f64| (at * old - offset) / new;
    let value_at = |at: f64| shape_value(&shape.points, (at * new + offset) / old);
    let mut points: Vec<ShapePoint> = shape
        .points
        .iter()
        .map(|p| ShapePoint {
            at: to_new(p.at),
            ..*p
        })
        .filter(|p| (0.0..=1.0).contains(&p.at))
        .collect();
    // The last point cut off the start sets how the shape leaves the edge.
    if let Some(cut) = shape.points.iter().rfind(|p| to_new(p.at) < 0.0) {
        points.insert(
            0,
            ShapePoint::new(0.0, value_at(0.0)).with_segment(cut.segment),
        );
    }
    if shape.points.iter().any(|p| to_new(p.at) > 1.0) {
        points.push(ShapePoint::new(1.0, value_at(1.0)));
    }
    // Widening: a point that sat on an edge moves out with it, holding its
    // value, so the drawn shape is untouched and its end follows the edge.
    // If it already ends in a flat run, the edge point just moves.
    let flat = |a: &ShapePoint, b: &ShapePoint| (a.value - b.value).abs() < 1e-12;
    if to_new(1.0) < 1.0 && shape.points.last().is_some_and(|p| p.at == 1.0) {
        let n = points.len();
        match points.as_mut_slice() {
            [.., before, last] if flat(before, last) => last.at = 1.0,
            _ => points.push(ShapePoint::new(1.0, points[n - 1].value)),
        }
    }
    if to_new(0.0) > 0.0 && shape.points.first().is_some_and(|p| p.at == 0.0) {
        match points.as_mut_slice() {
            [first, after, ..] if flat(first, after) => first.at = 0.0,
            _ => {
                let value = points[0].value;
                points.insert(0, ShapePoint::new(0.0, value));
            }
        }
    }
    Shape { points }
}

fn cycle_of(block: &Block) -> Ticks {
    block.cycle.unwrap_or(block.length)
}

/// The value at local `y` in a box `height` tall.
pub fn y_to_value(y: f32, height: f32) -> f64 {
    let span = (height - 2. * CURVE_PADDING).max(1.);
    f64::from(1. - (y - CURVE_PADDING) / span).clamp(0., 1.)
}

/// Local y of `value` in a box `height` tall.
pub fn value_to_y(value: f64, height: f32) -> f32 {
    CURVE_PADDING + (1. - value.clamp(0., 1.) as f32) * (height - 2. * CURVE_PADDING)
}

/// Where in the cycle (`0..1`) local `x` reads the shape.
pub fn phase_at(block: &Block, x: f32, pixels_per_beat: f32) -> f64 {
    let beats = f64::from(x / pixels_per_beat) + block.nudge.as_beats();
    (beats / cycle_of(block).as_beats()).rem_euclid(1.)
}

/// Every place a point is drawn, as (point index, local x): once per cycle
/// the block repeats.
pub fn point_positions(block: &Block, pixels_per_beat: f32) -> Vec<(usize, f32)> {
    let cycle = cycle_of(block).as_beats();
    let length = block.length.as_beats();
    let nudge = block.nudge.as_beats();
    let first = (nudge / cycle).floor() as i64;
    let last = ((length + nudge) / cycle).ceil() as i64;
    let mut positions = Vec::new();
    for k in first..=last {
        for (index, point) in block.shape.points.iter().enumerate() {
            let beat = (k as f64 + point.at) * cycle - nudge;
            // Not the next cycle's start at the right edge, nor the last
            // cycle's end at the left edge: those are not in the block.
            let phantom = (beat - length).abs() < 1e-9 && point.at < 1.0
                || beat.abs() < 1e-9 && point.at > 0.0 && k < 0;
            if (0.0..=length).contains(&beat) && !phantom {
                positions.push((index, beat as f32 * pixels_per_beat));
            }
        }
    }
    positions
}

/// The point drawn nearest local `(x, y)`, if one is close enough.
pub fn point_at(block: &Block, x: f32, y: f32, pixels_per_beat: f32, height: f32) -> Option<usize> {
    point_positions(block, pixels_per_beat)
        .into_iter()
        .map(|(index, px)| {
            let py = value_to_y(block.shape.points[index].value, height);
            (index, (px - x).hypot(py - y))
        })
        .filter(|(_, distance)| *distance <= POINT_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// `shape` with a point added at phase `at`, and the new point's index.
pub fn add_point(shape: &Shape, at: f64, value: f64) -> (Shape, usize) {
    let mut points = shape.points.clone();
    let index = points.partition_point(|p| p.at <= at);
    points.insert(
        index,
        ShapePoint::new(at.clamp(0., 1.), value.clamp(0., 1.)),
    );
    (Shape { points }, index)
}

/// `shape` with point `index` moved, kept between its neighbours.
pub fn move_point(shape: &Shape, index: usize, at: f64, value: f64) -> Shape {
    let mut points = shape.points.clone();
    let lowest = index.checked_sub(1).map_or(0., |i| points[i].at);
    let highest = points.get(index + 1).map_or(1., |p| p.at);
    if let Some(point) = points.get_mut(index) {
        point.at = at.clamp(lowest, highest);
        point.value = value.clamp(0., 1.);
    }
    Shape { points }
}

/// `shape` without point `index`; `None` for the last remaining point.
pub fn remove_point(shape: &Shape, index: usize) -> Option<Shape> {
    if shape.points.len() <= 1 || index >= shape.points.len() {
        return None;
    }
    let mut points = shape.points.clone();
    points.remove(index);
    Some(Shape { points })
}

/// A per-block setting in the settings bar, changed by dragging sideways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Depth,
    FadeIn,
    FadeOut,
    Cycle,
    Nudge,
}

impl Setting {
    pub const ALL: [Self; 5] = [
        Self::Depth,
        Self::FadeIn,
        Self::FadeOut,
        Self::Cycle,
        Self::Nudge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Depth => "Depth",
            Self::FadeIn => "Fade In",
            Self::FadeOut => "Fade Out",
            Self::Cycle => "Repeat",
            Self::Nudge => "Nudge",
        }
    }

    /// The setting's value on `block`, for display.
    pub fn format(self, block: &Block) -> String {
        match self {
            Self::Depth => format!("{:.0}%", block.depth * 100.),
            Self::FadeIn => format_length(block.fade_in),
            Self::FadeOut => format_length(block.fade_out),
            Self::Cycle => match block.cycle {
                Some(cycle) if cycle != block.length => format_length(cycle),
                _ => "Off".to_owned(),
            },
            Self::Nudge => format_length(block.nudge),
        }
    }
}

/// Pixels of sideways drag per percent of depth.
const DEPTH_PX_PER_PERCENT: f32 = 2.;
/// Pixels of sideways drag per grid step of a time setting.
const PX_PER_STEP: f32 = 12.;

/// `original` with `setting` changed by a sideways drag of `dx` pixels. Time
/// settings move in grid steps (64th notes when sliding).
pub fn scrub(original: &Block, setting: Setting, dx: f32, snap: Snap) -> Block {
    let mut block = original.clone();
    let steps = (dx / PX_PER_STEP).round() as i64;
    let step = snap.min_length();
    let shifted = |t: Ticks| Ticks(t.0 + steps * step.0);
    match setting {
        Setting::Depth => {
            let percent =
                (original.depth * 100.).round() + f64::from((dx / DEPTH_PX_PER_PERCENT).round());
            block.depth = (percent / 100.).clamp(0., 1.);
        }
        Setting::FadeIn => {
            block.fade_in = shifted(original.fade_in)
                .max(Ticks::ZERO)
                .min(original.length);
        }
        Setting::FadeOut => {
            block.fade_out = shifted(original.fade_out)
                .max(Ticks::ZERO)
                .min(original.length);
        }
        Setting::Cycle => {
            let cycle = shifted(cycle_of(original)).max(step).min(original.length);
            block.cycle = (cycle != original.length).then_some(cycle);
        }
        Setting::Nudge => {
            let cycle = cycle_of(original).0;
            block.nudge = Ticks(shifted(original.nudge).0.clamp(-cycle, cycle));
        }
    }
    block
}

/// A length in bars, beats or note values, for display.
pub fn format_length(t: Ticks) -> String {
    if t == Ticks::ZERO {
        return "0".to_owned();
    }
    let sign = if t.0 < 0 { "-" } else { "" };
    let ticks = t.0.abs();
    let bar = Ticks::from_bars(1).0;
    let text = if ticks % bar == 0 {
        let bars = ticks / bar;
        format!("{bars} {}", if bars == 1 { "bar" } else { "bars" })
    } else if ticks % TICKS_PER_BEAT == 0 {
        let beats = ticks / TICKS_PER_BEAT;
        format!("{beats} {}", if beats == 1 { "beat" } else { "beats" })
    } else if (TICKS_PER_BEAT * 4) % ticks == 0 {
        format!("1/{}", TICKS_PER_BEAT * 4 / ticks)
    } else {
        format!("{:.2} beats", t.as_beats().abs())
    };
    format!("{sign}{text}")
}

/// An id no block in `blocks` uses yet.
pub fn next_block_id<'a>(blocks: impl IntoIterator<Item = &'a BlockId>) -> BlockId {
    BlockId(blocks.into_iter().map(|id| id.0).max().map_or(1, |h| h + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PPB: f32 = 24.;
    const BEAT: Ticks = Ticks(TICKS_PER_BEAT);

    fn beats(n: i64) -> Ticks {
        Ticks::from_beats(n)
    }

    fn block(start: i64, length: i64) -> Block {
        Block::new(beats(start), beats(length), Shape::ramp(0., 1.))
    }

    fn lane(blocks: &[(u64, Block)]) -> Vec<(BlockId, Block)> {
        blocks
            .iter()
            .map(|(id, b)| (BlockId(*id), b.clone()))
            .collect()
    }

    const SLIDE: Snap = Snap {
        enabled: false,
        grid: Grid::Quarter,
    };
    const BEATS: Snap = Snap {
        enabled: true,
        grid: Grid::Quarter,
    };

    #[test]
    fn snapping_rounds_to_the_grid() {
        assert_eq!(BEATS.round(Ticks(500)), BEAT);
        assert_eq!(BEATS.round(Ticks(400)), Ticks::ZERO);
        assert_eq!(BEATS.floor(Ticks(1900)), BEAT);
        assert_eq!(SLIDE.round(Ticks(500)), Ticks(500));
        let bars = Snap {
            enabled: true,
            grid: Grid::Bar,
        };
        assert_eq!(bars.round(beats(3)), Ticks::from_bars(1));
        assert_eq!(Grid::SixtyFourth.ticks(), Ticks(TICKS_PER_BEAT / 16));
        assert_eq!(Grid::ThirtySecond.label(), "1/32");
        assert_eq!(BEATS.label(), "1/4");
        assert_eq!(SLIDE.label(), "Slide");
        assert_eq!(BEATS.inverted(), SLIDE);
    }

    #[test]
    fn room_is_the_gap_between_neighbours() {
        let blocks = lane(&[(1, block(0, 4)), (2, block(8, 4)), (3, block(16, 4))]);
        assert_eq!(
            room_around(&blocks, Some(BlockId(2)), beats(8)),
            Room {
                start: beats(4),
                end: Some(beats(16))
            }
        );
        assert_eq!(
            room_around(&blocks, None, beats(25)),
            Room {
                start: beats(20),
                end: None
            }
        );
    }

    #[test]
    fn a_new_block_is_a_bar_long_flat_line_on_the_grid() {
        let made = new_block(&[], Ticks(BEAT.0 * 5 + 100), BEATS).unwrap();
        assert_eq!(made.start, beats(5));
        assert_eq!(made.length, Ticks::from_bars(1));
        assert_eq!(made.shape, Shape::constant(0.5));
    }

    #[test]
    fn a_new_block_fits_before_the_next_one() {
        let blocks = lane(&[(1, block(6, 4))]);
        let made = new_block(&blocks, beats(4), BEATS).unwrap();
        assert_eq!((made.start, made.length), (beats(4), beats(2)));
    }

    #[test]
    fn no_new_block_inside_another() {
        let blocks = lane(&[(1, block(0, 4))]);
        assert_eq!(new_block(&blocks, beats(2), BEATS), None);
    }

    #[test]
    fn hits_find_edges_loop_handle_and_body() {
        let b = block(0, 4); // 96 px wide
        let h = 40.;
        assert_eq!(hit_test(&b, 2., 30., PPB, h, false), Hit::Start);
        assert_eq!(hit_test(&b, 94., 30., PPB, h, false), Hit::End);
        // The top quarter of the right edge loops, and so does the loop
        // icon just inside it; the rest of the edge resizes.
        assert_eq!(hit_test(&b, 94., 4., PPB, h, false), Hit::Loop);
        assert_eq!(hit_test(&b, 94., 12., PPB, h, false), Hit::End);
        assert_eq!(hit_test(&b, 83., 4., PPB, h, false), Hit::Loop);
        assert_eq!(hit_test(&b, 83., 12., PPB, h, false), Hit::Body);
        assert_eq!(hit_test(&b, 48., 20., PPB, h, false), Hit::Body);
    }

    #[test]
    fn windows_have_edges_and_a_body_only() {
        // A block on a listening layer: no points to drag, nothing to loop.
        let b = block(0, 4);
        assert_eq!(window_hit_test(&b, 2., PPB), Hit::Start);
        assert_eq!(window_hit_test(&b, 94., PPB), Hit::End, "no loop band");
        assert_eq!(window_hit_test(&b, 48., PPB), Hit::Body);
    }

    #[test]
    fn points_are_only_hit_on_selected_blocks() {
        let b = block(0, 4);
        let h = 40.;
        // The ramp's end point sits at the top right... under the loop
        // handle, so test its start point at the bottom left.
        let (x, y) = (1., value_to_y(0., h));
        assert_eq!(hit_test(&b, x, y, PPB, h, true), Hit::Point(0));
        assert_eq!(hit_test(&b, x, y, PPB, h, false), Hit::Start);
    }

    #[test]
    fn moving_snaps_and_stays_in_its_room() {
        let blocks = lane(&[(1, block(0, 4)), (2, block(8, 4)), (3, block(16, 4))]);
        let room = room_around(&blocks, Some(BlockId(2)), beats(8));
        let b = &blocks[1].1;
        let moved = drag_block(b, Hit::Body, PPB * 1.4, PPB, BEATS, room);
        assert_eq!(moved.start, beats(9));
        let far = drag_block(b, Hit::Body, PPB * 100., PPB, BEATS, room);
        assert_eq!(far.start, beats(12), "stops at the next block");
        let back = drag_block(b, Hit::Body, -PPB * 100., PPB, BEATS, room);
        assert_eq!(back.start, beats(4), "stops at the previous block");
    }

    #[test]
    fn sliding_moves_freely() {
        let room = Room {
            start: Ticks::ZERO,
            end: None,
        };
        let moved = drag_block(&block(4, 4), Hit::Body, PPB * 0.5, PPB, SLIDE, room);
        assert_eq!(moved.start, Ticks(BEAT.0 * 4 + BEAT.0 / 2));
    }

    /// The shape value a block plays at a song beat.
    fn value_at(block: &Block, beat: f64) -> f64 {
        daw_automation::CompiledBlock::compile(block)
            .sample(beat, None)
            .0
    }

    fn assert_same_contents(a: &Block, b: &Block, beats: &[f64]) {
        for &beat in beats {
            let (va, vb) = (value_at(a, beat), value_at(b, beat));
            assert!((va - vb).abs() < 1e-9, "beat {beat}: {va} != {vb}");
        }
    }

    const ANYWHERE: Room = Room {
        start: Ticks::ZERO,
        end: None,
    };

    #[test]
    fn edges_widen_the_window_without_moving_the_contents() {
        let b = block(4, 4); // ramps 0 to 1 over beats 4..8
        let later = drag_block(&b, Hit::End, PPB * 4., PPB, BEATS, ANYWHERE);
        assert_eq!((later.start, later.end()), (beats(4), beats(12)));
        assert_same_contents(&b, &later, &[4., 5., 6., 7.5]);
        assert_eq!(value_at(&later, 10.), 1., "holds the last value");

        let earlier = drag_block(&b, Hit::Start, -PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!((earlier.start, earlier.end()), (beats(2), beats(8)));
        assert_same_contents(&b, &earlier, &[4., 5., 6., 7.5]);
        assert_eq!(value_at(&earlier, 3.), 0., "holds the first value");
    }

    #[test]
    fn widening_moves_the_end_point_out_with_the_edge() {
        let b = block(4, 4); // ramp 0 to 1, points at both edges
        let later = drag_block(&b, Hit::End, PPB * 4., PPB, BEATS, ANYWHERE);
        let last = *later.shape.points.last().unwrap();
        assert_eq!(
            (last.at, last.value),
            (1.0, 1.0),
            "an end point at the new edge"
        );
        assert_eq!(later.shape.points.len(), 3);
        // Widening again moves that end point rather than adding another.
        let again = drag_block(&later, Hit::End, PPB * 4., PPB, BEATS, ANYWHERE);
        assert_eq!(again.shape.points.len(), 3);
        assert_same_contents(&b, &again, &[4., 6., 7.9]);
        assert_eq!(value_at(&again, 15.9), 1.0);

        let earlier = drag_block(&b, Hit::Start, -PPB * 2., PPB, BEATS, ANYWHERE);
        let first = earlier.shape.points[0];
        assert_eq!(
            (first.at, first.value),
            (0.0, 0.0),
            "a start point at the new edge"
        );
    }

    fn looped(start: i64, cycle: i64, repeats: i64) -> Block {
        let mut b = block(start, cycle * repeats);
        b.cycle = Some(beats(cycle));
        b
    }

    #[test]
    fn a_looped_block_has_an_edge_where_its_first_cycle_ends() {
        let b = looped(0, 2, 3); // cycles end at 48 and 96 px
        assert_eq!(cycle_end_x(&b, PPB), Some(48.));
        assert_eq!(edge_at(&b, 50., PPB), Some(Hit::CycleEnd));
        assert_eq!(edge_at(&b, 60., PPB), None);
        assert_eq!(cycle_end_x(&block(0, 4), PPB), None, "not looped");
        let mut nudged = looped(0, 2, 3);
        nudged.nudge = beats(1);
        assert_eq!(cycle_end_x(&nudged, PPB), Some(24.));
    }

    #[test]
    fn widening_the_repeating_block_keeps_its_shape_and_its_repeats() {
        let b = looped(0, 2, 3);
        let wider = drag_block(&b, Hit::CycleEnd, PPB, PPB, BEATS, ANYWHERE);
        assert_eq!(wider.cycle, Some(beats(3)));
        assert_eq!(wider.length, beats(9), "still three repeats");
        // The ramp is untouched within the cycle; the new beat holds its end.
        let points: Vec<(f64, f64)> = wider.shape.points.iter().map(|p| (p.at, p.value)).collect();
        assert_eq!(points.len(), 3);
        assert!((points[1].0 - 2. / 3.).abs() < 1e-9 && points[1].1 == 1.0);
        assert_eq!(points[2], (1.0, 1.0));
        let capped = drag_block(
            &b,
            Hit::CycleEnd,
            PPB * 10.,
            PPB,
            BEATS,
            Room {
                start: Ticks::ZERO,
                end: Some(beats(10)),
            },
        );
        assert_eq!(capped.end(), beats(10), "never past the next block");
    }

    #[test]
    fn a_new_block_has_no_phantom_point_at_its_end() {
        let b = new_block(&[], Ticks::ZERO, BEATS).unwrap();
        assert_eq!(point_positions(&b, PPB), [(0, 0.)]);
        // A ramp's own end point is real and shows.
        assert_eq!(point_positions(&block(0, 4), PPB), [(0, 0.), (1, 96.)]);
    }

    #[test]
    fn the_right_edge_resizes_everywhere_below_the_top_quarter() {
        let b = new_block(&[], Ticks::ZERO, BEATS).unwrap(); // 96 px wide
        let h = 32.;
        for y in [9., 16., 24., 31.] {
            assert_eq!(hit_test(&b, 94., y, PPB, h, true), Hit::End, "edge at {y}");
            assert_eq!(
                hit_test(&b, 80., y, PPB, h, true),
                Hit::Body,
                "beside the edge at {y}"
            );
        }
        assert_eq!(hit_test(&b, 94., 7., PPB, h, true), Hit::Loop);
        assert_eq!(
            hit_test(&b, 80., 7., PPB, h, true),
            Hit::Loop,
            "the loop icon"
        );
    }

    #[test]
    fn ambiguous_presses_wait_for_the_first_movement() {
        assert_eq!(decide(1., 1.), None);
        assert_eq!(decide(6., 1.), Some(true), "sideways: the edge");
        assert_eq!(decide(1., -6.), Some(false), "up or down: the point");
    }

    #[test]
    fn edges_trim_the_window_without_moving_the_contents() {
        let b = block(4, 4);
        let shorter = drag_block(&b, Hit::End, -PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!(shorter.end(), beats(6));
        assert_same_contents(&b, &shorter, &[4., 4.5, 5., 5.99]);

        let trimmed = drag_block(&b, Hit::Start, PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!(trimmed.start, beats(6));
        assert_same_contents(&b, &trimmed, &[6., 6.5, 7., 7.99]);
    }

    #[test]
    fn edges_keep_repeats_and_fades_in_place() {
        let mut b = block(4, 4);
        b.shape = Shape::ramp(0., 1.);
        b.cycle = Some(beats(1));
        b.fade_in = beats(1);
        let earlier = drag_block(&b, Hit::Start, -PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!(earlier.cycle, Some(beats(1)), "repeats keep their length");
        assert_eq!(earlier.fade_in, beats(1));
        assert_same_contents(&b, &earlier, &[5.25, 6.5, 7.75]);
        let later = drag_block(&b, Hit::End, PPB * 4., PPB, BEATS, ANYWHERE);
        assert_same_contents(&b, &later, &[4.25, 5.5, 7.75]);
    }

    #[test]
    fn edges_never_make_a_block_shorter_than_the_grid() {
        let room = Room {
            start: Ticks::ZERO,
            end: None,
        };
        let b = block(4, 4);
        let end = drag_block(&b, Hit::End, -PPB * 10., PPB, BEATS, room);
        assert_eq!(end.length, BEAT);
        let start = drag_block(&b, Hit::Start, PPB * 10., PPB, BEATS, room);
        assert_eq!((start.start, start.length), (beats(7), BEAT));
    }

    #[test]
    fn the_loop_handle_repeats_the_shape() {
        let room = Room {
            start: Ticks::ZERO,
            end: Some(beats(20)),
        };
        let looped = drag_block(&block(0, 4), Hit::Loop, PPB * 8., PPB, BEATS, room);
        assert_eq!(looped.length, beats(12));
        assert_eq!(looped.cycle, Some(beats(4)), "one cycle is the old block");
        let capped = drag_block(&block(0, 4), Hit::Loop, PPB * 100., PPB, BEATS, room);
        assert_eq!(capped.end(), beats(20));
    }

    #[test]
    fn dragging_the_loop_back_stops_at_the_original_block() {
        let b = looped(0, 2, 3);
        let back = drag_block(&b, Hit::Loop, -PPB * 100., PPB, BEATS, ANYWHERE);
        assert_eq!(
            back.length,
            beats(2),
            "every repeat removed, the block kept"
        );
        assert_eq!(back.cycle, None, "no longer looped");
        // A block that never looped cannot be shortened by the loop handle.
        let plain = drag_block(&block(0, 4), Hit::Loop, -PPB * 3., PPB, BEATS, ANYWHERE);
        assert_eq!((plain.length, plain.cycle), (beats(4), None));
    }

    #[test]
    fn a_block_whose_loops_were_removed_extends_as_a_window_again() {
        let b = looped(0, 2, 3);
        let unlooped = drag_block(&b, Hit::Loop, -PPB * 100., PPB, BEATS, ANYWHERE);
        let wider = drag_block(&unlooped, Hit::End, PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!(wider.cycle, None, "extends, does not repeat");
        assert_same_contents(&unlooped, &wider, &[0., 1., 1.9]);
        // Older blocks may hold a cycle as long as themselves: not looped.
        let mut full = block(0, 4);
        full.cycle = Some(beats(4));
        let wider = drag_block(&full, Hit::End, PPB * 2., PPB, BEATS, ANYWHERE);
        assert_eq!(wider.cycle, None);
        assert_same_contents(&full, &wider, &[0., 2., 3.9]);
    }

    #[test]
    fn values_map_to_heights_and_back() {
        let h = 40.;
        assert_eq!(value_to_y(1., h), CURVE_PADDING);
        assert_eq!(value_to_y(0., h), h - CURVE_PADDING);
        assert!((y_to_value(value_to_y(0.3, h), h) - 0.3).abs() < 1e-6);
        assert_eq!(y_to_value(-10., h), 1.);
    }

    #[test]
    fn points_repeat_with_the_cycle() {
        let mut b = block(0, 4);
        b.shape = Shape::constant(0.5);
        b.cycle = Some(beats(1));
        let xs: Vec<f32> = point_positions(&b, PPB).iter().map(|(_, x)| *x).collect();
        assert_eq!(
            xs,
            [0., 24., 48., 72.],
            "not the next cycle's start at the end"
        );
        assert!((phase_at(&b, 36., PPB) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn points_are_added_moved_and_removed_in_order() {
        let (shape, index) = add_point(&Shape::ramp(0., 1.), 0.5, 0.8);
        assert_eq!(index, 1);
        assert_eq!(shape.points[1], ShapePoint::new(0.5, 0.8));
        let moved = move_point(&shape, 1, 2.0, -1.);
        assert_eq!((moved.points[1].at, moved.points[1].value), (1., 0.));
        let held = move_point(&shape, 1, -1., 0.5);
        assert_eq!(held.points[1].at, 0., "stays after the previous point");
        assert_eq!(remove_point(&shape, 1).unwrap().points.len(), 2);
        assert_eq!(remove_point(&Shape::constant(0.5), 0), None);
    }

    #[test]
    fn settings_scrub_in_steps() {
        let b = block(0, 4);
        assert_eq!(scrub(&b, Setting::Depth, -20., BEATS).depth, 0.9);
        assert_eq!(scrub(&b, Setting::Depth, 50., BEATS).depth, 1.);
        assert_eq!(scrub(&b, Setting::FadeIn, 24., BEATS).fade_in, beats(2));
        assert_eq!(
            scrub(&b, Setting::FadeOut, -24., BEATS).fade_out,
            Ticks::ZERO
        );
        let repeated = scrub(&b, Setting::Cycle, -36., BEATS);
        assert_eq!(repeated.cycle, Some(beats(1)));
        assert_eq!(scrub(&b, Setting::Cycle, 36., BEATS).cycle, None);
        assert_eq!(scrub(&b, Setting::Nudge, -12., BEATS).nudge, Ticks(-BEAT.0));
    }

    #[test]
    fn settings_read_naturally() {
        let mut b = block(0, 4);
        assert_eq!(Setting::Depth.format(&b), "100%");
        assert_eq!(Setting::Cycle.format(&b), "Off");
        b.cycle = Some(Ticks(BEAT.0 / 2));
        assert_eq!(Setting::Cycle.format(&b), "1/8");
        assert_eq!(format_length(Ticks::from_bars(2)), "2 bars");
        assert_eq!(format_length(beats(3)), "3 beats");
        assert_eq!(format_length(Ticks(-BEAT.0 / 4)), "-1/16");
        assert_eq!(format_length(Ticks(100)), "0.10 beats");
    }

    #[test]
    fn block_ids_start_at_one_and_never_repeat() {
        assert_eq!(next_block_id([]), BlockId(1));
        assert_eq!(next_block_id(&[BlockId(3), BlockId(7)]), BlockId(8));
    }
}
