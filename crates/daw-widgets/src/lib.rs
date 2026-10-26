//! The look and feel shared by the DAW and the performance app: theme tokens
//! (`docs/design/look-and-feel.md`), widgets, painting helpers and icons.
//! Views never hard-code a colour; they read one from [`theme::Theme`].

pub mod frame_log;
pub mod icons;
pub mod paint;
pub mod scroll;
pub mod slider;
pub mod switch;
pub mod text_input;
pub mod theme;
pub mod tooltip;
