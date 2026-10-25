//! Evaluating parameter layer stacks and the signals they listen to.
//!
//! Pure functions, no engine knowledge: given a parameter's default state and
//! its layers, what is its value at a song position? The audio thread runs
//! [`Automation::value_at`] on compiled stacks; it never allocates.
//!
//! Positions here are in beats. All values are normalized (`0..=1`).

mod shape;
mod signal;
mod stack;
mod validate;

pub use daw_protocol::curve_value;
pub use shape::shape_value;
pub use signal::{CompiledSignal, GLIDE_BEATS, Live, NoLive, SignalNote, Signals};
pub use stack::{Automation, CompiledBlock, CompiledLayer, blend};
pub use validate::{validate_audio_source, validate_block, validate_curve, validate_notes_source};
