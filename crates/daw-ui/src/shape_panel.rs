//! The shape panel (`docs/design/curve-editor.md`): the bottom panel on the
//! Signals page for the selected notes signal. A toolbar (Trigger or Loop,
//! the window, the grid, Save as stamp, the selected point's kind), a strip
//! of stamp cards, and the curve editor over one window.
//!
//! Every change is one edit of the signal's notes source; drags preview
//! everywhere the signal is drawn and heard, and land as one edit.

use crate::block_edit::Snap;
use crate::curve_edit;
use crate::curve_editor::{self, CurveArea, CurveHit, Look};
use crate::icons::{Icon, icon};
use crate::menu::{self, OpenMenu, SnapFor};
use crate::parameters::{quiet_action, small_button};
use crate::project::{Page, ProjectView, RenameTarget};
use crate::stamps::{self, StampRef};
use crate::theme::Theme;
use crate::tooltip;
use daw_engine::{
    Command, Curve, Edit, HitMode, NotesSource, PointKind, SignalId, SignalSnapshot, SignalSource,
    Ticks,
};
use daw_host::stamps::Stamp;
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Div, FontWeight, Modifiers, MouseButton,
    MouseDownEvent, Pixels, Point, Stateful, canvas, div, prelude::*, px,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

const HEADER_HEIGHT: f32 = 36.;
const STRIP_HEIGHT: f32 = 64.;
const DEFAULT_HEIGHT: f32 = 300.;
const MIN_HEIGHT: f32 = 200.;
const CARD_WIDTH: f32 = 72.;
const CARD_GAP: f32 = 6.;
/// The strip's padding and the room the "more" button takes.
const STRIP_PADDING: f32 = 10.;
const MORE_WIDTH: f32 = 28.;

/// The window lengths offered, as the toolbar labels them.
const WINDOW_LABELS: [&str; 6] = ["1/8", "1/4", "1/2", "1", "2", "4"];

/// A drag in the panel.
#[derive(Debug, Clone)]
pub(crate) enum ShapeDrag {
    /// The panel's top edge.
    Height {
        origin: Point<Pixels>,
        original: f32,
    },
    /// A point or a handle of the curve, previewed until release.
    Curve {
        hit: CurveHit,
        original: NotesSource,
        preview: NotesSource,
    },
}

/// The open panel.
#[derive(Debug)]
pub struct ShapeEditor {
    pub signal: SignalId,
    pub height: f32,
    pub(crate) opened: Instant,
    /// The selected point, by index.
    pub selected: Option<usize>,
    pub(crate) drag: Option<ShapeDrag>,
    /// Where the curve's box was laid out.
    area: Rc<Cell<Bounds<Pixels>>>,
}

/// How many cells of `grid` fit across a window: at least one.
pub fn cells(window: Ticks, snap: Snap) -> usize {
    (window.0 / snap.grid.ticks().0.max(1)).max(1) as usize
}

/// How many stamp cards fit across a strip `width` wide, out of `count`;
/// the rest go in the "more" menu.
pub fn cards_that_fit(width: f32, count: usize) -> usize {
    let fit = |room: f32| {
        ((room + CARD_GAP) / (CARD_WIDTH + CARD_GAP))
            .floor()
            .max(0.) as usize
    };
    let room = width - 2. * STRIP_PADDING;
    if fit(room) >= count {
        return count;
    }
    fit(room - MORE_WIDTH - CARD_GAP).min(count)
}

/// "Stamp N", the first number not taken.
pub fn next_stamp_name(saved: &[Stamp]) -> String {
    (1..)
        .map(|n| format!("Stamp {n}"))
        .find(|name| !saved.iter().any(|s| &s.name == name))
        .unwrap_or_default()
}

impl ProjectView {
    /// Selects a signal on the Signals page; a notes signal's shape opens
    /// in the bottom panel.
    pub(crate) fn select_signal(&mut self, id: SignalId) {
        self.selected_signal = Some(id);
        let notes = self
            .session
            .snapshot()
            .signals
            .iter()
            .any(|s| s.id == id && matches!(s.source, SignalSource::Notes(_)));
        if !notes {
            self.shape_editor = None;
            return;
        }
        match &mut self.shape_editor {
            Some(editor) if editor.signal == id => {}
            Some(editor) => {
                editor.signal = id;
                editor.selected = None;
                editor.drag = None;
            }
            None => {
                self.shape_editor = Some(ShapeEditor {
                    signal: id,
                    height: DEFAULT_HEIGHT,
                    opened: Instant::now(),
                    selected: None,
                    drag: None,
                    area: Rc::default(),
                });
            }
        }
    }

    pub(crate) fn close_shape_editor(&mut self) {
        self.shape_editor = None;
    }

    /// The signal the panel shows and its source as shown: a drag's
    /// preview while dragging.
    fn shaped_signal(&self) -> Option<(&SignalSnapshot, NotesSource)> {
        let editor = self.shape_editor.as_ref()?;
        let signal = self
            .session
            .snapshot()
            .signals
            .iter()
            .find(|s| s.id == editor.signal)?;
        let SignalSource::Notes(source) = &signal.source else {
            return None;
        };
        let source = match &editor.drag {
            Some(ShapeDrag::Curve { preview, .. }) => preview.clone(),
            _ => source.clone(),
        };
        Some((signal, source))
    }

    /// The panel's height and whether it is still sliding up, if it shows:
    /// on the Signals page, for the selected notes signal.
    pub(crate) fn prepare_shape_panel(&mut self) -> Option<(f32, bool)> {
        if self.page != Page::Signals {
            return None;
        }
        let editor = self.shape_editor.as_ref()?;
        if self.selected_signal != Some(editor.signal) || self.shaped_signal().is_none() {
            self.shape_editor = None;
            return None;
        }
        let editor = self.shape_editor.as_mut()?;
        // A point removed by undo or an edit elsewhere.
        let points = match self
            .session
            .snapshot()
            .signals
            .iter()
            .find(|s| s.id == editor.signal)
        {
            Some(SignalSnapshot {
                source: SignalSource::Notes(source),
                ..
            }) => source.shape.points.len(),
            _ => 0,
        };
        if editor.selected.is_some_and(|i| i >= points) {
            editor.selected = None;
        }
        let slide = crate::piano_roll::slide(editor.opened);
        Some((editor.height * slide, slide < 1.))
    }

    /// Sends a changed notes source for the panel's signal as one edit.
    fn set_shape_source(&mut self, source: NotesSource) {
        let Some(editor) = &self.shape_editor else {
            return;
        };
        self.session.apply(Command::Edit(Edit::SetNotesSource {
            id: editor.signal,
            source,
        }));
    }

    /// The panel's source with `change` made, sent as one edit.
    fn change_shape(&mut self, change: impl FnOnce(&mut NotesSource)) {
        let Some((_, mut source)) = self.shaped_signal() else {
            return;
        };
        let before = source.clone();
        change(&mut source);
        if source != before {
            self.set_shape_source(source);
        }
    }

    fn shape_snap(&self, modifiers: Modifiers) -> Snap {
        if modifiers.alt {
            self.shape_grid.inverted()
        } else {
            self.shape_grid
        }
    }

    /// A press in the curve's box: grab a point or handle, add a point
    /// (double-click on empty space), remove one (double-click on it), or
    /// clear the selection.
    fn shape_mouse_down(&mut self, event: &MouseDownEvent) {
        let Some((_, source)) = self.shaped_signal() else {
            return;
        };
        let snap = self.shape_snap(event.modifiers);
        let cells = cells(source.window, snap);
        let Some(editor) = &mut self.shape_editor else {
            return;
        };
        let area = CurveArea::new(editor.area.get());
        let (x, y) = (event.position.x / px(1.), event.position.y / px(1.));
        let double = event.click_count >= 2;
        match curve_editor::hit(&source.shape, editor.selected, area, x, y) {
            Some(CurveHit::Point(index)) if double => {
                if let Some(shape) = curve_edit::remove_point(&source.shape, index) {
                    editor.selected = None;
                    let source = NotesSource { shape, ..source };
                    self.set_shape_source(source);
                }
            }
            Some(hit) => {
                if let CurveHit::Point(index) = hit {
                    editor.selected = Some(index);
                }
                editor.drag = Some(ShapeDrag::Curve {
                    hit,
                    original: source.clone(),
                    preview: source,
                });
            }
            None if double => {
                let (mut at, mut value) = area.curve_at(x, y);
                if snap.enabled {
                    at = curve_edit::snap_at(at, cells);
                    value = curve_edit::snap_value(value);
                }
                let (shape, index) = curve_edit::add_point(&source.shape, at, value);
                editor.selected = Some(index);
                self.set_shape_source(NotesSource { shape, ..source });
            }
            None => editor.selected = None,
        }
    }

    /// Follows the pointer; false if no panel drag is running.
    pub(crate) fn drag_shape_editor(
        &mut self,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        let snap = self.shape_snap(modifiers);
        let Some(editor) = &mut self.shape_editor else {
            return false;
        };
        match &mut editor.drag {
            Some(ShapeDrag::Height { origin, original }) => {
                let up = (origin.y - position.y) / px(1.);
                editor.height = (*original + up).max(MIN_HEIGHT);
                true
            }
            Some(ShapeDrag::Curve {
                hit,
                original,
                preview,
            }) => {
                let area = CurveArea::new(editor.area.get());
                let (x, y) = (position.x / px(1.), position.y / px(1.));
                let curve = &original.shape;
                let shape = match *hit {
                    CurveHit::Point(index) => {
                        let (mut at, mut value) = area.curve_at(x, y);
                        if snap.enabled {
                            at = curve_edit::snap_at(at, cells(original.window, snap));
                            value = curve_edit::snap_value(value);
                        }
                        curve_edit::move_point(curve, index, at, value)
                    }
                    CurveHit::Handle(index, side) => {
                        let Some(point) = curve.points.get(index) else {
                            return true;
                        };
                        let offset = area.handle_to(point, x, y);
                        curve_edit::move_handle(curve, index, side, offset, area.aspect())
                    }
                };
                *preview = NotesSource {
                    shape,
                    ..original.clone()
                };
                true
            }
            None => false,
        }
    }

    /// The edit a panel drag makes if let go now.
    pub(crate) fn shape_drag_edit(&self) -> Option<Edit> {
        let editor = self.shape_editor.as_ref()?;
        match &editor.drag {
            Some(ShapeDrag::Curve {
                original, preview, ..
            }) if preview != original => Some(Edit::SetNotesSource {
                id: editor.signal,
                source: preview.clone(),
            }),
            _ => None,
        }
    }

    /// Ends a panel drag: a changed curve lands as one edit.
    pub(crate) fn finish_shape_drag(&mut self) {
        if let Some(edit) = self.shape_drag_edit() {
            self.session.apply(Command::Edit(edit));
        }
        if let Some(editor) = &mut self.shape_editor {
            editor.drag = None;
        }
    }

    pub(crate) fn shape_dragging(&self) -> bool {
        self.shape_editor.as_ref().is_some_and(|e| e.drag.is_some())
    }

    /// Whether a curve drag is running: it previews on the page's lanes too.
    pub(crate) fn shape_curve_drag(&self) -> bool {
        matches!(
            self.shape_editor.as_ref().and_then(|e| e.drag.as_ref()),
            Some(ShapeDrag::Curve { .. })
        )
    }

    /// Switches the selected point's kind.
    fn set_point_kind(&mut self, kind: PointKind) {
        let Some(index) = self.shape_editor.as_ref().and_then(|e| e.selected) else {
            return;
        };
        let aspect = self
            .shape_editor
            .as_ref()
            .map_or(1., |e| CurveArea::new(e.area.get()).aspect());
        self.change_shape(|source| {
            source.shape = curve_edit::set_kind(&source.shape, index, kind, aspect);
        });
    }

    /// The curve a stamp writes.
    fn stamp_curve(&self, stamp: StampRef) -> Option<Curve> {
        match stamp {
            StampRef::BuiltIn(i) => stamps::built_in().into_iter().nth(i).map(|(_, c)| c),
            StampRef::Saved(i) => self.saved_stamps.get(i).map(|s| s.curve.clone()),
        }
    }

    /// Writes a stamp across the whole window, once per grid cell.
    fn apply_stamp(&mut self, stamp: StampRef) {
        let Some(curve) = self.stamp_curve(stamp) else {
            return;
        };
        let cells = self
            .shaped_signal()
            .map_or(1, |(_, s)| cells(s.window, self.shape_grid));
        if let Some(editor) = &mut self.shape_editor {
            editor.selected = None;
        }
        self.change_shape(|source| source.shape = curve_edit::stamp(&curve, cells));
    }

    /// Keeps the current curve as a new saved stamp.
    fn save_stamp(&mut self) {
        let Some((_, source)) = self.shaped_signal() else {
            return;
        };
        let name = next_stamp_name(&self.saved_stamps);
        self.saved_stamps.push(Stamp {
            name,
            curve: source.shape,
        });
        self.stamps_to_keep = true;
    }

    pub(crate) fn delete_stamp(&mut self, index: usize) {
        if index < self.saved_stamps.len() {
            self.saved_stamps.remove(index);
            self.stamps_to_keep = true;
            self.open_menu = None;
        }
    }

    pub(crate) fn rename_stamp(&mut self, index: usize, name: String) {
        let name = name.trim();
        if let Some(stamp) = self.saved_stamps.get_mut(index)
            && !name.is_empty()
        {
            stamp.name = name.to_owned();
            self.stamps_to_keep = true;
        }
    }

    /// Tells the app to keep the saved stamps, once they changed.
    pub(crate) fn keep_stamps(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.stamps_to_keep) {
            cx.emit(crate::project::ProjectEvent::Stamps(
                self.saved_stamps.clone(),
            ));
        }
    }

    /// The panel, if a notes signal's shape is open.
    pub(crate) fn render_shape_panel(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (signal, source) = self.shaped_signal()?;
        let signal = signal.clone();
        let editor = self.shape_editor.as_ref()?;
        let colors = theme.content(signal.color);
        let panel_height = editor.height * crate::piano_roll::slide(editor.opened);
        let selected = editor.selected;
        let selected_kind = selected
            .and_then(|i| source.shape.points.get(i))
            .map(|p| p.kind);
        let area_cell = editor.area.clone();
        let cells = cells(source.window, self.shape_grid);

        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().size(px(8.)).rounded(px(4.)).bg(colors.detail))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child(format!("{} · shape", signal.name)),
                    ),
            )
            .child(segments(
                theme,
                "shape-mode",
                [("Trigger", HitMode::Trigger), ("Loop", HitMode::Loop)]
                    .into_iter()
                    .map(|(label, mode)| {
                        let tip = match mode {
                            HitMode::Trigger => "Each note plays the shape once through",
                            HitMode::Loop => "The shape repeats while the note is held",
                        };
                        (label, source.mode == mode, true, tip, {
                            move |this: &mut Self| this.change_shape(|s| s.mode = mode)
                        })
                    })
                    .collect(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().text_color(theme.text_secondary).child("Window"))
                    .child(segments(
                        theme,
                        "shape-window",
                        NotesSource::WINDOWS
                            .into_iter()
                            .zip(WINDOW_LABELS)
                            .map(|(window, label)| {
                                (label, source.window == window, true, "Bars", {
                                    move |this: &mut Self| this.change_shape(|s| s.window = window)
                                })
                            })
                            .collect(),
                        cx,
                    ))
                    .child(div().text_color(theme.text_secondary).child("bars")),
            )
            .child(self.render_snap(SnapFor::Shape, theme, cx))
            .child(
                quiet_action(
                    div().flex().items_center(),
                    "shape-save-stamp",
                    "Save as stamp",
                    theme,
                )
                .debug_selector(|| "shape-save-stamp".into())
                .on_click(cx.listener(|this, _, _, _| this.save_stamp())),
            )
            .child(div().flex_1())
            .child(segments(
                theme,
                "shape-kind",
                [
                    (
                        "Corner",
                        PointKind::Corner,
                        "Corner: straight lines both sides",
                    ),
                    (
                        "Smooth",
                        PointKind::Smooth,
                        "Smooth: curves through, its handles in line",
                    ),
                    (
                        "Broken",
                        PointKind::Broken,
                        "Broken: curves both sides, each handle on its own",
                    ),
                ]
                .into_iter()
                .map(|(label, kind, tip)| {
                    (
                        label,
                        selected_kind == Some(kind),
                        selected_kind.is_some(),
                        tip,
                        { move |this: &mut Self| this.set_point_kind(kind) },
                    )
                })
                .collect(),
                cx,
            ))
            .child(
                small_button("shape-close", theme)
                    .debug_selector(|| "shape-close".into())
                    .child(icon(Icon::Close, theme.text_secondary, 14.))
                    .tooltip(tooltip::text("Close"))
                    .on_click(cx.listener(|this, _, _, _| this.close_shape_editor())),
            );

        let strip = self.stamp_strip(colors.detail, theme, cx);

        let mut fill = colors.detail;
        fill.a = 0.16;
        let look = Look {
            line: colors.detail,
            fill,
            grid: theme.hairline,
            centre: theme.edge,
            point: colors.detail,
            hollow: theme.window,
            selected: theme.accent,
        };
        let shape = source.shape.clone();
        let curve = div()
            .id("shape-curve")
            .debug_selector(|| "shape-curve".into())
            .flex_1()
            .min_h_0()
            .relative()
            .cursor_crosshair()
            .child(
                canvas(
                    move |bounds, _, _| area_cell.set(bounds),
                    move |bounds, (), window, _| {
                        curve_editor::paint(window, bounds, &shape, selected, cells, look);
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| this.shape_mouse_down(event)),
            );

        let mut handle_hover = theme.accent;
        handle_hover.a = 0.35;
        let handle = div()
            .id("shape-resize")
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
                    if let Some(editor) = &mut this.shape_editor {
                        editor.drag = Some(ShapeDrag::Height {
                            origin: event.position,
                            original: editor.height,
                        });
                    }
                }),
            );
        Some(
            div()
                .debug_selector(|| "shape-panel".into())
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
                .child(strip)
                .child(curve)
                .child(handle)
                .into_any_element(),
        )
    }

    /// The stamp cards: built-in ones, then saved ones, with a "more" menu
    /// for those that don't fit.
    fn stamp_strip(&self, line: gpui::Hsla, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut all: Vec<(StampRef, String, Curve)> = stamps::built_in()
            .into_iter()
            .enumerate()
            .map(|(i, (name, curve))| (StampRef::BuiltIn(i), name.to_owned(), curve))
            .collect();
        all.extend(
            self.saved_stamps
                .iter()
                .enumerate()
                .map(|(i, s)| (StampRef::Saved(i), s.name.clone(), s.curve.clone())),
        );
        let width = self
            .pane_bounds
            .borrow()
            .iter()
            .find(|(kind, _)| *kind == crate::live::PaneKind::Bottom)
            .map_or(f32::INFINITY, |(_, b)| b.size.width / px(1.));
        let shown = cards_that_fit(width, all.len());
        let rest = all.split_off(shown);
        let more_open = self.open_menu == Some(OpenMenu::MoreStamps);
        let more = (!rest.is_empty()).then(|| {
            let hover = theme.hover;
            div()
                .relative()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("shape-more-stamps")
                        .debug_selector(|| "shape-more-stamps".into())
                        .w(px(MORE_WIDTH))
                        .h(px(STRIP_HEIGHT - 16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .hover(move |s| s.bg(hover))
                        .child(icon(Icon::ChevronRight, theme.text_secondary, 14.))
                        .tooltip(tooltip::text("More stamps"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, _| this.toggle_menu(OpenMenu::MoreStamps)),
                        ),
                )
                .when(more_open, |d| {
                    d.child(self.more_stamps_menu(&rest, line, theme, cx))
                })
        });
        div()
            .h(px(STRIP_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(CARD_GAP))
            .px(px(STRIP_PADDING))
            .border_b_1()
            .border_color(theme.hairline)
            .overflow_hidden()
            .children(
                all.into_iter().map(|(stamp, name, curve)| {
                    self.stamp_card(stamp, name, curve, line, theme, cx)
                }),
            )
            .children(more)
    }

    fn stamp_card(
        &self,
        stamp: StampRef,
        name: String,
        curve: Curve,
        line: gpui::Hsla,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = theme.hover;
        let key = stamp_key(stamp);
        let saved = match stamp {
            StampRef::Saved(i) => Some(i),
            StampRef::BuiltIn(_) => None,
        };
        let label: AnyElement = match saved.and_then(|i| self.rename_input(RenameTarget::Stamp(i)))
        {
            Some(input) => div().w_full().child(input).into_any_element(),
            None => div()
                .id(("stamp-name", key))
                .w_full()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_center()
                .child(name.clone())
                .when_some(saved, |d, index| {
                    d.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            cx.stop_propagation();
                            this.start_rename(RenameTarget::Stamp(index), window, cx);
                        }
                    }))
                })
                .into_any_element(),
        };
        let delete = saved.map(|index| {
            small_button(("stamp-delete", key), theme)
                .debug_selector(move || format!("stamp-delete-{index}"))
                .absolute()
                .top(px(1.))
                .right(px(1.))
                .size(px(16.))
                .child(icon(Icon::Close, theme.text_tertiary, 10.))
                .tooltip(tooltip::text("Delete this stamp"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.delete_stamp(index);
                }))
        });
        div()
            .id(("stamp", key))
            .debug_selector(move || format!("stamp-{key}"))
            .relative()
            .w(px(CARD_WIDTH))
            .h(px(STRIP_HEIGHT - 16.))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .p(px(4.))
            .rounded(px(6.))
            .bg(theme.control)
            .hover(move |s| s.bg(hover))
            .text_size(px(10.))
            .text_color(theme.text_secondary)
            .child(preview(curve, line))
            .child(label)
            .children(delete)
            .tooltip(tooltip::text(format!(
                "{name}: fill the window, once per grid cell"
            )))
            .on_click(cx.listener(move |this, _, _, _| this.apply_stamp(stamp)))
    }

    fn more_stamps_menu(
        &self,
        rest: &[(StampRef, String, Curve)],
        line: gpui::Hsla,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = rest.iter().map(|(stamp, name, curve)| {
            let stamp = *stamp;
            let key = stamp_key(stamp);
            let hover = theme.hover;
            let delete = match stamp {
                StampRef::Saved(index) => Some(
                    small_button(("more-stamp-delete", key), theme)
                        .size(px(18.))
                        .child(icon(Icon::Close, theme.text_tertiary, 11.))
                        .tooltip(tooltip::text("Delete this stamp"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.delete_stamp(index);
                        })),
                ),
                StampRef::BuiltIn(_) => None,
            };
            div()
                .id(("more-stamp", key))
                .debug_selector(move || format!("more-stamp-{key}"))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(5.))
                .hover(move |s| s.bg(hover))
                .child(
                    div()
                        .w(px(40.))
                        .h(px(20.))
                        .child(preview(curve.clone(), line)),
                )
                .child(div().flex_1().child(name.clone()))
                .children(delete)
                .on_click(cx.listener(move |this, _, _, _| {
                    this.open_menu = None;
                    this.apply_stamp(stamp);
                }))
        });
        menu::float(
            menu::panel(theme)
                .min_w(px(200.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .children(rows),
        )
    }
}

/// A stamp's id among the cards: built-in ones first, then saved ones.
fn stamp_key(stamp: StampRef) -> usize {
    match stamp {
        StampRef::BuiltIn(i) => i,
        StampRef::Saved(i) => 1000 + i,
    }
}

/// A small drawing of a stamp's shape.
fn preview(curve: Curve, line: gpui::Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let mut fill = line;
            fill.a = 0.16;
            crate::parameters::paint_curve(window, bounds, 1., line, Some((fill, 0.5)), |x| {
                let width = f64::from(bounds.size.width / px(1.)).max(1.);
                daw_automation::curve_value(&curve.points, x / width)
            });
        },
    )
    .w_full()
    .flex_1()
}

/// A row of choices, the chosen one lit. Each choice: label, chosen,
/// enabled, tooltip, and what choosing it does.
fn segments<F: Fn(&mut ProjectView) + 'static>(
    theme: &Theme,
    name: &'static str,
    choices: Vec<(&'static str, bool, bool, &'static str, F)>,
    cx: &mut Context<ProjectView>,
) -> Div {
    let hover = theme.hover;
    let buttons: Vec<_> = choices
        .into_iter()
        .enumerate()
        .map(|(i, (label, chosen, enabled, tip, choose))| {
            div()
                .id((name, i))
                .debug_selector(move || format!("{name}-{i}"))
                .px(px(8.))
                .py(px(2.))
                .rounded(px(5.))
                .whitespace_nowrap()
                .when(chosen, |d| {
                    d.bg(theme.control_selected).text_color(theme.text)
                })
                .when(!chosen && enabled, |d| {
                    d.text_color(theme.text_secondary)
                        .hover(move |s| s.bg(hover))
                })
                .when(!enabled, |d| d.text_color(theme.text_tertiary))
                .child(label)
                .tooltip(tooltip::text(tip))
                .when(enabled, |d| {
                    d.on_click(cx.listener(move |this, _, _, _| choose(this)))
                })
        })
        .collect();
    div()
        .flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(7.))
        .bg(theme.control)
        .children(buttons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_edit::Grid;

    #[test]
    fn a_window_holds_as_many_cells_as_the_grid_gives() {
        let snap = |grid| Snap {
            enabled: true,
            grid,
        };
        assert_eq!(cells(Ticks(3840), snap(Grid::Sixteenth)), 16);
        assert_eq!(cells(Ticks(480), snap(Grid::Quarter)), 1, "at least one");
        assert_eq!(cells(Ticks(15360), snap(Grid::Bar)), 4);
    }

    #[test]
    fn cards_that_dont_fit_go_in_the_more_menu() {
        let full = |n: usize| 2. * STRIP_PADDING + n as f32 * (CARD_WIDTH + CARD_GAP) - CARD_GAP;
        assert_eq!(cards_that_fit(full(6), 6), 6, "all fit");
        assert_eq!(cards_that_fit(f32::INFINITY, 9), 9);
        // One short: room is kept for the more button.
        let shown = cards_that_fit(full(6) - 1., 6);
        assert!(shown < 6);
        assert!(full(shown) + MORE_WIDTH + CARD_GAP <= full(6) - 1.);
    }

    #[test]
    fn new_stamps_take_the_first_free_number() {
        let named = |name: &str| Stamp {
            name: name.to_owned(),
            curve: Curve::line(0., 1.),
        };
        assert_eq!(next_stamp_name(&[]), "Stamp 1");
        assert_eq!(
            next_stamp_name(&[named("Stamp 1"), named("Stamp 3")]),
            "Stamp 2"
        );
    }
}
