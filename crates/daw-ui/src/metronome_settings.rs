//! Preferences → Metronome: the click's sound, accent, pitch and volume
//! (`docs/design/metronome.md`). Kept with the preferences and sent to the
//! open project's engine as they change.

use crate::app::{AppView, Screen};
use crate::slider::{self, Axis};
use crate::theme::{MONO_FONT, Theme};
use daw_engine::{ClickSound, Command, Metronome};
use gpui::{
    AnyElement, Context, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, canvas, div, prelude::*, px,
};

/// The sliders' length.
const SLIDER_LENGTH: f32 = 200.;

/// A metronome setting with a slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slid {
    Pitch,
    Volume,
}

impl Slid {
    /// Where `metronome`'s value sits on the slider, `0..=1`.
    fn position(self, metronome: Metronome) -> f64 {
        match self {
            Self::Pitch => {
                let range = f64::from(Metronome::PITCH_RANGE);
                (f64::from(metronome.pitch) + range) / (2. * range)
            }
            Self::Volume => f64::from(metronome.volume) / f64::from(Metronome::MAX_VOLUME),
        }
    }

    /// `metronome` with this value at slider position `at`.
    fn at(self, metronome: Metronome, at: f64) -> Metronome {
        match self {
            Self::Pitch => {
                let range = f64::from(Metronome::PITCH_RANGE);
                Metronome {
                    pitch: (at * 2. * range - range).round() as i8,
                    ..metronome
                }
            }
            Self::Volume => Metronome {
                volume: (at * f64::from(Metronome::MAX_VOLUME)).round() as u8,
                ..metronome
            },
        }
    }

    /// The value as shown beside its slider.
    fn format(self, metronome: Metronome) -> String {
        match self {
            Self::Pitch if metronome.pitch > 0 => format!("+{} st", metronome.pitch),
            Self::Pitch => format!("{} st", metronome.pitch),
            Self::Volume => format!("{}%", metronome.volume),
        }
    }

    /// The setting a double-click puts back.
    fn reset(self, metronome: Metronome) -> Metronome {
        let default = Metronome::default();
        match self {
            Self::Pitch => Metronome {
                pitch: default.pitch,
                ..metronome
            },
            Self::Volume => Metronome {
                volume: default.volume,
                ..metronome
            },
        }
    }
}

/// A metronome slider being dragged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SettingDrag {
    slid: Slid,
    origin_x: Pixels,
    original: Metronome,
}

impl AppView {
    fn metronome_pref(&self) -> Metronome {
        self.preferences.preferences.metronome
    }

    /// Sets the metronome in the open project at once; saved unless
    /// `save` is false (a drag in progress, saved when it lands). With
    /// `hear`, one click plays to try it.
    pub(crate) fn set_metronome_pref(
        &mut self,
        metronome: Metronome,
        save: bool,
        hear: bool,
        cx: &mut Context<Self>,
    ) {
        let metronome = metronome.clamped();
        self.preferences.preferences.metronome = metronome;
        if save {
            self.save_preferences();
        }
        if let Screen::Project(project) = &self.screen {
            project.update(cx, |project, cx| {
                project.session.apply(Command::SetMetronome(metronome));
                if hear {
                    project.session.apply(Command::PreviewClick);
                }
                cx.notify();
            });
        }
        cx.notify();
    }

    fn drag_metronome_setting(&mut self, at: Point<Pixels>, fine: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.metronome_drag else {
            return;
        };
        let moved = (at.x - drag.origin_x) / px(1.);
        let position = slider::dragged(
            drag.slid.position(drag.original),
            moved,
            SLIDER_LENGTH,
            fine,
        );
        let metronome = drag.slid.at(drag.original, position);
        if metronome != self.metronome_pref() {
            self.set_metronome_pref(metronome, false, false, cx);
        }
    }

    /// Ends a slider drag: saved, and a pitch change plays a click to hear it.
    fn finish_metronome_setting(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.metronome_drag.take() {
            let metronome = self.metronome_pref();
            let changed = metronome != drag.original;
            self.set_metronome_pref(metronome, changed, changed && drag.slid == Slid::Pitch, cx);
        }
    }

    /// The Metronome page: sound, accent, pitch and volume.
    pub(crate) fn render_metronome_settings(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metronome = self.metronome_pref();
        let segments = ClickSound::ALL.map(|sound| {
            let selected = sound == metronome.sound;
            div()
                .id(sound.label())
                .debug_selector(move || format!("metronome-sound-{}", sound.label()))
                .flex_1()
                .flex()
                .justify_center()
                .py(px(3.))
                .rounded(px(5.))
                .when(selected, |d| d.bg(theme.control_selected))
                .text_color(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                })
                .child(sound.label())
                // Picking a sound plays it.
                .on_click(cx.listener(move |this, _, _, cx| {
                    let metronome = this.metronome_pref();
                    this.set_metronome_pref(Metronome { sound, ..metronome }, true, true, cx);
                }))
        });
        let sounds = div()
            .w(px(SLIDER_LENGTH + 60.))
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(7.))
            .bg(theme.control)
            .text_size(px(12.))
            .children(segments)
            .into_any_element();
        let accent = crate::switch::switch("metronome-accent", metronome.accent, theme)
            .debug_selector(|| "metronome-accent".into())
            .on_click(cx.listener(|this, _, _, cx| {
                let metronome = this.metronome_pref();
                this.set_metronome_pref(
                    Metronome {
                        accent: !metronome.accent,
                        ..metronome
                    },
                    true,
                    false,
                    cx,
                );
            }))
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .text_color(theme.text_tertiary)
                    .text_size(px(12.))
                    .child(
                        "A click on every beat while playing, switched on beside the transport. \
                         Heard over the output, never through the Song's channel, and never in exports.",
                    ),
            )
            .child(crate::app::setting_row(theme, "Sound", sounds))
            .child(crate::app::setting_row(theme, "Accent on beat 1", accent))
            .child(crate::app::setting_row(
                theme,
                "Pitch",
                self.metronome_slider(Slid::Pitch, metronome, theme, cx),
            ))
            .child(crate::app::setting_row(
                theme,
                "Volume",
                self.metronome_slider(Slid::Volume, metronome, theme, cx),
            ))
            .child(self.metronome_drag_tracker(cx))
    }

    fn metronome_slider(
        &self,
        slid: Slid,
        metronome: Metronome,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.metronome_drag.is_some_and(|d| d.slid == slid);
        let (id, selector) = match slid {
            Slid::Pitch => ("metronome-pitch", "metronome-pitch"),
            Slid::Volume => ("metronome-volume", "metronome-volume"),
        };
        let control = slider::slider(
            id,
            Axis::Horizontal,
            slid.position(metronome) as f32,
            SLIDER_LENGTH,
            active,
            theme,
        )
        .debug_selector(move || selector.into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                let metronome = this.metronome_pref();
                if event.click_count >= 2 {
                    // Double-click: back to the default.
                    this.metronome_drag = None;
                    let hear = slid == Slid::Pitch;
                    this.set_metronome_pref(slid.reset(metronome), true, hear, cx);
                    return;
                }
                this.metronome_drag = Some(SettingDrag {
                    slid,
                    origin_x: event.position.x,
                    original: metronome,
                });
                cx.notify();
            }),
        );
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(control)
            .child(
                div()
                    .w(px(48.))
                    .font_family(MONO_FONT)
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(slid.format(metronome)),
            )
            .into_any_element()
    }

    /// While a slider is held, follows the pointer anywhere in the window
    /// until it is let go.
    fn metronome_drag_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dragging = self.metronome_drag.is_some();
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| {
                            if event.pressed_button.is_none() {
                                this.finish_metronome_setting(cx);
                            } else {
                                this.drag_metronome_setting(
                                    event.position,
                                    event.modifiers.shift,
                                    cx,
                                );
                            }
                        });
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = view.update(cx, |this, cx| this.finish_metronome_setting(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_cover_each_settings_range() {
        let m = Metronome::default();
        assert_eq!(Slid::Pitch.at(m, 0.).pitch, -12);
        assert_eq!(Slid::Pitch.at(m, 1.).pitch, 12);
        assert_eq!(Slid::Pitch.at(m, 0.5).pitch, 0);
        assert_eq!(Slid::Volume.at(m, 1.).volume, 100);
        assert_eq!(Slid::Pitch.position(Metronome { pitch: 6, ..m }), 0.75);
        assert_eq!(Slid::Pitch.format(Metronome { pitch: 3, ..m }), "+3 st");
        assert_eq!(Slid::Pitch.format(Metronome { pitch: -3, ..m }), "-3 st");
        assert_eq!(Slid::Volume.format(m), "70%");
    }
}
