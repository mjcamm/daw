//! The Signals page: every signal down the left, its lane on the timeline
//! beside it, in the same frame as Arrange and Parameters.
//!
//! Drawn signals are edited exactly like layer lanes (see `parameters.rs`).
//! Signals published from a node show where they come from, with a go-to
//! icon; their lanes draw the line their notes make. Each row shows how many
//! layers listen to the signal, and lists them with go-to icons.

use crate::frame::{Timeline, grid_lines};
use crate::icons::{Icon, icon};
use crate::layout::{BEATS_PER_BAR, PIXELS_PER_BEAT, timeline_bars};
use crate::menu::{self, OpenMenu};
use crate::parameters::{Lane, paint_curve, quiet_action, small_button};
use crate::project::{ProjectView, RenameTarget};
use crate::scroll;
use crate::theme::{ContentColor, Theme};
use crate::zoom::Zoom;
use daw_automation::{CompiledSignal, SignalNote, Signals};
use daw_engine::{
    Command, Edit, Signal, SignalId, SignalSnapshot, SignalSource, SignalUse, Snapshot,
};
use gpui::{AnyElement, ClickEvent, Context, Div, FontWeight, Window, canvas, div, prelude::*, px};
use std::rc::Rc;

const LIST_WIDTH: f32 = 260.;
const RULER_HEIGHT: f32 = 28.;
/// Starting zoom: signal lanes are as tall as Arrange rows.
pub(crate) const DEFAULT_ZOOM: Zoom = Zoom::new(PIXELS_PER_BEAT, 56.);
/// Colours in the content palette.
const PALETTE_SIZE: u8 = 9;

/// An id no signal uses yet.
fn next_signal_id(snapshot: &Snapshot) -> SignalId {
    SignalId(
        snapshot
            .signals
            .iter()
            .map(|s| s.id.0)
            .max()
            .map_or(1, |h| h + 1),
    )
}

/// "Signal N", numbered after the signals there are, skipping names in use.
fn next_signal_name(snapshot: &Snapshot) -> String {
    (snapshot.signals.len() + 1..)
        .map(|n| format!("Signal {n}"))
        .find(|name| !snapshot.signals.iter().any(|s| &s.name == name))
        .unwrap_or_default()
}

/// The first palette colour no signal uses yet; once all are used, the
/// palette goes round again.
fn next_signal_color(snapshot: &Snapshot) -> u8 {
    (0..PALETTE_SIZE)
        .find(|c| !snapshot.signals.iter().any(|s| s.color == *c))
        .unwrap_or((snapshot.signals.len() % usize::from(PALETTE_SIZE)) as u8)
}

/// "<node> notes", numbered if that name is taken.
fn notes_signal_name(snapshot: &Snapshot) -> String {
    let base = format!("{} notes", snapshot.node.name);
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base} {n}")))
        .find(|name| !snapshot.signals.iter().any(|s| &s.name == name))
        .unwrap_or(base)
}

/// The signals the node publishes. Until the node tree there is one node,
/// so these are all the notes signals.
pub(crate) fn published_signals(snapshot: &Snapshot) -> impl Iterator<Item = &SignalSnapshot> {
    snapshot
        .signals
        .iter()
        .filter(|s| matches!(s.source, SignalSource::Notes(_)))
}

/// Whether the node has any notes to publish.
pub(crate) fn node_has_notes(snapshot: &Snapshot) -> bool {
    snapshot.clips.iter().any(|(_, c)| !c.notes.is_empty())
}

/// The chip text for how many layers listen to a signal.
fn uses_label(count: usize) -> String {
    match count {
        0 => "Unused".to_owned(),
        1 => "1 use".to_owned(),
        n => format!("{n} uses"),
    }
}

/// Where a listening layer is: "Device › Parameter › Layer".
fn use_label(snapshot: &Snapshot, used: SignalUse) -> String {
    let device = snapshot
        .devices
        .iter()
        .find(|d| d.device == used.param.device);
    let param = device.and_then(|d| d.params.iter().find(|p| p.id == used.param));
    let layer = param
        .and_then(|p| p.layers.iter().find(|(id, _)| *id == used.layer))
        .map(|(_, l)| if l.name.is_empty() { "Layer" } else { &l.name });
    [
        device.map(|d| d.name.as_str()),
        param.map(|p| p.name.as_str()),
        layer,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" › ")
}

/// Every note on the node, as a notes signal reads them.
fn signal_notes(snapshot: &Snapshot) -> Vec<SignalNote> {
    snapshot
        .clips
        .iter()
        .flat_map(|(_, clip)| {
            clip.notes.iter().filter_map(move |note| {
                // Only the part of a note inside its clip plays.
                let start = note.start.max(daw_engine::Ticks::ZERO);
                let end = (note.start + note.length).min(clip.length);
                (start < end).then(|| SignalNote {
                    start: (clip.start + start).as_beats(),
                    end: (clip.start + end).as_beats(),
                    pitch: note.pitch,
                    velocity: note.velocity,
                })
            })
        })
        .collect()
}

/// Every signal compiled, as the engine plays them.
fn compile_all(snapshot: &Snapshot) -> Signals {
    let notes = signal_notes(snapshot);
    let mut signals = Signals::default();
    for signal in &snapshot.signals {
        let source = Signal {
            name: signal.name.clone(),
            color: signal.color,
            source: signal.source.clone(),
        };
        signals.insert(
            signal.id,
            CompiledSignal::compile(&source, notes.iter().copied()),
        );
    }
    signals
}

impl ProjectView {
    /// Every signal compiled, recompiled only when the project changes.
    /// While a notes signal's settings are being dragged, that signal is
    /// compiled with the preview, so every lane and layer shows the change.
    pub(crate) fn compiled_signals(&self) -> Rc<Signals> {
        let snapshot = self.session.snapshot();
        let signals = {
            let mut cache = self.signal_cache.borrow_mut();
            match &*cache {
                Some((revision, signals)) if *revision == snapshot.revision => signals.clone(),
                _ => {
                    let signals = Rc::new(compile_all(snapshot));
                    *cache = Some((snapshot.revision, signals.clone()));
                    signals
                }
            }
        };
        let Some(drag) = &self.notes_drag else {
            return signals;
        };
        let Some(signal) = snapshot.signals.iter().find(|s| s.id == drag.signal) else {
            return signals;
        };
        let preview = Signal {
            name: signal.name.clone(),
            color: signal.color,
            source: SignalSource::Notes(drag.preview.clone()),
        };
        let mut signals = (*signals).clone();
        signals.insert(
            drag.signal,
            CompiledSignal::compile(&preview, signal_notes(snapshot)),
        );
        Rc::new(signals)
    }

    /// Shows a signal on the Signals page: scrolled into view, highlighted
    /// for a moment and selected, with the inspector open if it has
    /// settings there.
    pub(crate) fn go_to_signal(&mut self, id: SignalId) {
        self.page = crate::project::Page::Signals;
        let snapshot = self.session.snapshot();
        self.selected_signal = Some(id);
        if snapshot
            .signals
            .iter()
            .any(|s| s.id == id && matches!(s.source, SignalSource::Notes(_)))
        {
            self.inspector_open = true;
        }
        let row = self.signals_view.zoom.row_height;
        let index = snapshot
            .signals
            .iter()
            .position(|s| s.id == id)
            .unwrap_or(0);
        let offset = (index as f32 - 1.).max(0.) * row;
        self.signals_view
            .y
            .set_offset(gpui::point(px(0.), px(-offset)));
        self.signal_highlight = Some((id, std::time::Instant::now()));
    }

    pub(crate) fn render_signals(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pointer = window.mouse_position();
        let snapshot = self.session.snapshot();
        let zoom = self.signals_view.zoom;
        let bars = timeline_bars(snapshot.clips.iter().map(|(_, c)| c), snapshot.loop_range);
        let width = zoom.beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let compiled = self.compiled_signals();

        let list = div()
            .w(px(LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children(
                snapshot
                    .signals
                    .iter()
                    .map(|signal| self.signal_row(signal, theme, cx)),
            )
            .child(
                quiet_action(
                    div()
                        .h(px(30.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(12.)),
                    "add-signal",
                    "Add Signal",
                    theme,
                )
                .on_click(cx.listener(|this, _, _, _| this.add_signal())),
            )
            .child(
                div()
                    .px(px(12.))
                    .text_size(px(11.))
                    .text_color(theme.text_tertiary)
                    .child("Nodes can publish their notes as signals from the Arrange page."),
            )
            .child(
                // Clicking the empty space below the signals clears the
                // selection.
                div()
                    .id("signal-list-space")
                    .flex_1()
                    .min_h(px(24.))
                    .on_click(cx.listener(|this, _, _, _| this.selected_signal = None)),
            );

        let mut top = 0.;
        let lanes: Vec<AnyElement> = snapshot
            .signals
            .iter()
            .map(|signal| {
                let cell = div()
                    .h(px(zoom.row_height))
                    .flex_none()
                    .w(px(width))
                    .relative();
                let colors = theme.content(signal.color);
                let lane = match &signal.source {
                    SignalSource::Drawn(blocks) => self
                        .lane_cell(
                            cell,
                            Lane::Signal(signal.id),
                            blocks,
                            colors,
                            1.0,
                            None,
                            top,
                            pointer,
                            theme,
                            cx,
                        )
                        .into_any_element(),
                    SignalSource::Notes(_) => {
                        notes_lane(cell, signal.id, compiled.clone(), colors, zoom, theme)
                            .into_any_element()
                    }
                };
                top += zoom.row_height;
                lane
            })
            .collect();
        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(zoom.beat_to_x(self.session.position_beats())))
            .w(px(1.))
            .bg(theme.accent);
        let timeline = scroll::axis_locked(div())
            .id("signal-lanes")
            .flex_1()
            .min_w_0()
            // A row, so the content stretches to the full height.
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.signals_view.x)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .child(self.lanes_tracker())
                    .children(grid_lines(zoom, bars, theme))
                    .children(lanes)
                    .child(playhead),
            );

        // The ruler follows the lanes' sideways scroll.
        let scroll_x = self.signals_view.x.offset().x;
        let header = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .flex()
            .child(
                div()
                    .w(px(LIST_WIDTH))
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
                    .child("SIGNALS"),
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
            .id("signals-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.signals_view.y)
            .flex()
            .flex_col()
            .child(
                // Grows to fill the page (flex growth: percentage heights do
                // not work inside a vertical scroll).
                div()
                    .flex()
                    .flex_grow()
                    .flex_shrink_0()
                    .child(list)
                    .child(timeline),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(self.timeline_frame(
                Timeline::Signals,
                body.into_any_element(),
                LIST_WIDTH,
                theme,
                cx,
            ))
            .children(self.selected_settings(theme, cx))
    }

    /// A signal in the list: colour, name and source, uses, delete.
    fn signal_row(
        &self,
        signal: &SignalSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let id = signal.id;
        let colors = theme.content(signal.color);
        let row_height = self.signals_view.zoom.row_height;
        let hover = theme.hover;
        let highlighted = self
            .signal_highlight
            .is_some_and(|(s, at)| s == id && at.elapsed() < crate::parameters::HIGHLIGHT_TIME);
        let mut highlight = theme.accent;
        highlight.a = 0.18;
        let mut selected_bg = theme.accent;
        selected_bg.a = 0.16;
        let selected = self.selected_signal == Some(id);

        let color_open = self.open_menu == Some(OpenMenu::SignalColor(id));
        let swatch = div()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("signal-color", id.0))
                    .px(px(4.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .w(px(4.))
                            .h(px((row_height - 24.).clamp(10., 32.)))
                            .rounded(px(2.))
                            .bg(colors.detail),
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.toggle_menu(OpenMenu::SignalColor(id));
                    })),
            )
            .when(color_open, |d| {
                d.child(self.color_menu(id, signal.color, theme, cx))
            });

        let name: AnyElement = match self.rename_input(RenameTarget::Signal(id)) {
            Some(input) => div().text_size(px(13.)).child(input).into_any_element(),
            None => div()
                .id(("signal-name", id.0))
                .text_size(px(13.))
                .child(signal.name.clone())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(RenameTarget::Signal(id), window, cx);
                    }
                }))
                .into_any_element(),
        };
        let source: AnyElement = match &signal.source {
            SignalSource::Drawn(_) => div().child("Drawn").into_any_element(),
            SignalSource::Notes(_) => {
                let node = self.session.snapshot().node.name.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(format!("from {node}"))
                    .child(
                        small_button(("signal-source", id.0), theme)
                            .size(px(16.))
                            .child(icon(Icon::GoTo, theme.text_secondary, 11.))
                            .on_click(cx.listener(|this, _, _, _| this.go_to_node())),
                    )
                    .into_any_element()
            }
        };

        let uses_open = self.open_menu == Some(OpenMenu::SignalUses(id));
        let used = !signal.used_by.is_empty();
        let uses = div()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("signal-uses", id.0))
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .when(used, |d| {
                        d.bg(theme.control)
                            .hover(move |s| s.bg(hover))
                            .text_color(theme.text_secondary)
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.toggle_menu(OpenMenu::SignalUses(id));
                            }))
                    })
                    .when(!used, |d| d.text_color(theme.text_tertiary))
                    .child(uses_label(signal.used_by.len())),
            )
            .when(uses_open && used, |d| {
                d.child(self.uses_menu(&signal.used_by, theme, cx))
            });

        let delete = small_button(("signal-delete", id.0), theme)
            .child(icon(Icon::Close, theme.text_secondary, 12.))
            .on_click(cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::RemoveSignal { id }));
            }));

        div()
            .id(("signal-row", id.0))
            .h(px(row_height))
            .flex_none()
            .when(selected, |d| d.bg(selected_bg))
            .when(highlighted, |d| d.bg(highlight))
            .on_click(cx.listener(move |this, _, _, _| this.selected_signal = Some(id)))
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(8.))
            .pr(px(8.))
            // The same split as on the lanes.
            .border_b_2()
            .border_color(theme.edge)
            .child(swatch)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(name)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_secondary)
                            .child(source),
                    ),
            )
            .child(uses)
            .child(delete)
    }

    /// The palette, to pick a signal's colour.
    fn color_menu(
        &self,
        id: SignalId,
        current: u8,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let swatches = (0..PALETTE_SIZE).map(|color| {
            let colors = theme.content(color);
            div()
                .id(("signal-swatch", usize::from(color)))
                .size(px(22.))
                .rounded(px(11.))
                .p(px(2.))
                .border_2()
                .border_color(if color == current {
                    theme.text_secondary
                } else {
                    gpui::transparent_black()
                })
                .child(div().size_full().rounded(px(9.)).bg(colors.detail))
                .on_click(cx.listener(move |this, _, _, _| {
                    this.open_menu = None;
                    this.session
                        .apply(Command::Edit(Edit::SetSignalColor { id, color }));
                }))
        });
        menu::float(
            menu::panel(theme)
                .min_w(px(0.))
                .w(px(104.))
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .p(px(6.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .children(swatches),
        )
    }

    /// The layers listening to a signal, each with a go-to icon.
    fn uses_menu(
        &self,
        used_by: &[SignalUse],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let hover = theme.hover;
        let items = used_by.iter().enumerate().map(|(i, used)| {
            let layer = used.layer;
            div()
                .id(("signal-use", i))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(5.))
                .hover(move |s| s.bg(hover))
                .child(use_label(snapshot, *used))
                .child(icon(Icon::GoTo, theme.text_secondary, 12.))
                .on_click(cx.listener(move |this, _, _, _| {
                    this.open_menu = None;
                    this.go_to_layer(layer);
                }))
        });
        menu::float(
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .children(items),
        )
    }

    /// Adds a drawn signal with an empty lane, ready to draw on.
    fn add_signal(&mut self) {
        let snapshot = self.session.snapshot();
        let edit = Edit::AddSignal {
            id: next_signal_id(snapshot),
            signal: Signal {
                name: next_signal_name(snapshot),
                color: next_signal_color(snapshot),
                source: SignalSource::Drawn(Vec::new()),
            },
            listeners: Vec::new(),
        };
        self.session.apply(Command::Edit(edit));
    }

    /// Publishes the node's notes as a new motion signal: every note, a
    /// short snappy hit.
    pub(crate) fn publish_notes(&mut self) {
        let snapshot = self.session.snapshot();
        let edit = Edit::AddSignal {
            id: next_signal_id(snapshot),
            signal: Signal {
                name: notes_signal_name(snapshot),
                color: next_signal_color(snapshot),
                source: SignalSource::Notes(daw_engine::NotesSource::new()),
            },
            listeners: Vec::new(),
        };
        self.session.apply(Command::Edit(edit));
    }

    /// Shows the node on the Arrange page, selected.
    pub(crate) fn go_to_node(&mut self) {
        self.page = crate::project::Page::Arrange;
        self.node_selected = true;
    }
}

/// A notes signal's lane: the line its hits make, drawn, not edited.
fn notes_lane(
    cell: Div,
    id: SignalId,
    signals: Rc<Signals>,
    colors: ContentColor,
    zoom: Zoom,
    theme: &Theme,
) -> Div {
    let mut centre = theme.grid;
    centre.a *= 1.5;
    cell.border_b_2().border_color(theme.edge).child(
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let height = bounds.size.height / px(1.);
                let centre_line = gpui::Bounds::new(
                    gpui::point(bounds.origin.x, bounds.origin.y + px(height / 2. - 0.5)),
                    gpui::size(bounds.size.width, px(1.)),
                );
                window.paint_quad(gpui::fill(centre_line, centre));
                paint_curve(
                    window,
                    bounds,
                    zoom.pixels_per_beat,
                    colors.detail,
                    Some((colors.fill, 0.5)),
                    |beat| signals.get(id).map_or(0.5, |s| s.sample(beat).0),
                );
            },
        )
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{
        BlendMode, DeviceRef, Engine, EngineConfig, Layer, LayerId, NotesSource, ParamId,
        devices::channel,
    };

    fn snapshot_with(edits: Vec<Edit>) -> Snapshot {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        for edit in edits {
            engine.apply(Command::Edit(edit)).unwrap();
        }
        engine.snapshot()
    }

    fn drawn(name: &str, color: u8) -> Signal {
        Signal {
            name: name.to_owned(),
            color,
            source: SignalSource::Drawn(Vec::new()),
        }
    }

    fn add(id: u64, signal: Signal) -> Edit {
        Edit::AddSignal {
            id: SignalId(id),
            signal,
            listeners: Vec::new(),
        }
    }

    #[test]
    fn new_signals_are_numbered_and_never_reuse_a_name() {
        assert_eq!(next_signal_name(&snapshot_with(vec![])), "Signal 1");
        let snapshot = snapshot_with(vec![add(1, drawn("Signal 2", 0))]);
        assert_eq!(next_signal_name(&snapshot), "Signal 3");
        assert_eq!(next_signal_id(&snapshot), SignalId(2));
    }

    #[test]
    fn new_signals_take_the_first_unused_colour() {
        assert_eq!(next_signal_color(&snapshot_with(vec![])), 0);
        let snapshot = snapshot_with(vec![add(1, drawn("A", 0)), add(2, drawn("B", 2))]);
        assert_eq!(next_signal_color(&snapshot), 1);
        let all: Vec<Edit> = (0..9)
            .map(|c| add(u64::from(c) + 1, drawn("S", c)))
            .collect();
        assert_eq!(next_signal_color(&snapshot_with(all)), 0, "round again");
    }

    #[test]
    fn published_notes_signals_are_named_after_the_node() {
        let snapshot = snapshot_with(vec![]);
        let node = snapshot.node.name.clone();
        assert_eq!(notes_signal_name(&snapshot), format!("{node} notes"));
        let notes = |name: String| Signal {
            name,
            color: 0,
            source: SignalSource::Notes(NotesSource::new()),
        };
        let snapshot = snapshot_with(vec![
            add(1, notes(format!("{node} notes"))),
            add(2, drawn("Sweep", 1)),
        ]);
        assert_eq!(notes_signal_name(&snapshot), format!("{node} notes 2"));
        let published: Vec<SignalId> = published_signals(&snapshot).map(|s| s.id).collect();
        assert_eq!(published, [SignalId(1)], "drawn signals are not the node's");
    }

    #[test]
    fn uses_read_naturally() {
        assert_eq!(uses_label(0), "Unused");
        assert_eq!(uses_label(1), "1 use");
        assert_eq!(uses_label(3), "3 uses");
    }

    #[test]
    fn a_use_names_its_device_parameter_and_layer() {
        let volume = ParamId {
            device: DeviceRef::Channel,
            index: channel::VOLUME,
        };
        let snapshot = snapshot_with(vec![
            add(
                1,
                Signal {
                    name: "Kick".to_owned(),
                    color: 0,
                    source: SignalSource::Notes(NotesSource::new()),
                },
            ),
            Edit::AddLayer {
                param: volume,
                id: LayerId(4),
                index: 0,
                layer: Layer::named("Duck", BlendMode::Multiply).listening(SignalId(1)),
            },
        ]);
        let [used] = snapshot.signals[0].used_by[..] else {
            panic!("expected one use");
        };
        assert_eq!(used.param, volume);
        assert_eq!(use_label(&snapshot, used), "Channel › Volume › Duck");
    }
}
