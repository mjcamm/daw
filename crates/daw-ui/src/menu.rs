//! Popup menus: a quiet floating panel of rows, drawn above everything else.

use crate::theme::Theme;
use gpui::{
    Div, ElementId, Hsla, IntoElement, Stateful, anchored, deferred, div, point, prelude::*, px,
    relative,
};

/// Which popup menu is open; only one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMenu {
    /// Under the song name: save, rename, back to projects.
    Song,
    /// Choosing a parameter to give layers, for a device.
    AddParam(daw_engine::DeviceRef),
    /// Adding a layer to a parameter: its type, then (to listen) a signal.
    AddLayer(daw_engine::ParamId),
    /// A layer's blend mode.
    BlendMode(daw_engine::LayerId),
    /// Snap or slide, and the snap grid: Arrange's (`notes: false`) or the
    /// piano roll's.
    Snap { notes: bool },
    /// A signal's colour swatches.
    SignalColor(daw_engine::SignalId),
    /// The layers listening to a signal.
    SignalUses(daw_engine::SignalId),
    /// Adding a node: an instrument or a folder.
    AddNode,
    /// A SingleSampler's panel: its sample, and choosing another.
    Sampler(daw_engine::DeviceId),
    /// Publishing a node as a signal, and the signals it publishes.
    NodeSignals(daw_engine::NodeId),
}

/// The floating panel. Place it where the menu should appear; close it with
/// `on_mouse_down_out` on the returned element's panel.
pub fn panel(theme: &Theme) -> Div {
    div()
        .occlude()
        .min_w(px(180.))
        .p(px(4.))
        .rounded(px(9.))
        .bg(theme.group)
        .border_1()
        .border_color(theme.edge)
        .shadow_lg()
        .text_color(theme.text)
        .text_size(px(13.))
}

/// Floats `panel` just below the element that opens it. Place it as a
/// child of that element (or of a container holding it), which must be
/// `.relative()`: the panel hangs from its bottom-left corner. (An
/// `anchored` element on its own is laid out like any absolute element, at
/// the top of its parent, so it would cover the trigger.)
pub fn float(panel: Div) -> impl IntoElement {
    div()
        .absolute()
        .top(relative(1.))
        .left_0()
        .child(deferred(anchored().offset(point(px(0.), px(4.))).child(panel)).with_priority(1))
}

/// A menu row with a label and an optional hint on the right.
pub fn item(
    theme: &Theme,
    id: impl Into<ElementId>,
    label: impl Into<gpui::SharedString>,
    hint: impl Into<gpui::SharedString>,
) -> Stateful<Div> {
    let hover: Hsla = theme.hover;
    div()
        .id(id)
        .flex()
        .justify_between()
        .gap(px(24.))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(5.))
        .hover(move |s| s.bg(hover))
        .child(label.into())
        .child(div().text_color(theme.text_tertiary).child(hint.into()))
}

/// A thin line between groups of rows.
pub fn separator(theme: &Theme) -> Div {
    div().my(px(4.)).h(px(1.)).bg(theme.hairline)
}
