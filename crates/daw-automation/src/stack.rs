//! Compiled layer stacks and their evaluation.

use crate::shape::shape_value;
use daw_protocol::{BlendMode, Block, Layer, LayerId, ParamState, ShapePoint};

/// Combines the value from the layers below (`v`) with a block's shape value
/// (`x`) at weight `w`. The result may leave `0..=1`; clamping happens once,
/// after the whole stack.
#[inline]
pub fn blend(mode: BlendMode, v: f64, x: f64, w: f64) -> f64 {
    match mode {
        BlendMode::Replace => v + (x - v) * w,
        BlendMode::Add => v + x * w,
        BlendMode::Subtract => v - x * w,
        BlendMode::Multiply => v * (1.0 + (x - 1.0) * w),
        BlendMode::Modulate => v + (2.0 * x - 1.0) * w,
    }
}

/// A block with its timing in beats, ready to sample.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledBlock {
    pub start: f64,
    pub end: f64,
    pub cycle: f64,
    pub nudge: f64,
    pub depth: f64,
    pub invert: bool,
    pub fade_in: f64,
    pub fade_out: f64,
    pub points: Vec<ShapePoint>,
}

impl CompiledBlock {
    pub fn compile(block: &Block) -> Self {
        Self {
            start: block.start.as_beats(),
            end: block.end().as_beats(),
            cycle: block.cycle.unwrap_or(block.length).as_beats(),
            nudge: block.nudge.as_beats(),
            depth: block.depth,
            invert: block.invert,
            fade_in: block.fade_in.as_beats(),
            fade_out: block.fade_out.as_beats(),
            points: block.shape.points.clone(),
        }
    }

    pub fn contains(&self, beat: f64) -> bool {
        self.start <= beat && beat < self.end
    }

    /// Shape value and weight at `beat`, which must be inside the block.
    pub fn sample(&self, beat: f64) -> (f64, f64) {
        let phase = ((beat - self.start + self.nudge) / self.cycle).rem_euclid(1.0);
        let x = shape_value(&self.points, phase);
        let x = if self.invert { 1.0 - x } else { x };
        let mut fade: f64 = 1.0;
        if self.fade_in > 0.0 {
            fade = fade.min((beat - self.start) / self.fade_in);
        }
        if self.fade_out > 0.0 {
            fade = fade.min((self.end - beat) / self.fade_out);
        }
        (x, self.depth * fade.clamp(0.0, 1.0))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledLayer {
    pub mode: BlendMode,
    /// Sorted by start, non-overlapping.
    pub blocks: Vec<CompiledBlock>,
}

impl CompiledLayer {
    pub fn compile(layer: &Layer) -> Self {
        let mut blocks: Vec<CompiledBlock> = layer
            .blocks
            .iter()
            .map(|(_, block)| CompiledBlock::compile(block))
            .collect();
        blocks.sort_by(|a, b| a.start.total_cmp(&b.start));
        Self {
            mode: layer.mode,
            blocks,
        }
    }

    /// The block covering `beat`, if any; `None` is a gap.
    pub fn block_at(&self, beat: f64) -> Option<&CompiledBlock> {
        let after = self.blocks.partition_point(|b| b.start <= beat);
        after
            .checked_sub(1)
            .map(|i| &self.blocks[i])
            .filter(|b| b.contains(beat))
    }
}

/// A parameter's default state and active layers, ready to evaluate.
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    pub base: f64,
    /// Enabled, non-empty layers from bottom to top.
    pub layers: Vec<CompiledLayer>,
}

impl Automation {
    /// A parameter resting at `base` with nothing acting on it.
    pub fn fixed(base: f64) -> Self {
        Self {
            base,
            layers: Vec::new(),
        }
    }

    pub fn compile(state: &ParamState) -> Self {
        Self::compile_layers(state.base, &state.layers)
    }

    pub fn compile_layers(base: f64, layers: &[(LayerId, Layer)]) -> Self {
        Self {
            base,
            layers: layers
                .iter()
                .filter(|(_, l)| l.enabled && !l.blocks.is_empty())
                .map(|(_, l)| CompiledLayer::compile(l))
                .collect(),
        }
    }

    /// The parameter's normalized value at `beat`. Allocation-free.
    pub fn value_at(&self, beat: f64) -> f64 {
        let mut v = self.base;
        for layer in &self.layers {
            if let Some(block) = layer.block_at(beat) {
                let (x, w) = block.sample(beat);
                v = blend(layer.mode, v, x, w);
            }
        }
        v.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_protocol::{BlockId, Shape, Ticks};

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn beats(n: f64) -> Ticks {
        Ticks::from_beats_f64(n)
    }

    fn block(start: f64, length: f64, shape: Shape) -> Block {
        Block::new(beats(start), beats(length), shape)
    }

    fn layer(mode: BlendMode, blocks: Vec<Block>) -> (LayerId, Layer) {
        let mut layer = Layer::new(mode);
        layer.blocks = blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| (BlockId(i as u64), b))
            .collect();
        (LayerId(0), layer)
    }

    fn automation(base: f64, layers: Vec<(LayerId, Layer)>) -> Automation {
        Automation::compile_layers(base, &layers)
    }

    #[test]
    fn blend_modes() {
        let (v, x, w) = (0.5, 0.2, 0.5);
        assert!(close(blend(BlendMode::Replace, v, x, w), 0.35));
        assert!(close(blend(BlendMode::Add, v, x, w), 0.6));
        assert!(close(blend(BlendMode::Subtract, v, x, w), 0.4));
        assert!(close(blend(BlendMode::Multiply, v, x, w), 0.3));
        assert!(close(blend(BlendMode::Modulate, v, x, w), 0.2));
    }

    #[test]
    fn zero_weight_changes_nothing_in_every_mode() {
        for mode in [
            BlendMode::Replace,
            BlendMode::Add,
            BlendMode::Subtract,
            BlendMode::Multiply,
            BlendMode::Modulate,
        ] {
            assert_eq!(blend(mode, 0.42, 0.9, 0.0), 0.42, "{mode:?}");
        }
    }

    #[test]
    fn full_replace_takes_the_shape_value() {
        assert_eq!(blend(BlendMode::Replace, 0.42, 0.9, 1.0), 0.9);
    }

    #[test]
    fn modulate_is_neutral_at_the_centre_of_the_shape() {
        assert_eq!(blend(BlendMode::Modulate, 0.42, 0.5, 1.0), 0.42);
    }

    #[test]
    fn no_layers_rest_at_the_default_state() {
        assert_eq!(Automation::fixed(0.3).value_at(10.0), 0.3);
    }

    #[test]
    fn gaps_fall_through_to_the_layers_below() {
        // A block over beats 1..2 only: snap back is automatic.
        let a = automation(
            0.3,
            vec![layer(
                BlendMode::Replace,
                vec![block(1.0, 1.0, Shape::constant(0.9))],
            )],
        );
        assert_eq!(a.value_at(0.5), 0.3);
        assert!(close(a.value_at(1.0), 0.9));
        assert!(close(a.value_at(1.99), 0.9));
        assert_eq!(a.value_at(2.0), 0.3);
    }

    #[test]
    fn layers_apply_bottom_to_top() {
        // A rising ramp, then a wobble on top: a rising wobble.
        let ramp = layer(
            BlendMode::Replace,
            vec![block(0.0, 4.0, Shape::ramp(0.0, 0.8))],
        );
        let mut wobble_block = block(0.0, 4.0, Shape::ramp(0.0, 1.0));
        wobble_block.cycle = Some(beats(1.0));
        wobble_block.depth = 0.1;
        let wobble = layer(BlendMode::Modulate, vec![wobble_block]);
        let a = automation(0.5, vec![ramp, wobble]);
        // At each beat start the wobble is at its lowest (-0.1).
        assert!(close(a.value_at(2.0), 0.4 - 0.1));
        // Halfway through a beat the wobble is neutral.
        assert!(close(a.value_at(2.5), 0.5));
    }

    #[test]
    fn order_matters() {
        let replace = layer(
            BlendMode::Replace,
            vec![block(0.0, 1.0, Shape::constant(0.2))],
        );
        let add = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(0.3))]);
        let up = automation(0.0, vec![replace.clone(), add.clone()]);
        let down = automation(0.0, vec![add, replace]);
        assert!(close(up.value_at(0.5), 0.5));
        assert!(close(down.value_at(0.5), 0.2));
    }

    #[test]
    fn clamps_once_after_the_whole_stack() {
        let push = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        let pull = layer(
            BlendMode::Subtract,
            vec![block(0.0, 1.0, Shape::constant(0.6))],
        );
        // 0.8 + 1.0 = 1.8 (not clamped yet), then - 0.6 = 1.2, clamped to 1.
        assert_eq!(automation(0.8, vec![push.clone()]).value_at(0.5), 1.0);
        let pulled = automation(0.8, vec![push, pull.clone(), pull]);
        assert!(close(pulled.value_at(0.5), 0.6));
    }

    #[test]
    fn disabled_layers_are_skipped() {
        let mut off = layer(
            BlendMode::Replace,
            vec![block(0.0, 1.0, Shape::constant(1.0))],
        );
        off.1.enabled = false;
        assert_eq!(automation(0.2, vec![off]).value_at(0.5), 0.2);
    }

    #[test]
    fn multiply_ducks_proportionally() {
        // An inverted ramp: full duck at the start, recovering over the block.
        let mut duck = block(0.0, 1.0, Shape::ramp(0.0, 1.0));
        duck.depth = 1.0;
        let a = automation(0.8, vec![layer(BlendMode::Multiply, vec![duck])]);
        assert_eq!(a.value_at(0.0), 0.0);
        assert!(close(a.value_at(0.5), 0.4));
    }

    #[test]
    fn short_cycles_repeat_the_shape() {
        let mut b = block(2.0, 4.0, Shape::ramp(0.0, 1.0));
        b.cycle = Some(beats(1.0));
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b])]);
        for beat in [2.0, 3.0, 4.0, 5.0] {
            assert!(close(a.value_at(beat), 0.0), "{beat}");
            assert!(close(a.value_at(beat + 0.5), 0.5), "{beat}");
        }
    }

    #[test]
    fn stretching_a_block_without_a_cycle_stretches_the_shape() {
        let short = automation(
            0.0,
            vec![layer(
                BlendMode::Replace,
                vec![block(0.0, 1.0, Shape::ramp(0.0, 1.0))],
            )],
        );
        let long = automation(
            0.0,
            vec![layer(
                BlendMode::Replace,
                vec![block(0.0, 4.0, Shape::ramp(0.0, 1.0))],
            )],
        );
        assert!(close(short.value_at(0.5), long.value_at(2.0)));
    }

    #[test]
    fn invert_flips_the_shape() {
        let mut b = block(0.0, 1.0, Shape::ramp(0.0, 1.0));
        b.invert = true;
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b])]);
        assert!(close(a.value_at(0.25), 0.75));
    }

    #[test]
    fn nudge_shifts_where_the_shape_is_read() {
        let mut b = block(0.0, 4.0, Shape::ramp(0.0, 1.0));
        b.cycle = Some(beats(1.0));
        b.nudge = beats(0.25);
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b.clone()])]);
        assert!(close(a.value_at(1.0), 0.25));
        b.nudge = beats(-0.25);
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b])]);
        assert!(close(a.value_at(1.0), 0.75));
    }

    #[test]
    fn fades_ramp_the_weight_at_the_block_edges() {
        let mut b = block(0.0, 4.0, Shape::constant(1.0));
        b.fade_in = beats(1.0);
        b.fade_out = beats(2.0);
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b])]);
        assert_eq!(a.value_at(0.0), 0.0);
        assert!(close(a.value_at(0.5), 0.5));
        assert!(close(a.value_at(1.5), 1.0));
        assert!(close(a.value_at(3.0), 0.5));
        assert!(close(a.value_at(4.0 - 1e-9), 0.0));
    }

    #[test]
    fn depth_scales_the_effect() {
        let mut b = block(0.0, 1.0, Shape::constant(1.0));
        b.depth = 0.25;
        let a = automation(0.0, vec![layer(BlendMode::Replace, vec![b])]);
        assert!(close(a.value_at(0.5), 0.25));
    }

    #[test]
    fn finds_the_right_block_among_many() {
        let blocks = (0..100)
            .map(|i| {
                block(
                    f64::from(i) * 2.0,
                    1.0,
                    Shape::constant(f64::from(i) / 100.0),
                )
            })
            .collect();
        let a = automation(0.0, vec![layer(BlendMode::Replace, blocks)]);
        assert!(close(a.value_at(50.5), 0.25));
        assert_eq!(a.value_at(51.5), 0.0, "gap between blocks");
    }

    #[test]
    fn compile_drops_empty_and_disabled_layers() {
        let mut off = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        off.1.enabled = false;
        let empty = layer(BlendMode::Add, vec![]);
        let on = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        let state = ParamState {
            base: 0.0,
            layers: vec![off, empty, on],
        };
        assert_eq!(Automation::compile(&state).layers.len(), 1);
    }
}
