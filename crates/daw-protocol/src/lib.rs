//! The boundary between the engine and any front end.
//!
//! A front end never owns project state. It sends [`Command`]s to the engine
//! and renders the [`Snapshot`]s it gets back. This crate has no dependency on
//! the engine, so either side can be replaced without touching the other.

mod automation;
mod time;

pub use automation::{
    BlendMode, Block, BlockId, DeviceId, DeviceRef, Effect, EffectKind, Layer, LayerId, ParamId,
    ParamState, Scale, Segment, Shape, ShapePoint,
};
pub use time::{TICKS_PER_BEAT, Ticks};

use std::fmt;

/// A request from a front end to the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start the transport.
    Play,
    /// Stop the transport, keeping the current position.
    Stop,
    /// Move the transport to a song position.
    Locate { position: Ticks },
    /// Set or clear the loop region. Not part of undo history.
    SetLoop(Option<LoopRange>),
    /// Change the project. Edits are recorded in the undo history.
    Edit(Edit),
    /// Revert the most recent edit.
    Undo,
    /// Reapply the most recently undone edit.
    Redo,
}

/// An undoable change to the project.
///
/// Ids are chosen by the sender and must be unused anywhere in the project.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    RenameSong {
        name: String,
    },
    RenameNode {
        name: String,
    },
    /// Set the song tempo in beats per minute.
    SetTempo {
        bpm: f64,
    },
    AddClip {
        id: ClipId,
        clip: MidiClip,
    },
    RemoveClip {
        id: ClipId,
    },
    /// Silence the node.
    SetMute {
        muted: bool,
    },
    /// While any node is soloed, only soloed nodes are heard.
    SetSolo {
        soloed: bool,
    },
    /// Set a parameter's default state (normalized). This is what moving a
    /// knob does; it never records movement.
    SetParam {
        param: ParamId,
        value: f64,
    },
    /// Insert an effect into the chain at `index` (0 is first).
    AddEffect {
        id: DeviceId,
        index: usize,
        effect: Effect,
    },
    /// Remove an effect, with its parameters and layers.
    RemoveEffect {
        id: DeviceId,
    },
    /// Insert a layer into a parameter's stack at `index` (0 is the bottom).
    AddLayer {
        param: ParamId,
        id: LayerId,
        index: usize,
        layer: Layer,
    },
    /// Remove a layer with its blocks.
    RemoveLayer {
        id: LayerId,
    },
    RenameLayer {
        id: LayerId,
        name: String,
    },
    SetLayerOptions {
        id: LayerId,
        mode: BlendMode,
        enabled: bool,
    },
    AddBlock {
        layer: LayerId,
        id: BlockId,
        block: Block,
    },
    RemoveBlock {
        id: BlockId,
    },
    /// Replace a block: move, stretch, reshape or change its controls.
    SetBlock {
        id: BlockId,
        block: Block,
    },
}

/// Identifies a clip. Chosen by whoever creates the clip.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ClipId(pub u64);

/// A region of MIDI notes on the timeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MidiClip {
    /// Song position of the clip start.
    pub start: Ticks,
    pub length: Ticks,
    /// Notes positioned relative to the clip start. Only the parts of notes
    /// inside `0..length` play.
    pub notes: Vec<MidiNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MidiNote {
    /// Position relative to the clip start.
    pub start: Ticks,
    pub length: Ticks,
    /// MIDI note number, 0..=127. 60 is middle C, 69 is A4 (440 Hz).
    pub pitch: u8,
    /// 1..=127.
    pub velocity: u8,
}

/// A loop region from `start` (inclusive) to `end` (exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoopRange {
    pub start: Ticks,
    pub end: Ticks,
}

/// A read-only view of engine state for a front end to render.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Increases every time a command changes engine state.
    pub revision: u64,
    pub song_name: String,
    pub tempo_bpm: f64,
    pub playing: bool,
    /// Transport position in beats, as last reported by the audio side.
    pub position_beats: f64,
    pub loop_range: Option<LoopRange>,
    pub node: NodeSnapshot,
    /// Clips in id order.
    pub clips: Vec<(ClipId, MidiClip)>,
    /// The node's devices in signal order: instrument, effects, channel.
    pub devices: Vec<DeviceSnapshot>,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub name: String,
    /// Index into the nine-colour content palette.
    pub color: u8,
    pub muted: bool,
    pub soloed: bool,
}

/// Peak output levels since they were last read, as linear amplitudes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Meter {
    pub left: f32,
    pub right: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceSnapshot {
    pub device: DeviceRef,
    pub name: String,
    pub params: Vec<ParamSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSnapshot {
    pub id: ParamId,
    pub name: String,
    pub scale: Scale,
    /// Unit suffix for display, e.g. "Hz". Empty for unitless values.
    pub unit: String,
    /// Display names of the options, for [`Scale::Choice`] parameters.
    pub choices: Vec<String>,
    /// Normalized default state.
    pub base: f64,
    /// Normalized value at the playhead, after all layers.
    pub value: f64,
    /// The value at the playhead in real units, ready to show.
    pub display: String,
    /// Layers from bottom to top.
    pub layers: Vec<(LayerId, Layer)>,
}

/// Why the engine rejected a command. A rejected command changes nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandError {
    TempoOutOfRange {
        bpm: f64,
        min: f64,
        max: f64,
    },
    NegativePosition {
        position: Ticks,
    },
    InvalidLoop {
        range: LoopRange,
        min_length: Ticks,
    },
    DuplicateClip {
        id: ClipId,
    },
    UnknownClip {
        id: ClipId,
    },
    InvalidClip {
        reason: InvalidClip,
    },
    UnknownParam {
        param: ParamId,
    },
    /// Normalized values must be within `0..=1`.
    ValueOutOfRange {
        value: f64,
    },
    DuplicateDevice {
        id: DeviceId,
    },
    UnknownDevice {
        id: DeviceId,
    },
    TooManyEffects {
        max: usize,
    },
    /// An insert position past the end of a list.
    IndexOutOfRange {
        index: usize,
        len: usize,
    },
    /// An effect's parameter list does not match its kind.
    WrongParamCount {
        expected: usize,
        found: usize,
    },
    DuplicateLayer {
        id: LayerId,
    },
    UnknownLayer {
        id: LayerId,
    },
    DuplicateBlock {
        id: BlockId,
    },
    UnknownBlock {
        id: BlockId,
    },
    InvalidBlock {
        id: BlockId,
        reason: InvalidBlock,
    },
    /// The block would overlap another block on the same layer.
    OverlappingBlocks {
        id: BlockId,
        other: BlockId,
    },
    /// Names must contain something other than whitespace.
    EmptyName,
    NothingToUndo,
    NothingToRedo,
}

/// Why a clip was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidClip {
    NegativeStart,
    EmptyLength,
    /// The note at this index has a pitch above 127.
    NotePitch {
        index: usize,
    },
    /// The note at this index has a velocity of 0 or above 127.
    NoteVelocity {
        index: usize,
    },
    /// The note at this index has zero or negative length.
    NoteLength {
        index: usize,
    },
}

/// Why a block was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidBlock {
    NegativeStart,
    EmptyLength,
    EmptyCycle,
    NegativeFade,
    /// Depth must be within `0..=1`.
    Depth,
    /// A shape needs at least one point.
    NoPoints,
    /// Point positions must be within `0..=1` and in order.
    PointOrder {
        index: usize,
    },
    /// Point values must be within `0..=1`.
    PointValue {
        index: usize,
    },
    /// Curve tension must be within `-1..=1`.
    CurveTension {
        index: usize,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TempoOutOfRange { bpm, min, max } => {
                write!(f, "tempo {bpm} BPM is outside {min}..={max} BPM")
            }
            Self::NegativePosition { position } => {
                write!(f, "position {} ticks is before the song start", position.0)
            }
            Self::InvalidLoop { range, min_length } => write!(
                f,
                "loop {}..{} ticks must start at or after 0 and be at least {} ticks long",
                range.start.0, range.end.0, min_length.0
            ),
            Self::DuplicateClip { id } => write!(f, "clip {} already exists", id.0),
            Self::UnknownClip { id } => write!(f, "clip {} does not exist", id.0),
            Self::InvalidClip { reason } => write!(f, "invalid clip: {reason}"),
            Self::UnknownParam { param } => write!(f, "parameter {param:?} does not exist"),
            Self::ValueOutOfRange { value } => {
                write!(f, "normalized value {value} is outside 0..=1")
            }
            Self::DuplicateDevice { id } => write!(f, "device {} already exists", id.0),
            Self::UnknownDevice { id } => write!(f, "device {} does not exist", id.0),
            Self::TooManyEffects { max } => write!(f, "a node holds at most {max} effects"),
            Self::IndexOutOfRange { index, len } => {
                write!(f, "index {index} is past the end of a list of {len}")
            }
            Self::WrongParamCount { expected, found } => {
                write!(f, "expected {expected} parameters, found {found}")
            }
            Self::DuplicateLayer { id } => write!(f, "layer {} already exists", id.0),
            Self::UnknownLayer { id } => write!(f, "layer {} does not exist", id.0),
            Self::DuplicateBlock { id } => write!(f, "block {} already exists", id.0),
            Self::UnknownBlock { id } => write!(f, "block {} does not exist", id.0),
            Self::InvalidBlock { id, reason } => write!(f, "invalid block {}: {reason}", id.0),
            Self::OverlappingBlocks { id, other } => {
                write!(f, "block {} would overlap block {}", id.0, other.0)
            }
            Self::EmptyName => f.write_str("names cannot be empty"),
            Self::NothingToUndo => f.write_str("nothing to undo"),
            Self::NothingToRedo => f.write_str("nothing to redo"),
        }
    }
}

impl fmt::Display for InvalidClip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeStart => f.write_str("starts before the song start"),
            Self::EmptyLength => f.write_str("has no length"),
            Self::NotePitch { index } => write!(f, "note {index} has a pitch above 127"),
            Self::NoteVelocity { index } => {
                write!(f, "note {index} has a velocity outside 1..=127")
            }
            Self::NoteLength { index } => write!(f, "note {index} has no length"),
        }
    }
}

impl fmt::Display for InvalidBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeStart => f.write_str("starts before the song start"),
            Self::EmptyLength => f.write_str("has no length"),
            Self::EmptyCycle => f.write_str("has a cycle with no length"),
            Self::NegativeFade => f.write_str("has a negative fade"),
            Self::Depth => f.write_str("depth is outside 0..=1"),
            Self::NoPoints => f.write_str("shape has no points"),
            Self::PointOrder { index } => {
                write!(f, "point {index} is outside 0..=1 or out of order")
            }
            Self::PointValue { index } => write!(f, "point {index} value is outside 0..=1"),
            Self::CurveTension { index } => {
                write!(f, "point {index} curve tension is outside -1..=1")
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// A parameter value in real units formatted for display, using its unit and
/// option names (see [`ParamSnapshot`]).
pub fn format_param_value(unit: &str, choices: &[impl AsRef<str>], plain: f64) -> String {
    match unit {
        _ if !choices.is_empty() => choices
            .get(plain as usize)
            .map_or_else(|| "?".to_owned(), |c| c.as_ref().to_owned()),
        "Hz" if plain >= 1_000.0 => format!("{:.2} kHz", plain / 1_000.0),
        "Hz" => format!("{plain:.0} Hz"),
        "s" if plain < 1.0 => format!("{:.0} ms", plain * 1_000.0),
        "s" => format!("{plain:.2} s"),
        "dB" if plain == f64::NEG_INFINITY => "-inf dB".to_owned(),
        "dB" => format!("{plain:.1} dB"),
        "pan" if plain.abs() < 0.005 => "C".to_owned(),
        "pan" if plain < 0.0 => format!("L{:.0}", -plain * 100.0),
        "pan" => format!("R{:.0}", plain * 100.0),
        "" => format!("{plain:.2}"),
        unit => format!("{plain:.2} {unit}"),
    }
}

impl ParamSnapshot {
    /// A normalized value of this parameter, formatted for display.
    pub fn format_normalized(&self, normalized: f64) -> String {
        format_param_value(&self.unit, &self.choices, self.scale.to_plain(normalized))
    }
}
