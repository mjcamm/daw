//! The boundary between the engine and any front end.
//!
//! A front end never owns project state. It sends [`Command`]s to the engine
//! and renders the [`Snapshot`]s it gets back. This crate has no dependency on
//! the engine, so either side can be replaced without touching the other.

use std::fmt;

/// A request from a front end to the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start the transport.
    Play,
    /// Stop the transport, keeping the current position.
    Stop,
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
    SetTempo { bpm: f64 },
}

/// A read-only view of engine state for a front end to render.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Increases every time a command changes engine state.
    pub revision: u64,
    pub tempo_bpm: f64,
    pub playing: bool,
    /// Transport position in sample frames, as last reported by the audio side.
    pub position_frames: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

/// Why the engine rejected a command. A rejected command changes nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandError {
    TempoOutOfRange { bpm: f64, min: f64, max: f64 },
    NothingToUndo,
    NothingToRedo,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TempoOutOfRange { bpm, min, max } => {
                write!(f, "tempo {bpm} BPM is outside {min}..={max} BPM")
            }
            Self::NothingToUndo => f.write_str("nothing to undo"),
            Self::NothingToRedo => f.write_str("nothing to redo"),
        }
    }
}

impl std::error::Error for CommandError {}
