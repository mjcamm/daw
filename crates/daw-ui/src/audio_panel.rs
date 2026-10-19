//! The bottom panel for an audio node (`docs/design/audio-clips.md`): its
//! clips large along song time, each side of the audio on its own, and on
//! the right the selected clip's settings: its file, BPM, Follow tempo,
//! Gain and Fades. It takes the piano roll's place when an audio node is
//! open, with the same frame (ruler, scrollbars, zoom).

use crate::clip_edit::bar_beat;
use crate::frame::Timeline;
use crate::icons::{Icon, icon};
use crate::inspector::{format_beats, group, section_label};
use crate::parameters::small_button;
use crate::project::ProjectView;
use crate::slider::{self, Axis};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{MONO_FONT, Theme};
use crate::tooltip;
use crate::waveform;
use daw_engine::{
    AudioClip, ClipId, Command, Edit, MAX_CLIP_BPM, MAX_CLIP_GAIN_DB, MIN_CLIP_BPM,
    MIN_CLIP_GAIN_DB, NodeId, SampleStatus, Ticks,
};
use gpui::{
    AnyElement, Context, Entity, Focusable, FontWeight, MouseButton, MouseDownEvent, Pixels, Point,
    Subscription, Window, div, prelude::*, px,
};
use std::time::Instant;

const HEADER_HEIGHT: f32 = 34.;
const RULER_HEIGHT: f32 = 22.;
/// Width of the settings column on the right.
const INFO_WIDTH: f32 = 260.;
const DEFAULT_HEIGHT: f32 = 300.;
const MIN_HEIGHT: f32 = 160.;
/// The lane's height for each pixel of the frame's row zoom.
const LANE_PER_ROW: f32 = 10.;
const ROW_PADDING: f32 = 12.;
const LABEL_WIDTH: f32 = 76.;
const VALUE_WIDTH: f32 = 64.;

/// A setting changed by dragging its slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Gain,
    FadeIn,
    FadeOut,
}

impl Setting {
    fn label(self) -> &'static str {
        match self {
            Self::Gain => "Gain",
            Self::FadeIn => "Fade in",
            Self::FadeOut => "Fade out",
        }
    }

    /// Where the clip's value sits on the slider, `0..=1`.
    pub fn position(self, clip: &AudioClip) -> f64 {
        let fade = |t: Ticks| (t.0 as f64 / clip.length.0.max(1) as f64).sqrt();
        match self {
            Self::Gain => (clip.gain_db - MIN_CLIP_GAIN_DB) / (MAX_CLIP_GAIN_DB - MIN_CLIP_GAIN_DB),
            Self::FadeIn => fade(clip.fade_in),
            Self::FadeOut => fade(clip.fade_out),
        }
    }

    /// The clip with this setting at slider position `at`. Gain lands on
    /// tenths of a dB; fades favour short times (the square-root scale)
    /// and never overlap the other fade.
    pub fn at(self, clip: &AudioClip, at: f64) -> AudioClip {
        let at = at.clamp(0., 1.);
        let fade = |room: Ticks| Ticks((at * at * clip.length.0 as f64).round() as i64).min(room);
        match self {
            Self::Gain => {
                let db = MIN_CLIP_GAIN_DB + at * (MAX_CLIP_GAIN_DB - MIN_CLIP_GAIN_DB);
                AudioClip {
                    gain_db: (db * 10.).round() / 10.,
                    ..clip.clone()
                }
            }
            Self::FadeIn => AudioClip {
                fade_in: fade(clip.length - clip.fade_out),
                ..clip.clone()
            },
            Self::FadeOut => AudioClip {
                fade_out: fade(clip.length - clip.fade_in),
                ..clip.clone()
            },
        }
    }

    fn format(self, clip: &AudioClip) -> String {
        match self {
            Self::Gain if clip.gain_db <= MIN_CLIP_GAIN_DB => "-inf dB".to_owned(),
            Self::Gain => format!("{:+.1} dB", clip.gain_db),
            Self::FadeIn => format_beats(clip.fade_in),
            Self::FadeOut => format_beats(clip.fade_out),
        }
    }
}

/// A typed tempo, if it is one a clip can have.
pub fn parse_bpm(text: &str) -> Option<f64> {
    let bpm: f64 = text.trim().parse().ok()?;
    (MIN_CLIP_BPM..=MAX_CLIP_BPM).contains(&bpm).then_some(bpm)
}

/// "44.1 kHz", "48 kHz".
fn format_rate(rate: u32) -> String {
    let khz = f64::from(rate) / 1000.;
    if khz.fract() == 0. {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz:.1} kHz")
    }
}

fn format_bpm(bpm: f64) -> String {
    if bpm.fract().abs() < 1e-9 {
        format!("{bpm:.0}")
    } else {
        format!("{bpm:.2}")
    }
}

#[derive(Debug, Clone)]
enum PanelDrag {
    /// The panel's top edge.
    Height {
        origin: Point<Pixels>,
        original: f32,
    },
    /// A setting's slider, previewed until release.
    Setting {
        clip: ClipId,
        node: NodeId,
        setting: Setting,
        origin: Point<Pixels>,
        original: AudioClip,
        preview: AudioClip,
    },
}

/// The open panel for an audio node.
#[derive(Debug)]
pub struct AudioEditor {
    pub node: NodeId,
    pub height: f32,
    opened: Instant,
    /// Where to scroll to on the next draw, in beats.
    scroll_to: Option<f64>,
    drag: Option<PanelDrag>,
}

/// Typing a clip's BPM.
pub struct BpmInput {
    clip: ClipId,
    input: Entity<TextInput>,
    _subscription: Subscription,
}

impl std::fmt::Debug for BpmInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BpmInput")
            .field("clip", &self.clip)
            .finish()
    }
}

impl ProjectView {
    /// Opens the panel on an audio node at `beat`, taking the piano roll's
    /// place (and height) if it is open.
    pub(crate) fn open_audio_editor(&mut self, node: NodeId, beat: f64) {
        let height = self
            .audio_editor
            .as_ref()
            .map(|e| e.height)
            .or_else(|| self.note_editor.as_ref().map(|e| e.height))
            .unwrap_or(DEFAULT_HEIGHT);
        let opened = self
            .audio_editor
            .as_ref()
            .map(|e| e.opened)
            .or_else(|| {
                self.note_editor
                    .as_ref()
                    .map(|_| Instant::now() - std::time::Duration::from_secs(1))
            })
            .unwrap_or_else(Instant::now);
        self.note_editor = None;
        self.audio_editor = Some(AudioEditor {
            node,
            height,
            opened,
            scroll_to: Some(beat),
            drag: None,
        });
        self.selected_node = Some(node);
    }

    /// Opens the panel on an audio clip's node, with the clip selected.
    pub(crate) fn open_audio_clip(&mut self, clip: ClipId) {
        let snapshot = self.session.snapshot();
        let found = snapshot.nodes.iter().find_map(|n| {
            n.audio_clips
                .iter()
                .find(|(id, _)| *id == clip)
                .map(|(_, c)| (n.id, c.start.as_beats()))
        });
        if let Some((node, beat)) = found {
            self.open_audio_editor(node, beat);
            self.selected_clips.clear();
            self.selected_clips.insert(clip);
        }
    }

    pub(crate) fn close_audio_editor(&mut self) {
        self.audio_editor = None;
        self.bpm_input = None;
    }

    /// The bottom panel follows the node selected on Arrange: an audio
    /// node shows here, a MIDI node in the piano roll.
    pub(crate) fn switch_bottom_panel(&mut self) {
        let Some(selected) = self.selected_node else {
            return;
        };
        let snapshot = self.session.snapshot();
        let Some(node) = snapshot.node(selected).filter(|n| n.parent.is_some()) else {
            return;
        };
        let zoom = self.arrange_view.zoom;
        let shown =
            f64::from(-self.arrange_view.x.offset().x / px(1.) / zoom.pixels_per_beat).max(0.);
        let audio = node.kind == daw_engine::NodeKind::Audio;
        let busy = self.audio_editor.as_ref().is_some_and(|e| e.drag.is_some())
            || self.note_editor.as_ref().is_some_and(|e| e.dragging());
        if busy {
            return;
        }
        let elsewhere = self
            .audio_editor
            .as_ref()
            .is_some_and(|e| e.node != selected);
        if audio && (self.note_editor.is_some() || elsewhere) {
            self.open_audio_editor(selected, shown);
        } else if !audio && let Some(editor) = self.audio_editor.take() {
            self.bpm_input = None;
            self.open_node_editor(selected, shown);
            if let Some(notes) = &mut self.note_editor {
                notes.height = editor.height;
            }
        }
    }

    /// The selected clip, if exactly one of the panel's node's clips is.
    fn panel_clip(&self) -> Option<(ClipId, AudioClip)> {
        let editor = self.audio_editor.as_ref()?;
        let node = self.session.snapshot().node(editor.node)?;
        let mut chosen = node
            .audio_clips
            .iter()
            .filter(|(id, _)| self.selected_clips.contains(id));
        let (id, clip) = chosen.next()?;
        if chosen.next().is_some() {
            return None;
        }
        // A setting being dragged shows its preview.
        let clip = match &editor.drag {
            Some(PanelDrag::Setting {
                clip: dragged,
                preview,
                ..
            }) if dragged == id => preview.clone(),
            _ => clip.clone(),
        };
        Some((*id, clip))
    }

    /// Follows the pointer; false if no panel drag is running.
    pub(crate) fn drag_audio_editor(&mut self, position: Point<Pixels>, fine: bool) -> bool {
        let Some(editor) = &mut self.audio_editor else {
            return false;
        };
        match &mut editor.drag {
            Some(PanelDrag::Height { origin, original }) => {
                let up = (origin.y - position.y) / px(1.);
                editor.height = (*original + up).max(MIN_HEIGHT);
                true
            }
            Some(PanelDrag::Setting {
                setting,
                origin,
                original,
                preview,
                ..
            }) => {
                let moved = (position.x - origin.x) / px(1.);
                let at = slider::dragged(setting.position(original), moved, fine);
                *preview = setting.at(original, at);
                true
            }
            None => false,
        }
    }

    /// Ends a panel drag: a setting sends one edit.
    pub(crate) fn finish_audio_drag(&mut self) {
        let Some(editor) = &mut self.audio_editor else {
            return;
        };
        if let Some(PanelDrag::Setting {
            clip,
            node,
            original,
            preview,
            ..
        }) = editor.drag.take()
            && preview != original
        {
            self.session.apply(Command::Edit(Edit::SetAudioClip {
                id: clip,
                node,
                clip: preview,
            }));
        }
    }

    pub(crate) fn audio_dragging(&self) -> bool {
        self.audio_editor.as_ref().is_some_and(|e| e.drag.is_some())
    }

    fn set_panel_clip(&mut self, id: ClipId, clip: AudioClip) {
        let Some(editor) = &self.audio_editor else {
            return;
        };
        self.session.apply(Command::Edit(Edit::SetAudioClip {
            id,
            node: editor.node,
            clip,
        }));
    }

    /// Starts typing the clip's BPM.
    fn edit_bpm(&mut self, id: ClipId, current: f64, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(&format_bpm(current), "BPM", cx));
        window.focus(&input.read(cx).focus_handle(cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &TextInputEvent, window, _| {
                if let TextInputEvent::Confirm(text) = event
                    && let Some(bpm) = parse_bpm(text)
                    && let Some((clip_id, clip)) = this.panel_clip()
                    && clip_id == id
                {
                    this.set_panel_clip(id, clip.with_bpm(bpm));
                }
                this.bpm_input = None;
                window.focus(&this.focus);
            },
        );
        self.bpm_input = Some(BpmInput {
            clip: id,
            input,
            _subscription: subscription,
        });
    }

    /// The panel, if an audio node is open: header, the node's clips along
    /// song time, and the selected clip's settings.
    pub(crate) fn render_audio_editor(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let snapshot = self.session.snapshot();
        let editor = self.audio_editor.as_ref()?;
        if snapshot
            .node(editor.node)
            .is_none_or(|n| n.kind != daw_engine::NodeKind::Audio)
        {
            self.close_audio_editor();
            return None;
        }
        if let Some(beat) = self.audio_editor.as_mut().and_then(|e| e.scroll_to.take()) {
            let x = (beat as f32 * self.notes_view.zoom.pixels_per_beat - 40.).max(0.);
            let y = self.notes_view.y.offset().y;
            self.notes_view.x.set_offset(gpui::point(px(-x), y));
        }
        let editor = self.audio_editor.as_ref()?;
        let snapshot = self.session.snapshot();
        let node = snapshot.node(editor.node)?;
        let colors = theme.content(node.color);
        let zoom = self.notes_view.zoom;
        let ppb = zoom.pixels_per_beat;
        let bpm = snapshot.tempo_bpm;
        let song_beats =
            crate::layout::timeline_bars(crate::project::song_clips(snapshot), snapshot.loop_range)
                as f32
                * 4.;
        let audio_end = node
            .audio_clips
            .iter()
            .map(|(_, c)| c.end().as_beats() as f32)
            .fold(0., f32::max);
        let width = (song_beats.max(audio_end) + 4.) * ppb;
        let lane_height = (zoom.row_height * LANE_PER_ROW).max(80.);
        let opened = editor.opened.elapsed().as_secs_f32() / 0.18;
        let slide = 1. - (1. - opened.clamp(0., 1.)).powi(3);
        let panel_height = editor.height * slide;
        let offset = self.notes_view.x.offset();
        let selected = self.panel_clip().map(|(id, _)| id);

        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .child(div().size(px(8.)).rounded(px(4.)).bg(colors.detail))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} · audio", node.name)),
            )
            .child(div().flex_1())
            .child(
                small_button("audio-editor-close", theme)
                    .child(icon(Icon::Close, theme.text_secondary, 14.))
                    .tooltip(tooltip::text("Close"))
                    .on_click(cx.listener(|this, _, _, _| this.close_audio_editor())),
            );

        // Each clip large: its frame, a fade line at each end, and each
        // side of the audio in its own half.
        let clips: Vec<AnyElement> = node
            .audio_clips
            .iter()
            .map(|(id, clip)| {
                let id = *id;
                let shown = match &editor.drag {
                    Some(PanelDrag::Setting {
                        clip: dragged,
                        preview,
                        ..
                    }) if *dragged == id => preview.clone(),
                    _ => clip.clone(),
                };
                let sample = snapshot.samples.iter().find(|s| s.id == shown.sample);
                let name = sample.map_or("", |s| s.name.as_str());
                let reading = waveform::Reading::of(&shown, bpm, ppb);
                let halves = sample.and_then(|s| s.peaks.clone()).map(|peaks| {
                    let sides = if peaks.channels() > 1 { 2 } else { 1 };
                    (0..sides)
                        .map(|side| {
                            div()
                                .relative()
                                .flex_1()
                                .when(side == 1, |d| d.border_t_1().border_color(colors.outline))
                                .child(waveform::channel(
                                    peaks.clone(),
                                    reading,
                                    Some(side),
                                    colors.detail,
                                ))
                        })
                        .collect::<Vec<_>>()
                });
                let length = shown.length.as_beats() as f32 * ppb;
                let fade = |ticks: Ticks| ticks.as_beats() as f32 * ppb;
                let fade_in = fade(shown.fade_in);
                let fade_out = fade(shown.fade_out);
                let is_selected = selected == Some(id);
                div()
                    .id(("audio-clip", id.0))
                    .absolute()
                    .left(px(shown.start.as_beats() as f32 * ppb))
                    .top(px(4.))
                    .w(px(length))
                    .h(px(lane_height - 8.))
                    .rounded(px(5.))
                    .bg(colors.fill)
                    .border_1()
                    .border_color(if is_selected {
                        theme.accent
                    } else {
                        colors.outline
                    })
                    .when(is_selected, |d| d.border_2())
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_none()
                            .px(px(6.))
                            .pt(px(2.))
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.detail)
                            .whitespace_nowrap()
                            .child(format!("{name} · {}", bar_beat(shown.start))),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .children(halves.into_iter().flatten())
                            .child(fade_shape(fade_in, fade_out, length, theme)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.selected_clips.clear();
                            this.selected_clips.insert(id);
                        }),
                    )
                    .into_any_element()
            })
            .collect();
        let position = self.session.position_beats() as f32;
        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(position * ppb))
            .w(px(1.))
            .bg(theme.accent);
        let lane = div()
            .id("audio-lane-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&self.notes_view.x)
            .child(
                div()
                    .id("audio-lane")
                    .relative()
                    .w(px(width))
                    .h(px(lane_height))
                    .children(crate::frame::grid_lines(
                        zoom,
                        (width / ppb / 4.).ceil() as u32,
                        theme,
                    ))
                    .children(clips)
                    .child(playhead)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, _| this.selected_clips.clear()),
                    ),
            );
        let labels = (0..=(width / ppb / 4.).ceil() as i32).map(|bar| {
            div()
                .absolute()
                .left(px(bar as f32 * 4. * ppb + 3.))
                .bottom(px(2.))
                .text_size(px(10.))
                .text_color(theme.text_tertiary)
                .child((bar + 1).to_string())
        });
        let ruler = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .overflow_hidden()
            .child(
                div()
                    .relative()
                    .left(offset.x)
                    .w(px(width))
                    .h(px(RULER_HEIGHT))
                    .children(labels.collect::<Vec<_>>()),
            );
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(ruler)
            .child(div().flex_1().min_h_0().flex().child(lane))
            .into_any_element();
        let frame = self.timeline_frame(Timeline::Notes, body, 0., theme, cx);
        let info = self.render_clip_info(theme, cx);

        let mut handle_hover = theme.accent;
        handle_hover.a = 0.35;
        let handle = div()
            .id("audio-editor-resize")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(5.))
            .occlude()
            .cursor_ns_resize()
            .hover(move |s| s.bg(handle_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    if let Some(editor) = &mut this.audio_editor {
                        editor.drag = Some(PanelDrag::Height {
                            origin: event.position,
                            original: editor.height,
                        });
                    }
                }),
            );
        Some(
            div()
                .h(px(panel_height))
                .flex_none()
                .relative()
                .overflow_hidden()
                .flex()
                .flex_col()
                .bg(theme.window)
                .border_t_1()
                .border_color(theme.edge)
                .child(header)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(div().flex_1().min_w_0().flex().flex_col().child(frame))
                        .child(info),
                )
                .child(handle)
                .into_any_element(),
        )
    }

    /// The settings column: the selected clip's file, tempo, level and fades.
    fn render_clip_info(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // Scrolls when the panel is short; its parts keep their height.
        let column = div()
            .id("clip-info")
            .w(px(INFO_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .pb(px(12.))
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.edge)
            .overflow_y_scroll();
        let Some((id, clip)) = self.panel_clip() else {
            return column
                .child(
                    div()
                        .p(px(16.))
                        .text_size(px(12.))
                        .text_color(theme.text_tertiary)
                        .child("Select one of this node's clips to see its settings."),
                )
                .into_any_element();
        };
        let snapshot = self.session.snapshot();
        let sample = snapshot.samples.iter().find(|s| s.id == clip.sample);
        let name = sample.map_or("Missing sample".to_owned(), |s| s.name.clone());
        let details = match sample.map(|s| &s.status) {
            Some(SampleStatus::Loaded {
                seconds,
                channels,
                sample_rate,
            }) => format!(
                "{seconds:.1} s · {} · {}",
                format_rate(*sample_rate),
                if *channels > 1 { "Stereo" } else { "Mono" }
            ),
            Some(SampleStatus::Missing { reason }) => format!("Missing: {reason}"),
            None => String::new(),
        };
        let file = group(theme).child(
            div()
                .px(px(ROW_PADDING))
                .py(px(8.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_secondary)
                        .child(details),
                ),
        );

        // BPM: typed. Follow tempo: a switch.
        let hover = theme.hover;
        let bpm_value: AnyElement = match &self.bpm_input {
            Some(input) if input.clip == id => div()
                .w(px(VALUE_WIDTH))
                .font_family(MONO_FONT)
                .text_size(px(11.))
                .child(input.input.clone())
                .into_any_element(),
            _ => {
                let current = clip.bpm;
                div()
                    .id("clip-bpm")
                    .min_w(px(VALUE_WIDTH))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.control)
                    .hover(move |s| s.bg(hover))
                    .cursor_text()
                    .flex()
                    .justify_end()
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .child(format_bpm(current))
                    .tooltip(tooltip::text("The file's own tempo: click to type it"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_bpm(id, current, window, cx);
                    }))
                    .into_any_element()
            }
        };
        let follow = clip.follow_tempo;
        let toggle = {
            let track = if follow { theme.active } else { theme.control };
            div()
                .id("clip-follow")
                .relative()
                .w(px(26.))
                .h(px(15.))
                .flex_none()
                .rounded(px(7.5))
                .bg(track)
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(if follow { 13. } else { 2. }))
                        .size(px(11.))
                        .rounded(px(5.5))
                        .bg(theme.on_accent)
                        .shadow_sm(),
                )
                .tooltip(tooltip::text(if follow {
                    "Stretched to the song's tempo: click to play at the file's own speed"
                } else {
                    "Plays at the file's own speed: click to stretch it to the song's tempo"
                }))
                .on_click({
                    let clip = clip.clone();
                    cx.listener(move |this, _, _, _| {
                        this.set_panel_clip(
                            id,
                            AudioClip {
                                follow_tempo: !follow,
                                ..clip.clone()
                            },
                        );
                    })
                })
        };
        let tempo = group(theme)
            .child(
                row("BPM", false, theme)
                    .child(div().flex_1())
                    .child(bpm_value),
            )
            .child(
                row("Follow tempo", true, theme)
                    .child(div().flex_1())
                    .child(toggle),
            );

        let level =
            group(theme).child(self.setting_row(id, &clip, Setting::Gain, false, theme, cx));
        let fades = group(theme)
            .child(self.setting_row(id, &clip, Setting::FadeIn, false, theme, cx))
            .child(self.setting_row(id, &clip, Setting::FadeOut, true, theme, cx));
        column
            .child(section_label("File", theme).flex_none())
            .child(file.flex_none())
            .child(section_label("Tempo", theme).flex_none())
            .child(tempo.flex_none())
            .child(section_label("Level", theme).flex_none())
            .child(level.flex_none())
            .child(section_label("Fades", theme).flex_none())
            .child(fades.flex_none())
            .into_any_element()
    }

    /// A row with the app's slider and the value beside it.
    fn setting_row(
        &self,
        id: ClipId,
        clip: &AudioClip,
        setting: Setting,
        separated: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let active = self.audio_editor.as_ref().is_some_and(
            |e| matches!(&e.drag, Some(PanelDrag::Setting { setting: s, .. }) if *s == setting),
        );
        let original = clip.clone();
        let length =
            INFO_WIDTH - 2. * 10. - 2. - 2. * ROW_PADDING - LABEL_WIDTH - VALUE_WIDTH - 16.;
        let slider = slider::slider(
            ("clip-setting", setting as usize),
            Axis::Horizontal,
            setting.position(clip) as f32,
            length.max(40.),
            active,
            theme,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                if event.click_count >= 2 {
                    // Double-click: back to the default (0 dB, no fade).
                    let reset = match setting {
                        Setting::Gain => AudioClip {
                            gain_db: 0.,
                            ..original.clone()
                        },
                        Setting::FadeIn => AudioClip {
                            fade_in: Ticks::ZERO,
                            ..original.clone()
                        },
                        Setting::FadeOut => AudioClip {
                            fade_out: Ticks::ZERO,
                            ..original.clone()
                        },
                    };
                    this.set_panel_clip(id, reset);
                    return;
                }
                let Some(editor) = &mut this.audio_editor else {
                    return;
                };
                editor.drag = Some(PanelDrag::Setting {
                    clip: id,
                    node: editor.node,
                    setting,
                    origin: event.position,
                    original: original.clone(),
                    preview: original.clone(),
                });
            }),
        );
        row(setting.label(), separated, theme)
            .gap(px(8.))
            .child(slider)
            .child(
                div()
                    .w(px(VALUE_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(setting.format(clip)),
            )
    }
}

/// A labelled row in a group; `separated` draws the hairline above it.
fn row(label: &'static str, separated: bool, theme: &Theme) -> gpui::Div {
    div()
        .min_h(px(34.))
        .px(px(ROW_PADDING))
        .flex()
        .items_center()
        .when(separated, |d| d.border_t_1().border_color(theme.hairline))
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_none()
                .text_size(px(12.))
                .child(label),
        )
}

/// The fades drawn over a clip: a line rising from silence over the fade
/// in, and falling to silence over the fade out.
fn fade_shape(fade_in: f32, fade_out: f32, length: f32, theme: &Theme) -> impl IntoElement {
    let color = theme.text;
    gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let h = bounds.size.height;
            let at = |x: f32, top: bool| {
                bounds.origin + gpui::point(px(x), if top { px(0.) } else { h })
            };
            for (from, to) in [
                (fade_in > 0.).then(|| (at(0., false), at(fade_in, true))),
                (fade_out > 0.).then(|| (at(length - fade_out, true), at(length, false))),
            ]
            .into_iter()
            .flatten()
            {
                let mut path = gpui::PathBuilder::stroke(px(1.5));
                path.move_to(from);
                path.line_to(to);
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::SampleId;

    fn clip() -> AudioClip {
        AudioClip::whole_file(SampleId(1), Ticks::ZERO, 2.0, 120.0)
    }

    #[test]
    fn gain_runs_from_silence_to_plus_24_db_in_tenths() {
        let c = clip();
        assert_eq!(Setting::Gain.at(&c, 0.).gain_db, MIN_CLIP_GAIN_DB);
        assert_eq!(Setting::Gain.at(&c, 1.).gain_db, MAX_CLIP_GAIN_DB);
        let near = Setting::Gain.at(&c, 0.7);
        assert_eq!(near.gain_db, -1.2);
        assert!((Setting::Gain.position(&near) - 0.7).abs() < 0.002);
        assert_eq!(Setting::Gain.format(&Setting::Gain.at(&c, 0.)), "-inf dB");
        assert_eq!(Setting::Gain.format(&c), "+0.0 dB");
    }

    #[test]
    fn fades_favour_short_times_and_never_overlap() {
        let c = clip();
        // Half way along is a quarter of the clip.
        assert_eq!(Setting::FadeIn.at(&c, 0.5).fade_in, Ticks(c.length.0 / 4));
        let out = Setting::FadeOut.at(&Setting::FadeIn.at(&c, 1.), 1.);
        assert_eq!(out.fade_in, c.length);
        assert_eq!(out.fade_out, Ticks::ZERO, "no room left");
        let both = Setting::FadeOut.at(&Setting::FadeIn.at(&c, 0.5), 1.);
        assert_eq!(both.fade_in + both.fade_out, c.length);
    }

    #[test]
    fn tempos_are_typed_within_limits() {
        assert_eq!(parse_bpm(" 128 "), Some(128.0));
        assert_eq!(parse_bpm("87.5"), Some(87.5));
        assert_eq!(parse_bpm("0"), None);
        assert_eq!(parse_bpm("fast"), None);
        assert_eq!(format_bpm(128.0), "128");
        assert_eq!(format_bpm(87.5), "87.50");
        assert_eq!(format_rate(44_100), "44.1 kHz");
        assert_eq!(format_rate(48_000), "48 kHz");
    }
}
