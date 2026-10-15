//! The Parameters page: the selected node's parameters that have movement,
//! each with its layers.
//!
//! The left column lists devices in three sections (Global, Instrument,
//! Effects). Under a device, each parameter with layers forms a set: a
//! result row showing the final value over time, then its layers, listed
//! like image-editor layers (the top one applies last). The right side shows
//! the same rows on the timeline.

use crate::icons::{Icon, icon};
use crate::layout::{BEATS_PER_BAR, PIXELS_PER_BEAT, beat_to_x, timeline_bars};
use crate::menu::{self, OpenMenu};
use crate::project::{ProjectView, RenameTarget};
use crate::theme::{ContentColor, MONO_FONT, Theme};
use daw_automation::{Automation, CompiledBlock};
use daw_engine::{
    BlendMode, Command, DeviceRef, DeviceSnapshot, Edit, Layer, LayerId, ParamId, ParamSnapshot,
    Snapshot,
};
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Div, FontWeight, Hsla, PathBuilder, Pixels, Point,
    Window, canvas, div, point, prelude::*, px,
};

const LEFT_WIDTH: f32 = 260.;
const RULER_HEIGHT: f32 = 28.;
const SECTION_HEIGHT: f32 = 30.;
const DEVICE_HEIGHT: f32 = 30.;
const SET_HEIGHT: f32 = 48.;
const LAYER_HEIGHT: f32 = 40.;
const ADD_HEIGHT: f32 = 26.;
/// Space above and below a curve inside its row.
const CURVE_PADDING: f32 = 6.;

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
    /// A layer of a set.
    Layer {
        id: LayerId,
        layer: &'a Layer,
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
            Self::Layer { .. } => LAYER_HEIGHT,
            Self::NoEffects | Self::AddLayer(_) | Self::AddParam(_) => ADD_HEIGHT,
        }
    }
}

/// The rows for a node, in display order.
fn rows(snapshot: &Snapshot) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let find = |device| snapshot.devices.iter().find(|d| d.device == device);
    rows.push(Row::Section("Global"));
    if let Some(channel) = find(DeviceRef::Channel) {
        push_device(&mut rows, channel);
    }
    rows.push(Row::Section("Instrument"));
    if let Some(instrument) = find(DeviceRef::Instrument) {
        push_device(&mut rows, instrument);
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
        push_device(&mut rows, effect);
    }
    rows
}

fn push_device<'a>(rows: &mut Vec<Row<'a>>, device: &'a DeviceSnapshot) {
    rows.push(Row::Device(device));
    for param in device.params.iter().filter(|p| !p.layers.is_empty()) {
        rows.push(Row::Set(param));
        // Like image-editor layers: the last applied is listed first.
        for (id, layer) in param.layers.iter().rev() {
            rows.push(Row::Layer { id: *id, layer });
        }
        rows.push(Row::AddLayer(param));
    }
    rows.push(Row::AddParam(device));
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
        let rows = rows(snapshot);

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
        let lanes = div()
            .id("parameter-lanes")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.parameters_scroll)
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .children(grid_lines)
                    .children(rows.iter().map(|row| right_cell(row, color, theme, width)))
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

        div().flex_1().min_h_0().flex().flex_col().child(top).child(
            div()
                .id("parameter-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(div().flex().child(left).child(lanes)),
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
            Row::Layer { id, layer, .. } => self
                .layer_header(*id, layer, cell, theme, cx)
                .into_any_element(),
            Row::AddLayer(param) => {
                let (param_id, count) = (param.id, param.layers.len());
                quiet_action(cell.pl(px(32.)), ("add-layer", index), "Layer", theme)
                    .on_click(cx.listener(move |this, _, _, _| {
                        let name = format!("Layer {}", count + 1);
                        this.add_layer(param_id, count, &name);
                    }))
                    .into_any_element()
            }
            Row::AddParam(device) => {
                let open = self.open_menu == Some(OpenMenu::AddParam(device.device));
                let device_ref = device.device;
                let button = quiet_action(cell.pl(px(16.)), ("add-param", index), "Add", theme)
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
    fn layer_header(
        &self,
        id: LayerId,
        layer: &Layer,
        cell: Div,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (mode, enabled) = (layer.mode, layer.enabled);
        let group = format!("layer-{}", id.0);
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
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.))
            .on_click(cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::RemoveLayer { id }));
            }))
            .child(icon(Icon::Close, theme.text_secondary, 12.));

        cell.id(("layer-row", id.0))
            .group(group)
            .pl(px(32.))
            .pr(px(8.))
            .gap(px(6.))
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

/// The timeline side of a row.
fn right_cell(row: &Row<'_>, color: ContentColor, theme: &Theme, width: f32) -> AnyElement {
    let cell = div()
        .h(px(row.height()))
        .flex_none()
        .w(px(width))
        .relative();
    match row {
        Row::Set(param) => {
            let automation = Automation::compile_layers(param.base, &param.layers);
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
        Row::Layer { layer, .. } => {
            let colors = if layer.enabled {
                color
            } else {
                theme.muted_content
            };
            let blocks: Vec<CompiledBlock> = layer
                .blocks
                .iter()
                .map(|(_, b)| CompiledBlock::compile(b))
                .collect();
            cell.border_b_1()
                .border_color(theme.hairline)
                .children(
                    blocks
                        .into_iter()
                        .enumerate()
                        .map(move |(i, block)| block_view(i, block, colors)),
                )
                .into_any_element()
        }
        _ => cell.into_any_element(),
    }
}

/// A block on a layer lane: its span, with the shape it draws.
fn block_view(index: usize, block: CompiledBlock, colors: ContentColor) -> impl IntoElement {
    let left = beat_to_x(block.start);
    let width = beat_to_x(block.end - block.start);
    let start = block.start;
    div()
        .id(("block", index))
        .absolute()
        .left(px(left))
        .top(px(3.))
        .bottom(px(3.))
        .w(px(width))
        .rounded(px(4.))
        .bg(colors.fill)
        .border_1()
        .border_color(colors.outline)
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    paint_curve(window, bounds, colors.detail, None, |beat| {
                        let beat = start + beat;
                        if block.contains(beat) {
                            block.sample(beat).0
                        } else {
                            f64::NAN
                        }
                    });
                },
            )
            .size_full(),
        )
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
    let height = bounds.size.height / px(1.) - 2. * CURVE_PADDING;
    let to_point = |x: f32, v: f64| -> Point<Pixels> {
        point(
            bounds.origin.x + px(x),
            bounds.origin.y + px(CURVE_PADDING + (1. - v as f32) * height),
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
            describe(&rows(&snapshot)),
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
        let described = describe(&rows(&snapshot));
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
