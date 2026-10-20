//! The boundary between the engine and any front end.
//!
//! A front end never owns project state. It sends [`Command`]s to the engine
//! and renders the [`Snapshot`]s it gets back. This crate has no dependency on
//! the engine, so either side can be replaced without touching the other.

mod audio_clip;
mod automation;
mod clip_loop;
mod peaks;
mod time;

pub use audio_clip::{AudioClip, MAX_CLIP_BPM, MAX_CLIP_GAIN_DB, MIN_CLIP_BPM, MIN_CLIP_GAIN_DB};
pub use automation::{
    BlendMode, Block, BlockId, DEFAULT_TRIGGER_NOTE, DeviceId, DeviceRef, Effect, EffectKind,
    Instrument, InstrumentKind, Layer, LayerId, NodeId, NoteSet, NotesSource, ParamId, ParamState,
    SampleId, Scale, Segment, Shape, ShapePoint, Signal, SignalId, SignalSource,
};
pub use clip_loop::{ClipLoop, Piece, SEAM_FADE};
pub use peaks::{FINEST_SPAN, MinMax, PeakLevel, Peaks};
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
    /// Play a short 440 Hz tone (A4) at -12 dB through the output, over
    /// whatever is playing, to check the audio device.
    TestTone,
    /// Start (`on`) or end a note played straight away on a node's
    /// instruments, to hear it while editing. Not part of the song or undo.
    PreviewNote {
        node: NodeId,
        pitch: u8,
        velocity: u8,
        on: bool,
    },
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
        node: NodeId,
        name: String,
    },
    SetNodeColor {
        node: NodeId,
        color: u8,
    },
    /// Insert a node, with everything in it and under it, as `parent`'s
    /// child at `index` (0 is first). Every id inside must be unused.
    AddNode {
        id: NodeId,
        parent: NodeId,
        index: usize,
        node: Node,
    },
    /// Remove a node and everything under it. The Song cannot be removed.
    RemoveNode {
        id: NodeId,
    },
    /// Move a node (with everything under it) to be `parent`'s child at
    /// `index`, counted after it has been taken out. Not into itself.
    MoveNode {
        id: NodeId,
        parent: NodeId,
        index: usize,
    },
    /// Set the song tempo in beats per minute.
    SetTempo {
        bpm: f64,
    },
    AddClip {
        node: NodeId,
        id: ClipId,
        clip: MidiClip,
    },
    RemoveClip {
        id: ClipId,
    },
    /// Replace a clip, wherever it is, with `clip` on `node`: moving it in
    /// time or to another node, resizing it, or changing its notes.
    SetClip {
        id: ClipId,
        node: NodeId,
        clip: MidiClip,
    },
    /// Add an audio clip to an audio node. Removed with `RemoveClip`.
    AddAudioClip {
        node: NodeId,
        id: ClipId,
        clip: AudioClip,
    },
    /// Replace an audio clip, wherever it is, with `clip` on `node`:
    /// moving, trimming, or changing its settings.
    SetAudioClip {
        id: ClipId,
        node: NodeId,
        clip: AudioClip,
    },
    /// Silence the node and everything under it.
    SetMute {
        node: NodeId,
        muted: bool,
    },
    /// While any node is soloed, only instruments on soloed nodes, or under
    /// them, are heard.
    SetSolo {
        node: NodeId,
        soloed: bool,
    },
    /// Set a parameter's default state (normalized). This is what moving a
    /// knob does; it never records movement.
    SetParam {
        param: ParamId,
        value: f64,
    },
    /// Insert an effect into a node's chain at `index` (0 is first).
    AddEffect {
        node: NodeId,
        id: DeviceId,
        index: usize,
        effect: Effect,
    },
    /// Remove an effect, with its parameters and layers.
    RemoveEffect {
        id: DeviceId,
    },
    /// Add an instrument to a MIDI node's layers at `index` (0 is first).
    AddInstrument {
        node: NodeId,
        id: DeviceId,
        index: usize,
        instrument: Instrument,
    },
    /// Remove an instrument, with its parameters and layers.
    RemoveInstrument {
        id: DeviceId,
    },
    /// Switch an instrument or effect on or off. An effect switched off
    /// passes sound through untouched; an instrument switched off is silent.
    SetDeviceEnabled {
        id: DeviceId,
        enabled: bool,
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
    /// Move a layer to `index` in its parameter.s stack (0 is the bottom).
    MoveLayer {
        id: LayerId,
        index: usize,
    },
    SetLayerOptions {
        id: LayerId,
        mode: BlendMode,
        enabled: bool,
    },
    /// Set how much of a layer reaches the result, `0..=1`.
    SetLayerMix {
        id: LayerId,
        mix: f64,
    },
    /// Add an audio sample to the project's sample table. `file` is a path
    /// inside the project folder, with `/` between parts.
    AddSample {
        id: SampleId,
        sample: SampleRef,
    },
    /// Remove a sample from the table. Refused while an instrument uses it.
    RemoveSample {
        id: SampleId,
    },
    /// Choose the sample a SingleSampler plays, or none.
    SetInstrumentSample {
        id: DeviceId,
        sample: Option<SampleId>,
    },
    /// Choose the one note a SingleSampler plays.
    SetInstrumentNote {
        id: DeviceId,
        note: u8,
    },
    /// Make a layer listen to a signal, or draw its own blocks (`None`).
    /// The blocks are kept either way.
    SetLayerSignal {
        id: LayerId,
        signal: Option<SignalId>,
    },
    AddBlock {
        layer: LayerId,
        id: BlockId,
        block: Block,
    },
    RemoveBlock {
        id: BlockId,
    },
    /// Replace a block on a layer or a signal: move, resize, reshape or
    /// change its controls.
    SetBlock {
        id: BlockId,
        block: Block,
    },
    /// Add a signal. `listeners` are layers to make listen to it, so undoing
    /// a removal restores them; a front end sends none.
    AddSignal {
        id: SignalId,
        signal: Signal,
        listeners: Vec<LayerId>,
    },
    /// Remove a signal. Layers listening to it go back to drawing their
    /// blocks.
    RemoveSignal {
        id: SignalId,
    },
    RenameSignal {
        id: SignalId,
        name: String,
    },
    SetSignalColor {
        id: SignalId,
        color: u8,
    },
    /// Change a notes signal's filter and envelope.
    SetNotesSource {
        id: SignalId,
        source: NotesSource,
    },
    /// Add a block to a drawn signal's lane. Removed with `RemoveBlock`.
    AddSignalBlock {
        signal: SignalId,
        id: BlockId,
        block: Block,
    },
    /// Several edits as one step, applied in order: one undo reverts them
    /// all. If any is rejected, none take effect.
    Batch(Vec<Edit>),
}

/// What a node's clips are. Any node can hold other nodes (a folder is
/// just a node with children).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum NodeKind {
    /// MIDI clips, played through the node's instrument if it has one.
    #[default]
    Midi,
    /// Audio clips (coming with audio import). Never has an instrument.
    Audio,
}

/// A node and everything in it, as sent in [`Edit::AddNode`].
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
    /// Index into the nine-colour content palette.
    pub color: u8,
    pub muted: bool,
    pub soloed: bool,
    /// The instruments, layered: each plays every note. Only MIDI nodes
    /// have them.
    pub instruments: Vec<(DeviceId, Instrument)>,
    pub effects: Vec<(DeviceId, Effect)>,
    /// Volume and pan in index order. An empty list means the defaults.
    pub channel: Vec<ParamState>,
    pub clips: std::collections::BTreeMap<ClipId, MidiClip>,
    /// Audio clips, on audio nodes only. They share the clip ids.
    pub audio_clips: std::collections::BTreeMap<ClipId, AudioClip>,
    /// Children in order; their sound flows into this node.
    pub children: Vec<(NodeId, Node)>,
}

impl Node {
    /// A MIDI node with nothing in it, no instrument and default settings;
    /// see [`Node::with_instrument`] and [`Node::with_kind`].
    pub fn new(name: &str, color: u8) -> Self {
        Self {
            name: name.to_owned(),
            kind: NodeKind::Midi,
            color,
            muted: false,
            soloed: false,
            instruments: Vec::new(),
            effects: Vec::new(),
            channel: Vec::new(),
            clips: std::collections::BTreeMap::new(),
            audio_clips: std::collections::BTreeMap::new(),
            children: Vec::new(),
        }
    }

    /// The same node as another kind.
    pub fn with_kind(self, kind: NodeKind) -> Self {
        Self { kind, ..self }
    }

    /// The same node with a Synth (id `id`) added at its defaults.
    pub fn with_instrument(mut self, id: DeviceId) -> Self {
        self.instruments
            .push((id, Instrument::new(InstrumentKind::Synth)));
        self
    }
}

/// An audio file the project uses: its display name, and where it is inside
/// the project folder (`/` between parts, e.g. `audio/kick.wav`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SampleRef {
    pub name: String,
    pub file: String,
}

/// Whether a sample's audio could be read.
#[derive(Debug, Clone, PartialEq)]
pub enum SampleStatus {
    Loaded {
        seconds: f64,
        channels: u16,
        sample_rate: u32,
    },
    /// Missing or unreadable; instruments using it are silent.
    Missing { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SampleSnapshot {
    pub id: SampleId,
    pub name: String,
    pub file: String,
    pub status: SampleStatus,
    /// The waveform overview, once the audio has been read.
    pub peaks: Option<std::sync::Arc<Peaks>>,
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
    /// Notes positioned relative to the clip start, or in a looped clip to
    /// the cycle's start. Only the parts of notes inside `0..length` (in a
    /// looped clip, inside the cycle) play: see [`MidiClip::played_notes`].
    pub notes: Vec<MidiNote>,
    /// Repeats the content across the clip (`clip_loop.rs`); `None` plays
    /// it once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looping: Option<ClipLoop>,
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
    /// Every node in tree order: each parent before its children, the Song
    /// first.
    pub nodes: Vec<NodeSnapshot>,
    /// Every motion signal, in id order.
    pub signals: Vec<SignalSnapshot>,
    /// The sample table, in id order.
    pub samples: Vec<SampleSnapshot>,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeSnapshot {
    pub id: NodeId,
    pub kind: NodeKind,
    /// `None` for the Song.
    pub parent: Option<NodeId>,
    /// 0 for the Song, 1 for its children, and so on.
    pub depth: usize,
    pub children: Vec<NodeId>,
    pub name: String,
    /// Index into the nine-colour content palette.
    pub color: u8,
    pub muted: bool,
    pub soloed: bool,
    /// Clips in id order.
    pub clips: Vec<(ClipId, MidiClip)>,
    /// Audio clips in id order.
    pub audio_clips: Vec<(ClipId, AudioClip)>,
    /// Devices in signal order: instrument (if any), effects, channel.
    pub devices: Vec<DeviceSnapshot>,
}

impl NodeSnapshot {
    /// Whether MIDI clips can go on the node: a MIDI node other than the
    /// Song.
    pub fn takes_midi(&self) -> bool {
        self.kind == NodeKind::Midi && self.parent.is_some()
    }

    /// Whether the node has an instrument to play its notes.
    pub fn has_instrument(&self) -> bool {
        self.devices
            .iter()
            .any(|d| matches!(d.device, DeviceRef::Instrument(_)))
    }
}

impl Snapshot {
    /// A node by id.
    pub fn node(&self, id: NodeId) -> Option<&NodeSnapshot> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The Song: the root of the tree.
    pub fn root(&self) -> &NodeSnapshot {
        &self.nodes[0]
    }
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
    /// The kind, for an effect.
    pub effect: Option<EffectKind>,
    /// The kind, for an instrument.
    pub instrument: Option<InstrumentKind>,
    /// The sample, for a SingleSampler that has one.
    pub sample: Option<SampleId>,
    /// The notes an instrument plays (none for anything else).
    pub plays: NoteSet,
    /// A SingleSampler's trigger note.
    pub trigger: Option<u8>,
    /// False for an instrument or effect switched off.
    pub enabled: bool,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SignalSnapshot {
    pub id: SignalId,
    pub name: String,
    /// Index into the nine-colour content palette.
    pub color: u8,
    pub source: SignalSource,
    /// Every layer listening to this signal.
    pub used_by: Vec<SignalUse>,
}

/// A layer that listens to a signal, and its parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalUse {
    pub param: ParamId,
    pub layer: LayerId,
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
    TooManyInstruments {
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
    DuplicateSignal {
        id: SignalId,
    },
    UnknownSignal {
        id: SignalId,
    },
    InvalidSignal {
        id: SignalId,
        reason: InvalidSignal,
    },
    /// A new signal's listener already listens to another signal.
    AlreadyListening {
        layer: LayerId,
    },
    /// The edit needs a different kind of signal (drawn or notes).
    WrongSignalKind {
        id: SignalId,
    },
    DuplicateNode {
        id: NodeId,
    },
    UnknownNode {
        id: NodeId,
    },
    /// The Song cannot be removed or moved.
    RootNode,
    /// A node cannot move inside itself.
    NodeInsideItself {
        id: NodeId,
    },
    TooManyNodes {
        max: usize,
    },
    /// MIDI clips go on MIDI nodes, never an audio node or the Song.
    NotMidi {
        node: NodeId,
    },
    /// Audio clips go on audio nodes, never a MIDI node or the Song.
    NotAudio {
        node: NodeId,
    },
    /// Clips on a node never overlap (touching is fine): stacked MIDI
    /// clips hide each other's notes and tear apart when one is cut or
    /// moved, and a node plays one audio clip at a time.
    StackedClips {
        node: NodeId,
        clip: ClipId,
        over: ClipId,
    },
    /// Only MIDI nodes have instruments.
    InstrumentOnAudio {
        node: NodeId,
    },
    DuplicateSample {
        id: SampleId,
    },
    UnknownSample {
        id: SampleId,
    },
    /// A sample cannot be removed while an instrument or clip plays it.
    SampleInUse {
        id: SampleId,
    },
    /// Sample files are relative paths inside the project folder.
    InvalidSampleFile {
        file: String,
    },
    /// Only a SingleSampler plays a sample.
    NotASampler {
        id: DeviceId,
    },
    /// MIDI notes run 0 to 127.
    NoteOutOfRange {
        note: u8,
    },
    /// A branch is all MIDI or all Audio: a node goes only under a parent
    /// of its own kind (the Song takes either).
    MixedKinds {
        node: NodeId,
        parent: NodeId,
    },
    /// Colours index the nine-colour content palette.
    ColorOutOfRange {
        color: u8,
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
    /// An audio clip's offset is negative or not a number.
    Offset,
    /// An audio clip's tempo is outside its limits.
    Bpm,
    /// An audio clip's gain is outside its limits.
    Gain,
    /// An audio clip's fades are negative or longer than the clip.
    Fades,
    /// A loop's cycle is not longer than zero, or its phase is outside it.
    Loop,
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

/// Why a signal was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidSignal {
    /// Attack, decay and release cannot be negative.
    EnvelopeTime,
    /// Sustain must be within `0..=1`.
    EnvelopeLevel,
    /// The velocity clamp needs `0 <= min <= max <= 1`.
    VelocityRange,
    /// A notes filter needs `lowest <= highest <= 127`.
    PitchRange,
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
            Self::TooManyInstruments { max } => {
                write!(f, "a node holds at most {max} instruments")
            }
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
            Self::DuplicateSignal { id } => write!(f, "signal {} already exists", id.0),
            Self::UnknownSignal { id } => write!(f, "signal {} does not exist", id.0),
            Self::InvalidSignal { id, reason } => {
                write!(f, "invalid signal {}: {reason}", id.0)
            }
            Self::AlreadyListening { layer } => {
                write!(f, "layer {} already listens to a signal", layer.0)
            }
            Self::WrongSignalKind { id } => {
                write!(f, "signal {} is the wrong kind for this edit", id.0)
            }
            Self::DuplicateNode { id } => write!(f, "node {} already exists", id.0),
            Self::UnknownNode { id } => write!(f, "node {} does not exist", id.0),
            Self::RootNode => f.write_str("the Song cannot be removed or moved"),
            Self::NodeInsideItself { id } => {
                write!(f, "node {} cannot move inside itself", id.0)
            }
            Self::TooManyNodes { max } => write!(f, "a project holds at most {max} nodes"),
            Self::NotMidi { node } => write!(f, "node {} cannot hold MIDI clips", node.0),
            Self::NotAudio { node } => write!(f, "node {} cannot hold audio clips", node.0),
            Self::StackedClips { node, clip, over } => write!(
                f,
                "clips {} and {} overlap on node {}; clips on a node never overlap",
                clip.0, over.0, node.0
            ),
            Self::InstrumentOnAudio { node } => {
                write!(f, "node {} is an audio node and has no instrument", node.0)
            }
            Self::ColorOutOfRange { color } => write!(f, "colour {color} is not in the palette"),
            Self::EmptyName => f.write_str("names cannot be empty"),
            Self::DuplicateSample { id } => write!(f, "sample {} already exists", id.0),
            Self::UnknownSample { id } => write!(f, "sample {} does not exist", id.0),
            Self::SampleInUse { id } => {
                write!(
                    f,
                    "sample {} is still played by an instrument or clip",
                    id.0
                )
            }
            Self::InvalidSampleFile { file } => {
                write!(f, "{file:?} is not a path inside the project folder")
            }
            Self::NotASampler { id } => {
                write!(f, "device {} is not a SingleSampler", id.0)
            }
            Self::NoteOutOfRange { note } => write!(f, "note {note} is outside 0..=127"),
            Self::MixedKinds { node, parent } => write!(
                f,
                "node {} is not the same kind (MIDI or Audio) as node {}",
                node.0, parent.0
            ),
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
            Self::Offset => f.write_str("starts before its file's start"),
            Self::Bpm => write!(f, "tempo is outside {MIN_CLIP_BPM}..={MAX_CLIP_BPM} BPM"),
            Self::Gain => write!(
                f,
                "gain is outside {MIN_CLIP_GAIN_DB}..={MAX_CLIP_GAIN_DB} dB"
            ),
            Self::Fades => f.write_str("fades are negative or longer than the clip"),
            Self::Loop => f.write_str("loops with no cycle, or from outside its cycle"),
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

impl fmt::Display for InvalidSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnvelopeTime => f.write_str("envelope time is negative"),
            Self::EnvelopeLevel => f.write_str("sustain is outside 0..=1"),
            Self::VelocityRange => f.write_str("velocity clamp is not 0 <= min <= max <= 1"),
            Self::PitchRange => f.write_str("pitch range is empty or above 127"),
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
