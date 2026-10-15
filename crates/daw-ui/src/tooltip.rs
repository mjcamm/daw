//! Hover explanations: a small panel of text that appears after the pointer
//! rests on an element. Use with `.tooltip(tooltip::text("..."))` on an
//! element that has an id.

use crate::theme::Theme;
use gpui::{AnyView, App, Context, SharedString, Window, div, prelude::*, px};

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .max_w(px(260.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_md()
            .text_size(px(12.))
            .text_color(theme.text)
            .child(self.text.clone())
    }
}

/// Builds a tooltip showing `text`.
pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip { text: text.clone() }).into()
}
