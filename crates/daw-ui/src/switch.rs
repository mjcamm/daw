//! The app's on/off switch: a pill track with a round knob, lit when on.
//! Used wherever one setting switches (the inspector's sections, the
//! metronome's accent), so they all look the same.

use crate::theme::Theme;
use gpui::{Div, ElementId, Stateful, div, prelude::*, px};

/// A switch showing `on`. Attach `on_click` to flip what it shows.
pub fn switch(id: impl Into<ElementId>, on: bool, theme: &Theme) -> Stateful<Div> {
    let track = if on { theme.active } else { theme.control };
    let knob = div()
        .absolute()
        .top(px(2.))
        .left(px(if on { 13. } else { 2. }))
        .size(px(11.))
        .rounded(px(5.5))
        .bg(theme.on_accent)
        .shadow_sm();
    div()
        .id(id)
        .relative()
        .w(px(26.))
        .h(px(15.))
        .flex_none()
        .rounded(px(7.5))
        .bg(track)
        .cursor_pointer()
        .child(knob)
}
