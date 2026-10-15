//! Standalone DSP modules.
//!
//! Everything here is plain, portable signal processing with no knowledge of
//! the engine, so modules can later be packaged as standalone plugins. Code in
//! this crate runs on the audio thread: it must not allocate, lock or block.

pub mod gain;
