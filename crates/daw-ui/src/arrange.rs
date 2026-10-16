//! The Arrange page: the node list on the left, the timeline beside it,
//! framed by scrollbars and zoom sliders (see `frame.rs`).

use crate::frame::{Timeline, grid_lines};
use crate::icons::{Icon, icon};
use crate::layout::{
    BEATS_PER_BAR, meter_height, note_rows, ruler_label_step, timeline_bars, x_to_snapped_position,
};
use crate::menu::{self, OpenMenu};
use crate::parameters::small_button;
use crate::project::{ProjectView, RenameTarget, song_clips};
use crate::scroll;
use crate::signals::{node_has_notes, published_signals};
use crate::theme::{ContentColor, Theme};
use crate::tooltip;
use crate::zoom::Zoom;
use daw_engine::{ClipId, Command, Edit, MidiClip, NodeSnapshot, Snapshot};
use gpui::{
    ClickEvent, Context, FontWeight, Hsla, MouseButton, MouseDownEvent, canvas, div, prelude::*, px,
};

const RULER_HEIGHT: f32 = 28.;
const NODE_LIST_WIDTH: f32 = 260.;
const CLIP_INSET: f32 = 4.;
const CLIP_LABEL_HEIGHT: f32 = 16.;
const METER_HEIGHT: f32 = 32.;

impl ProjectView {
    pub(crate) fn render_arrange(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let bars = timeline_bars(song_clips(snapshot), snapshot.loop_range);
        let width = zoom.beat_to_x(f64::from(bars) * BEATS_PER_BAR);

        // The ruler stays at the top and follows the lanes' sideways scroll.
        let scroll_x = self.arrange_view.x.offset().x;
        let top = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .flex()
            .child(
                div()
                    .w(px(NODE_LIST_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .bg(theme.sidebar)
                    .border_r_1()
                    .border_b_1()
                    .border_color(theme.edge)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("NODES"),
            )
            .child(
                div().flex_1().min_w_0().overflow_hidden().child(
                    div()
                        .relative()
                        .left(scroll_x)
                        .w(px(width))
                        .child(self.render_ruler(theme, bars, zoom.pixels_per_beat, cx)),
                ),
            )
            .child(
                div()
                    .w(px(scroll::THICKNESS))
                    .flex_none()
                    .bg(theme.sidebar)
                    .border_l_1()
                    .border_b_1()
                    .border_color(theme.hairline),
            );

        let body = scroll::axis_locked(div())
            .id("arrange-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.arrange_view.y)
            .flex()
            .flex_col()
            .child(
                // Grows to fill the page (flex growth: percentage heights do
                // not work inside a vertical scroll).
                div()
                    .flex()
                    .flex_grow()
                    .flex_shrink_0()
                    .child(self.render_node_list(theme, cx))
                    .child(self.render_timeline(theme, bars, width)),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top)
            .child(self.timeline_frame(
                Timeline::Arrange,
                body.into_any_element(),
                NODE_LIST_WIDTH,
                theme,
                cx,
            ))
    }

    fn render_node_list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        div()
            .w(px(NODE_LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children(
                arranged_nodes(snapshot)
                    .map(|node| self.node_row(node, theme, cx))
                    .collect::<Vec<_>>(),
            )
            .child(
                // Clicking the empty space below the nodes clears the selection.
                div()
                    .id("node-list-space")
                    .flex_1()
                    .on_click(cx.listener(|this, _, _, _| this.selected_node = None)),
            )
    }

    /// A node: colour, name and kind, meter, signal button, mute and solo.
    fn node_row(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = node.id;
        let meter = self.session.meter(id);
        let color = theme.content(node.color);
        let row_height = self.arrange_view.zoom.row_height;
        let meter_size = (row_height - 16.).clamp(10., METER_HEIGHT);

        let toggle =
            |name: &'static str, label: &'static str, on: bool, on_bg: Hsla, on_fg: Hsla| {
                div()
                    .id((name, id.0))
                    .w(px(20.))
                    .h(px(18.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(if on { on_bg } else { theme.control })
                    .text_color(if on { on_fg } else { theme.text_secondary })
                    .child(label)
            };
        let (muted, soloed) = (node.muted, node.soloed);
        let mute = toggle("mute", "M", muted, theme.warning, theme.on_warning).on_click(
            cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::SetMute {
                    node: id,
                    muted: !muted,
                }));
            }),
        );
        let solo = toggle("solo", "S", soloed, theme.accent, theme.on_accent).on_click(
            cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::SetSolo {
                    node: id,
                    soloed: !soloed,
                }));
            }),
        );

        let publish = self.publish_button(node, theme, cx);
        let node_name: gpui::AnyElement = match self.rename_input(RenameTarget::Node(id)) {
            Some(input) => div().text_size(px(13.)).child(input).into_any_element(),
            None => div()
                .id(("node-name", id.0))
                .text_size(px(13.))
                .child(node.name.clone())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(RenameTarget::Node(id), window, cx);
                    }
                }))
                .into_any_element(),
        };
        let kind = if node.has_instrument() {
            "MIDI · Synth"
        } else {
            "Folder"
        };

        let mut selected_bg = theme.accent;
        selected_bg.a = 0.16;
        div()
            .id(("node-row", id.0))
            .h(px(row_height))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .border_b_1()
            .border_color(theme.hairline)
            .when(self.selected_node == Some(id), |d| d.bg(selected_bg))
            .on_click(cx.listener(move |this, _, _, _| this.selected_node = Some(id)))
            .child(
                div()
                    .w(px(4.))
                    .h(px(meter_size))
                    .rounded(px(2.))
                    .bg(color.detail),
            )
            .child(
                div().flex_1().flex().flex_col().child(node_name).child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_secondary)
                        .child(kind),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(meter_bar(meter.left, meter_size, theme))
                    .child(meter_bar(meter.right, meter_size, theme)),
            )
            .child(publish)
            .child(mute)
            .child(solo)
    }

    /// The node's signal button: publish as a signal, and the signals it
    /// publishes, each with a go-to icon. Tinted in the first published
    /// signal's colour once it publishes any.
    fn publish_button(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let snapshot = self.session.snapshot();
        let id = node.id;
        let color = published_signals(snapshot, id)
            .next()
            .map_or(theme.text_secondary, |s| theme.content(s.color).detail);
        let open = self.open_menu == Some(OpenMenu::NodeSignals(id));
        div()
            .relative()
            .flex()
            .flex_col()
            .child(
                small_button(("node-signals", id.0), theme)
                    .w(px(20.))
                    .h(px(18.))
                    .when(open, |d| d.bg(theme.control))
                    .child(icon(Icon::Signal, color, 13.))
                    // Hidden while the menu is open, so it never covers it.
                    .when(!open, |d| d.tooltip(tooltip::text("Publish as signal")))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.toggle_menu(OpenMenu::NodeSignals(id));
                    })),
            )
            .when(open, |d| d.child(self.publish_menu(node, theme, cx)))
    }

    /// Publishing stays on Arrange with the menu open, so the new signal
    /// appears in it with its go-to icon.
    fn publish_menu(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let node_id = node.id;
        let unavailable = |id: &'static str, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .flex()
                .justify_between()
                .gap(px(24.))
                .px(px(10.))
                .py(px(5.))
                .text_color(theme.text_tertiary)
                .child(label)
                .child(hint)
        };
        let notes = if node_has_notes(node) {
            menu::item(theme, "publish-notes", "Publish notes as motion signal", "")
                .on_click(cx.listener(move |this, _, _, _| this.publish_notes(node_id)))
        } else {
            unavailable(
                "publish-notes",
                "Publish notes as motion signal",
                "no notes",
            )
        };
        let hover = theme.hover;
        let published: Vec<_> = published_signals(snapshot, node_id)
            .map(|signal| {
                let id = signal.id;
                div()
                    .id(("published-signal", id.0))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded(px(4.))
                            .bg(theme.content(signal.color).detail),
                    )
                    .child(div().flex_1().child(signal.name.clone()))
                    .child(icon(Icon::GoTo, theme.text_secondary, 12.))
                    .tooltip(tooltip::text("Go to this signal"))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.open_menu = None;
                        this.go_to_signal(id);
                    }))
            })
            .collect();
        menu::float(
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .child(notes)
                .child(unavailable(
                    "publish-sound-motion",
                    "Publish sound as motion signal",
                    "coming later",
                ))
                .child(unavailable(
                    "publish-sound-audio",
                    "Publish sound as audio signal",
                    "coming later",
                ))
                .when(!published.is_empty(), |d| {
                    d.child(menu::separator(theme)).children(published)
                }),
        )
    }

    /// The lanes: sideways-scrolling, as wide as the song.
    fn render_timeline(&self, theme: &Theme, bars: u32, width: f32) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let lanes = arranged_nodes(snapshot).map(|node| {
            let color = if node.muted {
                theme.muted_content
            } else {
                theme.content(node.color)
            };
            div()
                .relative()
                .h(px(zoom.row_height))
                .flex_none()
                .border_b_1()
                .border_color(theme.hairline)
                .children(
                    node.clips
                        .iter()
                        .map(|(id, clip)| clip_view(*id, clip, &node.name, color, zoom)),
                )
        });

        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(zoom.beat_to_x(self.session.position_beats())))
            .w(px(1.))
            .bg(theme.accent);

        scroll::axis_locked(div())
            .id("timeline")
            .flex_1()
            .min_w_0()
            // A row, so the content stretches to the full height.
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.arrange_view.x)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .children(grid_lines(zoom, bars, theme))
                    .children(lanes)
                    .child(playhead),
            )
    }

    pub(crate) fn render_ruler(
        &self,
        theme: &Theme,
        bars: u32,
        pixels_per_beat: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bounds_cell = self.ruler_bounds.clone();
        let mut loop_fill = theme.accent;
        loop_fill.a = 0.2;

        let loop_band = self.session.snapshot().loop_range.map(|range| {
            let start = range.start.as_beats() as f32 * pixels_per_beat;
            let end = range.end.as_beats() as f32 * pixels_per_beat;
            div()
                .absolute()
                .top(px(2.))
                .h(px(6.))
                .left(px(start))
                .w(px(end - start))
                .rounded(px(2.))
                .bg(loop_fill)
        });

        let step = ruler_label_step(pixels_per_beat);
        let labels = (0..bars).step_by(step as usize).map(|bar| {
            div()
                .absolute()
                .left(px(bar as f32 * BEATS_PER_BAR as f32 * pixels_per_beat + 4.))
                .bottom(px(4.))
                .text_size(px(10.))
                .text_color(theme.text_tertiary)
                .child((bar + 1).to_string())
        });

        div()
            .relative()
            .h(px(RULER_HEIGHT))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .cursor_pointer()
            .child(
                // Records where the ruler is on screen, to map clicks to beats.
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(loop_band)
            .children(labels)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    let origin = this.ruler_bounds.get().origin.x;
                    let x = (event.position.x - origin) / px(1.);
                    this.session.apply(Command::Locate {
                        position: x_to_snapped_position(x, pixels_per_beat),
                    });
                }),
            )
    }
}

/// The nodes shown as rows, in tree order. The Song gets its own row with
/// the nesting map.
fn arranged_nodes(snapshot: &Snapshot) -> impl Iterator<Item = &NodeSnapshot> {
    snapshot.nodes.iter().filter(|n| n.parent.is_some())
}

fn meter_bar(peak: f32, size: f32, theme: &Theme) -> impl IntoElement {
    let height = meter_height(peak);
    div()
        .w(px(4.))
        .h(px(size))
        .rounded(px(1.))
        .bg(theme.control)
        .flex()
        .flex_col()
        .justify_end()
        .child(
            div()
                .w_full()
                .h(px(size * height))
                .rounded(px(1.))
                .bg(if height > 0.98 {
                    theme.meter_high
                } else {
                    theme.meter_low
                }),
        )
}

fn clip_view(
    id: ClipId,
    clip: &MidiClip,
    label: &str,
    color: ContentColor,
    zoom: Zoom,
) -> impl IntoElement {
    let left = zoom.beat_to_x(clip.start.as_beats());
    let width = zoom.beat_to_x(clip.length.as_beats());
    let note_area = (zoom.row_height - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.).max(0.);
    let notes = clip
        .notes
        .iter()
        .zip(note_rows(clip))
        .filter(|(note, _)| note.start < clip.length)
        .map(move |(note, row)| {
            let end = (note.start + note.length).min(clip.length);
            div()
                .absolute()
                .left(px(zoom.beat_to_x(note.start.as_beats())))
                .top(px(CLIP_LABEL_HEIGHT + row * note_area))
                .w(px(
                    (zoom.beat_to_x((end - note.start).as_beats()) - 1.).max(2.)
                ))
                .h(px(3.))
                .rounded(px(1.))
                .bg(color.detail)
        });
    div()
        .id(("clip", id.0))
        .absolute()
        .left(px(left))
        .top(px(CLIP_INSET))
        .w(px(width))
        .h(px(zoom.row_height - 2. * CLIP_INSET))
        .rounded(px(5.))
        .bg(color.fill)
        .border_1()
        .border_color(color.outline)
        .overflow_hidden()
        .child(
            div()
                .px(px(6.))
                .pt(px(2.))
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color.detail)
                .child(label.to_owned()),
        )
        .children(notes)
}
