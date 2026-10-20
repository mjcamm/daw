//! The Parameters page: the selected node's parameters that have movement,
//! each with its layers.
//!
//! The left column lists devices in three sections (Global, Instrument,
//! Effects). Under a device, each parameter with layers forms a set: a
//! result row showing the final value over time, then its layers, listed
//! like image-editor layers (the top one applies last). The right side shows
//! the same rows on the timeline.
//!
//! Blocks are edited on the layer lanes: double-click empty space for a new
//! block, drag it to move, drag its edges to show more or less of it (the
//! contents stay put), drag the corner handle to repeat it. A selected
//! block shows its points and a settings bar. A drag is previewed here and
//! becomes one edit when the mouse is released.

use crate::block_edit::{
    EDGE_ZONE, Hit, LOOP_HANDLE, Room, Setting, Snap, add_point, decide, drag_block, edge_at,
    has_loop_handle, hit_test, loop_band, move_point, new_block, next_block_id, phase_at, point_at,
    point_positions, remove_point, room_around, scrub, value_to_y, window_hit_test, y_to_value,
};
use crate::frame::{Timeline, grid_lines};
use crate::icons::{Icon, icon};
use crate::layout::{BEATS_PER_BAR, PIXELS_PER_BEAT, timeline_bars};
use crate::live::{Live, PaneKind};
use crate::menu::{self, OpenMenu};
use crate::project::{Page, ProjectView, RenameTarget, song_clips};
use crate::scroll;
use crate::slider;
use crate::theme::{ContentColor, MONO_FONT, Theme};
use crate::tooltip;
use crate::zoom::Zoom;
use daw_automation::{Automation, CompiledBlock, Signals};
use daw_engine::{
    BlendMode, Block, BlockId, Command, DeviceRef, DeviceSnapshot, Edit, Layer, LayerId,
    NodeSnapshot, ParamId, ParamSnapshot, SignalId, SignalSource, Snapshot, Ticks,
};
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Div, FontWeight, Hsla, Modifiers, MouseButton,
    MouseDownEvent, PathBuilder, Pixels, Point, SharedString, Window, canvas, div, fill, point,
    prelude::*, px, size,
};
use std::borrow::Cow;
use std::rc::Rc;

const LEFT_WIDTH: f32 = 320.;
const RULER_HEIGHT: f32 = 28.;
const SECTION_HEIGHT: f32 = 30.;
const DEVICE_HEIGHT: f32 = 30.;
/// Starting zoom: result rows and layer rows are all one lane height.
pub(crate) const DEFAULT_ZOOM: Zoom = Zoom::new(PIXELS_PER_BEAT, 56.);
const ADD_HEIGHT: f32 = 26.;
const SETTINGS_HEIGHT: f32 = 34.;
/// Gap between a lane's edges and its blocks.
const BLOCK_MARGIN: f32 = 3.;
/// From a lane's top to the top of a block's drawing box: the margin plus
/// the block's 1 px outline.
const BLOCK_INSET: f32 = BLOCK_MARGIN + 1.;
/// Radius of a drawn shape point.
const POINT_DOT: f32 = 3.;
/// One line of text in a two-line row.
const LINE_HEIGHT: f32 = 20.;
/// How long a layer jumped to from elsewhere stays highlighted.
pub(crate) const HIGHLIGHT_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

const BLEND_MODES: [BlendMode; 2] = [BlendMode::Add, BlendMode::Multiply];

fn mode_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Add => "Add",
        BlendMode::Multiply => "Multiply",
    }
}

/// One row of the page, shown in both the left column and the timeline.
enum Row<'a> {
    Section(&'static str),
    Device(&'a DeviceSnapshot),
    NoEffects,
    /// A parameter with layers: shows the final result.
    Set(&'a ParamSnapshot),
    /// A layer of a set, with its position in the engine.s bottom-to-top
    /// stack.
    Layer {
        param: ParamId,
        index: usize,
        id: LayerId,
        layer: &'a Layer,
    },
    AddLayer(&'a ParamSnapshot),
    AddParam(&'a DeviceSnapshot),
}

impl Row<'_> {
    /// The row's height with lanes `lane` pixels tall.
    fn height(&self, lane: f32) -> f32 {
        match self {
            Self::Section(_) => SECTION_HEIGHT,
            Self::Device(_) => DEVICE_HEIGHT,
            Self::Set(_) | Self::Layer { .. } => lane,
            Self::NoEffects | Self::AddLayer(_) | Self::AddParam(_) => ADD_HEIGHT,
        }
    }
}

/// The rows for a node, in display order.
fn rows(node: &NodeSnapshot) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let find = |device| node.devices.iter().find(|d| d.device == device);
    rows.push(Row::Section("Global"));
    if let Some(channel) = find(DeviceRef::Channel) {
        push_device(&mut rows, channel);
    }
    let instruments: Vec<_> = node
        .devices
        .iter()
        .filter(|d| matches!(d.device, DeviceRef::Instrument(_)))
        .collect();
    if !instruments.is_empty() {
        rows.push(Row::Section("Instruments"));
    }
    for instrument in instruments {
        push_device(&mut rows, instrument);
    }
    rows.push(Row::Section("Effects"));
    let effects: Vec<_> = node
        .devices
        .iter()
        .filter(|d| matches!(d.device, DeviceRef::Effect(_)))
        .collect();
    if effects.is_empty() {
        rows.push(Row::NoEffects);
    }
    for effect in effects {
        push_device(&mut rows, effect);
    }
    rows
}

fn push_device<'a>(rows: &mut Vec<Row<'a>>, device: &'a DeviceSnapshot) {
    rows.push(Row::Device(device));
    for param in device.params.iter().filter(|p| !p.layers.is_empty()) {
        rows.push(Row::Set(param));
        // Like image-editor layers: the last applied is listed first.
        for (index, (id, layer)) in param.layers.iter().enumerate().rev() {
            rows.push(Row::Layer {
                param: param.id,
                index,
                id: *id,
                layer,
            });
        }
        rows.push(Row::AddLayer(param));
    }
    rows.push(Row::AddParam(device));
}

/// Every layer in the project.
fn all_layers(snapshot: &Snapshot) -> impl Iterator<Item = &(LayerId, Layer)> {
    snapshot
        .nodes
        .iter()
        .flat_map(|n| &n.devices)
        .flat_map(|d| &d.params)
        .flat_map(|p| &p.layers)
}

/// A lane of blocks: a layer's on the Parameters page, or a drawn signal's
/// on the Signals page. Both are edited the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    Layer(LayerId),
    Signal(SignalId),
}

impl Lane {
    /// The page this lane is edited on.
    fn page(self) -> Page {
        match self {
            Self::Layer(_) => Page::Parameters,
            Self::Signal(_) => Page::Signals,
        }
    }
}

/// Every lane in the project with its blocks: layers, then drawn signals.
fn all_lanes(snapshot: &Snapshot) -> impl Iterator<Item = (Lane, &[(BlockId, Block)])> {
    let layers = all_layers(snapshot).map(|(id, l)| (Lane::Layer(*id), l.blocks.as_slice()));
    let signals = snapshot.signals.iter().filter_map(|s| match &s.source {
        SignalSource::Drawn(blocks) => Some((Lane::Signal(s.id), blocks.as_slice())),
        SignalSource::Notes(_) => None,
    });
    layers.chain(signals)
}

/// A lane's blocks, if the lane exists.
pub(crate) fn lane_blocks(snapshot: &Snapshot, lane: Lane) -> Option<&[(BlockId, Block)]> {
    all_lanes(snapshot)
        .find(|(l, _)| *l == lane)
        .map(|(_, b)| b)
}

/// The signal a lane's layer listens to, if any. Signal lanes never listen.
pub(crate) fn listened_signal(snapshot: &Snapshot, lane: Lane) -> Option<SignalId> {
    match lane {
        Lane::Layer(id) => all_layers(snapshot)
            .find(|(layer, _)| *layer == id)
            .and_then(|(_, layer)| layer.signal),
        Lane::Signal(_) => None,
    }
}

/// A block and the lane holding it.
fn find_block(snapshot: &Snapshot, id: BlockId) -> Option<(Lane, &Block)> {
    all_lanes(snapshot).find_map(|(lane, blocks)| {
        blocks
            .iter()
            .find(|(block_id, _)| *block_id == id)
            .map(|(_, block)| (lane, block))
    })
}

/// An id no block in the project uses yet, on layers or signals.
fn next_block_id_in(snapshot: &Snapshot) -> BlockId {
    next_block_id(all_lanes(snapshot).flat_map(|(_, blocks)| blocks.iter().map(|(id, _)| id)))
}

/// `blocks` with the block being dragged shown at its preview.
fn previewed_blocks<'a>(
    drag: Option<&BlockDrag>,
    blocks: &'a [(BlockId, Block)],
) -> Cow<'a, [(BlockId, Block)]> {
    match drag {
        Some(drag) if blocks.iter().any(|(id, _)| *id == drag.id) => Cow::Owned(
            blocks
                .iter()
                .map(|(id, block)| {
                    let block = if *id == drag.id { &drag.preview } else { block };
                    (*id, block.clone())
                })
                .collect(),
        ),
        _ => Cow::Borrowed(blocks),
    }
}

/// `layers` with the block being dragged shown at its preview.
fn previewed_layers<'a>(
    drag: Option<&BlockDrag>,
    layers: &'a [(LayerId, Layer)],
) -> Cow<'a, [(LayerId, Layer)]> {
    match drag {
        Some(drag)
            if layers
                .iter()
                .any(|(_, l)| l.blocks.iter().any(|(id, _)| *id == drag.id)) =>
        {
            Cow::Owned(
                layers
                    .iter()
                    .map(|(id, layer)| {
                        let blocks = previewed_blocks(Some(drag), &layer.blocks).into_owned();
                        (
                            *id,
                            Layer {
                                blocks,
                                ..layer.clone()
                            },
                        )
                    })
                    .collect(),
            )
        }
        _ => Cow::Borrowed(layers),
    }
}

/// A custom cursor drawn on a lane, since GPUI has no loop or four-way
/// move cursor: the system cursor hides and an icon is drawn at the pointer.
#[derive(Debug, Clone, Copy)]
struct LaneCursor {
    kind: CursorKind,
    /// Pointer position within the lane's row.
    x: f32,
    y: f32,
}

/// What the pointer shows over a lane: the block under it (its loop
/// handle shows) and the cursor. A pane redraws when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneHover {
    lane: Lane,
    block: Option<BlockId>,
    kind: Option<CursorKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorKind {
    Loop,
    Move,
    /// An edge or a cycle end: the system's sideways cursor.
    Resize,
}

/// Size of a custom cursor icon.
const CURSOR_ICON: f32 = 16.;

/// What a drag changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragKind {
    /// Part of a block on its lane.
    Block(Hit),
    /// A point sitting on an edge: the first movement decides which one is
    /// dragged (sideways: the edge; up or down: the point).
    Undecided { point: usize, edge: Hit },
    /// A setting in the settings bar.
    Setting(Setting),
}

/// A block being dragged. The engine is untouched until the mouse is
/// released, so a whole drag is one undo step.
#[derive(Debug, Clone)]
pub(crate) struct BlockDrag {
    id: BlockId,
    kind: DragKind,
    /// Pointer position when the drag began.
    origin: Point<Pixels>,
    original: Block,
    preview: Block,
    /// Free space around the block on its lane.
    room: Room,
    /// Window y of the top of the block's drawing box, and its height.
    box_top: Pixels,
    box_height: f32,
}

/// A parameter's resting value being dragged with its slider.
#[derive(Debug, Clone)]
pub(crate) struct BaseDrag {
    param: ParamId,
    origin_y: Pixels,
    original: f64,
    preview: f64,
}

impl BaseDrag {
    /// A resting-value drag starting at `origin_y` (dragging up raises it).
    pub(crate) fn new(param: ParamId, origin_y: Pixels, original: f64) -> Self {
        Self {
            param,
            origin_y,
            original,
            preview: original,
        }
    }
}

/// A layer's mix being dragged with its slider.
#[derive(Debug, Clone)]
pub(crate) struct MixDrag {
    layer: LayerId,
    origin_y: Pixels,
    original: f64,
    preview: f64,
}

/// `layers` with the mix being dragged shown at its preview.
fn previewed_mix<'a>(
    drag: Option<&MixDrag>,
    layers: Cow<'a, [(LayerId, Layer)]>,
) -> Cow<'a, [(LayerId, Layer)]> {
    match drag {
        Some(drag) if layers.iter().any(|(id, _)| *id == drag.layer) => Cow::Owned(
            layers
                .iter()
                .map(|(id, layer)| {
                    let mix = if *id == drag.layer {
                        drag.preview
                    } else {
                        layer.mix
                    };
                    (
                        *id,
                        Layer {
                            mix,
                            ..layer.clone()
                        },
                    )
                })
                .collect(),
        ),
        _ => layers,
    }
}

/// Length of a row's vertical slider: most of the row's height.
fn row_slider_length(row_height: f32) -> f32 {
    (row_height - 10.).max(16.)
}

/// An id no layer in the project uses yet.
fn next_layer_id(snapshot: &Snapshot) -> LayerId {
    let highest = snapshot
        .nodes
        .iter()
        .flat_map(|n| &n.devices)
        .flat_map(|d| &d.params)
        .flat_map(|p| &p.layers)
        .map(|(id, _)| id.0)
        .max();
    LayerId(highest.map_or(1, |h| h + 1))
}

impl ProjectView {
    pub(crate) fn render_parameters(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let pointer = window.mouse_position();
        let snapshot = self.session.snapshot();
        let Some(node) = self.shown_node() else {
            return div().into_any_element();
        };
        let color = theme.content(node.color);
        let bars = timeline_bars(song_clips(snapshot), snapshot.loop_range);
        let zoom = self.parameters_view.zoom;
        let width = zoom.beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let rows = rows(node);
        // Rows far off screen are empty boxes of their height, in both
        // columns (`paint::visible_y`).
        let near_y = crate::paint::visible_y(&self.parameters_view.y);
        let mut near = Vec::with_capacity(rows.len());
        let mut y = 0.;
        for row in &rows {
            let height = row.height(zoom.row_height);
            near.push(near_y.is_none_or(|(above, below)| y + height >= above && y <= below));
            y += height;
        }
        let blank = |row: &Row<'_>| {
            div()
                .flex_none()
                .h(px(row.height(zoom.row_height)))
                .into_any_element()
        };

        let left = div()
            .w(px(LEFT_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children(rows.iter().enumerate().map(|(i, row)| {
                if near[i] {
                    self.left_cell(i, row, theme, cx)
                } else {
                    blank(row)
                }
            }));

        // Drawn by the live layer, from beat 0 across the lanes.
        let playhead = self
            .live_slot(
                PaneKind::Main,
                Live::Playhead {
                    start: 0.,
                    ppb: zoom.pixels_per_beat,
                },
            )
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .right_0();
        let mut top = 0.;
        let right_cells: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let cell = if near[i] {
                    self.right_cell(row, top, color, theme, width, pointer, cx)
                } else {
                    blank(row)
                };
                top += row.height(zoom.row_height);
                cell
            })
            .collect();
        let lanes = scroll::axis_locked(div())
            .id("parameter-lanes")
            // Leaving the lanes drops any lane's hover look.
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.lane_hover.take().is_some() {
                    this.hover_changed(cx);
                }
            }))
            .flex_1()
            .min_w_0()
            // A row, so the content stretches to the full height.
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.parameters_view.x)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .child(self.lanes_tracker())
                    .children(grid_lines(zoom, bars, theme))
                    .children(right_cells)
                    .child(playhead),
            );

        // The ruler follows the lanes' horizontal scroll.
        let scroll_x = self.parameters_view.x.offset().x;
        let top = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(theme.hairline)
            .child(
                div()
                    .w(px(LEFT_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .bg(theme.sidebar)
                    .border_r_1()
                    .border_color(theme.edge)
                    .text_size(px(11.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text_tertiary)
                            .child("PARAMETERS"),
                    )
                    .child(
                        div()
                            .text_color(theme.text_secondary)
                            .child(node.name.clone()),
                    ),
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
                    .border_color(theme.hairline),
            );

        let settings = self.selected_settings(theme, cx);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top)
            .child(
                self.timeline_frame(
                    Timeline::Parameters,
                    scroll::axis_locked(div())
                        .id("parameter-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.parameters_view.y)
                        .flex()
                        .flex_col()
                        .child(
                            // Grows to fill the page when there are few rows (so
                            // the column and grid reach the bottom) and beyond it
                            // when there are many. Percentage heights do not work
                            // inside a vertical scroll, so this uses flex growth.
                            div()
                                .flex()
                                .flex_grow()
                                .flex_shrink_0()
                                .child(left)
                                .child(lanes),
                        )
                        .into_any_element(),
                    LEFT_WIDTH,
                    theme,
                    cx,
                ),
            )
            .children(settings)
            .into_any_element()
    }

    /// The selected block, if it still exists and is on the current page:
    /// its id, lane and contents.
    pub(crate) fn selected_block<'a>(
        &self,
        snapshot: &'a Snapshot,
    ) -> Option<(BlockId, Lane, &'a Block)> {
        let id = self.selected_block?;
        find_block(snapshot, id)
            .filter(|(lane, _)| lane.page() == self.page)
            .map(|(lane, block)| (id, lane, block))
    }

    /// The selected block's settings bar, if one is selected on this page.
    pub(crate) fn selected_settings(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let snapshot = self.session.snapshot();
        let (id, lane, block) = self.selected_block(snapshot)?;
        let shown = match &self.block_drag {
            Some(drag) if drag.id == id => &drag.preview,
            _ => block,
        };
        let windows = listened_signal(snapshot, lane).is_some();
        Some(self.settings_bar(id, shown, windows, theme, cx))
    }

    /// Shows a layer on the Parameters page: scrolled into view and
    /// highlighted for a moment.
    pub(crate) fn go_to_layer(&mut self, layer: LayerId) {
        let snapshot = self.session.snapshot();
        let Some(node) = snapshot.nodes.iter().find(|n| {
            n.devices
                .iter()
                .flat_map(|d| &d.params)
                .any(|p| p.layers.iter().any(|(id, _)| *id == layer))
        }) else {
            return;
        };
        self.page = Page::Parameters;
        self.selected_node = Some(node.id);
        let lane = self.parameters_view.zoom.row_height;
        let top: f32 = rows(node)
            .iter()
            .take_while(|row| !matches!(row, Row::Layer { id, .. } if *id == layer))
            .map(|row| row.height(lane))
            .sum();
        // A little of the rows above stays in view, for context.
        let offset = (top - 2. * lane).max(0.);
        self.parameters_view
            .y
            .set_offset(point(px(0.), px(-offset)));
        self.highlight = Some((layer, std::time::Instant::now()));
    }

    /// Whether `layer` was just jumped to.
    fn highlighted(&self, layer: LayerId) -> bool {
        let on = self
            .highlight
            .is_some_and(|(id, at)| id == layer && at.elapsed() < HIGHLIGHT_TIME);
        if on {
            // Redraw until the highlight has faded.
            self.animate();
        }
        on
    }

    /// The zoom of the page whose lanes are being edited.
    fn lane_zoom(&self) -> Zoom {
        match self.page {
            Page::Signals => self.signals_view.zoom,
            _ => self.parameters_view.zoom,
        }
    }

    /// Records where the lanes are drawn, to turn presses into positions.
    pub(crate) fn lanes_tracker(&self) -> impl IntoElement {
        let bounds_cell = self.lanes_bounds.clone();
        canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            |_, (), _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// The custom cursor for a lane, if the pointer is over (or dragging) a
    /// block's loop handle or one of its points. `top` and `height` place
    /// the lane's row; `windows` is true on a listening layer.
    fn lane_cursor(
        &self,
        blocks: &[(BlockId, Block)],
        windows: bool,
        top: f32,
        height: f32,
        pointer: Point<Pixels>,
    ) -> Option<LaneCursor> {
        let lanes = self.lanes_bounds.get();
        let zoom = self.lane_zoom();
        let x = (pointer.x - lanes.origin.x) / px(1.);
        let y = (pointer.y - lanes.origin.y) / px(1.) - top;
        let kind_of = |hit: Hit| match hit {
            Hit::Loop => Some(CursorKind::Loop),
            Hit::Point(_) => Some(CursorKind::Move),
            Hit::Start | Hit::End | Hit::CycleEnd => Some(CursorKind::Resize),
            Hit::Body => None,
        };
        let at = |kind| LaneCursor { kind, x, y };
        // While dragging, the cursor stays what the drag began as.
        if let Some(drag) = &self.block_drag {
            return blocks
                .iter()
                .any(|(id, _)| *id == drag.id)
                .then_some(drag.kind)
                .and_then(|kind| match kind {
                    DragKind::Block(hit) => kind_of(hit),
                    DragKind::Undecided { .. } => Some(CursorKind::Move),
                    DragKind::Setting(_) => None,
                })
                .map(at);
        }
        if !(0.0..height).contains(&y) {
            return None;
        }
        let (id, block) = blocks.iter().find(|(_, b)| {
            let left = zoom.beat_to_x(b.start.as_beats());
            (left..left + zoom.beat_to_x(b.length.as_beats())).contains(&x)
        })?;
        let local_x = x - zoom.beat_to_x(block.start.as_beats());
        let hit = if windows {
            window_hit_test(block, local_x, zoom.pixels_per_beat)
        } else {
            hit_test(
                block,
                local_x,
                y - BLOCK_INSET,
                zoom.pixels_per_beat,
                height - 2. * BLOCK_INSET,
                self.selected_block == Some(*id),
            )
        };
        kind_of(hit).map(at)
    }

    /// The snap to use with `modifiers` held: Alt flips snap and slide.
    fn snap_for(&self, modifiers: Modifiers) -> Snap {
        if modifiers.alt {
            self.snap.inverted()
        } else {
            self.snap
        }
    }

    /// A press on a lane whose row starts `top` pixels down the lanes and is
    /// `height` tall.
    fn lane_mouse_down(&mut self, lane: Lane, top: f32, height: f32, event: &MouseDownEvent) {
        let snapshot = self.session.snapshot();
        let Some(blocks) = lane_blocks(snapshot, lane) else {
            return;
        };
        let windows = listened_signal(snapshot, lane).is_some();
        let lanes = self.lanes_bounds.get();
        let x = (event.position.x - lanes.origin.x) / px(1.);
        let zoom = self.lane_zoom();
        let ppb = zoom.pixels_per_beat;
        let at = Ticks::from_beats_f64(zoom.x_to_beats(x));
        let snap = self.snap_for(event.modifiers);
        let box_top = lanes.origin.y + px(top + BLOCK_INSET);
        let box_height = height - 2. * BLOCK_INSET;

        let Some((id, block)) = blocks
            .iter()
            .find(|(_, b)| b.start <= at && at < b.end())
            .cloned()
        else {
            self.selected_block = None;
            if event.click_count >= 2
                && let Some(block) = new_block(blocks, at, snap)
            {
                let id = next_block_id_in(snapshot);
                let edit = match lane {
                    Lane::Layer(layer) => Edit::AddBlock { layer, id, block },
                    Lane::Signal(signal) => Edit::AddSignalBlock { signal, id, block },
                };
                self.session.apply(Command::Edit(edit));
                self.selected_block = Some(id);
            }
            return;
        };

        let local_x = x - zoom.beat_to_x(block.start.as_beats());
        let local_y = (event.position.y - box_top) / px(1.);
        let was_selected = self.selected_block == Some(id);
        let hit = if windows {
            window_hit_test(&block, local_x, ppb)
        } else {
            hit_test(&block, local_x, local_y, ppb, box_height, was_selected)
        };
        let room = room_around(blocks, Some(id), block.start);
        self.selected_block = Some(id);
        // Windows have no points: the signal decides what plays through.
        if !windows && event.click_count >= 2 && was_selected && !matches!(hit, Hit::Point(_)) {
            let at = phase_at(&block, local_x, ppb);
            let (shape, _) = add_point(&block.shape, at, y_to_value(local_y, box_height));
            self.session.apply(Command::Edit(Edit::SetBlock {
                id,
                block: Block { shape, ..block },
            }));
            return;
        }
        let kind = match (hit, edge_at(&block, local_x, ppb)) {
            (Hit::Point(point), Some(edge)) => DragKind::Undecided { point, edge },
            _ => DragKind::Block(hit),
        };
        self.block_drag = Some(BlockDrag {
            id,
            kind,
            origin: event.position,
            preview: block.clone(),
            original: block,
            room,
            box_top,
            box_height,
        });
    }

    /// A right-click on a lane: removes the point under the pointer from the
    /// selected block.
    fn lane_right_click(&mut self, top: f32, height: f32, event: &MouseDownEvent) {
        let snapshot = self.session.snapshot();
        let Some((id, lane, block)) = self.selected_block(snapshot) else {
            return;
        };
        if listened_signal(snapshot, lane).is_some() {
            return;
        }
        let lanes = self.lanes_bounds.get();
        let zoom = self.lane_zoom();
        let x =
            (event.position.x - lanes.origin.x) / px(1.) - zoom.beat_to_x(block.start.as_beats());
        let y = (event.position.y - lanes.origin.y) / px(1.) - top - BLOCK_INSET;
        let removed = point_at(block, x, y, zoom.pixels_per_beat, height - 2. * BLOCK_INSET)
            .and_then(|index| remove_point(&block.shape, index));
        if let Some(shape) = removed {
            let block = Block {
                shape,
                ..block.clone()
            };
            self.session
                .apply(Command::Edit(Edit::SetBlock { id, block }));
        }
    }

    /// Begins dragging a setting in the settings bar.
    fn start_scrub(&mut self, id: BlockId, setting: Setting, origin: Point<Pixels>) {
        let Some((_, block)) = find_block(self.session.snapshot(), id) else {
            return;
        };
        self.block_drag = Some(BlockDrag {
            id,
            kind: DragKind::Setting(setting),
            origin,
            original: block.clone(),
            preview: block.clone(),
            room: Room {
                start: Ticks::ZERO,
                end: None,
            },
            box_top: px(0.),
            box_height: 0.,
        });
    }

    /// Updates the preview of whatever is being dragged.
    pub(crate) fn drag_to(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        if self.drag_frame(position)
            || self.drag_inspector(position, modifiers.shift)
            || self.drag_clips(position, modifiers)
            || self.drag_mixer(position, modifiers.shift)
            || self.drag_note_editor(position, modifiers)
            || self.drag_audio_editor(position, modifiers.shift)
            || self.drag_combined(position, modifiers)
        {
            return;
        }
        if let Some(drag) = &mut self.base_drag {
            let up = (drag.origin_y - position.y) / px(1.);
            drag.preview = slider::dragged(drag.original, up, modifiers.shift);
            return;
        }
        if let Some(drag) = &mut self.mix_drag {
            let up = (drag.origin_y - position.y) / px(1.);
            drag.preview = slider::dragged(drag.original, up, modifiers.shift);
            return;
        }
        let snap = self.snap_for(modifiers);
        let ppb = self.lane_zoom().pixels_per_beat;
        let Some(drag) = &mut self.block_drag else {
            return;
        };
        let dx = (position.x - drag.origin.x) / px(1.);
        if let DragKind::Undecided { point, edge } = drag.kind {
            let dy = (position.y - drag.origin.y) / px(1.);
            match decide(dx, dy) {
                Some(true) => drag.kind = DragKind::Block(edge),
                Some(false) => drag.kind = DragKind::Block(Hit::Point(point)),
                None => return,
            }
        }
        let original = &drag.original;
        drag.preview = match drag.kind {
            DragKind::Block(Hit::Point(index)) => {
                let cycle = original.cycle.unwrap_or(original.length).as_beats();
                let at = original.shape.points.get(index).map_or(0., |p| p.at)
                    + f64::from(dx / ppb) / cycle;
                let value = y_to_value((position.y - drag.box_top) / px(1.), drag.box_height);
                Block {
                    shape: move_point(&original.shape, index, at, value),
                    ..original.clone()
                }
            }
            DragKind::Block(hit) => drag_block(original, hit, dx, ppb, snap, drag.room),
            DragKind::Setting(setting) => scrub(original, setting, dx, snap),
            DragKind::Undecided { .. } => original.clone(),
        };
    }

    /// Ends a drag, sending the change as one edit.
    pub(crate) fn finish_drag(&mut self) {
        self.frame_drag = None;
        self.finish_inspector_drag();
        self.finish_clip_drag();
        self.finish_mixer_drag();
        self.finish_note_drag();
        self.finish_audio_drag();
        self.finish_combined_drag();
        if let Some(drag) = self.mix_drag.take()
            && drag.preview != drag.original
        {
            self.session.apply(Command::Edit(Edit::SetLayerMix {
                id: drag.layer,
                mix: drag.preview,
            }));
        }
        if let Some(drag) = self.base_drag.take()
            && drag.preview != drag.original
        {
            self.session.apply(Command::Edit(Edit::SetParam {
                param: drag.param,
                value: drag.preview,
            }));
        }
        if let Some(drag) = self.block_drag.take()
            && drag.preview != drag.original
        {
            self.session.apply(Command::Edit(Edit::SetBlock {
                id: drag.id,
                block: drag.preview,
            }));
        }
    }

    /// The resting value being dragged for `param`, if it is.
    pub(crate) fn base_preview(&self, param: ParamId) -> Option<f64> {
        self.base_drag
            .as_ref()
            .filter(|d| d.param == param)
            .map(|d| d.preview)
    }

    /// A parameter's resting value as shown: the slider's preview while it
    /// is being dragged.
    fn base_of(&self, param: &ParamSnapshot) -> f64 {
        match &self.base_drag {
            Some(drag) if drag.param == param.id => drag.preview,
            _ => param.base,
        }
    }

    /// A vertical slider for a parameter's resting value (its default
    /// state). Drag up or down; hold Shift for fine control.
    fn base_slider(
        &self,
        param: &ParamSnapshot,
        base: f64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let param_id = param.id;
        let original = param.base;
        let dragging = self.base_drag.as_ref().is_some_and(|d| d.param == param_id);
        let id = SharedString::from(format!(
            "base-slider-{:?}-{}",
            param_id.device, param_id.index
        ));
        let length = row_slider_length(self.parameters_view.zoom.row_height);
        slider::slider(
            id,
            slider::Axis::Vertical,
            base as f32,
            length,
            dragging,
            theme,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.base_drag = Some(BaseDrag {
                    param: param_id,
                    origin_y: event.position.y,
                    original,
                    preview: original,
                });
            }),
        )
    }

    /// A layer's mix as shown: the slider's preview while it is dragged.
    fn mix_of(&self, layer: LayerId, mix: f64) -> f64 {
        match &self.mix_drag {
            Some(drag) if drag.layer == layer => drag.preview,
            _ => mix,
        }
    }

    /// A vertical slider for how much of a layer reaches the result, with
    /// its percentage. Drag up or down (Shift for fine); double-click for
    /// full.
    fn mix_slider(
        &self,
        layer: LayerId,
        mix: f64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let original = mix;
        let mix = self.mix_of(layer, mix);
        let dragging = self.mix_drag.as_ref().is_some_and(|d| d.layer == layer);
        let length = row_slider_length(self.parameters_view.zoom.row_height);
        let control = slider::slider(
            ("layer-mix", layer.0),
            slider::Axis::Vertical,
            mix as f32,
            length,
            dragging,
            theme,
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                if event.click_count >= 2 {
                    this.mix_drag = None;
                    this.session.apply(Command::Edit(Edit::SetLayerMix {
                        id: layer,
                        mix: 1.0,
                    }));
                    return;
                }
                this.mix_drag = Some(MixDrag {
                    layer,
                    origin_y: event.position.y,
                    original,
                    preview: original,
                });
            }),
        );
        div()
            .flex()
            .items_center()
            .gap(px(3.))
            .child(control)
            .child(
                div()
                    .w(px(28.))
                    .font_family(MONO_FONT)
                    .text_size(px(10.))
                    .text_color(theme.text_tertiary)
                    .child(format!("{:.0}%", mix * 100.)),
            )
    }

    /// Deletes the selected block (Delete or Backspace); on Arrange, the
    /// selected clips, or with none the selected node.
    pub(crate) fn delete_selection(&mut self) {
        if self.block_drag.is_some() {
            return;
        }
        if self.page == Page::Arrange {
            // Selected notes in the editor go first, then selected clips;
            // with neither, the selected node.
            if !self.delete_notes()
                && !self.delete_clips()
                && !self.delete_combined()
                && let Some(id) = self.selected_node
            {
                self.delete_node(id);
            }
            return;
        }
        if let Some((id, _, _)) = self.selected_block(self.session.snapshot()) {
            self.selected_block = None;
            self.session.apply(Command::Edit(Edit::RemoveBlock { id }));
        }
    }

    /// The selected block's settings: drag a value sideways to change it.
    /// A window (on a listening layer) has no repeat: the signal decides
    /// what plays through it.
    fn settings_bar(
        &self,
        id: BlockId,
        block: &Block,
        windows: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let hover = theme.hover;
        let settings = Setting::ALL
            .into_iter()
            .filter(|s| !(windows && *s == Setting::Cycle));
        let fields = settings.map(|setting| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .child(setting.label()),
                )
                .child(
                    div()
                        .id(("block-setting", setting as usize))
                        .min_w(px(58.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(theme.control)
                        .hover(move |s| s.bg(hover))
                        .cursor_ew_resize()
                        .font_family(MONO_FONT)
                        .text_size(px(11.))
                        .child(setting.format(block))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                this.start_scrub(id, setting, event.position);
                            }),
                        ),
                )
        });
        let inverted = block.invert;
        let mut active = theme.accent;
        active.a = 0.18;
        let invert = div()
            .id("block-invert")
            .px(px(8.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(if inverted { active } else { theme.control })
            .hover(move |s| s.bg(hover))
            .text_color(if inverted {
                theme.accent
            } else {
                theme.text_secondary
            })
            .child("Invert")
            .on_click(cx.listener(move |this, _, _, _| {
                if let Some((_, block)) = find_block(this.session.snapshot(), id) {
                    let block = Block {
                        invert: !block.invert,
                        ..block.clone()
                    };
                    this.session
                        .apply(Command::Edit(Edit::SetBlock { id, block }));
                }
            }));

        div()
            .h(px(SETTINGS_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(18.))
            .px(px(12.))
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.edge)
            .text_size(px(12.))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("BLOCK"),
            )
            .children(fields)
            .child(invert)
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.text_tertiary)
                    .child(if windows {
                        "A window onto the signal · Alt flips snap"
                    } else {
                        "Double-click to add a point · right-click to remove · Alt flips snap"
                    }),
            )
    }

    fn left_cell(
        &self,
        index: usize,
        row: &Row<'_>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cell = div()
            .h(px(row.height(self.parameters_view.zoom.row_height)))
            .flex_none()
            .flex()
            .items_center()
            // The same split as on the lanes, so each layer reads as one band.
            .when(matches!(row, Row::Set(_) | Row::Layer { .. }), |d| {
                d.border_b_2().border_color(theme.edge)
            });
        match row {
            Row::Section(label) => cell
                .px(px(12.))
                .pt(px(8.))
                .when(index > 0, |d| d.border_t_1().border_color(theme.hairline))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_tertiary)
                .child(label.to_uppercase())
                .into_any_element(),
            Row::Device(device) => cell
                .mx(px(8.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(theme.control)
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(device.name.clone())
                .into_any_element(),
            Row::NoEffects => cell
                .px(px(16.))
                .text_size(px(12.))
                .text_color(theme.text_tertiary)
                .child("No effects on this node")
                .into_any_element(),
            Row::Set(param) => {
                let base = self.base_of(param);
                let layers =
                    previewed_mix(self.mix_drag.as_ref(), Cow::Borrowed(&param.layers[..]));
                // The value at the song position, drawn by the live layer.
                let automation = Rc::new(Automation::compile_layers_with(
                    base,
                    &layers,
                    &self.compiled_signals(),
                ));
                let shown_param = (*param).clone();
                let value = self.live_slot(
                    PaneKind::Main,
                    Live::Value {
                        automation,
                        format: Rc::new(move |v| shown_param.format_normalized(v)),
                    },
                );
                cell.pl(px(16.))
                    .pr(px(12.))
                    .gap(px(8.))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(param.name.clone()),
                    )
                    .child(self.base_slider(param, base, theme, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .w(px(56.))
                            .h(px(16.))
                            .child(value.size_full()),
                    )
                    .into_any_element()
            }
            Row::Layer {
                param,
                index,
                id,
                layer,
                ..
            } => self
                .layer_header(*param, *index, *id, layer, cell, theme, cx)
                .into_any_element(),
            Row::AddLayer(param) => {
                let param_id = param.id;
                let open = self.open_menu == Some(OpenMenu::AddLayer(param_id));
                let button =
                    quiet_action(cell.pl(px(32.)), ("add-layer", index), "Add Layer", theme)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, _| {
                                this.toggle_menu(OpenMenu::AddLayer(param_id));
                            }),
                        );
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(button)
                    .when(open, |d| {
                        d.child(menu::float(self.layer_kind_panel(
                            param_id,
                            param.layers.len(),
                            theme,
                            cx,
                        )))
                    })
                    .into_any_element()
            }
            Row::AddParam(device) => {
                let open = self.open_menu == Some(OpenMenu::AddParam(device.device));
                let device_ref = device.device;
                let button = quiet_action(
                    cell.pl(px(16.)),
                    ("add-param", index),
                    "Add Parameter",
                    theme,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, _| {
                        this.toggle_menu(OpenMenu::AddParam(device_ref));
                    }),
                );
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(button)
                    .when(open, |d| d.child(self.add_param_menu(device, theme, cx)))
                    .into_any_element()
            }
        }
    }

    /// Name, blend mode, on/off and delete for one layer.
    #[allow(clippy::too_many_arguments, reason = "plain row data, passed through")]
    fn layer_header(
        &self,
        param: ParamId,
        index: usize,
        id: LayerId,
        layer: &Layer,
        cell: Div,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (mode, enabled) = (layer.mode, layer.enabled);
        let name: AnyElement = match self.rename_input(RenameTarget::Layer(id)) {
            Some(input) => div()
                .flex_1()
                .text_size(px(13.))
                .child(input)
                .into_any_element(),
            None => div()
                .id(("layer-name", id.0))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(if enabled {
                    theme.text
                } else {
                    theme.text_tertiary
                })
                .child(if layer.name.is_empty() {
                    "Layer".to_owned()
                } else {
                    layer.name.clone()
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 {
                        this.start_rename(RenameTarget::Layer(id), window, cx);
                    }
                }))
                .into_any_element(),
        };

        let mode_open = self.open_menu == Some(OpenMenu::BlendMode(id));
        let hover = theme.hover;
        let mode_chip = div()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("layer-mode", id.0))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .pl(px(6.))
                    .pr(px(3.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(theme.control)
                    .hover(move |s| s.bg(hover))
                    .text_size(px(11.))
                    .text_color(theme.text_secondary)
                    .child(mode_label(mode))
                    .child(icon(Icon::ChevronDown, theme.text_tertiary, 11.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, _| {
                            this.toggle_menu(OpenMenu::BlendMode(id));
                        }),
                    ),
            )
            .when(mode_open, |d| {
                d.child(self.blend_mode_menu(id, mode, enabled, theme, cx))
            });

        // What the layer is: drawn, or listening to a signal (fixed when the
        // layer was added). Hover explains; the arrow shows the signal.
        let source: AnyElement = match layer
            .signal
            .and_then(|s| self.session.snapshot().signals.iter().find(|x| x.id == s))
        {
            None => div()
                .id(("layer-source", id.0))
                .flex()
                .items_center()
                .gap(px(3.))
                .px(px(5.))
                .py(px(1.))
                .rounded(px(4.))
                .text_size(px(11.))
                .text_color(theme.text_tertiary)
                .child(icon(Icon::Pencil, theme.text_tertiary, 11.))
                .child("Drawn")
                .tooltip(tooltip::text(
                    "Manual drawn: the blocks on this layer draw their own shapes.",
                ))
                .into_any_element(),
            Some(signal) => {
                let signal_id = signal.id;
                let colors = theme.content(signal.color);
                div()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .child(
                        div()
                            .id(("layer-source", id.0))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(colors.fill)
                            .text_size(px(11.))
                            .text_color(colors.detail)
                            .child(icon(Icon::Inbound, colors.detail, 11.))
                            .child(div().max_w(px(72.)).truncate().child(signal.name.clone()))
                            .tooltip(tooltip::text(format!(
                                "Listens to {}: each block is a window that lets the signal through, at its depth, flipped by invert.",
                                signal.name
                            ))),
                    )
                    .child(
                        small_button(("layer-go-to-signal", id.0), theme)
                            .size(px(18.))
                            .child(icon(Icon::GoTo, theme.text_secondary, 11.))
                            .tooltip(tooltip::text(format!(
                                "Show {} on the Signals page",
                                signal.name
                            )))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.go_to_signal(signal_id);
                            })),
                    )
                    .into_any_element()
            }
        };

        let toggle = small_button(("layer-toggle", id.0), theme)
            .on_click(cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::SetLayerOptions {
                    id,
                    mode,
                    enabled: !enabled,
                }));
            }))
            .child(icon(
                if enabled { Icon::Eye } else { Icon::EyeOff },
                if enabled {
                    theme.text_secondary
                } else {
                    theme.text_tertiary
                },
                13.,
            ));

        let delete = small_button(("layer-delete", id.0), theme)
            .on_click(cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::RemoveLayer { id }));
            }))
            .child(icon(Icon::Close, theme.text_secondary, 12.));

        let dragged = DraggedLayer {
            param,
            id,
            name: layer.name.clone(),
        };
        let grip = div()
            .id(("layer-grip", id.0))
            .cursor_move()
            .child(icon(Icon::GripVertical, theme.text_tertiary, 13.))
            .on_drag(dragged, |dragged: &DraggedLayer, _, _, cx| {
                cx.new(|_| dragged.clone())
            });
        let mut drop_highlight = theme.accent;
        drop_highlight.a = 0.18;

        cell.id(("layer-row", id.0))
            .when(self.highlighted(id), |d| d.bg(drop_highlight))
            .pl(px(18.))
            .pr(px(8.))
            .gap(px(6.))
            .drag_over::<DraggedLayer>(move |style, dragged, _, _| {
                if dragged.param == param {
                    style.bg(drop_highlight)
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedLayer, _, _| {
                // Layers only move within their own parameter.
                if dragged.param == param && dragged.id != id {
                    this.session.apply(Command::Edit(Edit::MoveLayer {
                        id: dragged.id,
                        index,
                    }));
                }
            }))
            .child(grip)
            // Two lines: the name, then what the layer is and how it blends.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().h(px(LINE_HEIGHT)).flex().items_center().child(name))
                    .child(
                        div()
                            .h(px(LINE_HEIGHT))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(source)
                            .child(mode_chip),
                    ),
            )
            .child(self.mix_slider(id, layer.mix, theme, cx))
            // Show/hide and delete sit on the first line.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .h(px(LINE_HEIGHT))
                            .flex()
                            .items_center()
                            .child(toggle)
                            .child(delete),
                    )
                    .child(div().h(px(LINE_HEIGHT))),
            )
    }

    fn blend_mode_menu(
        &self,
        id: LayerId,
        current: BlendMode,
        enabled: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items = BLEND_MODES.map(|mode| {
            menu::item(
                theme,
                ("blend-mode", mode as usize),
                mode_label(mode),
                if mode == current { "✓" } else { "" },
            )
            .on_click(cx.listener(move |this, _, _, _| {
                this.open_menu = None;
                this.session
                    .apply(Command::Edit(Edit::SetLayerOptions { id, mode, enabled }));
            }))
        });
        menu::float(
            menu::panel(theme)
                .min_w(px(150.))
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .children(items),
        )
    }

    /// Picks a parameter of `device` that has no layers yet, then its first
    /// layer's type.
    fn add_param_menu(
        &self,
        device: &DeviceSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if let Some(param) = self.pending_param.filter(|p| p.device == device.device) {
            return menu::float(self.layer_kind_panel(param, 0, theme, cx));
        }
        let available: Vec<_> = device
            .params
            .iter()
            .filter(|p| p.layers.is_empty())
            .map(|p| {
                let param = p.id;
                menu::item(
                    theme,
                    ("add-param-item", p.id.index as usize),
                    p.name.clone(),
                    "",
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    this.pending_param = Some(param);
                    this.layer_menu = LayerMenu::Kind;
                }))
            })
            .collect();
        let empty = available.is_empty();
        menu::float(
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .children(available)
                .when(empty, |d| {
                    d.child(
                        div()
                            .px(px(10.))
                            .py(px(5.))
                            .text_color(theme.text_tertiary)
                            .child("Every parameter has layers"),
                    )
                }),
        )
    }

    /// The menu for adding a layer to `param` (which has `count` layers):
    /// first its type, then, to listen, which signal. The type is fixed
    /// once the layer exists.
    fn layer_kind_panel(
        &self,
        param: ParamId,
        count: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let panel =
            menu::panel(theme).on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()));
        match self.layer_menu {
            LayerMenu::Kind => panel
                .child(
                    menu_row(
                        theme,
                        "layer-kind-drawn",
                        Icon::Pencil,
                        "Manual drawn",
                        None,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.open_menu = None;
                        let name = format!("Layer {}", count + 1);
                        this.add_layer(param, &name, None);
                    })),
                )
                .child(
                    menu_row(
                        theme,
                        "layer-kind-signal",
                        Icon::Inbound,
                        "Listen to signal",
                        Some(Icon::ChevronRight),
                    )
                    .on_click(cx.listener(|this, _, _, _| {
                        this.layer_menu = LayerMenu::Signals;
                    })),
                )
                .child(menu::separator(theme))
                .child(
                    div()
                        .px(px(10.))
                        .py(px(3.))
                        .text_size(px(11.))
                        .text_color(theme.text_tertiary)
                        .child("A layer keeps its type once added."),
                ),
            LayerMenu::Signals => {
                let signals = &self.session.snapshot().signals;
                let back = menu_row(theme, "layer-kind-back", Icon::ChevronLeft, "Back", None)
                    .on_click(cx.listener(|this, _, _, _| {
                        this.layer_menu = LayerMenu::Kind;
                    }));
                let items = signals.iter().map(|signal| {
                    let id = signal.id;
                    let dot = theme.content(signal.color).detail;
                    let hover = theme.hover;
                    div()
                        .id(("layer-kind-pick", id.0))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(5.))
                        .hover(move |s| s.bg(hover))
                        .child(div().size(px(8.)).rounded(px(4.)).bg(dot))
                        .child(signal.name.clone())
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.open_menu = None;
                            let name = format!("Layer {}", count + 1);
                            this.add_layer(param, &name, Some(id));
                        }))
                });
                let empty = signals.is_empty();
                panel
                    .child(back)
                    .child(menu::separator(theme))
                    .children(items)
                    .when(empty, |d| {
                        d.child(
                            menu_row(
                                theme,
                                "layer-kind-none",
                                Icon::GoTo,
                                "No signals yet: open Signals",
                                None,
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                this.open_menu = None;
                                this.page = Page::Signals;
                            })),
                        )
                    })
            }
        }
    }

    /// Adds a layer at the bottom of the parameter's stack, beside the add
    /// button: drawn, or listening to `signal`.
    fn add_layer(&mut self, param: ParamId, name: &str, signal: Option<SignalId>) {
        let id = next_layer_id(self.session.snapshot());
        let mut layer = Layer::named(name, BlendMode::Add);
        layer.signal = signal;
        self.session.apply(Command::Edit(Edit::AddLayer {
            param,
            id,
            index: 0,
            layer,
        }));
    }
}

/// Which step the add-layer menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerMenu {
    /// Manual drawn, or listen to a signal.
    Kind,
    /// Which signal to listen to.
    Signals,
}

/// A menu row with an icon, a label and an optional trailing icon.
fn menu_row(
    theme: &Theme,
    id: &'static str,
    leading: Icon,
    label: &'static str,
    trailing: Option<Icon>,
) -> gpui::Stateful<Div> {
    let hover = theme.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(5.))
        .hover(move |s| s.bg(hover))
        .child(icon(leading, theme.text_secondary, 13.))
        .child(div().flex_1().child(label))
        .children(trailing.map(|t| icon(t, theme.text_tertiary, 13.)))
}

/// A small square icon button.
pub(crate) fn small_button(id: impl Into<gpui::ElementId>, theme: &Theme) -> gpui::Stateful<Div> {
    let hover = theme.hover;
    div()
        .id(id)
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .hover(move |s| s.bg(hover))
}

/// A quiet "+ Label" row action.
pub(crate) fn quiet_action(
    cell: Div,
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    theme: &Theme,
) -> gpui::Stateful<Div> {
    let text = theme.text;
    cell.id(id)
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(theme.text_tertiary)
        .hover(move |s| s.text_color(text))
        .child(icon(Icon::Plus, theme.text_tertiary, 12.))
        .child(label)
}

impl ProjectView {
    /// The timeline side of a row that starts `top` pixels down the lanes.
    #[allow(clippy::too_many_arguments, reason = "plain row data, passed through")]
    fn right_cell(
        &self,
        row: &Row<'_>,
        top: f32,
        color: ContentColor,
        theme: &Theme,
        width: f32,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = self.parameters_view.zoom;
        let height = row.height(zoom.row_height);
        let cell = div().h(px(height)).flex_none().w(px(width)).relative();
        let drag = self.block_drag.as_ref();
        match row {
            Row::Set(param) => {
                let layers = previewed_mix(
                    self.mix_drag.as_ref(),
                    previewed_layers(drag, &param.layers),
                );
                let base = self.base_of(param);
                let automation =
                    Automation::compile_layers_with(base, &layers, &self.compiled_signals());
                let (line, area) = (color.detail, color.fill);
                let mut rest_line = color.detail;
                rest_line.a *= 0.35;
                cell.bg(theme.window)
                    .border_b_2()
                    .border_color(theme.edge)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, (), window, _| {
                                // The resting value: where every layer's
                                // centre line lands.
                                let height = bounds.size.height / px(1.);
                                let rest = Bounds::new(
                                    point(
                                        bounds.origin.x,
                                        bounds.origin.y + px(value_to_y(base, height) - 0.5),
                                    ),
                                    size(bounds.size.width, px(1.)),
                                );
                                window.paint_quad(fill(rest, rest_line));
                                paint_curve(
                                    window,
                                    bounds,
                                    zoom.pixels_per_beat,
                                    line,
                                    Some((area, 0.)),
                                    |beat| automation.value_at(beat),
                                );
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
            Row::Layer { id, layer, .. } => {
                // A listening layer takes its signal's colour.
                let signal_color = layer.signal.and_then(|s| {
                    let signals = &self.session.snapshot().signals;
                    signals.iter().find(|x| x.id == s).map(|x| x.color)
                });
                let colors = match (layer.enabled, signal_color) {
                    (false, _) => theme.muted_content,
                    (true, Some(c)) => theme.content(c),
                    (true, None) => color,
                };
                let mix = self.mix_of(*id, layer.mix);
                self.lane_cell(
                    cell,
                    Lane::Layer(*id),
                    &layer.blocks,
                    colors,
                    mix,
                    layer.signal,
                    top,
                    pointer,
                    theme,
                    cx,
                )
                .into_any_element()
            }
            _ => cell.into_any_element(),
        }
    }

    /// Draws a lane of editable blocks into `cell`, a row that starts `top`
    /// pixels down the lanes: its centre line, blocks (with any drag
    /// previewed), presses and custom cursor. On a layer listening to
    /// `signal`, the blocks are windows showing the signal.
    #[allow(clippy::too_many_arguments, reason = "plain row data, passed through")]
    pub(crate) fn lane_cell(
        &self,
        cell: Div,
        lane: Lane,
        blocks: &[(BlockId, Block)],
        colors: ContentColor,
        mix: f64,
        signal: Option<SignalId>,
        top: f32,
        pointer: Point<Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let zoom = self.lane_zoom();
        let height = zoom.row_height;
        let blocks = previewed_blocks(self.block_drag.as_ref(), blocks);
        let cursor = self.lane_cursor(&blocks, signal.is_some(), top, height, pointer);
        // What the pointer shows over this lane: redraw only when it
        // changes, or always while a drawn cursor icon follows it.
        let hover_blocks = blocks.to_vec();
        let windows = signal.is_some();
        let hover = move |this: &mut ProjectView, at: Point<Pixels>| -> LaneHover {
            let zoom = this.lane_zoom();
            let lanes = this.lanes_bounds.get();
            let x = (at.x - lanes.origin.x) / px(1.);
            let block = hover_blocks
                .iter()
                .find(|(_, b)| {
                    let left = zoom.beat_to_x(b.start.as_beats());
                    (left..left + zoom.beat_to_x(b.length.as_beats())).contains(&x)
                })
                .map(|(id, _)| *id);
            let kind = this
                .lane_cursor(&hover_blocks, windows, top, height, at)
                .map(|c| c.kind);
            LaneHover { lane, block, kind }
        };
        // Only blocks near the screen get elements (`paint::visible_x`).
        let near_x = crate::paint::visible_x(match self.page {
            Page::Signals => &self.signals_view.x,
            _ => &self.parameters_view.x,
        });
        let near = |block: &Block| {
            near_x.is_none_or(|(left, right)| {
                zoom.beat_to_x(block.end().as_beats()) >= left
                    && zoom.beat_to_x(block.start.as_beats()) <= right
            })
        };
        let window = signal.map(|id| (self.compiled_signals(), id));
        // The centre line: no effect.
        let centre = BLOCK_INSET + value_to_y(0.5, height - 2. * BLOCK_INSET);
        cell.border_b_2()
            .border_color(theme.edge)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(centre - 0.5))
                    .h(px(1.))
                    .bg(theme.grid),
            )
            .child({
                // Every block near the screen, painted in one drawing.
                let lanes = self.lanes_bounds.get();
                let (pointer_x, pointer_y) = (
                    (pointer.x - lanes.origin.x) / px(1.),
                    (pointer.y - lanes.origin.y) / px(1.) - top,
                );
                let painted: Vec<PaintedBlock> = blocks
                    .iter()
                    .filter(|(_, block)| near(block))
                    .map(|(id, block)| {
                        let left = zoom.beat_to_x(block.start.as_beats());
                        let right = left + zoom.beat_to_x(block.length.as_beats());
                        let hovered = (0.0..height).contains(&pointer_y)
                            && (left..right).contains(&pointer_x);
                        painted_block(
                            block,
                            colors,
                            self.selected_block == Some(*id),
                            hovered,
                            mix,
                            zoom,
                            height - 2. * BLOCK_INSET,
                            window.clone(),
                            theme,
                        )
                    })
                    .collect();
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        for block in &painted {
                            paint_block(block, bounds, window, cx);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            })
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    let now = hover(this, event.position);
                    let follows = matches!(now.kind, Some(CursorKind::Loop | CursorKind::Move));
                    if follows || this.lane_hover != Some(now) {
                        this.lane_hover = Some(now);
                        this.hover_changed(cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.lane_mouse_down(lane, top, height, event);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.lane_right_click(top, height, event);
                }),
            )
            .when_some(cursor, |d, cursor| {
                // The system cursor is hidden; this icon takes its place.
                let (glyph, tint) = match cursor.kind {
                    CursorKind::Loop => (Icon::Loop, theme.accent),
                    CursorKind::Move => (Icon::Move, colors.detail),
                    CursorKind::Resize => return d.cursor_ew_resize(),
                };
                d.cursor_none(gpui::CursorStyle::Arrow).child(
                    div()
                        .absolute()
                        .left(px(cursor.x - CURSOR_ICON / 2.))
                        .top(px(cursor.y - CURSOR_ICON / 2.))
                        .child(icon(glyph, tint, CURSOR_ICON)),
                )
            })
    }
}

/// A block on a layer lane, ready to paint (`paint_block`): its span, the
/// shape it draws (or the signal through a window), its repeats, and its
/// points when selected. Presses and the cursor are the lane's, from the
/// same hit test (`lane_mouse_down`, `lane_cursor`).
struct PaintedBlock {
    left: f32,
    width: f32,
    selected: bool,
    /// Under the pointer: the loop handle shows.
    hovered: bool,
    colors: ContentColor,
    is_window: bool,
    band: f32,
    edge: f32,
    mix: f64,
    ppb: f32,
    compiled: CompiledBlock,
    signal: Option<(Rc<Signals>, SignalId)>,
    /// Where each cycle of a looped block ends and the next repeat begins.
    cycle_ends: Vec<f32>,
    points: Vec<(f32, f64)>,
    repeat_wash: Hsla,
    cycle_end: Hsla,
    outline: Hsla,
}

#[allow(
    clippy::too_many_arguments,
    reason = "plain drawing data, passed through"
)]
fn painted_block(
    block: &Block,
    colors: ContentColor,
    selected: bool,
    hovered: bool,
    mix: f64,
    zoom: Zoom,
    box_height: f32,
    window: Option<(Rc<Signals>, SignalId)>,
    theme: &Theme,
) -> PaintedBlock {
    // A window shows its signal, has no points to edit and does not loop.
    let is_window = window.is_some();
    let ppb = zoom.pixels_per_beat;
    let width = zoom.beat_to_x(block.length.as_beats());
    let compiled = CompiledBlock::compile(block);
    let start = compiled.start;
    let cycle_ends: Vec<f32> = if !is_window && block.cycle.is_some_and(|c| c < block.length) {
        let (cycle, nudge, length) = (compiled.cycle, compiled.nudge, compiled.end - start);
        let first = (nudge / cycle).floor() as i64 + 1;
        (first..)
            .map(|k| k as f64 * cycle - nudge)
            .take_while(|beat| *beat < length - 1e-9)
            .filter(|beat| *beat > 1e-9)
            .map(|beat| zoom.beat_to_x(beat))
            .collect()
    } else {
        Vec::new()
    };
    let points: Vec<(f32, f64)> = if selected && !is_window {
        point_positions(block, ppb)
            .into_iter()
            .map(|(index, x)| (x, block.shape.points[index].value))
            .collect()
    } else {
        Vec::new()
    };
    let mut repeat_wash = theme.window;
    repeat_wash.a = 0.45;
    let mut cycle_end = colors.detail;
    cycle_end.a *= 0.7;
    PaintedBlock {
        left: zoom.beat_to_x(block.start.as_beats()),
        width,
        selected,
        hovered,
        colors,
        is_window,
        band: loop_band(box_height),
        edge: EDGE_ZONE.min(width / 4.),
        mix,
        ppb,
        compiled,
        signal: window,
        cycle_ends,
        points,
        repeat_wash,
        cycle_end,
        outline: if selected {
            theme.accent
        } else {
            colors.outline
        },
    }
}

/// Paints a block on its lane, `lane` being the lane's box.
fn paint_block(b: &PaintedBlock, lane: Bounds<Pixels>, window: &mut Window, cx: &mut gpui::App) {
    let rect = Bounds::new(
        point(lane.left() + px(b.left), lane.top() + px(BLOCK_MARGIN)),
        size(px(b.width), lane.size.height - px(2. * BLOCK_MARGIN)),
    );
    if !rect.intersects(&window.content_mask().bounds) {
        return;
    }
    window.paint_quad(
        fill(rect, b.colors.fill)
            .corner_radii(px(4.))
            .border_widths(px(1.))
            .border_color(b.outline),
    );
    // Inside the border, as the block's contents were.
    let bounds = Bounds::new(
        rect.origin + point(px(1.), px(1.)),
        size(rect.size.width - px(2.), rect.size.height - px(2.)),
    );
    let mask = bounds.intersect(&window.content_mask().bounds);
    window.with_content_mask(Some(gpui::ContentMask { bounds: mask }), |window| {
        let height = bounds.size.height / px(1.);
        let centre = Bounds::new(
            point(
                bounds.origin.x,
                bounds.origin.y + px(value_to_y(0.5, height) - 0.5),
            ),
            size(bounds.size.width, px(1.)),
        );
        let mut centre_color = b.colors.outline;
        centre_color.a *= 0.4;
        window.paint_quad(fill(centre, centre_color));
        let start = b.compiled.start;
        let signal = b.signal.as_ref().and_then(|(signals, id)| signals.get(*id));
        // Faint: what the layer actually does, with depth, fades and mix
        // applied, around the centre line.
        let mut faint = b.colors.detail;
        faint.a *= 0.4;
        let (compiled, mix) = (&b.compiled, b.mix);
        paint_curve(window, bounds, b.ppb, faint, None, |beat| {
            let beat = start + beat;
            if compiled.contains(beat) {
                let (x, w) = compiled.sample(beat, signal);
                0.5 + (x - 0.5) * w * mix
            } else {
                f64::NAN
            }
        });
        // The shape as drawn, or the signal through the window.
        paint_curve(window, bounds, b.ppb, b.colors.detail, None, |beat| {
            let beat = start + beat;
            if compiled.contains(beat) {
                compiled.sample(beat, signal).0
            } else {
                f64::NAN
            }
        });
        // Repeats of a looped block are softer, and every cycle shows its
        // end: block, block, block.
        if let Some(first) = b.cycle_ends.first() {
            let soft = Bounds::new(
                point(bounds.origin.x + px(*first), bounds.origin.y),
                size(bounds.size.width - px(*first), bounds.size.height),
            );
            window.paint_quad(fill(soft, b.repeat_wash));
        }
        for x in &b.cycle_ends {
            let line = Bounds::new(
                point(bounds.origin.x + px(*x - 0.5), bounds.origin.y),
                size(px(1.5), bounds.size.height),
            );
            window.paint_quad(fill(line, b.cycle_end));
        }
        for (x, value) in &b.points {
            let centre = point(
                bounds.origin.x + px(*x),
                bounds.origin.y + px(value_to_y(*value, height)),
            );
            let dot = Bounds::new(
                point(centre.x - px(POINT_DOT), centre.y - px(POINT_DOT)),
                size(px(2. * POINT_DOT), px(2. * POINT_DOT)),
            );
            window.paint_quad(fill(dot, b.colors.detail).corner_radii(px(POINT_DOT)));
        }
        // The loop handle in the top-right corner, while selected or under
        // the pointer.
        if !b.is_window && has_loop_handle(b.width) && (b.selected || b.hovered) {
            let glyph = b.band.min(10.);
            let handle_left = bounds.right() - px(b.edge + LOOP_HANDLE);
            let at = Bounds::new(
                point(
                    handle_left + px((LOOP_HANDLE - glyph) / 2.),
                    bounds.top() + px((b.band - glyph) / 2.),
                ),
                size(px(glyph), px(glyph)),
            );
            let _ = window.paint_svg(
                at,
                Icon::Loop.path().into(),
                gpui::TransformationMatrix::unit(),
                b.colors.detail,
                cx,
            );
        }
    });
}

/// Draws `value(beat)` (0..=1, NaN for "nothing here") across `bounds`, with
/// beat 0 at the left edge. Optionally fills between the line and a
/// baseline value (0 is the bottom, 0.5 the centre line).
pub(crate) fn paint_curve(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    pixels_per_beat: f32,
    line: Hsla,
    fill: Option<(Hsla, f64)>,
    value: impl Fn(f64) -> f64,
) {
    let width = bounds.size.width / px(1.);
    let height = bounds.size.height / px(1.);
    let to_point = |x: f32, v: f64| -> Point<Pixels> {
        point(
            bounds.origin.x + px(x),
            bounds.origin.y + px(value_to_y(v, height)),
        )
    };
    let step = 2.;
    // Only the part on screen (and a step either side).
    let visible = window.content_mask().bounds;
    let from = ((visible.left() - bounds.left()) / px(1.) / step)
        .floor()
        .max(0.) as usize;
    let to = (((visible.right() - bounds.left()) / px(1.)).min(width) / step).ceil() as usize + 1;
    let points: Vec<Point<Pixels>> = (from..=to.max(from))
        .filter_map(|i| {
            let x = (i as f32 * step).min(width);
            let v = value(f64::from(x / pixels_per_beat));
            (!v.is_nan()).then(|| to_point(x, v.clamp(0., 1.)))
        })
        .collect();
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    if let Some((fill, baseline)) = fill {
        let bottom = bounds.origin.y + px(value_to_y(baseline, height));
        let mut area = PathBuilder::fill();
        area.move_to(point(first.x, bottom));
        points.iter().for_each(|p| area.line_to(*p));
        area.line_to(point(last.x, bottom));
        area.close();
        if let Ok(path) = area.build() {
            window.paint_path(path, fill);
        }
    }
    let mut stroke = PathBuilder::stroke(px(1.5));
    stroke.move_to(*first);
    points.iter().skip(1).for_each(|p| stroke.line_to(*p));
    if let Ok(path) = stroke.build() {
        window.paint_path(path, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Engine, EngineConfig, FIRST_NODE, Shape, Ticks};

    fn snapshot_with(edits: Vec<Edit>) -> Snapshot {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        for edit in edits {
            engine.apply(Command::Edit(edit)).unwrap();
        }
        engine.snapshot()
    }

    fn describe(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Section(s) => format!("section {s}"),
                Row::Device(d) => format!("device {}", d.name),
                Row::NoEffects => "no effects".to_owned(),
                Row::Set(p) => format!("set {}", p.name),
                Row::Layer { layer, .. } => format!("layer {}", layer.name),
                Row::AddLayer(_) => "+ layer".to_owned(),
                Row::AddParam(d) => format!("+ add to {}", d.name),
            })
            .collect()
    }

    const VOLUME: ParamId = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Channel,
        index: 0,
    };

    #[test]
    fn an_untouched_node_lists_its_devices_only() {
        let snapshot = snapshot_with(vec![]);
        assert_eq!(
            describe(&rows(snapshot.node(FIRST_NODE).unwrap())),
            [
                "section Global",
                "device Channel",
                "+ add to Channel",
                "section Instruments",
                "device Synth",
                "+ add to Synth",
                "section Effects",
                "no effects",
            ]
        );
    }

    #[test]
    fn layers_list_top_of_stack_first() {
        let add = |id: u64, index: usize, name: &str| Edit::AddLayer {
            param: VOLUME,
            id: LayerId(id),
            index,
            layer: Layer::named(name, BlendMode::Add),
        };
        let snapshot = snapshot_with(vec![add(1, 0, "Bottom"), add(2, 1, "Top")]);
        let described = describe(&rows(snapshot.node(FIRST_NODE).unwrap()));
        assert_eq!(
            &described[1..6],
            [
                "device Channel",
                "set Volume",
                "layer Top",
                "layer Bottom",
                "+ layer"
            ]
        );
        assert_eq!(next_layer_id(&snapshot), LayerId(3));
    }

    #[test]
    fn layer_ids_start_at_one() {
        assert_eq!(next_layer_id(&snapshot_with(vec![])), LayerId(1));
    }

    fn layer_with_block() -> Vec<Edit> {
        vec![
            Edit::AddLayer {
                param: VOLUME,
                id: LayerId(1),
                index: 0,
                layer: Layer::named("Layer 1", BlendMode::Add),
            },
            Edit::AddBlock {
                layer: LayerId(1),
                id: BlockId(4),
                block: Block::new(Ticks::ZERO, Ticks::from_beats(4), Shape::constant(0.5)),
            },
        ]
    }

    #[test]
    fn signal_blocks_share_the_id_pool_and_layers_know_what_they_hear() {
        let mut edits = layer_with_block();
        edits.extend([
            Edit::AddSignal {
                id: SignalId(2),
                signal: daw_engine::Signal {
                    name: "Sweep".to_owned(),
                    color: 0,
                    source: SignalSource::Drawn(Vec::new()),
                },
                listeners: Vec::new(),
            },
            Edit::AddSignalBlock {
                signal: SignalId(2),
                id: BlockId(9),
                block: Block::new(Ticks::ZERO, Ticks::from_beats(1), Shape::constant(0.5)),
            },
            Edit::AddLayer {
                param: VOLUME,
                id: LayerId(2),
                index: 0,
                layer: Layer::named("Hear", BlendMode::Add).listening(SignalId(2)),
            },
        ]);
        let snapshot = snapshot_with(edits);
        assert_eq!(
            next_block_id_in(&snapshot),
            BlockId(10),
            "signal blocks count"
        );
        let (lane, _) = find_block(&snapshot, BlockId(9)).unwrap();
        assert_eq!(lane, Lane::Signal(SignalId(2)));
        assert_eq!(
            listened_signal(&snapshot, Lane::Layer(LayerId(2))),
            Some(SignalId(2))
        );
        assert_eq!(listened_signal(&snapshot, Lane::Layer(LayerId(1))), None);
        assert_eq!(listened_signal(&snapshot, lane), None);
    }

    #[test]
    fn blocks_are_found_with_their_layer() {
        let snapshot = snapshot_with(layer_with_block());
        let (layer, block) = find_block(&snapshot, BlockId(4)).unwrap();
        assert_eq!(
            (layer, block.length),
            (Lane::Layer(LayerId(1)), Ticks::from_beats(4))
        );
        assert!(find_block(&snapshot, BlockId(5)).is_none());
        assert_eq!(next_block_id_in(&snapshot), BlockId(5));
    }

    #[test]
    fn result_rows_and_layer_rows_share_the_lane_height() {
        let snapshot = snapshot_with(layer_with_block());
        let heights: Vec<f32> = rows(snapshot.node(FIRST_NODE).unwrap())
            .iter()
            .filter(|r| matches!(r, Row::Set(_) | Row::Layer { .. }))
            .map(|r| r.height(55.))
            .collect();
        assert_eq!(heights, [55., 55.]);
    }

    #[test]
    fn grid_lines_thin_out_when_zoomed_out() {
        assert_eq!(crate::layout::grid_bar_step(24.), 1);
        assert_eq!(crate::layout::grid_bar_step(1.), 2);
        assert_eq!(crate::layout::grid_bar_step(0.25), 8);
    }

    #[test]
    fn a_drag_previews_only_its_own_block() {
        let snapshot = snapshot_with(layer_with_block());
        let layers = &snapshot
            .node(FIRST_NODE)
            .unwrap()
            .devices
            .last()
            .unwrap()
            .params[0]
            .layers;
        let original = layers[0].1.blocks[0].1.clone();
        let mut preview = original.clone();
        preview.start = Ticks::from_beats(2);
        let drag = BlockDrag {
            id: BlockId(4),
            kind: DragKind::Block(Hit::Body),
            origin: point(px(0.), px(0.)),
            original,
            preview,
            room: Room {
                start: Ticks::ZERO,
                end: None,
            },
            box_top: px(0.),
            box_height: 0.,
        };
        let shown = previewed_layers(Some(&drag), layers);
        assert_eq!(shown[0].1.blocks[0].1.start, Ticks::from_beats(2));
        let mut other = drag.clone();
        other.id = BlockId(9);
        assert!(matches!(
            previewed_layers(Some(&other), layers),
            Cow::Borrowed(_)
        ));
        assert!(matches!(previewed_layers(None, layers), Cow::Borrowed(_)));
    }

    #[test]
    fn a_mix_drag_previews_only_its_own_layer() {
        let snapshot = snapshot_with(layer_with_block());
        let layers = &snapshot
            .node(FIRST_NODE)
            .unwrap()
            .devices
            .last()
            .unwrap()
            .params[0]
            .layers;
        let drag = MixDrag {
            layer: LayerId(1),
            origin_y: px(0.),
            original: 1.0,
            preview: 0.3,
        };
        let shown = previewed_mix(Some(&drag), Cow::Borrowed(&layers[..]));
        assert_eq!(shown[0].1.mix, 0.3);
        let other = MixDrag {
            layer: LayerId(7),
            ..drag
        };
        assert!(matches!(
            previewed_mix(Some(&other), Cow::Borrowed(&layers[..])),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn block_contents_compile_for_drawing() {
        let mut layer = Layer::named("Layer 1", BlendMode::Add);
        layer.blocks.push((
            daw_engine::BlockId(1),
            daw_engine::Block::new(Ticks::ZERO, Ticks::from_beats(4), Shape::ramp(0., 1.)),
        ));
        let block = CompiledBlock::compile(&layer.blocks[0].1);
        assert!((block.sample(2.0, None).0 - 0.5).abs() < 1e-9);
    }
}

/// A layer being dragged to a new place in its stack. Also its own drag
/// preview: a small chip with the layer's name.
#[derive(Debug, Clone)]
struct DraggedLayer {
    param: ParamId,
    id: LayerId,
    name: String,
}

impl Render for DraggedLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.accent)
            .shadow_md()
            .text_size(px(12.))
            .text_color(theme.text)
            .child(self.name.clone())
    }
}
