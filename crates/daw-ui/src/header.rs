//! The header bar: song details, page tabs, then the transport.

use crate::app::{AppView, Page};
use crate::icons::{Icon, icon};
use crate::layout::format_position;
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{Command, Ticks};
use gpui::{Context, FontWeight, Hsla, SharedString, div, prelude::*, px};

impl AppView {
    pub(crate) fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let details = format!("{:.0} BPM · 4/4", snapshot.tempo_bpm);

        let song = div()
            .flex()
            .flex_col()
            .min_w(px(140.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(snapshot.song_name.clone()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(details),
            );

        div()
            .h(px(52.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(20.))
            .px(px(18.))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.edge)
            .child(song)
            .child(self.render_tabs(theme, cx))
            .child(div().flex_1())
            .child(self.render_transport(theme, cx))
    }

    fn render_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Page::ALL.map(|page| {
            let selected = page == self.page;
            div()
                .id(SharedString::from(page.label()))
                .px(px(12.))
                .py(px(3.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(theme.control_selected))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                })
                .on_click(cx.listener(move |this, _, _, _| this.page = page))
                .child(page.label())
        });
        div()
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(theme.control)
            .children(tabs)
    }

    fn render_transport(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let playing = snapshot.playing;
        let looping = snapshot.loop_range.is_some();

        let to_start = transport_button("to-start", theme, false)
            .on_click(cx.listener(|this, _, _, _| {
                this.session.apply(Command::Locate {
                    position: Ticks::ZERO,
                });
            }))
            .child(icon(Icon::ToStart, theme.text_secondary, 12.));
        let play = transport_button("play", theme, false)
            .on_click(cx.listener(|this, _, _, _| this.toggle_play()))
            .child(icon(
                if playing { Icon::Stop } else { Icon::Play },
                theme.accent,
                14.,
            ));
        let loop_button = transport_button("loop", theme, looping)
            .on_click(cx.listener(|this, _, _, _| this.toggle_loop()))
            .child(icon(
                Icon::Loop,
                if looping {
                    theme.accent
                } else {
                    theme.text_secondary
                },
                14.,
            ));

        let position = div()
            .px(px(10.))
            .py(px(3.))
            .rounded(px(6.))
            .bg(theme.control)
            .font_family(MONO_FONT)
            .text_size(px(15.))
            .min_w(px(120.))
            .flex()
            .justify_center()
            .child(format_position(self.session.position_beats()));

        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(8.))
                    .bg(theme.control)
                    .child(to_start)
                    .child(play)
                    .child(loop_button),
            )
            .child(position)
    }
}

fn transport_button(id: &'static str, theme: &Theme, active: bool) -> gpui::Stateful<gpui::Div> {
    let hover: Hsla = theme.hover;
    let mut active_bg = theme.accent;
    active_bg.a = 0.18;
    div()
        .id(id)
        .w(px(32.))
        .h(px(26.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .when(active, |d| d.bg(active_bg))
        .hover(move |s| s.bg(hover))
}
