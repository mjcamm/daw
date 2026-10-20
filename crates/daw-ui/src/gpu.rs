//! Asks for the faster graphics chip on computers that have two.
//!
//! Many laptops have a GPU built into the processor and a faster separate
//! one. Windows hands programs the built-in one unless they ask; GPUI takes
//! whichever comes first. On a full song the built-in GPU drew Arrange at
//! 11–15 ms a frame (worse spikes), the separate one at 6.9 ms: every frame
//! on time at 144 Hz (`docs/design/ui-speed.md`). The user chose speed over
//! battery life.
//!
//! The NVIDIA and AMD drivers look for these two values exported from the
//! program (`build.rs` exports them). A computer with one GPU ignores them,
//! so the app runs the same everywhere else.

/// NVIDIA Optimus: 1 asks for the separate GPU.
#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "a plain exported value the graphics driver reads"
)]
#[unsafe(no_mangle)]
#[used]
pub static NvOptimusEnablement: u32 = 1;

/// AMD switchable graphics: 1 asks for the separate GPU.
#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "a plain exported value the graphics driver reads"
)]
#[unsafe(no_mangle)]
#[used]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;
