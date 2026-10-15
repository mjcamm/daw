//! Evaluating parameter layer stacks and the motion signals they link to.
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

pub use shape::shape_value;
pub use signal::{CompiledSignal, Signals};
pub use stack::{Automation, CompiledBlock, CompiledLayer, CompiledLink, blend};
pub use validate::{validate_block, validate_notes_source};
