//! Parameters, layers, blocks and shapes: the data of the one rule.
//!
//! > Every parameter has a stack of layers. Each layer holds blocks. A layer
//! > either draws its blocks or listens to a signal through them.
//!
//! All values here are normalized (`0..=1`); a parameter's [`Scale`] maps
//! them to real units.

use crate::Ticks;

/// Identifies a device in a node's effect chain. Chosen by its creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct DeviceId(pub u64);

/// Identifies an audio sample in the project's sample table. Chosen by its
/// creator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct SampleId(pub u64);

/// Identifies a node anywhere in the project. Chosen by its creator; the
/// Song, the root of the tree, is always [`NodeId::ROOT`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct NodeId(pub u64);

impl NodeId {
    /// The Song: the root of the tree and the master output.
    pub const ROOT: Self = Self(0);
}

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
    /// One of the node's instruments.
    Instrument(DeviceId),
    /// The node's own volume and pan.
    Channel,
    /// An effect in the node's chain.
    Effect(DeviceId),
}

/// A parameter: its node, the device it belongs to and its index on that
/// device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId {
    pub node: NodeId,
    pub device: DeviceRef,
    pub index: u32,
}

/// A set of MIDI notes (0..=127): which notes an instrument plays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct NoteSet([u64; 2]);

impl NoteSet {
    pub const NONE: Self = Self([0; 2]);
    pub const ALL: Self = Self([u64::MAX; 2]);

    /// Just one note.
    pub const fn single(note: u8) -> Self {
        let mut bits = [0; 2];
        bits[(note as usize >> 6) & 1] = 1 << (note & 63);
        Self(bits)
    }

    pub fn contains(self, note: u8) -> bool {
        note < 128 && self.0[usize::from(note >> 6)] & (1 << (note & 63)) != 0
    }

    pub fn union(self, other: Self) -> Self {
        Self([self.0[0] | other.0[0], self.0[1] | other.0[1]])
    }

    pub fn is_empty(self) -> bool {
        self == Self::NONE
    }

    pub fn len(self) -> u32 {
        self.0[0].count_ones() + self.0[1].count_ones()
    }

    /// The notes, lowest first.
    pub fn iter(self) -> impl Iterator<Item = u8> {
        (0..=127u8).filter(move |&n| self.contains(n))
    }
}

/// The note a SingleSampler plays until told otherwise: C1, where drum
/// kits put the kick.
pub const DEFAULT_TRIGGER_NOTE: u8 = 36;

/// Built-in instruments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum InstrumentKind {
    /// Saw synth with a filter and envelopes.
    Synth,
    /// Plays one audio sample, from the top, for every note (one-shot).
    SingleSampler,
    /// A plugin, run in its own process; `Instrument::plugin` says which.
    Plugin,
}

/// An instrument and the state of its parameters. A MIDI node can hold
/// several: each plays every note, and their sounds add up (layering).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Instrument {
    pub kind: InstrumentKind,
    /// Off, it is silent.
    #[serde(default = "on")]
    pub enabled: bool,
    /// One entry per parameter, in index order. Empty means all defaults.
    pub params: Vec<ParamState>,
    /// The sample a SingleSampler plays; `None` for none (silent) and for
    /// other instruments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<SampleId>,
    /// The one note a SingleSampler plays; `None` is
    /// [`DEFAULT_TRIGGER_NOTE`]. Other instruments ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<u8>,
    /// The plugin, for a plugin instrument (and only then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginSpec>,
    /// A plugin instrument's parameters added to the song, by the plugin's
    /// ids, as for a plugin effect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_params: Vec<PluginParam>,
}

impl Instrument {
    /// An instrument, on, with every parameter at its default.
    pub fn new(kind: InstrumentKind) -> Self {
        Self {
            kind,
            enabled: true,
            params: Vec::new(),
            sample: None,
            note: None,
            plugin: None,
            plugin_params: Vec::new(),
        }
    }

    /// A plugin instrument, on.
    pub fn plugin(spec: PluginSpec) -> Self {
        Self {
            plugin: Some(spec),
            ..Self::new(InstrumentKind::Plugin)
        }
    }

    /// The notes this instrument plays: a SingleSampler its trigger note,
    /// anything else every note.
    pub fn plays(&self) -> NoteSet {
        match self.kind {
            InstrumentKind::Synth | InstrumentKind::Plugin => NoteSet::ALL,
            InstrumentKind::SingleSampler => {
                NoteSet::single(self.note.unwrap_or(DEFAULT_TRIGGER_NOTE).min(127))
            }
        }
    }
}

/// Built-in effects, and plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    /// Multi-mode resonant filter.
    Filter,
    /// A plugin, run in its own process; `Effect::plugin` says which.
    Plugin,
}

/// Which plugin a plugin effect runs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PluginSpec {
    /// Built-in devices for testing the plugin plumbing.
    Test(TestPlugin),
    /// A VST3 plugin: its bundle and class id (Steinberg's text form), with
    /// the name and vendor it had, to show if it goes missing.
    Vst3 {
        bundle: std::path::PathBuf,
        cid: String,
        name: String,
        vendor: String,
    },
}

impl PluginSpec {
    /// The name to show for it.
    pub fn name(&self) -> &str {
        match self {
            Self::Test(_) => "Test",
            Self::Vst3 { name, .. } => name,
        }
    }
}

/// One of a plugin's automatable parameters, as the plugin describes it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginParamInfo {
    /// The plugin's id for it, stable across plugin versions.
    pub id: u32,
    pub name: String,
    /// The plugin's group for it ("Osc A", nested groups joined with
    /// " / "); empty for none.
    pub group: String,
    /// The plugin's unit label, e.g. "dB"; empty for none.
    pub unit: String,
    /// Steps for a stepped parameter (1 is a switch); 0 is continuous.
    pub steps: u32,
    /// The value the plugin holds now (normalized).
    pub value: f64,
    /// The plugin's own bypass switch (`kIsBypass`): the DAW uses it when
    /// the plugin is switched off.
    #[serde(default)]
    pub bypass: bool,
}

/// A plugin parameter added to the song: the plugin's id for it, its name
/// (kept to show if the plugin goes missing), and its resting value and
/// layers.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginParam {
    pub id: u32,
    pub name: String,
    #[serde(flatten)]
    pub state: ParamState,
}

/// A plugin instance's saved state: the processor's and the controller's
/// own data (each opaque to the DAW), written as base64 in files and
/// messages.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PluginData {
    #[serde(with = "base64_bytes")]
    pub component: Vec<u8>,
    #[serde(with = "base64_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub controller: Vec<u8>,
}

impl PluginData {
    pub fn is_empty(&self) -> bool {
        self.component.is_empty() && self.controller.is_empty()
    }
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TestPlugin {
    /// Output = input x 2 x parameter 0 (unity until it is set).
    Gain,
    /// Passes sound, then crashes its process on this block.
    CrashAt(u64),
    /// Passes sound, then never returns from this block.
    HangAt(u64),
    /// Passes sound, taking this long over every block.
    Slow { millis: u64 },
    /// Passes sound `frames` late, and says so (its latency).
    Delay { frames: u32 },
}

/// An effect and the state of its parameters.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    /// Off, the effect passes sound through untouched. Files from before
    /// the switch existed load with it on.
    #[serde(default = "on")]
    pub enabled: bool,
    /// One entry per parameter, in index order. Empty means all defaults.
    pub params: Vec<ParamState>,
    /// The plugin, for a plugin effect (and only then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginSpec>,
    /// A plugin effect's parameters added to the song, each named by the
    /// plugin's parameter id (`ParamId::index`), in the order added.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_params: Vec<PluginParam>,
}

fn on() -> bool {
    true
}

impl Effect {
    /// An effect, on, with every parameter at its default.
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            enabled: true,
            params: Vec::new(),
            plugin: None,
            plugin_params: Vec::new(),
        }
    }

    /// A plugin effect, on.
    pub fn plugin(spec: PluginSpec) -> Self {
        Self {
            plugin: Some(spec),
            ..Self::new(EffectKind::Plugin)
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
    /// Add the other way round, for the whole layer: `v - (2x - 1)·w`. Above
    /// the centre pulls down, below it pushes up.
    Subtract,
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
    /// The signal the layer listens to. Its blocks are then windows: the
    /// signal plays wherever a block sits, read at song position, and each
    /// block keeps its own shape for when the layer goes back to drawing.
    /// `None` is a drawn layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalId>,
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
            signal: None,
            blocks: Vec::new(),
        }
    }

    /// The same layer listening to `signal`.
    pub fn listening(self, signal: SignalId) -> Self {
        Self {
            signal: Some(signal),
            ..self
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
    /// How strongly the block acts, `0..=MAX_DEPTH`: 1 is as drawn, 2 twice
    /// as strong.
    pub depth: f64,
    /// Use `1 - x` instead of the shape value `x`.
    pub invert: bool,
    /// Shifts where the shape is read, without moving the block.
    pub nudge: Ticks,
    /// The block's weight ramps up from zero over this long from its start.
    pub fade_in: Ticks,
    /// The block's weight ramps down to zero over this long before its end.
    pub fade_out: Ticks,
}

/// Movement drawn once, used anywhere: a centred shape over song time with
/// no units or target of its own, like a layer lane. Layers listen to it.
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
    /// start. Gaps say nothing (no effect).
    Drawn(Vec<(BlockId, Block)>),
    /// A hit from every matching note.
    Notes(NotesSource),
    /// An audio node's loudness.
    Audio(AudioSource),
}

/// A signal that follows an audio node's loudness, measured before its
/// effects (nest the node in a parent and follow the parent to hear them).
/// Silence is the centre line (no effect); at or above `ceiling_db` the
/// signal is at the top. It is followed live as the song plays, so it is
/// always in step with what is heard. If the node is removed, the signal is
/// silent until it comes back.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioSource {
    /// The audio node whose sound is followed.
    pub node: NodeId,
    /// How fast the signal rises, in milliseconds.
    pub attack_ms: f64,
    /// How fast it falls back, in milliseconds.
    pub release_ms: f64,
    /// At or below this the signal rests on the centre line.
    pub floor_db: f64,
    /// At or above this the signal is at the top.
    pub ceiling_db: f64,
}

impl AudioSource {
    /// Longest attack or release.
    pub const MAX_TIME_MS: f64 = 2_000.0;
    /// Lowest floor and highest ceiling.
    pub const MIN_DB: f64 = -96.0;
    pub const MAX_DB: f64 = 24.0;

    /// A quick rise, a gentle fall, and -48 to 0 dB.
    pub fn new(node: NodeId) -> Self {
        Self {
            node,
            attack_ms: 5.0,
            release_ms: 150.0,
            floor_db: -48.0,
            ceiling_db: 0.0,
        }
    }
}

/// A signal made from MIDI notes. Each note (with the pitch range on, each
/// note with a pitch in `lowest..=highest`) is a hit that plays `shape` over
/// `window`, scaled towards the centre line by its velocity. In
/// [`HitMode::Trigger`] a hit plays the shape once through; in
/// [`HitMode::Loop`] it repeats it while the note is held. A new note
/// restarts the shape. Between hits the signal rests on the centre line,
/// which has no effect. The notes come from one node, `node`; if it is
/// removed, the signal is silent until it comes back.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NotesSource {
    /// The node whose notes make the hits.
    pub node: NodeId,
    /// Whether only notes in `lowest..=highest` hit. Off, every note hits;
    /// the range is kept for when it is switched back on.
    pub pitch_range: bool,
    pub lowest: u8,
    pub highest: u8,
    pub mode: HitMode,
    /// How long one play of the shape lasts.
    pub window: Ticks,
    /// What each hit plays, over the full range (`0.5` is the centre line).
    pub shape: crate::Curve,
    /// A hit's size is its velocity, `velocity / 127`. With the clamp on, it
    /// is held to `velocity_min..=velocity_max`, both within `0..=1`; with
    /// both at 1 every hit plays full size. Off, the clamp is kept for when
    /// it is switched back on.
    pub velocity_clamp: bool,
    pub velocity_min: f64,
    pub velocity_max: f64,
}

/// How a notes signal's hit plays its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HitMode {
    /// Once through from the note's start, whatever the note's length.
    Trigger,
    /// Repeated from the note's start while the note is held.
    Loop,
}

impl NotesSource {
    /// The window lengths offered: 1/8, 1/4, 1/2, 1, 2 and 4 bars (4/4).
    pub const WINDOWS: [Ticks; 6] = [
        Ticks(480),
        Ticks(960),
        Ticks(1920),
        Ticks(3840),
        Ticks(7680),
        Ticks(15360),
    ];

    /// Every note, as a short, snappy hit: straight to the top, then a line
    /// back to the centre over an eighth of a bar. Trigger mode; the pitch
    /// range and the velocity clamp start off.
    pub fn new(node: NodeId) -> Self {
        Self {
            node,
            pitch_range: false,
            velocity_clamp: false,
            lowest: 0,
            highest: 127,
            mode: HitMode::Trigger,
            window: Self::WINDOWS[0],
            shape: crate::Curve::line(1.0, 0.5),
            velocity_min: 0.0,
            velocity_max: 1.0,
        }
    }
}

impl Block {
    /// The strongest a block acts: twice as drawn.
    pub const MAX_DEPTH: f64 = 2.0;

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
    #[allow(clippy::unwrap_used, reason = "tests fail on error")]
    fn plugin_data_is_base64_and_round_trips() {
        let data = PluginData {
            component: vec![0, 1, 2, 250, 255],
            controller: Vec::new(),
        };
        let json = serde_json::to_string(&data).unwrap();
        assert_eq!(json, r#"{"component":"AAEC+v8="}"#);
        assert_eq!(serde_json::from_str::<PluginData>(&json).unwrap(), data);
        assert!(serde_json::from_str::<PluginData>(r#"{"component":"%%"}"#).is_err());
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
