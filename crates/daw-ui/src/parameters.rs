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
    EDGE_ZONE, Hit, LOOP_HANDLE, Room, Setting, Snap, add_point, drag_block, hit_test, move_point,
    new_block, next_block_id, phase_at, point_at, point_positions, remove_point, room_around,
    scrub, value_to_y, y_to_value,
};
use crate::icons::{Icon, icon};
use crate::layout::{BEATS_PER_BAR, PIXELS_PER_BEAT, beat_to_x, timeline_bars};
use crate::menu::{self, OpenMenu};
use crate::project::{Page, ProjectView, RenameTarget};
use crate::theme::{ContentColor, MONO_FONT, Theme};
use daw_automation::{Automation, CompiledBlock};
use daw_engine::{
    BlendMode, Block, BlockId, Command, DeviceRef, DeviceSnapshot, Edit, Layer, LayerId, ParamId,
    ParamSnapshot, Snapshot, Ticks,
};
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, DispatchPhase, Div, FontWeight, Hsla, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point,
    SharedString, Window, canvas, div, fill, point, prelude::*, px, size,
};
use std::borrow::Cow;

const LEFT_WIDTH: f32 = 260.;
const RULER_HEIGHT: f32 = 28.;
const SECTION_HEIGHT: f32 = 30.;
const DEVICE_HEIGHT: f32 = 30.;
const SET_HEIGHT: f32 = 48.;
const LAYER_HEIGHT: f32 = 40.;
/// The layer holding the selected block grows, to give room for its points.
const TALL_LAYER_HEIGHT: f32 = 96.;
const ADD_HEIGHT: f32 = 26.;
const SETTINGS_HEIGHT: f32 = 34.;
/// Gap between a lane's edges and its blocks.
const BLOCK_MARGIN: f32 = 3.;
/// From a lane's top to the top of a block's drawing box: the margin plus
/// the block's 1 px outline.
const BLOCK_INSET: f32 = BLOCK_MARGIN + 1.;
/// Radius of a drawn shape point.
const POINT_DOT: f32 = 3.;

const BLEND_MODES: [BlendMode; 5] = [
    BlendMode::Replace,
    BlendMode::Add,
    BlendMode::Subtract,
    BlendMode::Multiply,
    BlendMode::Modulate,
];

fn mode_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Replace => "Replace",
        BlendMode::Add => "Add",
        BlendMode::Subtract => "Subtract",
        BlendMode::Multiply => "Multiply",
        BlendMode::Modulate => "Modulate",
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
        /// Holds the selected block, so drawn taller.
        tall: bool,
    },
    AddLayer(&'a ParamSnapshot),
    AddParam(&'a DeviceSnapshot),
}

impl Row<'_> {
    fn height(&self) -> f32 {
        match self {
            Self::Section(_) => SECTION_HEIGHT,
            Self::Device(_) => DEVICE_HEIGHT,
            Self::Set(_) => SET_HEIGHT,
            Self::Layer { tall: true, .. } => TALL_LAYER_HEIGHT,
            Self::Layer { .. } => LAYER_HEIGHT,
            Self::NoEffects | Self::AddLayer(_) | Self::AddParam(_) => ADD_HEIGHT,
        }
    }
}

/// The rows for a node, in display order. `tall` is the layer holding the
/// selected block.
fn rows(snapshot: &Snapshot, tall: Option<LayerId>) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let find = |device| snapshot.devices.iter().find(|d| d.device == device);
    rows.push(Row::Section("Global"));
    if let Some(channel) = find(DeviceRef::Channel) {
        push_device(&mut rows, channel, tall);
    }
    rows.push(Row::Section("Instrument"));
    if let Some(instrument) = find(DeviceRef::Instrument) {
        push_device(&mut rows, instrument, tall);
    }
    rows.push(Row::Section("Effects"));
    let effects: Vec<_> = snapshot
        .devices
        .iter()
        .filter(|d| matches!(d.device, DeviceRef::Effect(_)))
        .collect();
    if effects.is_empty() {
        rows.push(Row::NoEffects);
    }
    for effect in effects {
        push_device(&mut rows, effect, tall);
    }
    rows
}

fn push_device<'a>(rows: &mut Vec<Row<'a>>, device: &'a DeviceSnapshot, tall: Option<LayerId>) {
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
                tall: tall == Some(*id),
            });
        }
        rows.push(Row::AddLayer(param));
    }
    rows.push(Row::AddParam(device));
}

/// Every layer in the project.
fn all_layers(snapshot: &Snapshot) -> impl Iterator<Item = &(LayerId, Layer)> {
    snapshot
        .devices
        .iter()
        .flat_map(|d| &d.params)
        .flat_map(|p| &p.layers)
}

/// A block and the layer holding it.
fn find_block(snapshot: &Snapshot, id: BlockId) -> Option<(LayerId, &Block)> {
    all_layers(snapshot).find_map(|(layer_id, layer)| {
        layer
            .blocks
            .iter()
            .find(|(block_id, _)| *block_id == id)
            .map(|(_, block)| (*layer_id, block))
    })
}

/// An id no block in the project uses yet.
fn next_block_id_in(snapshot: &Snapshot) -> BlockId {
    next_block_id(all_layers(snapshot).flat_map(|(_, l)| l.blocks.iter().map(|(id, _)| id)))
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

/// What a drag changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragKind {
    /// Part of a block on its lane.
    Block(Hit),
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

/// An id no layer in the project uses yet.
fn next_layer_id(snapshot: &Snapshot) -> LayerId {
    let highest = snapshot
        .devices
        .iter()
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let color = theme.content(snapshot.node.color);
        let bars = timeline_bars(snapshot.clips.iter().map(|(_, c)| c), snapshot.loop_range);
        let width = beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let position = self.session.position_beats();
        let selected = self.selected_block(snapshot);
        let rows = rows(snapshot, selected.map(|(_, layer, _)| layer));

        let left = div()
            .w(px(LEFT_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children(
                rows.iter()
                    .enumerate()
                    .map(|(i, row)| self.left_cell(i, row, position, theme, cx)),
            );

        let grid_lines = (0..bars * 4).map(|beat| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(beat_to_x(f64::from(beat))))
                .w(px(1.))
                .bg(if beat % 4 == 0 {
                    theme.hairline
                } else {
                    theme.grid
                })
        });
        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(beat_to_x(position)))
            .w(px(1.))
            .bg(theme.accent);
        let mut top = 0.;
        let right_cells: Vec<AnyElement> = rows
            .iter()
            .map(|row| {
                let cell = self.right_cell(row, top, color, theme, width, cx);
                top += row.height();
                cell
            })
            .collect();
        let lanes = div()
            .id("parameter-lanes")
            .flex_1()
            .min_w_0()
            // A row, so the content stretches to the full height.
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.parameters_scroll)
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .child(self.lanes_tracker(cx))
                    .children(grid_lines)
                    .children(right_cells)
                    .child(playhead),
            );

        // The ruler follows the lanes' horizontal scroll.
        let scroll_x = self.parameters_scroll.offset().x;
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
                            .child(snapshot.node.name.clone()),
                    ),
            )
            .child(
                div().flex_1().min_w_0().overflow_hidden().child(
                    div()
                        .relative()
                        .left(scroll_x)
                        .w(px(width))
                        .child(self.render_ruler(theme, bars, cx)),
                ),
            );

        let settings = selected.map(|(id, _, block)| {
            let shown = match &self.block_drag {
                Some(drag) if drag.id == id => &drag.preview,
                _ => block,
            };
            self.settings_bar(id, shown, theme, cx)
        });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top)
            .child(
                div()
                    .id("parameter-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .child(
                        // Grows to fill the page when there are few rows (so the
                        // column and grid reach the bottom) and beyond it when there
                        // are many. Percentage heights do not work inside a vertical
                        // scroll, so this uses flex growth instead.
                        div()
                            .flex()
                            .flex_grow()
                            .flex_shrink_0()
                            .child(left)
                            .child(lanes),
                    ),
            )
            .children(settings)
    }

    /// The selected block, if it still exists: its id, layer and contents.
    fn selected_block<'a>(&self, snapshot: &'a Snapshot) -> Option<(BlockId, LayerId, &'a Block)> {
        let id = self.selected_block?;
        find_block(snapshot, id).map(|(layer, block)| (id, layer, block))
    }

    /// Records where the lanes are drawn, and while a block is being dragged,
    /// follows the mouse anywhere in the window until it is released.
    fn lanes_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.lanes_bounds.clone();
        let dragging = self.block_drag.is_some();
        let view = cx.entity().downgrade();
        canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            move |_, (), window, _| {
                if !dragging {
                    return;
                }
                let moved = view.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = moved.update(cx, |this, cx| {
                            this.drag_to(event.position, event.modifiers);
                            cx.notify();
                        });
                    }
                });
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = view.update(cx, |this, cx| {
                            this.finish_drag();
                            cx.notify();
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// The snap to use with `modifiers` held: Alt flips snap and slide.
    fn snap_for(&self, modifiers: Modifiers) -> Snap {
        if modifiers.alt {
            self.snap.inverted()
        } else {
            self.snap
        }
    }

    /// A press on a layer lane whose row starts `top` pixels down the lanes
    /// and is `height` tall.
    fn lane_mouse_down(&mut self, layer: LayerId, top: f32, height: f32, event: &MouseDownEvent) {
        let snapshot = self.session.snapshot();
        let Some((_, lane)) = all_layers(snapshot).find(|(id, _)| *id == layer) else {
            return;
        };
        let lanes = self.lanes_bounds.get();
        let x = (event.position.x - lanes.origin.x) / px(1.);
        let at = Ticks::from_beats_f64(f64::from(x / PIXELS_PER_BEAT));
        let snap = self.snap_for(event.modifiers);
        let box_top = lanes.origin.y + px(top + BLOCK_INSET);
        let box_height = height - 2. * BLOCK_INSET;

        let Some((id, block)) = lane
            .blocks
            .iter()
            .find(|(_, b)| b.start <= at && at < b.end())
            .cloned()
        else {
            self.selected_block = None;
            if event.click_count >= 2
                && let Some(block) = new_block(&lane.blocks, at, snap)
            {
                let id = next_block_id_in(snapshot);
                self.session
                    .apply(Command::Edit(Edit::AddBlock { layer, id, block }));
                self.selected_block = Some(id);
            }
            return;
        };

        let local_x = x - beat_to_x(block.start.as_beats());
        let local_y = (event.position.y - box_top) / px(1.);
        let was_selected = self.selected_block == Some(id);
        let hit = hit_test(
            &block,
            local_x,
            local_y,
            PIXELS_PER_BEAT,
            box_height,
            was_selected,
        );
        let room = room_around(&lane.blocks, Some(id), block.start);
        self.selected_block = Some(id);
        if event.click_count >= 2 && was_selected && !matches!(hit, Hit::Point(_)) {
            let at = phase_at(&block, local_x, PIXELS_PER_BEAT);
            let (shape, _) = add_point(&block.shape, at, y_to_value(local_y, box_height));
            self.session.apply(Command::Edit(Edit::SetBlock {
                id,
                block: Block { shape, ..block },
            }));
            return;
        }
        self.block_drag = Some(BlockDrag {
            id,
            kind: DragKind::Block(hit),
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
        let Some((id, _, block)) = self.selected_block(self.session.snapshot()) else {
            return;
        };
        let lanes = self.lanes_bounds.get();
        let x = (event.position.x - lanes.origin.x) / px(1.) - beat_to_x(block.start.as_beats());
        let y = (event.position.y - lanes.origin.y) / px(1.) - top - BLOCK_INSET;
        let removed = point_at(block, x, y, PIXELS_PER_BEAT, height - 2. * BLOCK_INSET)
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

    /// Updates the preview of the block being dragged.
    fn drag_to(&mut self, position: Point<Pixels>, modifiers: Modifiers) {
        let snap = self.snap_for(modifiers);
        let Some(drag) = &mut self.block_drag else {
            return;
        };
        let dx = (position.x - drag.origin.x) / px(1.);
        let original = &drag.original;
        drag.preview = match drag.kind {
            DragKind::Block(Hit::Point(index)) => {
                let cycle = original.cycle.unwrap_or(original.length).as_beats();
                let at = original.shape.points.get(index).map_or(0., |p| p.at)
                    + f64::from(dx / PIXELS_PER_BEAT) / cycle;
                let value = y_to_value((position.y - drag.box_top) / px(1.), drag.box_height);
                Block {
                    shape: move_point(&original.shape, index, at, value),
                    ..original.clone()
                }
            }
            DragKind::Block(hit) => drag_block(original, hit, dx, PIXELS_PER_BEAT, snap, drag.room),
            DragKind::Setting(setting) => scrub(original, setting, dx, snap),
        };
    }

    /// Ends a drag, sending the change as one edit.
    fn finish_drag(&mut self) {
        if let Some(drag) = self.block_drag.take()
            && drag.preview != drag.original
        {
            self.session.apply(Command::Edit(Edit::SetBlock {
                id: drag.id,
                block: drag.preview,
            }));
        }
    }

    /// Deletes the selected block (Delete or Backspace).
    pub(crate) fn delete_selection(&mut self) {
        if self.page != Page::Parameters || self.block_drag.is_some() {
            return;
        }
        if let Some(id) = self.selected_block.take()
            && find_block(self.session.snapshot(), id).is_some()
        {
            self.session.apply(Command::Edit(Edit::RemoveBlock { id }));
        }
    }

    /// The selected block's settings: drag a value sideways to change it.
    fn settings_bar(
        &self,
        id: BlockId,
        block: &Block,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = theme.hover;
        let fields = Setting::ALL.map(|setting| {
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
                    .child("Double-click to add a point · right-click to remove · Alt flips snap"),
            )
    }

    fn left_cell(
        &self,
        index: usize,
        row: &Row<'_>,
        position: f64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cell = div().h(px(row.height())).flex_none().flex().items_center();
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
                let value =
                    Automation::compile_layers(param.base, &param.layers).value_at(position);
                cell.pl(px(16.))
                    .pr(px(12.))
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(param.name.clone()),
                    )
                    .child(
                        div()
                            .font_family(MONO_FONT)
                            .text_size(px(11.))
                            .text_color(theme.text_secondary)
                            .child(param.format_normalized(value)),
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
                let (param_id, count) = (param.id, param.layers.len());
                quiet_action(cell.pl(px(32.)), ("add-layer", index), "Add Layer", theme)
                    .on_click(cx.listener(move |this, _, _, _| {
                        let name = format!("Layer {}", count + 1);
                        // New layers go at the bottom, beside the button.
                        this.add_layer(param_id, 0, &name);
                    }))
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
                .on_click(cx.listener(move |this, _, _, _| {
                    this.open_menu = Some(OpenMenu::AddParam(device_ref));
                }));
                div()
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.open_menu = Some(OpenMenu::BlendMode(id));
                    })),
            )
            .when(mode_open, |d| {
                d.child(self.blend_mode_menu(id, mode, enabled, theme, cx))
            });

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
            .child(name)
            .child(mode_chip)
            .child(toggle)
            .child(delete)
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
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.open_menu = None))
                .children(items),
        )
    }

    /// Picks a parameter of `device` that has no layers yet.
    fn add_param_menu(
        &self,
        device: &DeviceSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                    this.open_menu = None;
                    this.add_layer(param, 0, "Layer 1");
                }))
            })
            .collect();
        let empty = available.is_empty();
        menu::float(
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.open_menu = None))
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

    /// Adds a layer at `index` in the parameter's stack (0 is the bottom;
    /// the stack length puts it on top).
    fn add_layer(&mut self, param: ParamId, index: usize, name: &str) {
        let id = next_layer_id(self.session.snapshot());
        self.session.apply(Command::Edit(Edit::AddLayer {
            param,
            id,
            index,
            layer: Layer::named(name, BlendMode::Replace),
        }));
    }
}

/// A small square icon button.
fn small_button(id: impl Into<gpui::ElementId>, theme: &Theme) -> gpui::Stateful<Div> {
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
fn quiet_action(
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
    fn right_cell(
        &self,
        row: &Row<'_>,
        top: f32,
        color: ContentColor,
        theme: &Theme,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = row.height();
        let cell = div().h(px(height)).flex_none().w(px(width)).relative();
        let drag = self.block_drag.as_ref();
        match row {
            Row::Set(param) => {
                let layers = previewed_layers(drag, &param.layers);
                let automation = Automation::compile_layers(param.base, &layers);
                let (line, fill) = (color.detail, color.fill);
                cell.bg(theme.window)
                    .border_b_1()
                    .border_color(theme.hairline)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, (), window, _| {
                                paint_curve(window, bounds, line, Some(fill), |beat| {
                                    automation.value_at(beat)
                                });
                            },
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
            Row::Layer { id, layer, .. } => {
                let layer_id = *id;
                let colors = if layer.enabled {
                    color
                } else {
                    theme.muted_content
                };
                let blocks = previewed_blocks(drag, &layer.blocks);
                cell.border_b_1()
                    .border_color(theme.hairline)
                    .children(blocks.iter().map(|(id, block)| {
                        block_view(*id, block, colors, self.selected_block == Some(*id), theme)
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.lane_mouse_down(layer_id, top, height, event);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.lane_right_click(top, height, event);
                        }),
                    )
                    .into_any_element()
            }
            _ => cell.into_any_element(),
        }
    }
}

/// A block on a layer lane: its span and the shape it draws, with stretch
/// zones at its edges and a loop handle in its top-right corner. Selected
/// blocks show their points.
fn block_view(
    id: BlockId,
    block: &Block,
    colors: ContentColor,
    selected: bool,
    theme: &Theme,
) -> impl IntoElement {
    let left = beat_to_x(block.start.as_beats());
    let width = beat_to_x(block.length.as_beats());
    let compiled = CompiledBlock::compile(block);
    let start = compiled.start;
    // Where each repeat of the shape begins, after the first.
    let cycle = compiled.cycle;
    let repeats: Vec<f32> = (1..)
        .map(|k| beat_to_x(f64::from(k) * cycle))
        .take_while(|x| *x < width - 0.5)
        .collect();
    let points: Vec<(f32, f64)> = if selected {
        point_positions(block, PIXELS_PER_BEAT)
            .into_iter()
            .map(|(index, x)| (x, block.shape.points[index].value))
            .collect()
    } else {
        Vec::new()
    };
    let group = SharedString::from(format!("block-{}", id.0));
    let edge = EDGE_ZONE.min(width / 4.);
    let has_loop_handle = width >= 2. * LOOP_HANDLE;

    div()
        .id(("block", id.0))
        .group(group.clone())
        .absolute()
        .left(px(left))
        .top(px(BLOCK_MARGIN))
        .bottom(px(BLOCK_MARGIN))
        .w(px(width))
        .rounded(px(4.))
        .bg(colors.fill)
        .border_1()
        .border_color(if selected {
            theme.accent
        } else {
            colors.outline
        })
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    for x in &repeats {
                        let line = Bounds::new(
                            point(bounds.origin.x + px(*x), bounds.origin.y),
                            size(px(1.), bounds.size.height),
                        );
                        window.paint_quad(fill(line, colors.outline));
                    }
                    paint_curve(window, bounds, colors.detail, None, |beat| {
                        let beat = start + beat;
                        if compiled.contains(beat) {
                            compiled.sample(beat).0
                        } else {
                            f64::NAN
                        }
                    });
                    let height = bounds.size.height / px(1.);
                    for (x, value) in &points {
                        let centre = point(
                            bounds.origin.x + px(*x),
                            bounds.origin.y + px(value_to_y(*value, height)),
                        );
                        let dot = Bounds::new(
                            point(centre.x - px(POINT_DOT), centre.y - px(POINT_DOT)),
                            size(px(2. * POINT_DOT), px(2. * POINT_DOT)),
                        );
                        window.paint_quad(fill(dot, colors.detail).corner_radii(px(POINT_DOT)));
                    }
                },
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(edge))
                .cursor_ew_resize(),
        )
        .child(
            div()
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(edge))
                .cursor_ew_resize(),
        )
        .when(has_loop_handle, |d| {
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .size(px(LOOP_HANDLE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_ew_resize()
                    .opacity(if selected { 1. } else { 0. })
                    .group_hover(group, |s| s.opacity(1.))
                    .child(icon(Icon::Loop, colors.detail, 10.)),
            )
        })
}

/// Draws `value(beat)` (0..=1, NaN for "nothing here") across `bounds`, with
/// beat 0 at the left edge. Optionally fills under the line.
fn paint_curve(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    line: Hsla,
    fill: Option<Hsla>,
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
    let points: Vec<Point<Pixels>> = (0..=((width / step) as usize))
        .filter_map(|i| {
            let x = i as f32 * step;
            let v = value(f64::from(x / PIXELS_PER_BEAT));
            (!v.is_nan()).then(|| to_point(x, v.clamp(0., 1.)))
        })
        .collect();
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    if let Some(fill) = fill {
        let bottom = bounds.origin.y + bounds.size.height;
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
    use daw_engine::{Engine, EngineConfig, Shape, Ticks};

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
        device: DeviceRef::Channel,
        index: 0,
    };

    #[test]
    fn an_untouched_node_lists_its_devices_only() {
        let snapshot = snapshot_with(vec![]);
        assert_eq!(
            describe(&rows(&snapshot, None)),
            [
                "section Global",
                "device Channel",
                "+ add to Channel",
                "section Instrument",
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
            layer: Layer::named(name, BlendMode::Replace),
        };
        let snapshot = snapshot_with(vec![add(1, 0, "Bottom"), add(2, 1, "Top")]);
        let described = describe(&rows(&snapshot, None));
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
    fn blocks_are_found_with_their_layer() {
        let snapshot = snapshot_with(layer_with_block());
        let (layer, block) = find_block(&snapshot, BlockId(4)).unwrap();
        assert_eq!((layer, block.length), (LayerId(1), Ticks::from_beats(4)));
        assert!(find_block(&snapshot, BlockId(5)).is_none());
        assert_eq!(next_block_id_in(&snapshot), BlockId(5));
    }

    #[test]
    fn the_layer_with_the_selected_block_is_tall() {
        let snapshot = snapshot_with(layer_with_block());
        let heights: Vec<f32> = rows(&snapshot, Some(LayerId(1)))
            .iter()
            .filter(|r| matches!(r, Row::Layer { .. }))
            .map(Row::height)
            .collect();
        assert_eq!(heights, [TALL_LAYER_HEIGHT]);
    }

    #[test]
    fn a_drag_previews_only_its_own_block() {
        let snapshot = snapshot_with(layer_with_block());
        let layers = &snapshot.devices.last().unwrap().params[0].layers;
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
    fn block_contents_compile_for_drawing() {
        let mut layer = Layer::named("Layer 1", BlendMode::Replace);
        layer.blocks.push((
            daw_engine::BlockId(1),
            daw_engine::Block::new(Ticks::ZERO, Ticks::from_beats(4), Shape::ramp(0., 1.)),
        ));
        let block = CompiledBlock::compile(&layer.blocks[0].1);
        assert!((block.sample(2.0).0 - 0.5).abs() < 1e-9);
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
