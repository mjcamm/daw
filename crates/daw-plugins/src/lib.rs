//! Plugins in their own processes (`docs/design/vst3-hosting.md`).
//!
//! - [`block`]: the per-instance shared-memory audio channel and its hand-off.
//! - [`control`]: the control channel's messages (stdin and stdout, JSON).
//! - [`client`]: the DAW side: starting a host process and calling it.
//! - [`host`]: the host process side.
//! - [`scan`]: finding VST3 plugins and caching what they offer.
//! - [`vst3`]: loading a VST3 module (scanner and host processes only).
//! - [`sys`]: the operating system pieces (Windows only for now).
//!
//! No plugin code ever runs in the DAW process: the DAW's audio thread only
//! copies into shared memory, signals, and waits until a deadline.

pub mod block;
pub mod client;
pub mod control;
pub mod host;
pub mod scan;
pub mod sys;
pub mod vst3;

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;
