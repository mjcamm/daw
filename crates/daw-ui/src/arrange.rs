//! The Arrange page: the node list on the left, the timeline beside it.

use crate::app::AppView;
use crate::layout::{
    BEATS_PER_BAR, beat_to_x, meter_height, note_rows, timeline_bars, x_to_snapped_position,
};
use crate::theme::{ContentColor, Theme};
use daw_engine::{ClipId, Command, Edit, MidiClip};
use gpui::{Context, FontWeight, Hsla, MouseButton, MouseDownEvent, canvas, div, prelude::*, px};

const RULER_HEIGHT: f32 = 28.;
const ROW_HEIGHT: f32 = 56.;
const NODE_LIST_WIDTH: f32 = 260.;
const CLIP_INSET: f32 = 4.;
const CLIP_LABEL_HEIGHT: f32 = 16.;
const METER_HEIGHT: f32 = 32.;

impl AppView {
    pub(crate) fn render_arrange(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(self.render_node_list(theme, cx))
            .child(self.render_timeline(theme, cx))
    }

    fn render_node_list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let node = &self.session.snapshot().node;
        let meter = self.session.meter();
        let color = theme.content(node.color);

        let toggle = |id: &'static str, label: &'static str, on: bool, on_bg: Hsla, on_fg: Hsla| {
            div()
                .id(id)
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
                this.session
                    .apply(Command::Edit(Edit::SetMute { muted: !muted }));
            }),
        );
        let solo = toggle("solo", "S", soloed, theme.accent, theme.on_accent).on_click(
            cx.listener(move |this, _, _, _| {
                this.session
                    .apply(Command::Edit(Edit::SetSolo { soloed: !soloed }));
            }),
        );

        let row = div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .border_b_1()
            .border_color(theme.hairline)
            .child(div().w(px(4.)).h(px(32.)).rounded(px(2.)).bg(color.detail))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(13.)).child(node.name.clone()))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_secondary)
                            .child("MIDI · Synth"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(meter_bar(meter.left, theme))
                    .child(meter_bar(meter.right, theme)),
            )
            .child(mute)
            .child(solo);

        div()
            .w(px(NODE_LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .child(
                div()
                    .h(px(RULER_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .border_b_1()
                    .border_color(theme.hairline)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("NODES"),
            )
            .child(row)
    }

    fn render_timeline(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let bars = timeline_bars(snapshot.clips.iter().map(|(_, c)| c), snapshot.loop_range);
        let width = beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let color = if snapshot.node.muted {
            theme.muted_content
        } else {
            theme.content(snapshot.node.color)
        };

        // Bar lines, stronger than the beat lines between them.
        let grid_lines = (0..bars * 4).map(|beat| {
            let strong = beat % 4 == 0;
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(beat_to_x(f64::from(beat))))
                .w(px(1.))
                .bg(if strong { theme.hairline } else { theme.grid })
        });

        let lane = div()
            .relative()
            .h(px(ROW_HEIGHT))
            .border_b_1()
            .border_color(theme.hairline)
            .children(
                snapshot
                    .clips
                    .iter()
                    .map(|(id, clip)| clip_view(*id, clip, &snapshot.node.name, color)),
            );

        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(beat_to_x(self.session.position_beats())))
            .w(px(1.))
            .bg(theme.accent);

        div()
            .id("timeline")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .h_full()
                    .children(grid_lines)
                    .child(self.render_ruler(theme, bars, cx))
                    .child(lane)
                    .child(playhead),
            )
    }

    fn render_ruler(&self, theme: &Theme, bars: u32, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.ruler_bounds.clone();
        let mut loop_fill = theme.accent;
        loop_fill.a = 0.2;

        let loop_band = self.session.snapshot().loop_range.map(|range| {
            let start = beat_to_x(range.start.as_beats());
            let end = beat_to_x(range.end.as_beats());
            div()
                .absolute()
                .top(px(2.))
                .h(px(6.))
                .left(px(start))
                .w(px(end - start))
                .rounded(px(2.))
                .bg(loop_fill)
        });

        let labels = (0..bars).map(|bar| {
            div()
                .absolute()
                .left(px(beat_to_x(f64::from(bar) * BEATS_PER_BAR) + 4.))
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
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    let origin = this.ruler_bounds.get().origin.x;
                    let x = (event.position.x - origin) / px(1.);
                    this.session.apply(Command::Locate {
                        position: x_to_snapped_position(x),
                    });
                }),
            )
    }
}

fn meter_bar(peak: f32, theme: &Theme) -> impl IntoElement {
    let height = meter_height(peak);
    div()
        .w(px(4.))
        .h(px(METER_HEIGHT))
        .rounded(px(1.))
        .bg(theme.control)
        .flex()
        .flex_col()
        .justify_end()
        .child(
            div()
                .w_full()
                .h(px(METER_HEIGHT * height))
                .rounded(px(1.))
                .bg(if height > 0.98 {
                    theme.meter_high
                } else {
                    theme.meter_low
                }),
        )
}

fn clip_view(id: ClipId, clip: &MidiClip, label: &str, color: ContentColor) -> impl IntoElement {
    let left = beat_to_x(clip.start.as_beats());
    let width = beat_to_x(clip.length.as_beats());
    let note_area = ROW_HEIGHT - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.;
    let notes = clip
        .notes
        .iter()
        .zip(note_rows(clip))
        .filter(|(note, _)| note.start < clip.length)
        .map(move |(note, row)| {
            let end = (note.start + note.length).min(clip.length);
            div()
                .absolute()
                .left(px(beat_to_x(note.start.as_beats())))
                .top(px(CLIP_LABEL_HEIGHT + row * note_area))
                .w(px((beat_to_x((end - note.start).as_beats()) - 1.).max(2.)))
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
        .h(px(ROW_HEIGHT - 2. * CLIP_INSET))
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
