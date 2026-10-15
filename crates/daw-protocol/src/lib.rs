//! The boundary between the engine and any front end.
//!
//! A front end never owns project state. It sends [`Command`]s to the engine
//! and renders the [`Snapshot`]s it gets back. This crate has no dependency on
//! the engine, so either side can be replaced without touching the other.

mod time;

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
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Set the song tempo in beats per minute.
    SetTempo {
        bpm: f64,
    },
    /// Add a MIDI clip. The id is chosen by the sender and must be unused.
    AddClip {
        id: ClipId,
        clip: MidiClip,
    },
    RemoveClip {
        id: ClipId,
    },
    /// Set the node volume in decibels. `-inf` mutes.
    SetVolume {
        db: f32,
    },
    /// Set the node pan, from -1 (left) to 1 (right).
    SetPan {
        pan: f32,
    },
}

/// Identifies a clip. Chosen by whoever creates the clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipId(pub u64);

/// A region of MIDI notes on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiClip {
    /// Song position of the clip start.
    pub start: Ticks,
    pub length: Ticks,
    /// Notes positioned relative to the clip start. Only the parts of notes
    /// inside `0..length` play.
    pub notes: Vec<MidiNote>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopRange {
    pub start: Ticks,
    pub end: Ticks,
}

/// A read-only view of engine state for a front end to render.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Increases every time a command changes engine state.
    pub revision: u64,
    pub tempo_bpm: f64,
    pub playing: bool,
    /// Transport position in beats, as last reported by the audio side.
    pub position_beats: f64,
    pub loop_range: Option<LoopRange>,
    pub volume_db: f32,
    pub pan: f32,
    /// Clips in id order.
    pub clips: Vec<ClipSummary>,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipSummary {
    pub id: ClipId,
    pub start: Ticks,
    pub length: Ticks,
    pub note_count: usize,
}

/// Why the engine rejected a command. A rejected command changes nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandError {
    TempoOutOfRange { bpm: f64, min: f64, max: f64 },
    VolumeOutOfRange { db: f32, max: f32 },
    PanOutOfRange { pan: f32 },
    NegativePosition { position: Ticks },
    InvalidLoop { range: LoopRange, min_length: Ticks },
    DuplicateClip { id: ClipId },
    UnknownClip { id: ClipId },
    InvalidClip { reason: InvalidClip },
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

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TempoOutOfRange { bpm, min, max } => {
                write!(f, "tempo {bpm} BPM is outside {min}..={max} BPM")
            }
            Self::VolumeOutOfRange { db, max } => {
                write!(f, "volume {db} dB is not a number at most {max} dB")
            }
            Self::PanOutOfRange { pan } => write!(f, "pan {pan} is outside -1..=1"),
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

impl std::error::Error for CommandError {}
