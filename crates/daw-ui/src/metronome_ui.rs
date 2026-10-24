//! The metronome in the transport: a button that switches the click on and
//! off, and a volume button whose vertical slider opens while the pointer
//! is over it (`docs/design/metronome.md`). The settings are the app's:
//! changes go to the engine at once and to the preferences when they land.

use crate::icons::{Icon, icon};
use crate::project::{ProjectEvent, ProjectView};
use crate::slider::{self, Axis};
use crate::theme::{MONO_FONT, Theme};
use crate::tooltip;
use daw_engine::{Command, Metronome};
use gpui::{
    Context, MouseButton, MouseDownEvent, Pixels, Point, anchored, deferred, div, point,
    prelude::*, px, relative,
};

/// The volume slider's length in the popover.
const SLIDER_LENGTH: f32 = 96.;
/// The popover's width.
const POPOVER_WIDTH: f32 = 44.;
/// The transparent strip between the button and the popover, part of the
/// popover: moving down into it never leaves both.
const BRIDGE: f32 = 4.;

/// Where the pointer is over the volume control: the popover shows while
/// it is over the button or the popover, or the slider is held.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct VolumeHover {
    pub button: bool,
    pub popover: bool,
}

/// The volume slider being dragged.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VolumeDrag {
    origin_y: Pixels,
    /// The settings when the drag began.
    original: Metronome,
    /// The level the drag has set (the snapshot shows it a frame later).
    volume: u8,
}

impl ProjectView {
    /// The metronome's settings as the engine has them.
    fn metronome(&self) -> Metronome {
        self.session.snapshot().metronome
    }

    /// Sets the metronome now; the app keeps it (`ProjectEvent::Metronome`)
    /// unless `keep` is false (a drag in progress, kept when it lands).
    pub(crate) fn set_metronome(&mut self, metronome: Metronome, keep: bool) {
        self.session.apply(Command::SetMetronome(metronome));
        if keep {
            self.metronome_to_keep = Some(metronome);
        }
    }

    /// Tells the app to keep the metronome's settings, once they changed.
    pub(crate) fn keep_metronome(&mut self, cx: &mut Context<Self>) {
        if let Some(metronome) = self.metronome_to_keep.take() {
            cx.emit(ProjectEvent::Metronome(metronome));
        }
    }

    /// Whether the volume popover shows.
    pub(crate) fn volume_open(&self) -> bool {
        self.volume_hover.button || self.volume_hover.popover || self.volume_drag.is_some()
    }

    /// Follows a volume drag; false if none is running.
    pub(crate) fn drag_metronome_volume(&mut self, position: Point<Pixels>, fine: bool) -> bool {
        let Some(drag) = &mut self.volume_drag else {
            return false;
        };
        let up = (drag.origin_y - position.y) / px(1.);
        let max = f64::from(Metronome::MAX_VOLUME);
        let start = f64::from(drag.original.volume) / max;
        let volume = (slider::dragged(start, up, SLIDER_LENGTH, fine) * max).round() as u8;
        if volume != drag.volume {
            drag.volume = volume;
            let metronome = Metronome {
                volume,
                ..drag.original
            };
            self.set_metronome(metronome, false);
        }
        true
    }

    /// Ends a volume drag: the app keeps the level it landed on.
    pub(crate) fn finish_metronome_drag(&mut self) {
        if let Some(drag) = self.volume_drag.take()
            && drag.volume != drag.original.volume
        {
            self.metronome_to_keep = Some(Metronome {
                volume: drag.volume,
                ..drag.original
            });
        }
    }

    /// The metronome's pill in the transport: on/off and volume.
    pub(crate) fn render_metronome(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metronome = self.metronome();
        let toggle = crate::header::transport_button("metronome", theme, metronome.on)
            .debug_selector(|| "transport-metronome".into())
            .tooltip(tooltip::text(if metronome.on {
                "Metronome on: a click on every beat while playing"
            } else {
                "Metronome off"
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                let metronome = this.metronome();
                this.set_metronome(
                    Metronome {
                        on: !metronome.on,
                        ..metronome
                    },
                    true,
                );
                this.keep_metronome(cx);
            }))
            .child(icon(
                Icon::Metronome,
                if metronome.on {
                    theme.accent
                } else {
                    theme.text_secondary
                },
                14.,
            ));
        let volume_icon = if metronome.volume == 0 {
            Icon::VolumeOff
        } else {
            Icon::Volume
        };
        let volume = crate::header::transport_button("metronome-volume", theme, self.volume_open())
            .debug_selector(|| "transport-metronome-volume".into())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.volume_hover.button = *hovered;
                cx.notify();
            }))
            .child(icon(volume_icon, theme.text_secondary, 13.));
        div()
            .relative()
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(theme.control)
            .child(toggle)
            .child(
                div()
                    .relative()
                    .child(volume)
                    .when(self.volume_open(), |d| {
                        d.child(self.volume_popover(metronome, theme, cx))
                    }),
            )
    }

    /// The popover under the volume button: a vertical slider and the level.
    fn volume_popover(
        &self,
        metronome: Metronome,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let level = f32::from(metronome.volume) / f32::from(Metronome::MAX_VOLUME);
        let dragging = self.volume_drag.is_some();
        let control = slider::slider(
            "metronome-volume-slider",
            Axis::Vertical,
            level,
            SLIDER_LENGTH,
            dragging,
            theme,
        )
        .debug_selector(|| "metronome-volume-slider".into())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let metronome = this.metronome();
                if event.click_count >= 2 {
                    // Double-click: back to the default level.
                    this.volume_drag = None;
                    this.set_metronome(
                        Metronome {
                            volume: Metronome::default().volume,
                            ..metronome
                        },
                        true,
                    );
                    this.keep_metronome(cx);
                    return;
                }
                this.volume_drag = Some(VolumeDrag {
                    origin_y: event.position.y,
                    original: metronome,
                    volume: metronome.volume,
                });
            }),
        );
        let panel = div()
            .w(px(POPOVER_WIDTH))
            .py(px(8.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .rounded(px(8.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_lg()
            .child(control)
            .child(
                div()
                    .font_family(MONO_FONT)
                    .text_size(px(10.))
                    .text_color(theme.text_secondary)
                    .child(format!("{}%", metronome.volume)),
            );
        // The bridge strip and the panel are one hover area.
        let popover = div()
            .id("metronome-volume-popover")
            .debug_selector(|| "metronome-volume-popover".into())
            .pt(px(BRIDGE))
            .occlude()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.volume_hover.popover = *hovered;
                cx.notify();
            }))
            .child(panel);
        // Centred under the button (32 px wide), hanging from its bottom.
        div().absolute().top(relative(1.)).left_0().child(
            deferred(
                anchored()
                    .offset(point(px((32. - POPOVER_WIDTH) / 2.), px(0.)))
                    .child(popover),
            )
            .with_priority(1),
        )
    }
}
