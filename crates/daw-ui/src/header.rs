//! The header bar: song details, page tabs, then the transport.

use crate::block_edit::{Grid, Snap};
use crate::icons::{Icon, icon};
use crate::layout::format_position;
use crate::menu::{self, OpenMenu};
use crate::project::{Page, ProjectView, RenameTarget};
use crate::theme::{MONO_FONT, Theme};
use crate::tooltip;
use daw_engine::{Command, Ticks};
use gpui::{
    Context, FontWeight, Hsla, MouseButton, MouseDownEvent, SharedString, div, prelude::*, px,
};

impl ProjectView {
    pub(crate) fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let details = format!("{:.0} BPM · 4/4", snapshot.tempo_bpm);

        let status = if self.session.is_edited() {
            "Edited"
        } else {
            "Saved"
        };
        let name: gpui::AnyElement = match self.rename_input(RenameTarget::Song) {
            Some(input) => div()
                .w(px(180.))
                .text_size(px(13.))
                .child(input)
                .into_any_element(),
            None => div()
                .id("song-name")
                .debug_selector(|| "song-name".into())
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .child(snapshot.song_name.clone())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        if event.click_count >= 2 {
                            this.start_rename(RenameTarget::Song, window, cx);
                        } else {
                            this.toggle_menu(OpenMenu::Song);
                        }
                    }),
                )
                .into_any_element(),
        };

        let song = div()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(140.))
            .child(name)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(format!("{details} · {status}")),
            )
            .when(self.open_menu == Some(OpenMenu::Song), |d| {
                d.child(self.render_song_menu(theme, cx))
            });

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
            .when(self.page == crate::project::Page::Arrange, |d| {
                d.child(self.render_arrange_tools(theme, cx))
            })
            .child(self.render_snap(false, theme, cx))
            .child(self.render_transport(theme, cx))
            .children(crate::frame_log::measuring().then(|| render_frame_time(theme)))
            .child(self.render_load(theme))
    }

    /// How hard the audio thread works: the share of each buffer's time
    /// spent rendering. Amber past 70%, red after a dropout.
    fn render_load(&self, theme: &Theme) -> impl IntoElement {
        let (load, dropout) = self.session.load();
        let percent = (load * 100.).round().min(999.) as u32;
        let color = if dropout {
            theme.record
        } else if load >= 0.7 {
            theme.warning
        } else {
            theme.text_secondary
        };
        div()
            .id("audio-load")
            .w(px(58.))
            .flex()
            .flex_col()
            .items_end()
            .text_size(px(10.))
            .child(div().text_color(theme.text_tertiary).child("CPU"))
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(px(12.))
                    .text_color(color)
                    .child(format!("{percent}%")),
            )
            .tooltip(tooltip::text(if dropout {
                "Audio load: a dropout just happened. A larger buffer size (File > Preferences > Audio) gives more room."
            } else {
                "Audio load: how much of each buffer's time rendering takes"
            }))
    }

    fn render_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Page::ALL.map(|page| {
            let selected = page == self.page;
            // Parameters belong to a node, so that page needs one selected.
            let available = page != Page::Parameters || self.shown_node().is_some();
            div()
                .id(SharedString::from(page.label()))
                .px(px(12.))
                .py(px(3.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(theme.control_selected))
                .text_color(if selected {
                    theme.text
                } else if available {
                    theme.text_secondary
                } else {
                    theme.text_tertiary
                })
                .when(available, |d| {
                    d.on_click(cx.listener(move |this, _, _, _| this.page = page))
                })
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

impl ProjectView {
    fn snap_setting(&mut self, notes: bool) -> &mut Snap {
        if notes {
            &mut self.note_grid
        } else {
            &mut self.snap
        }
    }

    /// Snap or slide and the grid: a chip that opens a small panel. Arrange
    /// has one in the header, the piano roll its own (`notes`).
    pub(crate) fn render_snap(
        &self,
        notes: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snap = if notes { self.note_grid } else { self.snap };
        let menu = OpenMenu::Snap { notes };
        let open = self.open_menu == Some(menu);
        let hover = theme.hover;
        let chip = div()
            .id(if notes { "note-snap" } else { "snap" })
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(26.))
            .pl(px(8.))
            .pr(px(6.))
            .rounded(px(6.))
            .bg(theme.control)
            .hover(move |s| s.bg(hover))
            .child(icon(
                Icon::Magnet,
                if snap.enabled {
                    theme.accent
                } else {
                    theme.text_tertiary
                },
                13.,
            ))
            .child(
                div()
                    .min_w(px(34.))
                    .text_color(theme.text_secondary)
                    .child(snap.label()),
            )
            .child(icon(Icon::ChevronDown, theme.text_tertiary, 11.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, _| this.toggle_menu(menu)),
            );
        div()
            .relative()
            .flex()
            .flex_col()
            .child(chip)
            .when(open, |d| {
                d.child(self.render_snap_panel(notes, snap, theme, cx))
            })
    }

    fn render_snap_panel(
        &self,
        notes: bool,
        snap: Snap,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let segment = |id: &'static str, label: &'static str, selected: bool| {
            div()
                .id(id)
                .flex_1()
                .flex()
                .justify_center()
                .py(px(2.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(theme.control_selected))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                })
                .child(label)
        };
        let mode = div()
            .flex()
            .m(px(4.))
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(theme.control)
            .text_size(px(12.))
            .child(
                segment("snap-on", "Snap", snap.enabled).on_click(cx.listener(
                    move |this, _, _, _| {
                        this.snap_setting(notes).enabled = true;
                    },
                )),
            )
            .child(
                segment("snap-off", "Slide", !snap.enabled).on_click(cx.listener(
                    move |this, _, _, _| {
                        this.snap_setting(notes).enabled = false;
                    },
                )),
            );
        let grids = Grid::ALL.map(|grid| {
            let current = snap.enabled && snap.grid == grid;
            menu::item(
                theme,
                (
                    if notes { "note-snap-grid" } else { "snap-grid" },
                    grid as usize,
                ),
                grid.label(),
                if current { "✓" } else { "" },
            )
            .when(!snap.enabled, |d| d.text_color(theme.text_tertiary))
            .on_click(cx.listener(move |this, _, _, _| {
                // Picking a grid while sliding switches snapping back on.
                *this.snap_setting(notes) = Snap {
                    enabled: true,
                    grid,
                };
                this.open_menu = None;
            }))
        });
        menu::float(
            menu::panel(theme)
                .min_w(px(160.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .child(mode)
                .child(menu::separator(theme))
                .children(grids),
        )
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

/// The save shortcut as the platform writes it.
const SAVE_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "⌘S"
} else {
    "Ctrl+S"
};

impl ProjectView {
    /// The menu under the song name: save, rename, back to projects.
    fn render_song_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        menu::float(
            menu::panel(theme)
                .min_w(px(220.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .child(
                    menu::item(theme, "menu-save", "Save", SAVE_SHORTCUT)
                        .on_click(cx.listener(|this, _, _, _| this.save())),
                )
                .child(
                    menu::item(theme, "menu-rename", "Rename", "Double-click").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.start_rename(RenameTarget::Song, window, cx);
                        }),
                    ),
                )
                .child(menu::separator(theme))
                .child(
                    menu::item(theme, "menu-close", "Back to projects", "")
                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                ),
        )
    }
}

/// While measuring (`DAW_FRAME_LOG`), the UI's CPU work per frame beside
/// the audio load: the median over the last 2 s.
fn render_frame_time(theme: &Theme) -> impl IntoElement {
    let latest = crate::frame_log::latest();
    let text = latest.map_or_else(|| "–".to_owned(), |(work, _)| format!("{work:.1} ms"));
    let hint = latest.map_or_else(
        || "UI frame time: measuring…".to_owned(),
        |(work, fps)| {
            format!(
                "UI frame time: {work:.1} ms of work per frame, {fps:.0} frames a second (median over 2 s). Shown while DAW_FRAME_LOG is set."
            )
        },
    );
    div()
        .id("frame-time")
        .w(px(58.))
        .flex()
        .flex_col()
        .items_end()
        .text_size(px(10.))
        .child(div().text_color(theme.text_tertiary).child("UI"))
        .child(
            div()
                .font_family(MONO_FONT)
                .text_size(px(12.))
                .text_color(theme.text_secondary)
                .child(text),
        )
        .tooltip(tooltip::text(hint))
}
