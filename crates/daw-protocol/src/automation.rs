//! Parameters, layers, blocks and shapes: the data of the one rule.
//!
//! > Every parameter has a stack of layers. Each layer holds blocks. Each
//! > block is either drawn there or linked to a signal.
//!
//! All values here are normalized (`0..=1`); a parameter's [`Scale`] maps
//! them to real units.

use crate::Ticks;

/// Identifies a device in a node's effect chain. Chosen by its creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DeviceId(pub u64);

/// Identifies a layer anywhere in the project. Chosen by its creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct LayerId(pub u64);

/// Identifies a block anywhere in the project. Chosen by its creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct BlockId(pub u64);

/// Identifies a signal. Chosen by its creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct SignalId(pub u64);

/// A device on a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceRef {
    /// The node's instrument.
    Instrument,
    /// The node's own volume and pan.
    Channel,
    /// An effect in the node's chain.
    Effect(DeviceId),
}

/// A parameter: the device it belongs to and its index on that device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId {
    pub device: DeviceRef,
    pub index: u32,
}

/// Built-in effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    /// Multi-mode resonant filter.
    Filter,
}

/// An effect and the state of its parameters.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    /// One entry per parameter, in index order. Empty means all defaults.
    pub params: Vec<ParamState>,
}

impl Effect {
    /// An effect with every parameter at its default.
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            params: Vec::new(),
        }
    }
}

/// A parameter's default state and its layer stack.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamState {
    /// Normalized value the parameter rests at when no layer acts on it.
    pub base: f64,
    /// Layers from bottom to top.
    pub layers: Vec<(LayerId, Layer)>,
}

/// How a layer combines with the layers below it. Shapes are centred: the
/// middle of the lane (`x = 0.5`) has no effect, the top is +1 and the
/// bottom is -1, scaled by the block's weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BlendMode {
    /// Push up or pull down by up to the full range: `v + (2x - 1)·w`.
    Add,
    /// Scale: zero at the bottom, unchanged at the centre, doubled at the
    /// top: `v · (1 + (2x - 1)·w)`. Ducking is a shape drawn below the centre.
    Multiply,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Layer {
    /// Shown in the layer list. Files from before names existed load with
    /// an empty name.
    #[serde(default)]
    pub name: String,
    pub mode: BlendMode,
    pub enabled: bool,
    /// How much of the layer reaches the result, `0..=1`, like a layer's
    /// opacity: it scales every block's weight. Files from before mix
    /// existed load at full.
    #[serde(default = "full_mix")]
    pub mix: f64,
    /// Non-overlapping blocks, sorted by start.
    pub blocks: Vec<(BlockId, Block)>,
}

impl Layer {
    pub fn new(mode: BlendMode) -> Self {
        Self::named("Layer", mode)
    }

    pub fn named(name: &str, mode: BlendMode) -> Self {
        Self {
            name: name.to_owned(),
            mode,
            enabled: true,
            mix: 1.0,
            blocks: Vec::new(),
        }
    }
}

fn full_mix() -> f64 {
    1.0
}

/// A span of movement on a layer.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    /// Song position of the block start.
    pub start: Ticks,
    pub length: Ticks,
    /// Length of one cycle of the shape; it repeats to fill the block.
    /// `None` stretches one cycle across the whole block.
    pub cycle: Option<Ticks>,
    pub shape: Shape,
    /// How strongly the block acts, `0..=1`.
    pub depth: f64,
    /// Use `1 - x` instead of the shape value `x`.
    pub invert: bool,
    /// Shifts where the shape is read, without moving the block.
    pub nudge: Ticks,
    /// The block's weight ramps up from zero over this long from its start.
    pub fade_in: Ticks,
    /// The block's weight ramps down to zero over this long before its end.
    pub fade_out: Ticks,
    /// When set, the block plays this signal instead of its own shape. The
    /// shape and cycle are kept, so unlinking brings the drawing back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<SignalLink>,
}

/// A block's link to a signal: which one, and where the bottom and top of
/// the signal land (`x = low + (high - low) · signal`). Both `0..=1`;
/// `low > high` flips it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SignalLink {
    pub signal: SignalId,
    pub low: f64,
    pub high: f64,
}

impl SignalLink {
    /// The full signal, bottom to top.
    pub fn new(signal: SignalId) -> Self {
        Self {
            signal,
            low: 0.0,
            high: 1.0,
        }
    }
}

/// Movement drawn once, used anywhere: a shape over song time with no units
/// or target of its own. Blocks link to it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Signal {
    pub name: String,
    /// Index into the nine-colour content palette.
    pub color: u8,
    pub source: SignalSource,
}

/// Where a signal's shape comes from.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SignalSource {
    /// Drawn blocks on the signal's own lane: non-overlapping, sorted by
    /// start, never linked. Gaps are silence.
    Drawn(Vec<(BlockId, Block)>),
    /// An envelope restarted by every matching note.
    Notes(NotesSource),
}

/// A signal made from MIDI notes: each note with a pitch in
/// `lowest..=highest` restarts `shape`, played over `length` from the note's
/// start. After the envelope ends, or before the first note, the signal is
/// silent. The notes come from the project's node.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NotesSource {
    pub lowest: u8,
    pub highest: u8,
    pub length: Ticks,
    pub shape: Shape,
}

impl Block {
    /// A block at full depth with no repeat, nudge or fades.
    pub fn new(start: Ticks, length: Ticks, shape: Shape) -> Self {
        Self {
            start,
            length,
            cycle: None,
            shape,
            depth: 1.0,
            invert: false,
            nudge: Ticks::ZERO,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            link: None,
        }
    }

    /// The same block linked to `signal`, full range.
    pub fn linked(self, signal: SignalId) -> Self {
        Self {
            link: Some(SignalLink::new(signal)),
            ..self
        }
    }

    pub fn end(&self) -> Ticks {
        self.start + self.length
    }
}

/// A shape over one cycle, as points joined by segments.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Shape {
    /// Sorted by `at`. The shape holds the first point's value before it and
    /// the last point's value after it.
    pub points: Vec<ShapePoint>,
}

impl Shape {
    /// A flat line.
    pub fn constant(value: f64) -> Self {
        Self {
            points: vec![ShapePoint::new(0.0, value)],
        }
    }

    /// A straight line from `from` to `to` across the cycle.
    pub fn ramp(from: f64, to: f64) -> Self {
        Self {
            points: vec![ShapePoint::new(0.0, from), ShapePoint::new(1.0, to)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShapePoint {
    /// Position within the cycle, `0..=1`.
    pub at: f64,
    /// `0..=1`.
    pub value: f64,
    /// How the shape travels from this point to the next.
    pub segment: Segment,
}

impl ShapePoint {
    pub fn new(at: f64, value: f64) -> Self {
        Self {
            at,
            value,
            segment: Segment::Linear,
        }
    }

    pub fn with_segment(self, segment: Segment) -> Self {
        Self { segment, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Segment {
    Linear,
    /// Stay at this point's value until the next point (a step).
    Hold,
    /// Bend the line: positive tension eases in (slow start), negative eases
    /// out (fast start). `-1..=1`.
    Curve(f32),
}

/// Maps a parameter's normalized value to real units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scale {
    Linear {
        min: f64,
        max: f64,
    },
    /// Even steps in ratio: frequencies and times. `min` must be positive.
    Logarithmic {
        min: f64,
        max: f64,
    },
    /// Decibels, linear in dB between `min_db` and `max_db`, with 0 as
    /// silence (`-inf` dB).
    Decibels {
        min_db: f64,
        max_db: f64,
    },
    /// One of `count` options, numbered from 0.
    Choice {
        count: u32,
    },
}

impl Scale {
    pub fn to_plain(self, normalized: f64) -> f64 {
        let n = normalized.clamp(0.0, 1.0);
        match self {
            Self::Linear { min, max } => min + (max - min) * n,
            Self::Logarithmic { min, max } => min * (max / min).powf(n),
            Self::Decibels { min_db, max_db } => {
                if n == 0.0 {
                    f64::NEG_INFINITY
                } else {
                    min_db + (max_db - min_db) * n
                }
            }
            Self::Choice { count } => (n * f64::from(count.saturating_sub(1))).round(),
        }
    }

    pub fn to_normalized(self, plain: f64) -> f64 {
        if plain.is_nan() {
            return 0.0;
        }
        let n = match self {
            Self::Linear { min, max } => (plain - min) / (max - min),
            Self::Logarithmic { min, max } => (plain / min).ln() / (max / min).ln(),
            Self::Decibels { min_db, max_db } => {
                if plain == f64::NEG_INFINITY {
                    0.0
                } else {
                    // Anything audible stays above zero, which means silence.
                    ((plain - min_db) / (max_db - min_db)).max(f64::MIN_POSITIVE)
                }
            }
            Self::Choice { count } => plain / f64::from(count.saturating_sub(1).max(1)),
        };
        if n.is_nan() { 0.0 } else { n.clamp(0.0, 1.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn linear_scale() {
        let s = Scale::Linear {
            min: -1.0,
            max: 1.0,
        };
        assert_eq!(s.to_plain(0.5), 0.0);
        assert_eq!(s.to_normalized(1.0), 1.0);
    }

    #[test]
    fn logarithmic_scale_moves_evenly_in_ratio() {
        let s = Scale::Logarithmic {
            min: 20.0,
            max: 20_000.0,
        };
        assert!(close(s.to_plain(0.0), 20.0));
        assert!(close(s.to_plain(1.0 / 3.0), 200.0));
        assert!(close(s.to_plain(2.0 / 3.0), 2_000.0));
        assert!(close(s.to_normalized(2_000.0), 2.0 / 3.0));
    }

    #[test]
    fn decibel_scale_reaches_silence_only_at_zero() {
        let s = Scale::Decibels {
            min_db: -60.0,
            max_db: 12.0,
        };
        assert_eq!(s.to_plain(0.0), f64::NEG_INFINITY);
        assert!(close(s.to_plain(f64::MIN_POSITIVE), -60.0));
        assert!(close(s.to_plain(60.0 / 72.0), 0.0));
        assert_eq!(s.to_normalized(f64::NEG_INFINITY), 0.0);
        assert!(s.to_normalized(-200.0) > 0.0);
    }

    #[test]
    fn choice_scale_rounds_to_an_option() {
        let s = Scale::Choice { count: 3 };
        assert_eq!(s.to_plain(0.0), 0.0);
        assert_eq!(s.to_plain(0.3), 1.0);
        assert_eq!(s.to_plain(1.0), 2.0);
        assert_eq!(s.to_normalized(2.0), 1.0);
    }

    #[test]
    fn conversions_round_trip_and_clamp() {
        let scales = [
            Scale::Linear { min: 0.0, max: 8.0 },
            Scale::Logarithmic {
                min: 0.001,
                max: 10.0,
            },
            Scale::Decibels {
                min_db: -60.0,
                max_db: 0.0,
            },
        ];
        for scale in scales {
            for n in [0.1, 0.25, 0.5, 0.9, 1.0] {
                assert!(
                    close(scale.to_normalized(scale.to_plain(n)), n),
                    "{scale:?} {n}"
                );
            }
            assert_eq!(scale.to_plain(2.0), scale.to_plain(1.0));
            assert_eq!(scale.to_normalized(f64::NAN), 0.0);
        }
    }
}
