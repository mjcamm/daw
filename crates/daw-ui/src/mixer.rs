//! The Mixer page: every node as a strip in one flat, aligned row (each
//! parent before its children, the Song first), with the nesting map along
//! the bottom. See `docs/design/mixer.md`.
//!
//! Strip, top to bottom: effect slots, pan, fader with meters, the volume
//! in dB, mute and solo, colour and name. Every strip has the same parts at
//! the same heights; only the faders grow and shrink, all together, as the
//! map takes rows for deeper nesting (up to three, then it scrolls).

use crate::icons::{Icon, icon};
use crate::layout::meter_height;
use crate::menu::{self, OpenMenu};
use crate::motion::RowLayout;
use crate::parameters::{BaseDrag, small_button};
use crate::project::ProjectView;
use crate::scroll::{self, Extent, ScrollDrag};
use crate::slider::{self, Axis};
use crate::theme::Theme;
use crate::tooltip;
use crate::tree::{self, Bar, display_order};
use daw_engine::devices::{
    CHANNEL_PARAMS, INSTRUMENTS, channel, instrument_description, instrument_name,
};
use daw_engine::{
    Command, DeviceId, DeviceRef, Edit, Effect, EffectKind, Instrument, InstrumentKind, NodeId,
    NodeSnapshot, ParamId,
};
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, Pixels,
    ScrollHandle, canvas, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub const DEFAULT_STRIP_WIDTH: f32 = 76.;
const MIN_STRIP_WIDTH: f32 = 60.;
const MAX_STRIP_WIDTH: f32 = 140.;
/// The Song's strip is a little wider, set apart as the master.
const SONG_EXTRA: f32 = 10.;
const SLOT_HEIGHT: f32 = 20.;
const PAN_HEIGHT: f32 = 26.;
const READOUT_HEIGHT: f32 = 18.;
const BUTTONS_HEIGHT: f32 = 26.;
const NAME_HEIGHT: f32 = 30.;
const MAP_ROW: f32 = 19.;
const MAP_PAD: f32 = 4.;
/// Map rows that show before the map scrolls.
const MAP_VISIBLE_ROWS: usize = 3;
/// A pan within this of the centre snaps to it.
const PAN_DETENT: f64 = 0.02;
const METER_WIDTH: f32 = 4.;
/// How long a peak line holds before it falls.
const PEAK_HOLD: std::time::Duration = std::time::Duration::from_millis(1200);

/// The mixer's view state: strip width, scrolling, and where the faders
/// were last drawn.
pub struct MixerView {
    pub strip_width: f32,
    pub x: ScrollHandle,
    pub map_y: ScrollHandle,
    /// The first strip's fader area, to size every fader.
    fader_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The fader area of the strip in the side panel on Arrange.
    panel_fader_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where the horizontal scrollbar's track was drawn.
    track: Rc<Cell<Bounds<Pixels>>>,
    /// The map's height, easing as levels come and go.
    map_height: crate::motion::Eased,
    /// Each node's held peak and when it was reached.
    peaks: std::cell::RefCell<std::collections::HashMap<NodeId, (f32, Instant)>>,
}

impl Default for MixerView {
    fn default() -> Self {
        Self {
            strip_width: DEFAULT_STRIP_WIDTH,
            x: ScrollHandle::new(),
            map_y: ScrollHandle::new(),
            fader_bounds: Rc::default(),
            panel_fader_bounds: Rc::default(),
            track: Rc::default(),
            map_height: crate::motion::Eased::default(),
            peaks: std::cell::RefCell::default(),
        }
    }
}

/// Where a strip is drawn: on the mixer desk, or alone in the side panel
/// on Arrange (the same element, as wide as the panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripPlace {
    Mixer { first: bool },
    Panel,
}

/// A drag on the mixer other than a fader (faders use the resting-value
/// drag shared with Parameters).
#[derive(Debug, Clone, Copy)]
pub enum MixerDrag {
    Pan {
        param: ParamId,
        origin_x: Pixels,
        original: f64,
        preview: f64,
    },
    Width {
        origin_x: Pixels,
        original: f32,
    },
    Scroll(ScrollDrag),
}

/// A pan after a drag: snapped to the centre when close to it.
pub fn detented(pan: f64) -> f64 {
    if (pan - 0.5).abs() < PAN_DETENT {
        0.5
    } else {
        pan
    }
}

/// The strip width after dragging its slider `dx` pixels.
pub fn resized_strip(original: f32, dx: f32) -> f32 {
    (original + dx / 2.).clamp(MIN_STRIP_WIDTH, MAX_STRIP_WIDTH)
}

/// The map's height for `levels` rows: none without folders; at most
/// three show.
pub fn map_height(levels: usize) -> f32 {
    if levels == 0 {
        return 0.;
    }
    levels.min(MAP_VISIBLE_ROWS) as f32 * MAP_ROW + 2. * MAP_PAD
}

fn volume_id(node: NodeId) -> ParamId {
    ParamId {
        node,
        device: DeviceRef::Channel,
        index: channel::VOLUME,
    }
}

fn pan_id(node: NodeId) -> ParamId {
    ParamId {
        node,
        device: DeviceRef::Channel,
        index: channel::PAN,
    }
}

/// A channel parameter's resting value and current value at the playhead
/// (moved by its layers), both normalized.
fn channel_param(node: &NodeSnapshot, index: u32) -> Option<&daw_engine::ParamSnapshot> {
    node.devices
        .iter()
        .find(|d| d.device == DeviceRef::Channel)?
        .params
        .get(index as usize)
}

impl ProjectView {
    fn strip_width(&self, node: NodeId) -> f32 {
        self.mixer_view.strip_width + if node == NodeId::ROOT { SONG_EXTRA } else { 0. }
    }

    pub(crate) fn render_mixer(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.session.snapshot();
        let shown = display_order(snapshot, &self.folded);
        let layout = self
            .strip_motion
            .layout(&shown, self.mixer_view.strip_width, Instant::now());
        let placed: Vec<(&NodeSnapshot, &RowLayout)> = layout
            .iter()
            .filter_map(|l| Some((l.ghost.as_deref().or_else(|| snapshot.node(l.id))?, l)))
            .collect();
        let nodes: Vec<&NodeSnapshot> = placed.iter().map(|(n, _)| *n).collect();
        let bars = tree::bars(&nodes);
        let levels = tree::columns(&bars);
        // The most effects on any strip sets every strip's slot area.
        let slots = placed
            .iter()
            .map(|(n, _)| n.devices.iter().filter(|d| d.effect.is_some()).count())
            .max()
            .unwrap_or(0)
            + 1;
        // Room for the busiest MIDI strip's instruments and its "+"; none
        // when no strip takes MIDI.
        let instrument_slots = placed
            .iter()
            .filter(|(n, _)| n.takes_midi())
            .map(|(n, _)| n.devices.iter().filter(|d| d.instrument.is_some()).count() + 1)
            .max()
            .unwrap_or(0);

        // Where each strip is drawn this frame, so the map follows motion.
        let mut lefts = Vec::with_capacity(placed.len());
        let mut x = 0.;
        for (node, layout) in &placed {
            lefts.push(x + layout.offset);
            let full = self.strip_width(node.id);
            x += full * layout.height / self.mixer_view.strip_width;
        }
        let widths: Vec<f32> = placed
            .iter()
            .map(|(n, l)| self.strip_width(n.id) * l.height / self.mixer_view.strip_width)
            .collect();
        let content_width = x;

        let strips = placed
            .iter()
            .enumerate()
            .map(|(i, (node, layout))| {
                let place = StripPlace::Mixer { first: i == 0 };
                let strip = self.strip(node, instrument_slots, slots, place, theme, cx);
                animated_strip(layout, widths[i], self.strip_width(node.id), strip)
            })
            .collect::<Vec<_>>();
        let map = self.mixer_map(
            &placed,
            &bars,
            levels,
            &lefts,
            &widths,
            content_width,
            theme,
            cx,
        );

        let desk = scroll::axis_locked(div())
            .id("mixer-desk")
            .flex_1()
            .min_h_0()
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.mixer_view.x)
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(content_width))
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().flex().children(strips))
                    .child(map),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.window)
            .child(desk)
            .child(self.mixer_footer(theme, cx))
            .into_any_element()
    }

    /// One node's strip.
    #[allow(clippy::too_many_arguments, reason = "layout figures, passed through")]
    fn strip(
        &self,
        node: &NodeSnapshot,
        instrument_slots: usize,
        slots: usize,
        place: StripPlace,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = node.id;
        let song = id == NodeId::ROOT;
        let colors = theme.content(node.color);
        // In the panel the strip is the selection itself: no highlight.
        let selected = self.selected_node == Some(id) && place != StripPlace::Panel;
        let measure = match place {
            StripPlace::Mixer { first } => first.then(|| self.mixer_view.fader_bounds.clone()),
            StripPlace::Panel => Some(self.mixer_view.panel_fader_bounds.clone()),
        };
        let mut selected_bg = theme.accent;
        selected_bg.a = 0.12;

        let fader_area = div()
            .flex_1()
            .min_h_0()
            .flex()
            .justify_center()
            .gap(px(6.))
            .py(px(6.))
            .when_some(measure, |d, cell| {
                d.child(
                    canvas(move |bounds, _, _| cell.set(bounds), |_, (), _, _| {})
                        .absolute()
                        .size_full(),
                )
            })
            .relative()
            .child(self.fader(node, place, theme, cx))
            .child(self.meters(id, place, theme));

        let volume = channel_param(node, channel::VOLUME);
        let base = volume.map_or(0., |p| self.fader_value(id, p.base));
        let info = &CHANNEL_PARAMS[channel::VOLUME as usize];
        let readout = div()
            .h(px(READOUT_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .font_family(crate::theme::MONO_FONT)
            .text_size(px(10.))
            .text_color(theme.text_secondary)
            .child(info.format(info.scale.to_plain(base)));

        let toggle = |name: &'static str, label: &'static str, on: bool, on_bg, on_fg| {
            div()
                .id((name, id.0))
                .w(px(22.))
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
        let buttons = div()
            .h(px(BUTTONS_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .child(
                toggle("strip-mute", "M", muted, theme.warning, theme.on_warning).on_click(
                    cx.listener(move |this, _, _, _| {
                        this.session.apply(Command::Edit(Edit::SetMute {
                            node: id,
                            muted: !muted,
                        }));
                    }),
                ),
            )
            .child(
                toggle("strip-solo", "S", soloed, theme.accent, theme.on_accent).on_click(
                    cx.listener(move |this, _, _, _| {
                        this.session.apply(Command::Edit(Edit::SetSolo {
                            node: id,
                            soloed: !soloed,
                        }));
                    }),
                ),
            );

        let name = div()
            .h(px(NAME_HEIGHT))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(4.))
            .px(px(6.))
            .child(div().h(px(3.)).rounded(px(2.)).bg(colors.detail))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .text_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(node.name.clone()),
            );

        div()
            .id(("strip", id.0))
            .size_full()
            .flex()
            .flex_col()
            .bg(if song { theme.sidebar } else { theme.window })
            .border_r_1()
            .border_color(theme.hairline)
            .when(song, |d| d.border_l_1().border_color(theme.edge))
            .when(selected, |d| d.bg(selected_bg))
            .on_click(cx.listener(move |this, _, _, _| this.selected_node = Some(id)))
            .child(self.instrument_slots(node, instrument_slots, theme, cx))
            .child(self.effect_slots(node, slots, theme, cx))
            .child(self.pan(node, place, theme, cx))
            .child(fader_area)
            .child(readout)
            .child(buttons)
            .child(name)
    }

    /// One instrument or effect in a strip: on/off dot, name, and when
    /// selected a remove icon. Clicking it selects the node and the device.
    fn slot_row(
        &self,
        node: NodeId,
        device: &daw_engine::DeviceSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (id, remove_edit) = match device.device {
            DeviceRef::Effect(id) => (id, Edit::RemoveEffect { id }),
            DeviceRef::Instrument(id) => (id, Edit::RemoveInstrument { id }),
            DeviceRef::Channel => return None,
        };
        let hover = theme.hover;
        let enabled = device.enabled;
        let selected = self.selected_effect == Some(id);
        // A SingleSampler shows its sample and opens its panel.
        let sampler = device.instrument == Some(InstrumentKind::SingleSampler);
        let sample_name = device.sample.and_then(|s| {
            self.session
                .snapshot()
                .samples
                .iter()
                .find(|x| x.id == s)
                .map(|x| x.name.clone())
        });
        let label = if sampler {
            sample_name.unwrap_or_else(|| "No sample".to_owned())
        } else {
            device.name.clone()
        };
        let panel_open = self.open_menu == Some(OpenMenu::Sampler(id));
        let mut selected_bg = theme.accent;
        selected_bg.a = 0.18;
        let dot = div()
            .id(("slot-switch", id.0))
            .size(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(6.)).rounded(px(3.)).bg(if enabled {
                theme.active
            } else {
                theme.text_tertiary
            }))
            .tooltip(tooltip::text(if enabled {
                "On: click to switch off"
            } else {
                "Off: click to switch on"
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.session.apply(Command::Edit(Edit::SetDeviceEnabled {
                    id,
                    enabled: !enabled,
                }));
            }));
        let remove = selected.then(|| {
            small_button(("slot-remove", id.0), theme)
                .size(px(14.))
                .child(icon(Icon::Close, theme.text_secondary, 10.))
                .tooltip(tooltip::text("Remove"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.selected_effect = None;
                    this.session.apply(Command::Edit(remove_edit.clone()));
                }))
        });
        Some(
            div()
                .id(("slot", id.0))
                .h(px(SLOT_HEIGHT - 2.))
                .mx(px(4.))
                .px(px(2.))
                .flex()
                .items_center()
                .gap(px(2.))
                .rounded(px(4.))
                .bg(if selected { selected_bg } else { theme.control })
                .hover(move |s| s.bg(hover))
                .text_size(px(10.))
                .text_color(if enabled {
                    theme.text
                } else {
                    theme.text_tertiary
                })
                .child(dot)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .children(remove)
                .when(sampler && !panel_open, |d| {
                    d.tooltip(tooltip::text("SingleSampler: click to choose its sample"))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.selected_node = Some(node);
                    this.selected_effect = Some(id);
                    if sampler {
                        this.toggle_menu(OpenMenu::Sampler(id));
                    }
                }))
                .into_any_element(),
        )
        .map(|slot| {
            if panel_open {
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(slot)
                    .child(menu::float(self.sampler_panel(
                        id,
                        device.sample,
                        device.trigger.unwrap_or(daw_engine::DEFAULT_TRIGGER_NOTE),
                        theme,
                        cx,
                    )))
                    .into_any_element()
            } else {
                slot
            }
        })
    }

    /// A SingleSampler's panel: its sample (with how long it is, or why it
    /// is missing) and a button to choose another.
    fn sampler_panel(
        &self,
        id: DeviceId,
        sample: Option<daw_engine::SampleId>,
        trigger: u8,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let snapshot = self.session.snapshot();
        let found = sample.and_then(|s| snapshot.samples.iter().find(|x| x.id == s));
        let (title, detail) = match found {
            Some(s) => (
                s.name.clone(),
                match &s.status {
                    daw_engine::SampleStatus::Loaded {
                        seconds,
                        channels,
                        sample_rate,
                    } => format!(
                        "{seconds:.2} s · {} · {:.1} kHz",
                        if *channels == 1 { "mono" } else { "stereo" },
                        f64::from(*sample_rate) / 1000.
                    ),
                    daw_engine::SampleStatus::Missing { reason } => format!("Missing: {reason}"),
                },
            ),
            None => (
                "No sample".to_owned(),
                "Choose a WAV file to play on every note.".to_owned(),
            ),
        };
        let hover = theme.hover;
        let error = self.sample_error.clone();
        menu::panel(theme)
            .w(px(240.))
            .p(px(10.))
            .gap(px(6.))
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("SINGLESAMPLER"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(detail),
            )
            .child({
                let step = |name: &'static str, glyph: &'static str, delta: i16| {
                    let note = (i16::from(trigger) + delta).clamp(0, 127) as u8;
                    div()
                        .id((name, id.0))
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(theme.control)
                        .hover(move |s| s.bg(hover))
                        .child(glyph)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.session
                                .apply(Command::Edit(Edit::SetInstrumentNote { id, note }));
                        }))
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text_secondary)
                            .child("Plays note"),
                    )
                    .child(step("note-down", "−", -1))
                    .child(
                        div()
                            .w(px(40.))
                            .flex()
                            .justify_center()
                            .child(crate::inspector::note_name(trigger)),
                    )
                    .child(step("note-up", "+", 1))
            })
            .children(error.map(|e| div().text_size(px(11.)).text_color(theme.record).child(e)))
            .child(
                div()
                    .id(("choose-sample", id.0))
                    .mt(px(4.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(theme.control)
                    .hover(move |s| s.bg(hover))
                    .text_size(px(12.))
                    .child("Choose sample…")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.choose_sample(id, cx);
                    })),
            )
    }

    /// Asks for an audio file, then copies it into the project and plays
    /// it on the SingleSampler.
    fn choose_sample(&mut self, id: DeviceId, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose sample".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.sample_error = this.session.import_sample(id, &path).err();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The dashed "+" at the end of a slot list, with its menu when open.
    #[allow(
        clippy::too_many_arguments,
        reason = "the slot's names and menu, passed through"
    )]
    fn add_slot(
        &self,
        name: &'static str,
        text: &'static str,
        node: NodeId,
        menu: OpenMenu,
        hint: &'static str,
        panel: impl FnOnce(&mut Context<Self>) -> gpui::Div,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let hover = theme.hover;
        let open = self.open_menu == Some(menu);
        div()
            .relative()
            .flex()
            .flex_col()
            .mx(px(4.))
            .child(
                div()
                    .id((name, node.0))
                    .h(px(SLOT_HEIGHT - 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(3.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.hairline)
                    .hover(move |s| s.bg(hover))
                    .when(open, |d| d.bg(theme.control))
                    .text_size(px(10.))
                    .text_color(theme.text_tertiary)
                    .child(icon(Icon::Plus, theme.text_tertiary, 11.))
                    .child(text)
                    .when(!open, |d| d.tooltip(tooltip::text(hint)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.toggle_menu(menu);
                    })),
            )
            .when(open, |d| d.child(menu::float(panel(cx))))
    }

    /// A MIDI node's instruments, layered (each plays every note), then "+"
    /// to open the instrument browser. Other strips leave the space empty,
    /// so every strip stays aligned.
    fn instrument_slots(
        &self,
        node: &NodeSnapshot,
        slots: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let area = div()
            .h(px(slots as f32 * SLOT_HEIGHT + 6.))
            .pt(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.));
        if slots == 0 || !node.takes_midi() {
            return area;
        }
        let id = node.id;
        let instruments: Vec<AnyElement> = node
            .devices
            .iter()
            .filter(|d| d.instrument.is_some())
            .filter_map(|d| self.slot_row(id, d, theme, cx))
            .collect();
        let browser = move |cx: &mut Context<Self>| self.instrument_browser(id, theme, cx);
        let add = self.add_slot(
            "instrument-add",
            "Instrument",
            id,
            OpenMenu::InstrumentBrowser(id),
            "Add an instrument (they layer: each plays every note)",
            browser,
            theme,
            cx,
        );
        area.children(instruments).child(add)
    }

    /// The instrument browser: built-in instruments with a line on each;
    /// plugins come later. Choosing one adds it as a new layer.
    fn instrument_browser(&self, node: NodeId, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let hover = theme.hover;
        let label = |text: &'static str| {
            div()
                .px(px(10.))
                .pt(px(8.))
                .pb(px(4.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_tertiary)
                .child(text)
        };
        let builtin = INSTRUMENTS.iter().map(|&kind| {
            div()
                .id(("browse-instrument", kind as usize))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .hover(move |s| s.bg(hover))
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(div().text_size(px(13.)).child(instrument_name(kind)))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_secondary)
                        .child(instrument_description(kind)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.add_instrument(node, kind);
                }))
        });
        menu::panel(theme)
            .w(px(260.))
            .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
            .child(label("BUILT-IN"))
            .children(builtin.collect::<Vec<_>>())
            .child(menu::separator(theme))
            .child(label("PLUGINS"))
            .child(
                div()
                    .px(px(10.))
                    .pb(px(8.))
                    .text_size(px(11.))
                    .text_color(theme.text_tertiary)
                    .child("VST3 and CLAP instruments come in a later phase."),
            )
    }

    /// Adds an instrument as the node's last layer and selects it.
    fn add_instrument(&mut self, node: NodeId, kind: InstrumentKind) {
        self.open_menu = None;
        let snapshot = self.session.snapshot();
        let id = crate::tree::next_device_id(snapshot);
        let index = snapshot.node(node).map_or(0, |n| {
            n.devices.iter().filter(|d| d.instrument.is_some()).count()
        });
        let mut instrument = Instrument::new(kind);
        if kind == InstrumentKind::SingleSampler {
            instrument.note = Some(crate::tree::next_trigger_note(snapshot, node));
        }
        self.session.apply(Command::Edit(Edit::AddInstrument {
            node,
            id,
            index,
            instrument,
        }));
        self.selected_node = Some(node);
        self.selected_effect = Some(id);
    }

    /// The node's effect slots: one row per effect, then "+" to add.
    fn effect_slots(
        &self,
        node: &NodeSnapshot,
        slots: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let id = node.id;
        let effects: Vec<AnyElement> = node
            .devices
            .iter()
            .filter(|d| d.effect.is_some())
            .filter_map(|d| self.slot_row(id, d, theme, cx))
            .collect();
        let effect_menu = move |cx: &mut Context<Self>| {
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .child(
                    menu::item(theme, "add-filter", "Filter", "").on_click(cx.listener(
                        move |this, _, _, _| {
                            this.add_effect(id, EffectKind::Filter);
                        },
                    )),
                )
        };
        let add = self.add_slot(
            "slot-add",
            "Effect",
            id,
            OpenMenu::AddEffect(id),
            "Add an effect",
            effect_menu,
            theme,
            cx,
        );
        div()
            .h(px(slots as f32 * SLOT_HEIGHT + 6.))
            .pt(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .border_t_1()
            .border_color(theme.hairline)
            .children(effects)
            .child(add)
    }

    /// The node's strip, alone, for the side panel on Arrange.
    pub(crate) fn panel_strip(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = |instrument: bool| {
            node.devices
                .iter()
                .filter(|d| {
                    if instrument {
                        d.instrument.is_some()
                    } else {
                        d.effect.is_some()
                    }
                })
                .count()
        };
        let instrument_slots = if node.takes_midi() {
            count(true) + 1
        } else {
            0
        };
        self.strip(
            node,
            instrument_slots,
            count(false) + 1,
            StripPlace::Panel,
            theme,
            cx,
        )
        .into_any_element()
    }

    /// Adds an effect at the end of a node's chain and selects it.
    fn add_effect(&mut self, node: NodeId, kind: EffectKind) {
        self.open_menu = None;
        let snapshot = self.session.snapshot();
        let id = crate::tree::next_device_id(snapshot);
        let index = snapshot.node(node).map_or(0, |n| {
            n.devices.iter().filter(|d| d.effect.is_some()).count()
        });
        self.session.apply(Command::Edit(Edit::AddEffect {
            node,
            id,
            index,
            effect: Effect::new(kind),
        }));
        self.selected_node = Some(node);
        self.selected_effect = Some(id);
    }

    /// The pan slider: centre-detented, double-click to centre.
    fn pan(
        &self,
        node: &NodeSnapshot,
        place: StripPlace,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let param = pan_id(node.id);
        let resting = channel_param(node, channel::PAN).map_or(0.5, |p| p.base);
        let shown = match self.mixer_drag {
            Some(MixerDrag::Pan {
                param: p, preview, ..
            }) if p == param => preview,
            _ => resting,
        };
        let active = matches!(self.mixer_drag, Some(MixerDrag::Pan { param: p, .. }) if p == param);
        let width = match place {
            StripPlace::Mixer { .. } => self.strip_width(node.id),
            StripPlace::Panel => self.inspector_width.min(220.),
        };
        let length = (width - 16.).max(30.);
        let centre = CHANNEL_PARAMS[channel::PAN as usize].default_normalized();
        div()
            .h(px(PAN_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .child(
                slider::slider(
                    ("pan", node.id.0),
                    Axis::Horizontal,
                    shown as f32,
                    length,
                    active,
                    theme,
                )
                .tooltip(tooltip::text("Pan · double-click to centre"))
                .on_click(cx.listener(move |this, event: &ClickEvent, _, _| {
                    if event.click_count() >= 2 {
                        this.session.apply(Command::Edit(Edit::SetParam {
                            param,
                            value: centre,
                        }));
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.mixer_drag = Some(MixerDrag::Pan {
                            param,
                            origin_x: event.position.x,
                            original: resting,
                            preview: resting,
                        });
                    }),
                ),
            )
    }

    /// A volume's resting value as shown: the fader's preview while dragged.
    fn fader_value(&self, node: NodeId, base: f64) -> f64 {
        self.base_preview(volume_id(node)).unwrap_or(base)
    }

    /// The fader: the volume's resting value, with a marker where layers
    /// have moved it at the playhead. Double-click for 0 dB.
    /// How long faders and meters are where `place` draws them.
    fn fader_length(&self, place: StripPlace) -> f32 {
        let cell = match place {
            StripPlace::Mixer { .. } => &self.mixer_view.fader_bounds,
            StripPlace::Panel => &self.mixer_view.panel_fader_bounds,
        };
        (cell.get().size.height / px(1.) - 12.).max(40.)
    }

    fn fader(
        &self,
        node: &NodeSnapshot,
        place: StripPlace,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(volume) = channel_param(node, channel::VOLUME) else {
            return div().into_any_element();
        };
        let param = volume.id;
        let resting = volume.base;
        let shown = self.fader_value(node.id, resting);
        let length = self.fader_length(place);
        let active = self.base_preview(param).is_some();
        let unity = CHANNEL_PARAMS[channel::VOLUME as usize].default_normalized();
        // Where the layers have it now, if they move it.
        let live = (!volume.layers.is_empty()).then(|| {
            daw_automation::Automation::compile_layers_with(
                shown,
                &volume.layers,
                &self.compiled_signals(),
            )
            .value_at(self.session.position_beats())
        });
        let marker = live.map(|value| {
            let travel = length - 9.;
            div()
                .absolute()
                .left(px(-3.))
                .w(px(slider::THICKNESS + 6.))
                .h(px(2.))
                .rounded(px(1.))
                .bottom(px(4.5 + value.clamp(0., 1.) as f32 * travel - 1.))
                .bg(theme.warning)
        });
        div()
            .relative()
            .child(
                slider::slider(
                    ("fader", node.id.0),
                    Axis::Vertical,
                    shown as f32,
                    length,
                    active,
                    theme,
                )
                .tooltip(tooltip::text("Volume · double-click for 0 dB"))
                .on_click(cx.listener(move |this, event: &ClickEvent, _, _| {
                    if event.click_count() >= 2 {
                        this.session.apply(Command::Edit(Edit::SetParam {
                            param,
                            value: unity,
                        }));
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.base_drag = Some(BaseDrag::new(param, event.position.y, resting));
                    }),
                ),
            )
            .children(marker)
            .into_any_element()
    }

    /// Stereo meters with a peak line that holds, then falls.
    fn meters(&self, node: NodeId, place: StripPlace, theme: &Theme) -> gpui::Div {
        let meter = self.session.meter(node);
        let length = self.fader_length(place);
        let level = meter.left.max(meter.right);
        let held = {
            let mut peaks = self.mixer_view.peaks.borrow_mut();
            let entry = peaks.entry(node).or_insert((0., Instant::now()));
            if level >= entry.0 || entry.1.elapsed() > PEAK_HOLD {
                *entry = (level, Instant::now());
            }
            entry.0
        };
        let bar =
            |peak: f32| {
                let height = meter_height(peak);
                div()
                    .w(px(METER_WIDTH))
                    .h(px(length))
                    .rounded(px(1.))
                    .bg(theme.control)
                    .flex()
                    .flex_col()
                    .justify_end()
                    .child(div().w_full().h(px(length * height)).rounded(px(1.)).bg(
                        if height > 0.98 {
                            theme.meter_high
                        } else {
                            theme.meter_low
                        },
                    ))
            };
        let hold_height = meter_height(held);
        div()
            .relative()
            .flex()
            .gap(px(2.))
            .child(bar(meter.left))
            .child(bar(meter.right))
            .when(hold_height > 0., |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .h(px(1.))
                        .bottom(px(length * hold_height))
                        .bg(if hold_height > 0.98 {
                            theme.meter_high
                        } else {
                            theme.text_secondary
                        }),
                )
            })
    }

    /// The nesting map along the bottom: a row per level, the innermost
    /// nearest the strips, the Song at the bottom. Each folder's bar runs
    /// under its children with an arrow up into its own strip; the chevron
    /// folds it (shared with Arrange).
    #[allow(clippy::too_many_arguments, reason = "layout figures, passed through")]
    fn mixer_map(
        &self,
        placed: &[(&NodeSnapshot, &RowLayout)],
        bars: &[Bar],
        levels: usize,
        lefts: &[f32],
        widths: &[f32],
        content_width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = theme.hover;
        let deepest = levels.saturating_sub(1);
        let marks = bars.iter().map(|bar| {
            let node = placed[bar.row].0;
            let id = bar.node;
            let colors = theme.content(node.color);
            let top = MAP_PAD + (deepest - bar.column) as f32 * MAP_ROW;
            let folded = self.folded.contains(&id);
            let foldable = id != NodeId::ROOT && !node.children.is_empty();
            const PILL: f32 = 18.;
            // From the left edge of the folder's own strip right beneath
            // what is under it, or (folded) a short pill under its strip.
            let left = lefts[bar.row] + 4.;
            let right = lefts[bar.last] + widths[bar.last] - 4.;
            let width = (right - left).max(PILL.min(widths[bar.row]));
            let chevron = foldable.then(|| {
                div()
                    .id(("mixer-fold", id.0))
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .hover(move |s| s.bg(hover))
                    // Arrange's chevrons turned a quarter: open points
                    // right, at the children; folded points down.
                    .child(icon(
                        if folded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        colors.detail,
                        11.,
                    ))
                    .tooltip(tooltip::text(if folded { "Unfold" } else { "Fold" }))
                    .cursor_pointer()
                    // A press here folds; it does not grab the map.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .on_click(cx.listener(move |this, _, _, _| this.toggle_fold(id)))
            });
            div()
                .id(("mixer-bar", id.0))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(MAP_ROW - 4.))
                .rounded(px(4.))
                .bg(colors.fill)
                .border_1()
                .border_color(colors.outline)
                .flex()
                .items_center()
                .pl(px(1.))
                .children(chevron)
                .tooltip(tooltip::text(node.name.clone()))
        });
        let full = levels as f32 * MAP_ROW + 2. * MAP_PAD;
        scroll::axis_locked(div())
            .id("mixer-map")
            .w(px(content_width))
            .h(px(self.mixer_view.map_height.toward(map_height(levels))))
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&self.mixer_view.map_y)
            // Grab anywhere but a chevron and pull up or down to scroll.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.grab_map(true, event.position);
                }),
            )
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.edge)
            .child(
                div()
                    .relative()
                    .w(px(content_width))
                    .h(px(full))
                    .children(marks.collect::<Vec<_>>()),
            )
    }

    /// The footer: the desk's sideways scrollbar and the strip-width slider.
    fn mixer_footer(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let extent = Extent::of(&self.mixer_view.x, Axis::Horizontal);
        let scrolling = matches!(self.mixer_drag, Some(MixerDrag::Scroll(_)));
        let track = self.mixer_view.track.clone();
        let widening = matches!(self.mixer_drag, Some(MixerDrag::Width { .. }));
        let position =
            (self.mixer_view.strip_width - MIN_STRIP_WIDTH) / (MAX_STRIP_WIDTH - MIN_STRIP_WIDTH);
        let original = self.mixer_view.strip_width;
        div()
            .h(px(scroll::THICKNESS))
            .flex_none()
            .flex()
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.hairline)
            .child(
                scroll::scrollbar("mixer-scroll", Axis::Horizontal, extent, scrolling, theme)
                    .child(
                        canvas(move |bounds, _, _| track.set(bounds), |_, (), _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            let handle = &this.mixer_view.x;
                            this.mixer_drag = Some(MixerDrag::Scroll(ScrollDrag {
                                axis: Axis::Horizontal,
                                origin: event.position,
                                extent: Extent::of(handle, Axis::Horizontal),
                                track: this.mixer_view.track.get().size.width / px(1.),
                            }));
                        }),
                    ),
            )
            .child(
                div().px(px(6.)).flex().items_center().child(
                    slider::slider(
                        "strip-width",
                        Axis::Horizontal,
                        position,
                        80.,
                        widening,
                        theme,
                    )
                    .tooltip(tooltip::text("Strip width"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.mixer_drag = Some(MixerDrag::Width {
                                origin_x: event.position.x,
                                original,
                            });
                        }),
                    ),
                ),
            )
    }

    /// Follows a mixer drag; false if none is running.
    pub(crate) fn drag_mixer(&mut self, position: gpui::Point<Pixels>, fine: bool) -> bool {
        let Some(drag) = &mut self.mixer_drag else {
            return false;
        };
        match drag {
            MixerDrag::Pan {
                origin_x,
                original,
                preview,
                ..
            } => {
                let dx = (position.x - *origin_x) / px(1.);
                *preview = detented(slider::dragged(*original, dx, fine));
            }
            MixerDrag::Width { origin_x, original } => {
                let dx = (position.x - *origin_x) / px(1.);
                self.mixer_view.strip_width = resized_strip(*original, dx);
            }
            MixerDrag::Scroll(drag) => {
                scroll::scroll_to(
                    &self.mixer_view.x,
                    Axis::Horizontal,
                    drag.scrolled_at(position),
                );
            }
        }
        true
    }

    /// Ends a mixer drag; a pan change is one edit.
    pub(crate) fn finish_mixer_drag(&mut self) {
        if let Some(MixerDrag::Pan {
            param,
            original,
            preview,
            ..
        }) = self.mixer_drag.take()
            && preview != original
        {
            self.session.apply(Command::Edit(Edit::SetParam {
                param,
                value: preview,
            }));
        }
    }
}

/// A strip as drawn this frame: as wide as it has grown, shifted while it
/// glides, faded in or out. Its content keeps its full width anchored at
/// the left edge, so an opening strip slides out from under its folder's
/// strip (to its left) and a closing one slides back under it.
fn animated_strip(
    layout: &RowLayout,
    width: f32,
    full_width: f32,
    content: impl IntoElement,
) -> gpui::Div {
    div()
        .relative()
        .flex_none()
        .w(px(width))
        .h_full()
        .left(px(layout.offset))
        .opacity(layout.opacity)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(full_width))
                .child(content),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_snaps_to_the_centre_when_close() {
        assert_eq!(detented(0.51), 0.5);
        assert_eq!(detented(0.49), 0.5);
        assert_eq!(detented(0.6), 0.6);
    }

    #[test]
    fn strips_resize_within_limits() {
        assert_eq!(resized_strip(76., 20.), 86.);
        assert_eq!(resized_strip(76., -500.), MIN_STRIP_WIDTH);
        assert_eq!(resized_strip(76., 500.), MAX_STRIP_WIDTH);
    }

    #[test]
    fn the_map_shows_at_most_three_rows() {
        assert_eq!(map_height(0), 0., "no folders, no map");
        assert_eq!(map_height(1), MAP_ROW + 2. * MAP_PAD);
        assert_eq!(map_height(5), map_height(3));
    }
}
