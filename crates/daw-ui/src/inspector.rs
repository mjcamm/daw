//! The inspector: a panel down the right of every page for configuring the
//! selected thing in detail.
//!
//! It folds away into a narrow tray. The tray's icon lights up when the
//! selection has something to show; clicking it opens or closes the panel,
//! and the panel's left edge drags to resize it. For now only a notes signal
//! (selected on the Signals page) has settings here: its pitch range, its
//! envelope and its velocity clamp. Dragging a value previews the change
//! everywhere the signal is drawn and sends one edit on release.

use crate::icons::{Icon, icon};
use crate::parameters::{paint_curve, small_button};
use crate::project::{Page, ProjectView};
use crate::slider::{self, Axis};
use crate::theme::{MONO_FONT, Theme};
use crate::tooltip;
use daw_automation::{CompiledSignal, SignalNote};
use daw_engine::{
    Command, Edit, NotesSource, Signal, SignalId, SignalSnapshot, SignalSource, Ticks,
};
use gpui::{
    AnyElement, Context, Div, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, canvas, div,
    prelude::*, px,
};

/// Width of the folded tray.
pub const TRAY_WIDTH: f32 = 32.;
pub const DEFAULT_WIDTH: f32 = 280.;
const MIN_WIDTH: f32 = 220.;
const MAX_WIDTH: f32 = 520.;
/// Longest attack, decay or release the sliders reach, in beats.
const MAX_TIME_BEATS: f64 = 4.;
/// Pixels of sideways drag per semitone on a pitch value.
const PIXELS_PER_SEMITONE: f32 = 6.;
const ENVELOPE_HEIGHT: f32 = 64.;
const VALUE_WIDTH: f32 = 64.;
const LABEL_WIDTH: f32 = 56.;
const GROUP_MARGIN: f32 = 10.;
const ROW_PADDING: f32 = 12.;
const SLIDER_GAP: f32 = 10.;

/// A notes signal setting in the inspector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesField {
    Lowest,
    Highest,
    Attack,
    Decay,
    Sustain,
    Release,
    VelocityMin,
    VelocityMax,
}

impl NotesField {
    fn label(self) -> &'static str {
        match self {
            Self::Lowest => "Lowest",
            Self::Highest => "Highest",
            Self::Attack => "Attack",
            Self::Decay => "Decay",
            Self::Sustain => "Sustain",
            Self::Release => "Release",
            Self::VelocityMin => "Min",
            Self::VelocityMax => "Max",
        }
    }

    /// The value as shown.
    fn format(self, source: &NotesSource) -> String {
        match self {
            Self::Lowest => note_name(source.lowest),
            Self::Highest => note_name(source.highest),
            Self::Attack => format_beats(source.attack),
            Self::Decay => format_beats(source.decay),
            Self::Release => format_beats(source.release),
            Self::Sustain => percent(source.sustain),
            Self::VelocityMin => percent(source.velocity_min),
            Self::VelocityMax => percent(source.velocity_max),
        }
    }

    /// Where a slider for this field sits, `0..=1`.
    fn position(self, source: &NotesSource) -> f64 {
        match self {
            Self::Lowest => f64::from(source.lowest) / 127.,
            Self::Highest => f64::from(source.highest) / 127.,
            Self::Attack => time_position(source.attack),
            Self::Decay => time_position(source.decay),
            Self::Release => time_position(source.release),
            Self::Sustain => source.sustain,
            Self::VelocityMin => source.velocity_min,
            Self::VelocityMax => source.velocity_max,
        }
    }

    /// `original` after dragging this field `dx` pixels sideways. Ranges
    /// stay valid: a minimum never passes its maximum, nor the other way.
    pub fn dragged(self, original: &NotesSource, dx: f32, fine: bool) -> NotesSource {
        let mut source = original.clone();
        let slid = |value: f64| slider::dragged(value, dx, fine);
        let semitones = |pitch: u8| {
            let step = if fine { 5. } else { 1. } * PIXELS_PER_SEMITONE;
            (f32::from(pitch) + (dx / step).round()).clamp(0., 127.) as u8
        };
        match self {
            Self::Lowest => source.lowest = semitones(original.lowest).min(original.highest),
            Self::Highest => source.highest = semitones(original.highest).max(original.lowest),
            Self::Attack => source.attack = time_at(slid(time_position(original.attack))),
            Self::Decay => source.decay = time_at(slid(time_position(original.decay))),
            Self::Release => source.release = time_at(slid(time_position(original.release))),
            Self::Sustain => source.sustain = slid(original.sustain),
            Self::VelocityMin => {
                source.velocity_min = slid(original.velocity_min).min(original.velocity_max);
            }
            Self::VelocityMax => {
                source.velocity_max = slid(original.velocity_max).max(original.velocity_min);
            }
        }
        source
    }
}

/// A notes signal section that can be switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotesSwitch {
    PitchRange,
    VelocityClamp,
}

impl NotesSwitch {
    fn label(self) -> &'static str {
        match self {
            Self::PitchRange => "Pitch range",
            Self::VelocityClamp => "Velocity clamp",
        }
    }

    fn tooltip(self, on: bool) -> &'static str {
        match (self, on) {
            (Self::PitchRange, true) => "On: only notes in the range hit",
            (Self::PitchRange, false) => "Off: every note hits",
            (Self::VelocityClamp, true) => "On: hit heights are kept between Min and Max",
            (Self::VelocityClamp, false) => "Off: hit heights follow velocity",
        }
    }

    /// `source` with this switch flipped; the range or clamp is kept.
    pub fn flipped(self, source: &NotesSource) -> NotesSource {
        let mut source = source.clone();
        match self {
            Self::PitchRange => source.pitch_range = !source.pitch_range,
            Self::VelocityClamp => source.velocity_clamp = !source.velocity_clamp,
        }
        source
    }
}

/// A notes signal setting being dragged; previewed until release.
#[derive(Debug, Clone)]
pub struct NotesDrag {
    pub signal: SignalId,
    pub field: NotesField,
    pub origin: Point<Pixels>,
    pub original: NotesSource,
    pub preview: NotesSource,
}

/// The panel's left edge being dragged to resize it.
#[derive(Debug, Clone, Copy)]
pub struct ResizeDrag {
    pub origin_x: Pixels,
    pub original: f32,
}

/// The panel's width after dragging its left edge `dx` pixels (left widens).
pub fn resized(original: f32, dx: f32) -> f32 {
    (original - dx).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// A MIDI pitch as a note name, with middle C (60) as C3.
pub fn note_name(pitch: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    let octave = i32::from(pitch / 12) - 2;
    format!("{}{octave}", NAMES[usize::from(pitch % 12)])
}

/// A time in beats: "0", "0.25 beat", "1 beat", "1.5 beats".
pub fn format_beats(time: Ticks) -> String {
    let beats = time.as_beats();
    if beats == 0. {
        return "0".to_owned();
    }
    let number = format!("{beats:.2}");
    let number = number.trim_end_matches('0').trim_end_matches('.');
    if beats <= 1. {
        format!("{number} beat")
    } else {
        format!("{number} beats")
    }
}

fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.)
}

/// Times sit on their sliders by square root, so short times get most of
/// the travel.
fn time_position(time: Ticks) -> f64 {
    (time.as_beats() / MAX_TIME_BEATS).clamp(0., 1.).sqrt()
}

fn time_at(position: f64) -> Ticks {
    Ticks::from_beats_f64(position * position * MAX_TIME_BEATS)
}

/// The envelope of one full-velocity note, as a level (`0..=1`) over beats,
/// and how many beats to show: the note is held long enough to show the
/// sustain, then released.
fn envelope_shape(source: &NotesSource) -> (impl Fn(f64) -> f64 + 'static, f64) {
    let (attack, decay, release) = (
        source.attack.as_beats(),
        source.decay.as_beats(),
        source.release.as_beats(),
    );
    let hold = ((attack + decay + release) * 0.4).max(0.25);
    let held = attack + decay + hold;
    let signal = Signal {
        name: String::new(),
        color: 0,
        source: SignalSource::Notes(NotesSource {
            lowest: 0,
            highest: 127,
            velocity_min: 1.,
            velocity_max: 1.,
            ..source.clone()
        }),
    };
    let note = SignalNote {
        start: 0.,
        end: held,
        pitch: 60,
        velocity: 127,
    };
    let compiled = CompiledSignal::compile(&signal, [note]);
    // Signal values are centred: a hit runs from 0.5 up to 1.
    let level = move |beat: f64| 2. * compiled.sample(beat).0 - 1.;
    (level, held + release + hold * 0.25)
}

impl ProjectView {
    /// The selected signal, if the Signals page is showing and it still
    /// exists.
    fn inspected_signal(&self) -> Option<&SignalSnapshot> {
        let id = self
            .selected_signal
            .filter(|_| self.page == Page::Signals)?;
        self.session.snapshot().signals.iter().find(|s| s.id == id)
    }

    /// Whether the selection has anything to show in the inspector.
    fn has_inspector_content(&self) -> bool {
        self.inspected_signal()
            .is_some_and(|s| matches!(s.source, SignalSource::Notes(_)))
    }

    /// A notes signal's settings as shown: the preview while dragging.
    fn shown_notes(&self, id: SignalId, source: &NotesSource) -> NotesSource {
        match &self.notes_drag {
            Some(drag) if drag.signal == id => drag.preview.clone(),
            _ => source.clone(),
        }
    }

    /// Opens or closes the inspector.
    pub(crate) fn toggle_inspector(&mut self) {
        self.inspector_open = !self.inspector_open;
    }

    /// Follows an inspector drag; false if none is in progress.
    pub(crate) fn drag_inspector(&mut self, position: Point<Pixels>, fine: bool) -> bool {
        if let Some(drag) = self.inspector_resize {
            self.inspector_width = resized(drag.original, (position.x - drag.origin_x) / px(1.));
            return true;
        }
        if let Some(drag) = &mut self.notes_drag {
            let dx = (position.x - drag.origin.x) / px(1.);
            drag.preview = drag.field.dragged(&drag.original, dx, fine);
            return true;
        }
        false
    }

    /// Ends an inspector drag, sending a changed setting as one edit.
    pub(crate) fn finish_inspector_drag(&mut self) {
        self.inspector_resize = None;
        if let Some(drag) = self.notes_drag.take()
            && drag.preview != drag.original
        {
            self.session.apply(Command::Edit(Edit::SetNotesSource {
                id: drag.signal,
                source: drag.preview,
            }));
        }
    }

    /// The page with the inspector and its tray beside it.
    pub(crate) fn with_inspector(
        &self,
        page: AnyElement,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(div().flex_1().min_w_0().flex().flex_col().child(page))
            .when(self.inspector_open, |d| {
                d.child(self.inspector_panel(theme, cx))
            })
            .child(self.inspector_tray(theme, cx))
    }

    /// The folded tray: one button, lit when there is something to show.
    fn inspector_tray(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let content = self.has_inspector_content();
        let open = self.inspector_open;
        let color = if content {
            theme.accent
        } else {
            theme.text_tertiary
        };
        // One fixed text: a tooltip keeps the text it was shown with, so
        // one naming the state goes stale after a click.
        let hint = "Details of the selection";
        let button = small_button("inspector-toggle", theme)
            .relative()
            .when(open, |d| d.bg(theme.control))
            .child(icon(Icon::Panel, color, 14.))
            .when(content && !open, |d| {
                // A dot says "something here" while folded away.
                d.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(1.))
                        .size(px(8.))
                        .rounded(px(4.))
                        .border_2()
                        .border_color(theme.sidebar)
                        .bg(theme.accent),
                )
            })
            .tooltip(tooltip::text(hint))
            .on_click(cx.listener(|this, _, _, _| this.toggle_inspector()));
        div()
            .w(px(TRAY_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(6.))
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.edge)
            .child(button)
    }

    fn inspector_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let resizing = self.inspector_resize.is_some();
        let mut handle_color = theme.accent;
        handle_color.a = if resizing { 0.6 } else { 0. };
        let mut handle_hover = theme.accent;
        handle_hover.a = 0.35;
        let handle = div()
            .id("inspector-resize")
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(5.))
            // Inside the panel, and blocking presses from reaching the
            // page's scrollbar beside it.
            .occlude()
            .bg(handle_color)
            .hover(move |s| s.bg(handle_hover))
            .cursor_ew_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.inspector_resize = Some(ResizeDrag {
                        origin_x: event.position.x,
                        original: this.inspector_width,
                    });
                }),
            );
        let body = match self.inspected_signal() {
            Some(signal) => match &signal.source {
                SignalSource::Notes(source) => self
                    .notes_settings(signal, source, theme, cx)
                    .into_any_element(),
                SignalSource::Drawn(_) => empty(theme, "A drawn signal has no settings here yet."),
            },
            None => empty(theme, "Select a signal to see its settings."),
        };
        div()
            .w(px(self.inspector_width))
            .flex_none()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.edge)
            .child(
                div()
                    .id("inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(handle)
    }

    fn notes_settings(
        &self,
        signal: &SignalSnapshot,
        source: &NotesSource,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = signal.id;
        let source = self.shown_notes(id, source);
        let colors = theme.content(signal.color);
        let node = self.session.snapshot().node.name.clone();

        let header = div()
            .px(px(12.))
            .pt(px(12.))
            .pb(px(4.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().size(px(10.)).rounded(px(5.)).bg(colors.detail))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(signal.name.clone()),
                    ),
            )
            .child(
                div()
                    .pl(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(format!("Notes from {node}"))
                    .child(
                        small_button("inspector-source", theme)
                            .size(px(16.))
                            .child(icon(Icon::GoTo, theme.text_secondary, 11.))
                            .on_click(cx.listener(|this, _, _, _| this.go_to_node())),
                    ),
            );

        let pitches = group(theme).children(
            [NotesField::Lowest, NotesField::Highest]
                .into_iter()
                .enumerate()
                .map(|(i, field)| {
                    setting_row(field, i > 0, theme)
                        .child(div().flex_1())
                        .child(self.scrub_value(id, field, &source, theme, cx))
                }),
        );

        let (level, span) = envelope_shape(&source);
        let line = colors.detail;
        let mut area = colors.detail;
        area.a = 0.18;
        let graph = div().h(px(ENVELOPE_HEIGHT)).mx(px(12.)).my(px(8.)).child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    let width = bounds.size.width / px(1.);
                    let ppb = width / span as f32;
                    paint_curve(window, bounds, ppb, line, Some((area, 0.)), &level);
                },
            )
            .size_full(),
        );
        let envelope = group(theme).child(graph).children(
            [
                NotesField::Attack,
                NotesField::Decay,
                NotesField::Sustain,
                NotesField::Release,
            ]
            .into_iter()
            .map(|field| self.slider_row(id, field, &source, true, theme, cx)),
        );

        let velocity = group(theme).children(
            [NotesField::VelocityMin, NotesField::VelocityMax]
                .into_iter()
                .enumerate()
                .map(|(i, field)| self.slider_row(id, field, &source, i > 0, theme, cx)),
        );

        let (ranged, clamped) = (source.pitch_range, source.velocity_clamp);
        div()
            .flex()
            .flex_col()
            .pb(px(16.))
            .child(header)
            .child(section_label("Envelope", theme))
            .child(envelope)
            .child(self.switched_label(id, NotesSwitch::PitchRange, ranged, theme, cx))
            .when(ranged, |d| d.child(pitches))
            .child(hint(
                if ranged {
                    "Only notes in this range hit. Pick one drum to follow it."
                } else {
                    "Every note hits."
                },
                theme,
            ))
            .child(self.switched_label(id, NotesSwitch::VelocityClamp, clamped, theme, cx))
            .when(clamped, |d| d.child(velocity))
            .child(hint(
                if clamped {
                    "Each hit rises as high as its note's velocity, kept between Min and Max."
                } else {
                    "Each hit rises as high as its note's velocity."
                },
                theme,
            ))
    }

    /// A section heading with an on/off switch at its right. Off folds the
    /// section away; its values are kept for when it is switched back on.
    fn switched_label(
        &self,
        id: SignalId,
        switch: NotesSwitch,
        on: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let track = if on { theme.active } else { theme.control };
        let knob = div()
            .absolute()
            .top(px(2.))
            .left(px(if on { 13. } else { 2. }))
            .size(px(11.))
            .rounded(px(5.5))
            .bg(theme.on_accent)
            .shadow_sm();
        let toggle = div()
            .id(("notes-switch", switch as usize))
            .relative()
            .w(px(26.))
            .h(px(15.))
            .flex_none()
            .rounded(px(7.5))
            .bg(track)
            .cursor_pointer()
            .child(knob)
            .tooltip(tooltip::text(switch.tooltip(on)))
            .on_click(cx.listener(move |this, _, _, _| this.flip_notes_switch(id, switch)));
        section_label(switch.label(), theme)
            .pr(px(14.))
            .flex()
            .items_center()
            .justify_between()
            .child(toggle)
    }

    /// Turns a notes signal's pitch range or velocity clamp on or off, as
    /// one edit.
    fn flip_notes_switch(&mut self, id: SignalId, switch: NotesSwitch) {
        let Some(SignalSource::Notes(source)) = self
            .session
            .snapshot()
            .signals
            .iter()
            .find(|s| s.id == id)
            .map(|s| &s.source)
        else {
            return;
        };
        let source = switch.flipped(source);
        self.session
            .apply(Command::Edit(Edit::SetNotesSource { id, source }));
    }

    /// A row with the shared slider and the value beside it.
    fn slider_row(
        &self,
        id: SignalId,
        field: NotesField,
        source: &NotesSource,
        separated: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = self
            .notes_drag
            .as_ref()
            .is_some_and(|d| d.signal == id && d.field == field);
        let original = source.clone();
        let slider = slider::slider(
            ("notes-slider", field as usize),
            Axis::Horizontal,
            field.position(source) as f32,
            slider_length(self.inspector_width),
            active,
            theme,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.notes_drag = Some(NotesDrag {
                    signal: id,
                    field,
                    origin: event.position,
                    original: original.clone(),
                    preview: original.clone(),
                });
            }),
        );
        setting_row(field, separated, theme)
            .gap(px(SLIDER_GAP))
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
                    .child(field.format(source)),
            )
    }

    /// A value changed by dragging it sideways, like the block settings.
    fn scrub_value(
        &self,
        id: SignalId,
        field: NotesField,
        source: &NotesSource,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = theme.hover;
        let original = source.clone();
        div()
            .id(("notes-value", field as usize))
            .min_w(px(VALUE_WIDTH))
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(theme.control)
            .hover(move |s| s.bg(hover))
            .cursor_ew_resize()
            .flex()
            .justify_end()
            .font_family(MONO_FONT)
            .text_size(px(11.))
            .child(field.format(source))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.notes_drag = Some(NotesDrag {
                        signal: id,
                        field,
                        origin: event.position,
                        original: original.clone(),
                        preview: original.clone(),
                    });
                }),
            )
    }
}

/// How long a setting's slider is in a panel `width` wide: what the row
/// leaves after the group's margins and border, the row's padding, the
/// label, the value and the gaps between them.
fn slider_length(width: f32) -> f32 {
    const FIXED: f32 =
        2. * GROUP_MARGIN + 2. + 2. * ROW_PADDING + LABEL_WIDTH + VALUE_WIDTH + 2. * SLIDER_GAP;
    (width - FIXED).max(40.)
}

fn empty(theme: &Theme, text: &'static str) -> AnyElement {
    div()
        .p(px(16.))
        .text_size(px(12.))
        .text_color(theme.text_tertiary)
        .child(text)
        .into_any_element()
}

/// A heading above a group.
fn section_label(text: &'static str, theme: &Theme) -> Div {
    div()
        .px(px(14.))
        .pt(px(14.))
        .pb(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_tertiary)
        .child(text)
}

/// A rounded inset panel of rows.
fn group(theme: &Theme) -> Div {
    div()
        .mx(px(GROUP_MARGIN))
        .flex()
        .flex_col()
        .rounded(px(10.))
        .bg(theme.group)
        .border_1()
        .border_color(theme.hairline)
        .overflow_hidden()
}

/// A row in a group, labelled; `separated` draws the hairline above it.
fn setting_row(field: NotesField, separated: bool, theme: &Theme) -> Div {
    div()
        .min_h(px(34.))
        .px(px(ROW_PADDING))
        .flex()
        .items_center()
        .gap(px(8.))
        .when(separated, |d| d.border_t_1().border_color(theme.hairline))
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_none()
                .text_size(px(12.))
                .child(field.label()),
        )
}

fn hint(text: &'static str, theme: &Theme) -> Div {
    div()
        .px(px(14.))
        .pt(px(6.))
        .text_size(px(11.))
        .text_color(theme.text_tertiary)
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_named_with_middle_c_as_c3() {
        assert_eq!(note_name(60), "C3");
        assert_eq!(note_name(36), "C1");
        assert_eq!(note_name(0), "C-2");
        assert_eq!(note_name(127), "G8");
        assert_eq!(note_name(37), "C♯1");
    }

    #[test]
    fn times_read_in_beats() {
        assert_eq!(format_beats(Ticks::ZERO), "0");
        assert_eq!(format_beats(Ticks(240)), "0.25 beat");
        assert_eq!(format_beats(Ticks(960)), "1 beat");
        assert_eq!(format_beats(Ticks(1440)), "1.5 beats");
    }

    #[test]
    fn time_sliders_round_trip_and_favour_short_times() {
        for ticks in [0, 120, 480, 960, 3840] {
            assert_eq!(time_at(time_position(Ticks(ticks))), Ticks(ticks));
        }
        assert!(time_position(Ticks(960)) > 0.4, "one beat is half way");
    }

    #[test]
    fn ranges_stay_valid_while_dragging() {
        let source = NotesSource {
            lowest: 36,
            highest: 38,
            velocity_min: 0.2,
            velocity_max: 0.6,
            ..NotesSource::new()
        };
        let lowest = NotesField::Lowest.dragged(&source, 600., false);
        assert_eq!(lowest.lowest, 38, "stops at the highest");
        let highest = NotesField::Highest.dragged(&source, -600., false);
        assert_eq!(highest.highest, 36, "stops at the lowest");
        let up = NotesField::Highest.dragged(&source, 4. * PIXELS_PER_SEMITONE, false);
        assert_eq!(up.highest, 42);
        let min = NotesField::VelocityMin.dragged(&source, 500., false);
        assert!((min.velocity_min - 0.6).abs() < 1e-9, "stops at the max");
        let max = NotesField::VelocityMax.dragged(&source, -500., false);
        assert!((max.velocity_max - 0.2).abs() < 1e-9, "stops at the min");
        let sustain = NotesField::Sustain.dragged(&source, 100., false);
        assert!((sustain.sustain - 0.5).abs() < 1e-9);
    }

    #[test]
    fn sliders_fill_what_the_row_leaves() {
        assert_eq!(
            slider_length(DEFAULT_WIDTH + 100.) - slider_length(DEFAULT_WIDTH),
            100.
        );
        assert!(slider_length(MIN_WIDTH) >= 40.);
    }

    #[test]
    fn switches_flip_and_keep_their_values() {
        let source = NotesSource {
            lowest: 36,
            highest: 36,
            ..NotesSource::new()
        };
        let on = NotesSwitch::PitchRange.flipped(&source);
        assert!(on.pitch_range && !on.velocity_clamp);
        assert_eq!((on.lowest, on.highest), (36, 36), "range kept");
        let off = NotesSwitch::PitchRange.flipped(&on);
        assert_eq!(off, source);
        assert!(NotesSwitch::VelocityClamp.flipped(&source).velocity_clamp);
    }

    #[test]
    fn the_panel_resizes_within_limits() {
        assert_eq!(resized(280., -40.), 320., "dragging left widens");
        assert_eq!(resized(280., 400.), MIN_WIDTH);
        assert_eq!(resized(280., -900.), MAX_WIDTH);
    }

    #[test]
    fn the_envelope_graph_shows_attack_decay_sustain_release() {
        let source = NotesSource {
            attack: Ticks(960),
            decay: Ticks(960),
            sustain: 0.5,
            release: Ticks(960),
            ..NotesSource::new()
        };
        let (level, span) = envelope_shape(&source);
        assert!((level(1.0) - 1.0).abs() < 1e-9, "peak after the attack");
        assert!((level(2.5) - 0.5).abs() < 1e-9, "sustaining");
        assert!(level(span - 0.01).abs() < 1e-9, "released by the end");
    }
}
