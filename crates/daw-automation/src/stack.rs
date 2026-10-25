//! Compiled layer stacks and their evaluation.

use crate::shape::shape_value;
use crate::signal::{CompiledSignal, Live, NoLive, Signals};
use daw_protocol::{BlendMode, Block, Layer, LayerId, ParamState, ShapePoint};

/// Combines the value from the layers below (`v`) with a block's shape value
/// (`x`) at weight `w`. Shapes are centred: `x = 0.5` is the centre line and
/// has no effect, so a layer moves the value both ways. The result may leave
/// `0..=1`; clamping happens once, after the whole stack.
#[inline]
pub fn blend(mode: BlendMode, v: f64, x: f64, w: f64) -> f64 {
    // -1 at the bottom of the lane, 0 on the centre line, +1 at the top.
    let amount = (2.0 * x - 1.0) * w;
    match mode {
        BlendMode::Add => v + amount,
        BlendMode::Multiply => v * (1.0 + amount),
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

    /// Shape value and weight at `beat`, which must be inside the block. On
    /// a layer listening to `signal`, the block is a window onto it.
    pub fn sample(&self, beat: f64, signal: Option<&CompiledSignal>) -> (f64, f64) {
        self.sample_live(beat, signal, &NoLive)
    }

    /// As [`Self::sample`], audio signals read from `live`. A followed
    /// signal is only known as it plays, so the nudge doesn't move it.
    pub fn sample_live(
        &self,
        beat: f64,
        signal: Option<&CompiledSignal>,
        live: &dyn Live,
    ) -> (f64, f64) {
        let (x, presence) = match signal {
            // Signals run in song time: read at the song position.
            Some(signal) => signal.sample_live(beat + self.nudge, live),
            None => {
                let phase = ((beat - self.start + self.nudge) / self.cycle).rem_euclid(1.0);
                (shape_value(&self.points, phase), 1.0)
            }
        };
        let x = if self.invert { 1.0 - x } else { x };
        let mut fade: f64 = 1.0;
        if self.fade_in > 0.0 {
            fade = fade.min((beat - self.start) / self.fade_in);
        }
        if self.fade_out > 0.0 {
            fade = fade.min((self.end - beat) / self.fade_out);
        }
        (x, self.depth * fade.clamp(0.0, 1.0) * presence)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledLayer {
    pub mode: BlendMode,
    /// Scales every block's weight.
    pub mix: f64,
    /// The signal the layer listens to, its own copy. `None` draws.
    pub signal: Option<CompiledSignal>,
    /// Sorted by start, non-overlapping.
    pub blocks: Vec<CompiledBlock>,
}

impl CompiledLayer {
    /// Compiles a layer with no signals available: a listening layer has no
    /// effect.
    pub fn compile(layer: &Layer) -> Self {
        Self::compile_with(layer, &Signals::default())
    }

    /// Compiles a layer, taking the signal it listens to (if any) from
    /// `signals`. A missing signal has no effect.
    pub fn compile_with(layer: &Layer, signals: &Signals) -> Self {
        let mut blocks: Vec<CompiledBlock> = layer
            .blocks
            .iter()
            .map(|(_, block)| CompiledBlock::compile(block))
            .collect();
        blocks.sort_by(|a, b| a.start.total_cmp(&b.start));
        Self {
            mode: layer.mode,
            mix: layer.mix,
            signal: layer
                .signal
                .map(|id| signals.get(id).cloned().unwrap_or(CompiledSignal::Silent)),
            blocks,
        }
    }

    /// The layer's shape value and weight at `beat`; `None` in a gap.
    pub fn sample(&self, beat: f64) -> Option<(f64, f64)> {
        self.sample_live(beat, &NoLive)
    }

    /// As [`Self::sample`], audio signals read from `live`.
    pub fn sample_live(&self, beat: f64, live: &dyn Live) -> Option<(f64, f64)> {
        let (x, w) = self
            .block_at(beat)?
            .sample_live(beat, self.signal.as_ref(), live);
        Some((x, w * self.mix))
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
        Self::compile_with(state, &Signals::default())
    }

    /// Compiles a parameter, with the signals its layers listen to.
    pub fn compile_with(state: &ParamState, signals: &Signals) -> Self {
        Self::compile_layers_with(state.base, &state.layers, signals)
    }

    pub fn compile_layers(base: f64, layers: &[(LayerId, Layer)]) -> Self {
        Self::compile_layers_with(base, layers, &Signals::default())
    }

    pub fn compile_layers_with(base: f64, layers: &[(LayerId, Layer)], signals: &Signals) -> Self {
        Self {
            base,
            layers: layers
                .iter()
                .filter(|(_, l)| l.enabled && !l.blocks.is_empty())
                .map(|(_, l)| CompiledLayer::compile_with(l, signals))
                .collect(),
        }
    }

    /// The parameter's normalized value at `beat`, with nothing followed.
    /// Allocation-free.
    pub fn value_at(&self, beat: f64) -> f64 {
        self.value_at_live(beat, &NoLive)
    }

    /// The value at `beat`, audio signals read from `live`.
    /// Allocation-free.
    pub fn value_at_live(&self, beat: f64, live: &dyn Live) -> f64 {
        let mut v = self.base;
        for layer in &self.layers {
            if let Some((x, w)) = layer.sample_live(beat, live) {
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

    /// A block's shape value and weight at `beat`.
    fn sample(block: &Block, beat: f64) -> (f64, f64) {
        CompiledBlock::compile(block).sample(beat, None)
    }

    const MODES: [BlendMode; 2] = [BlendMode::Add, BlendMode::Multiply];

    #[test]
    fn add_pushes_up_above_the_centre_and_down_below_it() {
        assert!(close(blend(BlendMode::Add, 0.5, 0.75, 1.0), 1.0));
        assert!(close(blend(BlendMode::Add, 0.5, 0.25, 1.0), 0.0));
        assert!(close(blend(BlendMode::Add, 0.5, 1.0, 0.25), 0.75));
        assert!(
            close(blend(BlendMode::Add, 0.2, 0.0, 1.0), -0.8),
            "unclamped"
        );
    }

    #[test]
    fn multiply_scales_up_above_the_centre_and_down_below_it() {
        assert!(close(blend(BlendMode::Multiply, 0.8, 0.0, 1.0), 0.0));
        assert!(close(blend(BlendMode::Multiply, 0.8, 0.25, 1.0), 0.4));
        assert!(close(blend(BlendMode::Multiply, 0.4, 1.0, 1.0), 0.8));
        assert!(close(blend(BlendMode::Multiply, 0.8, 0.0, 0.5), 0.4));
    }

    #[test]
    fn the_centre_line_has_no_effect_in_every_mode() {
        for mode in MODES {
            assert_eq!(blend(mode, 0.42, 0.5, 1.0), 0.42, "{mode:?}");
        }
    }

    #[test]
    fn zero_weight_changes_nothing_in_every_mode() {
        for mode in MODES {
            assert_eq!(blend(mode, 0.42, 0.9, 0.0), 0.42, "{mode:?}");
        }
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
                BlendMode::Add,
                vec![block(1.0, 1.0, Shape::constant(0.75))],
            )],
        );
        assert_eq!(a.value_at(0.5), 0.3);
        assert!(close(a.value_at(1.0), 0.8));
        assert!(close(a.value_at(1.99), 0.8));
        assert_eq!(a.value_at(2.0), 0.3);
    }

    #[test]
    fn layers_apply_bottom_to_top() {
        // A rising ramp, then a wobble on top: a rising wobble.
        let ramp = layer(BlendMode::Add, vec![block(0.0, 4.0, Shape::ramp(0.5, 0.9))]);
        let mut wobble_block = block(0.0, 4.0, Shape::ramp(0.0, 1.0));
        wobble_block.cycle = Some(beats(1.0));
        wobble_block.depth = 0.1;
        let wobble = layer(BlendMode::Add, vec![wobble_block]);
        let a = automation(0.2, vec![ramp, wobble]);
        // At beat 2 the ramp has pushed up 0.4; each beat starts with the
        // wobble at its lowest (-0.1).
        assert!(close(a.value_at(2.0), 0.2 + 0.4 - 0.1));
        // Halfway through a beat the wobble is on the centre line.
        assert!(close(a.value_at(2.5), 0.2 + 0.5));
    }

    #[test]
    fn order_matters() {
        let add = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(0.75))]);
        let halve = layer(
            BlendMode::Multiply,
            vec![block(0.0, 1.0, Shape::constant(0.25))],
        );
        let up = automation(0.4, vec![add.clone(), halve.clone()]);
        let down = automation(0.4, vec![halve, add]);
        assert!(close(up.value_at(0.5), 0.45));
        assert!(close(down.value_at(0.5), 0.7));
    }

    #[test]
    fn clamps_once_after_the_whole_stack() {
        let push = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        let pull = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(0.2))]);
        // 0.8 + 1.0 = 1.8 (not clamped yet), then - 0.6 = 1.2, clamped to 1.
        assert_eq!(automation(0.8, vec![push.clone()]).value_at(0.5), 1.0);
        let pulled = automation(0.8, vec![push, pull.clone(), pull]);
        assert!(close(pulled.value_at(0.5), 0.6));
    }

    #[test]
    fn disabled_layers_are_skipped() {
        let mut off = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        off.1.enabled = false;
        assert_eq!(automation(0.2, vec![off]).value_at(0.5), 0.2);
    }

    #[test]
    fn multiply_ducks_by_drawing_below_the_centre() {
        // Full duck at the start, back to the centre line over the block.
        let duck = block(0.0, 1.0, Shape::ramp(0.0, 0.5));
        let a = automation(0.8, vec![layer(BlendMode::Multiply, vec![duck])]);
        assert_eq!(a.value_at(0.0), 0.0);
        assert!(close(a.value_at(0.5), 0.4));
    }

    #[test]
    fn mix_scales_how_much_of_the_layer_reaches_the_result() {
        let mut half = layer(BlendMode::Add, vec![block(0.0, 1.0, Shape::constant(1.0))]);
        half.1.mix = 0.5;
        assert!(close(
            automation(0.2, vec![half.clone()]).value_at(0.5),
            0.7
        ));
        half.1.mix = 0.0;
        assert_eq!(automation(0.2, vec![half]).value_at(0.5), 0.2);
    }

    #[test]
    fn short_cycles_repeat_the_shape() {
        let mut b = block(2.0, 4.0, Shape::ramp(0.0, 1.0));
        b.cycle = Some(beats(1.0));
        for beat in [2.0, 3.0, 4.0, 5.0] {
            assert!(close(sample(&b, beat).0, 0.0), "{beat}");
            assert!(close(sample(&b, beat + 0.5).0, 0.5), "{beat}");
        }
    }

    #[test]
    fn stretching_a_block_without_a_cycle_stretches_the_shape() {
        let short = block(0.0, 1.0, Shape::ramp(0.0, 1.0));
        let long = block(0.0, 4.0, Shape::ramp(0.0, 1.0));
        assert!(close(sample(&short, 0.5).0, sample(&long, 2.0).0));
    }

    #[test]
    fn invert_flips_the_shape() {
        let mut b = block(0.0, 1.0, Shape::ramp(0.0, 1.0));
        b.invert = true;
        assert!(close(sample(&b, 0.25).0, 0.75));
    }

    #[test]
    fn nudge_shifts_where_the_shape_is_read() {
        let mut b = block(0.0, 4.0, Shape::ramp(0.0, 1.0));
        b.cycle = Some(beats(1.0));
        b.nudge = beats(0.25);
        assert!(close(sample(&b, 1.0).0, 0.25));
        b.nudge = beats(-0.25);
        assert!(close(sample(&b, 1.0).0, 0.75));
    }

    #[test]
    fn fades_ramp_the_weight_at_the_block_edges() {
        let mut b = block(0.0, 4.0, Shape::constant(1.0));
        b.fade_in = beats(1.0);
        b.fade_out = beats(2.0);
        assert_eq!(sample(&b, 0.0).1, 0.0);
        assert!(close(sample(&b, 0.5).1, 0.5));
        assert!(close(sample(&b, 1.5).1, 1.0));
        assert!(close(sample(&b, 3.0).1, 0.5));
        assert!(close(sample(&b, 4.0 - 1e-9).1, 0.0));
    }

    #[test]
    fn depth_scales_the_effect() {
        let mut b = block(0.0, 1.0, Shape::constant(1.0));
        b.depth = 0.25;
        assert!(close(sample(&b, 0.5).1, 0.25));
        let a = automation(0.0, vec![layer(BlendMode::Add, vec![b])]);
        assert!(close(a.value_at(0.5), 0.25));
    }

    #[test]
    fn finds_the_right_block_among_many() {
        let blocks: Vec<Block> = (0..100)
            .map(|i| block(f64::from(i) * 2.0, 1.0, Shape::constant(0.5)))
            .collect();
        let (_, l) = layer(BlendMode::Add, blocks);
        let compiled = CompiledLayer::compile(&l);
        assert_eq!(compiled.block_at(50.5).map(|b| b.start), Some(50.0));
        assert!(compiled.block_at(51.5).is_none(), "gap between blocks");
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
