//! An audio signal's settings in the inspector: how its follower hears the
//! node's sound (`docs/design/signals.md`, From audio). Each setting is a
//! slider; double-click resets it. Dragging previews the line everywhere
//! the signal is drawn and sends one edit on release. A meter shows the
//! signal's level at the song position while it plays.

use crate::icons::{Icon, icon};
use crate::inspector::{
    SLIDER_GAP, VALUE_WIDTH, group, hint, labeled_row, section_label, slider_length,
};
use crate::live::{Live, PaneKind};
use crate::parameters::small_button;
use crate::project::ProjectView;
use crate::slider::{self, Axis};
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{AudioSource, Command, Edit, SignalId, SignalSnapshot};
use gpui::{
    Context, Div, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, div, prelude::*, px,
};
use std::rc::Rc;

/// An audio signal setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowField {
    Attack,
    Release,
    Floor,
    Ceiling,
}

const FIELDS: [FollowField; 4] = [
    FollowField::Attack,
    FollowField::Release,
    FollowField::Floor,
    FollowField::Ceiling,
];

/// The floor stays at least this far below the ceiling.
const MIN_RANGE_DB: f64 = 1.0;

impl FollowField {
    fn label(self) -> &'static str {
        match self {
            Self::Attack => "Attack",
            Self::Release => "Release",
            Self::Floor => "Floor",
            Self::Ceiling => "Ceiling",
        }
    }

    pub fn selector(self) -> &'static str {
        match self {
            Self::Attack => "follow-attack",
            Self::Release => "follow-release",
            Self::Floor => "follow-floor",
            Self::Ceiling => "follow-ceiling",
        }
    }

    fn value(self, source: &AudioSource) -> f64 {
        match self {
            Self::Attack => source.attack_ms,
            Self::Release => source.release_ms,
            Self::Floor => source.floor_db,
            Self::Ceiling => source.ceiling_db,
        }
    }

    fn format(self, source: &AudioSource) -> String {
        let value = self.value(source);
        match self {
            Self::Attack | Self::Release if value >= 1_000. => format!("{:.2} s", value / 1_000.),
            Self::Attack | Self::Release if value < 10. => format!("{value:.1} ms"),
            Self::Attack | Self::Release => format!("{value:.0} ms"),
            Self::Floor | Self::Ceiling if value > 0. => format!("+{value:.1} dB"),
            Self::Floor | Self::Ceiling => format!("{value:.1} dB"),
        }
    }

    /// Where its slider sits, `0..=1`. Times are spread evenly in log
    /// time, so short ones get room; levels evenly in dB.
    pub fn position(self, source: &AudioSource) -> f64 {
        let value = self.value(source);
        match self {
            Self::Attack | Self::Release => {
                (1. + value).ln() / (1. + AudioSource::MAX_TIME_MS).ln()
            }
            Self::Floor | Self::Ceiling => {
                (value - AudioSource::MIN_DB) / (AudioSource::MAX_DB - AudioSource::MIN_DB)
            }
        }
    }

    /// `source` with this setting where its slider is at `position`,
    /// rounded to a tenth of a millisecond under 10 ms (else a whole one),
    /// or half a decibel. The floor stays below the ceiling.
    pub fn at(self, source: &AudioSource, position: f64) -> AudioSource {
        let position = position.clamp(0., 1.);
        let mut out = source.clone();
        let time = || {
            let ms = (position * (1. + AudioSource::MAX_TIME_MS).ln()).exp() - 1.;
            let ms = if ms < 10. {
                (ms * 10.).round() / 10.
            } else {
                ms.round()
            };
            ms.clamp(0., AudioSource::MAX_TIME_MS)
        };
        let db = || {
            let db = AudioSource::MIN_DB + position * (AudioSource::MAX_DB - AudioSource::MIN_DB);
            (db * 2.).round() / 2.
        };
        match self {
            Self::Attack => out.attack_ms = time(),
            Self::Release => out.release_ms = time(),
            Self::Floor => {
                out.floor_db = db()
                    .min(source.ceiling_db - MIN_RANGE_DB)
                    .max(AudioSource::MIN_DB);
            }
            Self::Ceiling => {
                out.ceiling_db = db()
                    .max(source.floor_db + MIN_RANGE_DB)
                    .min(AudioSource::MAX_DB);
            }
        }
        out
    }

    /// `original` after dragging this setting's slider `dx` pixels on a
    /// slider `length` long.
    pub fn dragged(self, original: &AudioSource, dx: f32, length: f32, fine: bool) -> AudioSource {
        let position = slider::dragged(self.position(original), dx, length, fine);
        self.at(original, position)
    }

    /// `source` with this setting back where a new signal has it.
    pub fn reset(self, source: &AudioSource) -> AudioSource {
        let fresh = AudioSource::new(source.node);
        let mut out = source.clone();
        match self {
            Self::Attack => out.attack_ms = fresh.attack_ms,
            Self::Release => out.release_ms = fresh.release_ms,
            Self::Floor => out.floor_db = fresh.floor_db.min(source.ceiling_db - MIN_RANGE_DB),
            Self::Ceiling => out.ceiling_db = fresh.ceiling_db.max(source.floor_db + MIN_RANGE_DB),
        }
        out
    }
}

/// An audio signal setting being dragged; previewed until release.
#[derive(Debug, Clone)]
pub struct FollowDrag {
    pub signal: SignalId,
    pub field: FollowField,
    pub origin: Point<Pixels>,
    pub length: f32,
    pub original: AudioSource,
    pub preview: AudioSource,
}

impl FollowDrag {
    /// Follows the pointer at `position`.
    pub fn follow(&mut self, position: Point<Pixels>, fine: bool) {
        let dx = (position.x - self.origin.x) / px(1.);
        self.preview = self.field.dragged(&self.original, dx, self.length, fine);
    }

    /// The edit if let go now: the changed settings.
    pub fn edit(&self) -> Option<Edit> {
        (self.preview != self.original).then(|| Edit::SetAudioSource {
            id: self.signal,
            source: self.preview.clone(),
        })
    }
}

impl ProjectView {
    /// An audio signal's settings as shown: the preview while dragging.
    pub(crate) fn shown_follow(&self, id: SignalId, source: &AudioSource) -> AudioSource {
        match &self.follow_drag {
            Some(drag) if drag.signal == id => drag.preview.clone(),
            _ => source.clone(),
        }
    }

    pub(crate) fn follow_settings(
        &self,
        signal: &SignalSnapshot,
        source: &AudioSource,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = signal.id;
        let source = self.shown_follow(id, source);
        let colors = theme.content(signal.color);
        let node_id = source.node;
        let from = self.session.snapshot().node(node_id).map_or_else(
            || "Audio from a removed node".to_owned(),
            |n| format!("Audio from {}", n.name),
        );
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
                    .child(from)
                    .child(
                        small_button("inspector-follow-source", theme)
                            .size(px(16.))
                            .child(icon(Icon::GoTo, theme.text_secondary, 11.))
                            .on_click(cx.listener(move |this, _, _, _| this.go_to_node(node_id))),
                    ),
            );
        let rows = FIELDS
            .into_iter()
            .enumerate()
            .map(|(i, field)| self.follow_row(id, field, &source, i > 0, theme, cx));
        // The signal's level at the song position, drawn by the live layer.
        let signal = self
            .compiled_signals()
            .get(id)
            .cloned()
            .unwrap_or(daw_automation::CompiledSignal::Silent);
        let meter = self
            .live_slot(
                PaneKind::Inspector,
                Live::Level {
                    signal: Rc::new(signal),
                    color: colors.detail,
                },
            )
            .flex_1()
            .h(px(6.));
        let level = labeled_row("Level", true, theme)
            .gap(px(SLIDER_GAP))
            .child(meter)
            .child(div().w(px(VALUE_WIDTH)).flex_none());
        div()
            .flex()
            .flex_col()
            .pb(px(16.))
            .child(header)
            .child(section_label("Follow", theme))
            .child(group(theme).children(rows).child(level))
            .child(hint(
                "Quieter than the Floor rests on the centre line; louder than the Ceiling is the \
                 top. Followed before the node's effects: to follow them too, put the node in a \
                 parent and follow the parent.",
                theme,
            ))
    }

    /// A setting on a slider with its value beside it. Double-click resets.
    fn follow_row(
        &self,
        id: SignalId,
        field: FollowField,
        source: &AudioSource,
        separated: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = self
            .follow_drag
            .as_ref()
            .is_some_and(|d| d.signal == id && d.field == field);
        let original = source.clone();
        let length = slider_length(self.inspector_width);
        let control = slider::slider(
            ("follow-slider", field as usize),
            Axis::Horizontal,
            field.position(source) as f32,
            length,
            active,
            theme,
        )
        .debug_selector(move || field.selector().into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                if event.click_count >= 2 {
                    this.session.apply(Command::Edit(Edit::SetAudioSource {
                        id,
                        source: field.reset(&original),
                    }));
                    return;
                }
                this.follow_drag = Some(FollowDrag {
                    signal: id,
                    field,
                    origin: event.position,
                    length,
                    original: original.clone(),
                    preview: original.clone(),
                });
            }),
        );
        labeled_row(field.label(), separated, theme)
            .gap(px(SLIDER_GAP))
            .child(control)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::NodeId;

    fn source() -> AudioSource {
        AudioSource::new(NodeId(5))
    }

    #[test]
    fn sliders_round_trip_their_values() {
        let source = source();
        for field in FIELDS {
            let back = field.at(&source, field.position(&source));
            assert_eq!(back, source, "{field:?}");
        }
    }

    #[test]
    fn times_spread_in_log_time_and_round_to_tidy_values() {
        let s = source();
        assert_eq!(FollowField::Attack.at(&s, 0.).attack_ms, 0.);
        assert_eq!(
            FollowField::Attack.at(&s, 1.).attack_ms,
            AudioSource::MAX_TIME_MS
        );
        let middle = FollowField::Release.at(&s, 0.5).release_ms;
        assert!(middle > 30. && middle < 60., "half way is short: {middle}");
        let short = FollowField::Attack.at(&s, 0.2).attack_ms;
        assert_eq!(short, (short * 10.).round() / 10.);
    }

    #[test]
    fn the_floor_stays_below_the_ceiling() {
        let s = source();
        assert_eq!(
            FollowField::Floor.at(&s, 1.).floor_db,
            s.ceiling_db - MIN_RANGE_DB
        );
        assert_eq!(
            FollowField::Ceiling.at(&s, 0.).ceiling_db,
            s.floor_db + MIN_RANGE_DB
        );
        assert_eq!(FollowField::Floor.at(&s, 0.).floor_db, AudioSource::MIN_DB);
        assert_eq!(
            FollowField::Ceiling.at(&s, 1.).ceiling_db,
            AudioSource::MAX_DB
        );
    }

    #[test]
    fn a_reset_puts_back_a_new_signals_setting() {
        let changed = AudioSource {
            attack_ms: 80.,
            floor_db: -20.,
            ..source()
        };
        assert_eq!(FollowField::Attack.reset(&changed).attack_ms, 5.);
        assert_eq!(FollowField::Floor.reset(&changed).floor_db, -48.);
    }

    #[test]
    fn values_read_as_units() {
        let s = AudioSource {
            attack_ms: 2.5,
            release_ms: 1500.,
            floor_db: -48.,
            ceiling_db: 6.,
            ..source()
        };
        assert_eq!(FollowField::Attack.format(&s), "2.5 ms");
        assert_eq!(FollowField::Release.format(&s), "1.50 s");
        assert_eq!(FollowField::Floor.format(&s), "-48.0 dB");
        assert_eq!(FollowField::Ceiling.format(&s), "+6.0 dB");
    }
}
