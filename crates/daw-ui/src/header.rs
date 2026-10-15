//! The header bar: song details, page tabs, then the transport.

use crate::block_edit::Grid;
use crate::icons::{Icon, icon};
use crate::layout::format_position;
use crate::menu::{self, OpenMenu};
use crate::project::{Page, ProjectView, RenameTarget};
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{Command, Ticks};
use gpui::{ClickEvent, Context, FontWeight, Hsla, SharedString, div, prelude::*, px};

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
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .child(snapshot.song_name.clone())
                .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(RenameTarget::Song, window, cx);
                    } else {
                        this.open_menu = match this.open_menu {
                            Some(OpenMenu::Song) => None,
                            _ => Some(OpenMenu::Song),
                        };
                    }
                }))
                .into_any_element(),
        };

        let song = div()
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
            .child(self.render_snap(theme, cx))
            .child(self.render_transport(theme, cx))
    }

    fn render_tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Page::ALL.map(|page| {
            let selected = page == self.page;
            // Parameters belong to a node, so that page needs one selected.
            let available = page != Page::Parameters || self.node_selected;
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
    /// Snap or slide for block edits: a chip that opens a small panel.
    fn render_snap(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap;
        let open = self.open_menu == Some(OpenMenu::Snap);
        let hover = theme.hover;
        let chip = div()
            .id("snap")
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
            .on_click(cx.listener(|this, _, _, _| {
                this.open_menu = match this.open_menu {
                    Some(OpenMenu::Snap) => None,
                    _ => Some(OpenMenu::Snap),
                };
            }));
        div()
            .flex()
            .flex_col()
            .child(chip)
            .when(open, |d| d.child(self.render_snap_panel(theme, cx)))
    }

    fn render_snap_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap;
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
                segment("snap-on", "Snap", snap.enabled)
                    .on_click(cx.listener(|this, _, _, _| this.snap.enabled = true)),
            )
            .child(
                segment("snap-off", "Slide", !snap.enabled)
                    .on_click(cx.listener(|this, _, _, _| this.snap.enabled = false)),
            );
        let grids = Grid::ALL.map(|grid| {
            let current = snap.enabled && snap.grid == grid;
            menu::item(
                theme,
                ("snap-grid", grid as usize),
                grid.label(),
                if current { "✓" } else { "" },
            )
            .when(!snap.enabled, |d| d.text_color(theme.text_tertiary))
            .on_click(cx.listener(move |this, _, _, _| {
                // Picking a grid while sliding switches snapping back on.
                this.snap.enabled = true;
                this.snap.grid = grid;
                this.open_menu = None;
            }))
        });
        menu::float(
            menu::panel(theme)
                .min_w(px(160.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.open_menu = None))
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
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.open_menu = None))
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
