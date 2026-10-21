//! The Arrange page: the node list on the left, the timeline beside it,
//! framed by scrollbars and zoom sliders (see `frame.rs`).

use crate::audio_import::{FileDrop, file_drop};

/// Arrange's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrangeTool {
    /// Select, move and resize clips.
    Pointer,
    /// Click a clip to cut it in two.
    Scissors,
}
use crate::clip_edit;
use crate::clip_edit::Grab;
use crate::frame::{Timeline, grid_lines};
use crate::icons::{Icon, icon};
use crate::layout::{BEATS_PER_BAR, ruler_label_step, timeline_bars, x_to_snapped_position};
use crate::live::{Live, PaneKind};
use crate::menu::{self, OpenMenu};
use crate::motion::RowLayout;
use crate::parameters::small_button;
use crate::project::{ProjectView, RenameTarget, song_clips};
use crate::scroll;
use crate::signals::{node_has_notes, published_signals};
use crate::slider::Axis;
use crate::theme::Theme;
use crate::tooltip;
use crate::tree::{self, Bar, Drop, display_order};
use daw_engine::{Command, Edit, Node, NodeId, NodeKind, NodeSnapshot};
use gpui::{
    ClickEvent, Context, DragMoveEvent, ExternalPaths, FontWeight, Hsla, MouseButton,
    MouseDownEvent, ScrollWheelEvent, canvas, div, prelude::*, px,
};
use std::time::Instant;

const RULER_HEIGHT: f32 = 28.;
/// Size of the drawn loop cursor over a clip's loop zone.
const LOOP_CURSOR: f32 = 16.;
const NODE_LIST_WIDTH: f32 = 260.;
const METER_HEIGHT: f32 = 32.;
/// Width of one column of the nesting map, and its padding.
const MAP_COLUMN: f32 = 18.;
const MAP_PAD: f32 = 4.;
/// Columns the map shows before it scrolls sideways: four and half of the
/// next, so the half column says there is more.
const MAP_VISIBLE_COLUMNS: f32 = 4.5;

impl ProjectView {
    pub(crate) fn render_arrange(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let bars = timeline_bars(song_clips(snapshot), snapshot.loop_range);
        let width = zoom.beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let shown = display_order(snapshot, &self.folded);
        let layout = self
            .row_motion
            .layout(&shown, zoom.row_height, Instant::now());
        // Rows as drawn this frame, ghosts of departing rows included.
        let placed: Vec<(&NodeSnapshot, &RowLayout)> = layout
            .iter()
            .filter_map(|l| Some((l.ghost.as_deref().or_else(|| snapshot.node(l.id))?, l)))
            .collect();
        let rows: Vec<&NodeSnapshot> = placed.iter().map(|(n, _)| *n).collect();
        let map_bars = tree::bars(&rows);
        // While a node is dragged the map shows every column, one more for
        // the drop slots, so every slot can be reached without scrolling.
        let dragging = cx.has_active_drag();
        let columns = tree::columns(&map_bars) + usize::from(dragging);
        let map_target = if dragging {
            full_map_width(columns)
        } else {
            map_width(columns)
        };
        let map_width = self.map_width.toward(map_target);
        if map_width != map_target || self.row_motion.animating() {
            self.animate();
        }
        let left_width = map_width + NODE_LIST_WIDTH;

        // The ruler stays at the top and follows the lanes' sideways scroll.
        let scroll_x = self.arrange_view.x.offset().x;
        let top = div()
            .h(px(RULER_HEIGHT))
            .flex_none()
            .flex()
            .child(self.node_toolbar(left_width, theme, cx))
            .child(
                div().flex_1().min_w_0().overflow_hidden().child(
                    div()
                        .relative()
                        .left(scroll_x)
                        .w(px(width))
                        .child(self.render_ruler(
                            theme,
                            bars,
                            zoom.pixels_per_beat,
                            &self.arrange_view.x,
                            cx,
                        )),
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
            .id("arrange-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.arrange_view.y)
            .flex()
            .flex_col()
            .child(
                // Grows to fill the page (flex growth: percentage heights do
                // not work inside a vertical scroll).
                div()
                    .flex()
                    .flex_grow()
                    .flex_shrink_0()
                    .child(self.render_map(&placed, &map_bars, map_width, theme, cx))
                    // Its own pane: lanes redraw without it.
                    .child(self.pane_element(
                        PaneKind::ArrangeList,
                        div().w(px(NODE_LIST_WIDTH)).flex_none(),
                    ))
                    .child(self.render_timeline(&placed, theme, bars, width, cx)),
            );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(top)
            .child(self.timeline_frame(
                Timeline::Arrange,
                body.into_any_element(),
                left_width,
                theme,
                cx,
            ))
    }

    /// The node list as its own pane (`PaneKind::ArrangeList`): the rows
    /// as Arrange shows them this frame.
    pub(crate) fn render_arrange_list(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let shown = display_order(snapshot, &self.folded);
        let layout = self
            .row_motion
            .layout(&shown, zoom.row_height, Instant::now());
        if self.row_motion.animating() {
            self.animate();
        }
        let placed: Vec<(&NodeSnapshot, &RowLayout)> = layout
            .iter()
            .filter_map(|l| Some((l.ghost.as_deref().or_else(|| snapshot.node(l.id))?, l)))
            .collect();
        self.render_node_list(&placed, theme, cx).into_any_element()
    }

    fn render_node_list(
        &self,
        rows: &[(&NodeSnapshot, &RowLayout)],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row_height = self.arrange_view.zoom.row_height;
        div()
            .w(px(NODE_LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children({
                // Rows far off screen are empty boxes of the same height.
                let near_y = crate::paint::visible_y(&self.arrange_view.y);
                let mut top = 0.;
                rows.iter()
                    .map(|(node, layout)| {
                        let row_top = top;
                        top += layout.height;
                        let near = near_y.is_none_or(|(above, below)| {
                            row_top + layout.height >= above && row_top <= below
                        });
                        if near {
                            animated(layout, row_height, self.node_row(node, theme, cx))
                                .into_any_element()
                        } else {
                            div().h(px(layout.height)).flex_none().into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .child(
                // Clicking the empty space below the nodes clears the selection.
                div()
                    .id("node-list-space")
                    .flex_1()
                    .on_click(cx.listener(|this, _, _, _| this.selected_node = None)),
            )
    }

    /// A node: colour, name and kind, meter, signal button, mute and solo.
    fn node_row(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = node.id;
        let color = theme.content(node.color);
        let row_height = self.arrange_view.zoom.row_height;
        let meter_size = (row_height - 16.).clamp(10., METER_HEIGHT);

        let toggle =
            |name: &'static str, label: &'static str, on: bool, on_bg: Hsla, on_fg: Hsla| {
                div()
                    .id((name, id.0))
                    .w(px(20.))
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
        let mute = toggle("mute", "M", muted, theme.warning, theme.on_warning).on_click(
            cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::SetMute {
                    node: id,
                    muted: !muted,
                }));
            }),
        );
        let solo = toggle("solo", "S", soloed, theme.accent, theme.on_accent).on_click(
            cx.listener(move |this, _, _, _| {
                this.session.apply(Command::Edit(Edit::SetSolo {
                    node: id,
                    soloed: !soloed,
                }));
            }),
        );

        let song = id == NodeId::ROOT;
        let publish = (!song).then(|| self.publish_button(node, theme, cx));
        let node_name: gpui::AnyElement = match self.rename_input(RenameTarget::Node(id)) {
            Some(input) => div().text_size(px(13.)).child(input).into_any_element(),
            None => div()
                .id(("node-name", id.0))
                .text_size(px(13.))
                .child(node.name.clone())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() >= 2 && !song {
                        this.start_rename(RenameTarget::Node(id), window, cx);
                    }
                }))
                .into_any_element(),
        };
        // The kind, and for a MIDI node its first instrument (and how many
        // more are layered with it).
        let instruments: Vec<&str> = node
            .devices
            .iter()
            .filter(|d| d.instrument.is_some())
            .map(|d| d.name.as_str())
            .collect();
        let kind = match (node.kind, instruments.as_slice()) {
            _ if song => "Master".to_owned(),
            (NodeKind::Audio, _) => "Audio".to_owned(),
            (NodeKind::Midi, []) => "MIDI".to_owned(),
            (NodeKind::Midi, [only]) => format!("MIDI · {only}"),
            (NodeKind::Midi, [first, rest @ ..]) => format!("MIDI · {first} +{}", rest.len()),
        };
        let mut drop_highlight = theme.accent;
        drop_highlight.a = 0.18;
        let dragged = DraggedNode {
            id,
            name: node.name.clone(),
            kind: node.kind,
        };
        // A node dropped on this row goes under this row's parent.
        let parent_kind = node
            .parent
            .filter(|p| *p != NodeId::ROOT)
            .and_then(|p| self.session.snapshot().node(p))
            .map(|p| p.kind);

        let mut selected_bg = theme.accent;
        selected_bg.a = 0.16;
        div()
            .id(("node-row", id.0))
            .h(px(row_height))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .border_b_1()
            .border_color(theme.hairline)
            .when(self.selected_node == Some(id), |d| d.bg(selected_bg))
            .on_click(cx.listener(move |this, _, _, _| this.selected_node = Some(id)))
            .when(!song, |d| {
                d.on_drag(dragged, |dragged: &DraggedNode, _, _, cx| {
                    cx.new(|_| dragged.clone())
                })
            })
            // Dropped on a row: just after it (on the Song's: last in it).
            .drag_over::<DraggedNode>(move |style, dragged, _, _| {
                if dragged.id == id || parent_kind.is_some_and(|k| k != dragged.kind) {
                    style
                } else {
                    style.bg(drop_highlight)
                }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedNode, _, _| {
                this.move_node(dragged.id, Drop::After(id));
            }))
            .child(
                div()
                    .w(px(4.))
                    .h(px(meter_size))
                    .rounded(px(2.))
                    .bg(color.detail),
            )
            .child(
                div().flex_1().flex().flex_col().child(node_name).child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.text_secondary)
                        .child(kind),
                ),
            )
            // The stereo meter, drawn by the live layer.
            .child(
                div().flex_none().w(px(10.)).h(px(meter_size)).child(
                    self.live_slot(
                        PaneKind::ArrangeList,
                        Live::Meter {
                            node: id,
                            bar: 4.,
                            gap: 2.,
                        },
                    )
                    .size_full(),
                ),
            )
            .children(publish)
            .child(mute)
            .child(solo)
    }

    /// The heading over the node list: "NODES", then the actions on the
    /// selected node: add, group into a folder, duplicate, delete.
    fn node_toolbar(&self, width: f32, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let selected = self.shown_node().map(|n| n.id);
        let movable = selected.filter(|id| *id != NodeId::ROOT);
        let open = self.open_menu == Some(OpenMenu::AddNode);
        // Tooltips hide while a menu hangs below, so they never cover it.
        let action = |name: &'static str, kind: Icon, hint: &'static str, enabled: bool| {
            let color = if enabled {
                theme.text_secondary
            } else {
                theme.text_tertiary
            };
            small_button(name, theme)
                .child(icon(kind, color, 14.))
                .when(!open, |d| d.tooltip(tooltip::text(hint)))
        };
        let add = div()
            .relative()
            .flex()
            .flex_col()
            .child(
                action("node-add", Icon::Plus, "Add a node", true)
                    .when(open, |d| d.bg(theme.control))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.toggle_menu(OpenMenu::AddNode)),
                    ),
            )
            .when(open, |d| {
                d.child(menu::float(
                    menu::panel(theme)
                        .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                        .child(
                            menu::item(theme, "add-midi", "MIDI node", "with a Synth").on_click(
                                cx.listener(|this, _, _, _| this.add_node(NodeKind::Midi)),
                            ),
                        )
                        .child(
                            menu::item(theme, "add-audio", "Audio node", "").on_click(
                                cx.listener(|this, _, _, _| this.add_node(NodeKind::Audio)),
                            ),
                        ),
                ))
            });
        let group = action(
            "node-group",
            Icon::Group,
            "Group into a new node (of the same kind)",
            movable.is_some(),
        )
        .when_some(movable, |d, id| {
            d.on_click(cx.listener(move |this, _, _, _| this.group_node(id)))
        });
        let duplicate = action(
            "node-duplicate",
            Icon::Copy,
            "Duplicate, with its clips",
            movable.is_some(),
        )
        .when_some(movable, |d, id| {
            d.on_click(cx.listener(move |this, _, _, _| this.duplicate_node(id)))
        });
        let delete = action("node-delete", Icon::Trash, "Delete", movable.is_some())
            .when_some(movable, |d, id| {
                d.on_click(cx.listener(move |this, _, _, _| this.delete_node(id)))
            });
        div()
            .w(px(width))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(12.))
            .pr(px(6.))
            .bg(theme.sidebar)
            .border_r_1()
            .border_b_1()
            .border_color(theme.edge)
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_tertiary)
                    .child("NODES"),
            )
            .child(add)
            .child(group)
            .child(duplicate)
            .child(delete)
    }

    /// The nesting map: a column per level (the Song has none). Each
    /// folder's bar starts at its own row and runs beside what is under it.
    /// The chevron folds a folder; dropping a node on a bar puts it last in
    /// that folder.
    fn render_map(
        &self,
        rows: &[(&NodeSnapshot, &RowLayout)],
        bars: &[Bar],
        width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Where each row is drawn this frame, so bars follow moving rows.
        let mut tops = Vec::with_capacity(rows.len());
        let mut y = 0.;
        for (_, layout) in rows {
            tops.push(y + layout.offset);
            y += layout.height;
        }
        let total_height = y;
        let height_of = |row: usize| rows[row].1.height;
        // One column more while dragging, for the drop slots.
        let columns = tree::columns(bars) + usize::from(cx.has_active_drag());
        let content_width = columns as f32 * MAP_COLUMN + 2. * MAP_PAD;
        let hover = theme.hover;
        let marks = bars.iter().map(|bar| {
            let node = rows[bar.row].0;
            let id = bar.node;
            let colors = theme.content(node.color);
            let x = MAP_PAD + bar.column as f32 * MAP_COLUMN;
            let folded = self.folded.contains(&id);
            let foldable = id != NodeId::ROOT && !node.children.is_empty();
            let bar_kind = node.kind;
            let mut drop_fill = colors.detail;
            drop_fill.a = 0.35;
            let chevron = foldable.then(|| {
                div()
                    .id(("map-fold", id.0))
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .hover(move |s| s.bg(hover))
                    .child(icon(
                        if folded {
                            Icon::ChevronRight
                        } else {
                            Icon::ChevronDown
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
            // The bar: from the folder's own row down beside what is under
            // it, or (folded) a short pill on its row. While children close
            // up under the folder, the bar shrinks with them into the pill.
            const PILL: f32 = 14.;
            let start = tops[bar.row] + 4.;
            let bottom = tops[bar.last] + height_of(bar.last) - 4.;
            let height = (bottom - start).max(PILL.min(height_of(bar.row)));
            let top = start;
            let body = div()
                .id(("map-bar", id.0))
                .absolute()
                .left(px(x + 1.))
                .top(px(top))
                .w(px(MAP_COLUMN - 4.))
                .h(px(height))
                .rounded(px(4.))
                .bg(colors.fill)
                .border_1()
                .border_color(colors.outline)
                .flex()
                .flex_col()
                .items_center()
                .pt(px(1.))
                .children(chevron)
                .tooltip(tooltip::text(node.name.clone()))
                .drag_over::<DraggedNode>(move |style, dragged, _, _| {
                    // Only a node of this branch's kind can go in.
                    if dragged.kind == bar_kind {
                        style.bg(drop_fill)
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedNode, _, _| {
                    this.move_node(dragged.id, Drop::Into(id));
                }));
            body.into_any_element()
        });
        // While a node is dragged, every other row offers a slot to drop it
        // into (making that node a folder).
        let slots = cx.has_active_drag().then(|| {
            rows.iter()
                .enumerate()
                .filter(|(_, (node, layout))| {
                    layout.ghost.is_none() && !tree::is_folder(node) && node.id != NodeId::ROOT
                })
                .map(|(row, (node, _))| {
                    let id = node.id;
                    let slot_kind = node.kind;
                    let colors = theme.content(node.color);
                    let mut fill = colors.detail;
                    fill.a = 0.35;
                    div()
                        .id(("map-slot", id.0))
                        .absolute()
                        // Where its bar will be once something is inside.
                        .left(px(MAP_PAD + 1. + (node.depth - 1) as f32 * MAP_COLUMN))
                        .top(px(tops[row] + height_of(row) / 2. - 7.))
                        .w(px(MAP_COLUMN - 4.))
                        .h(px(14.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(colors.outline)
                        .drag_over::<DraggedNode>(move |style, dragged, _, _| {
                            if dragged.kind == slot_kind {
                                style.bg(fill)
                            } else {
                                style
                            }
                        })
                        .on_drop(cx.listener(move |this, dragged: &DraggedNode, _, _| {
                            this.move_node(dragged.id, Drop::Into(id));
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        });
        let content = div()
            .relative()
            .w(px(content_width))
            .h(px(total_height))
            .flex_shrink_0()
            .children(marks.collect::<Vec<_>>())
            .children(slots.into_iter().flatten());
        scroll::axis_locked(div())
            .id("node-map")
            .w(px(width))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.node_map_x)
            // The wheel scrolls the map sideways while it has more columns
            // than fit (Shift+wheel is sideways anyway); otherwise it passes
            // on to the rows.
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(20.));
                let sideways = if delta.x == px(0.) { delta.y } else { delta.x };
                let handle = &this.node_map_x;
                let extent = scroll::Extent::of(handle, Axis::Horizontal);
                if extent.content - extent.view <= 0.5 {
                    return;
                }
                let scrolled =
                    (extent.scrolled - sideways / px(1.)).clamp(0., extent.content - extent.view);
                scroll::scroll_to(handle, Axis::Horizontal, scrolled);
                cx.stop_propagation();
                cx.notify();
            }))
            // Grab anywhere but a chevron and pull sideways to scroll.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.grab_map(false, event.position);
                }),
            )
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.hairline)
            .child(content)
    }

    /// Folds or unfolds a folder, on Arrange and the mixer alike.
    pub(crate) fn toggle_fold(&mut self, node: NodeId) {
        if !self.folded.remove(&node) {
            self.folded.insert(node);
        }
    }

    /// Adds a MIDI node (with a Synth) or an audio node after the selected
    /// node, and selects it.
    fn add_node(&mut self, kind: NodeKind) {
        self.open_menu = None;
        let snapshot = self.session.snapshot();
        let (parent, index) = tree::new_node_place(snapshot, self.selected_node, kind);
        let id = tree::next_node_id(snapshot);
        let color = tree::next_color(snapshot);
        let node = match kind {
            NodeKind::Midi => Node::new(&tree::next_name(snapshot, "Synth"), color)
                .with_instrument(tree::next_device_id(snapshot)),
            NodeKind::Audio => {
                Node::new(&tree::next_name(snapshot, "Audio"), color).with_kind(NodeKind::Audio)
            }
        };
        self.session.apply(Command::Edit(Edit::AddNode {
            id,
            parent,
            index,
            node,
        }));
        self.selected_node = Some(id);
    }

    /// Imports audio files onto new audio nodes after the selected node (or
    /// at the end of the Song), at the playhead (Track → Import Audio…).
    pub(crate) fn import_audio_files(&mut self, files: &[std::path::PathBuf]) {
        let snapshot = self.session.snapshot();
        let (parent, index) = tree::new_node_place(snapshot, self.selected_node, NodeKind::Audio);
        self.import_error = self
            .session
            .import_audio(
                files,
                crate::session::ImportTarget::NewNodes { parent, index },
            )
            .err();
    }

    /// The clip under `pointer` and where the scissors would cut it (the
    /// snapped song position), with its row's top and height.
    fn scissors_target(
        &self,
        pointer: gpui::Point<gpui::Pixels>,
        rows: &[(NodeId, f32, f32)],
    ) -> Option<(daw_engine::ClipId, daw_engine::Ticks, f32, f32)> {
        let bounds = self.arrange_lanes_bounds.get();
        let x = (pointer.x - bounds.left()) / px(1.);
        let y = (pointer.y - bounds.top()) / px(1.);
        let &(node, top, height) = rows.iter().find(|(_, top, h)| y >= *top && y < top + h)?;
        let snapshot = self.session.snapshot();
        let beat = daw_engine::Ticks::from_beats_f64(self.arrange_view.zoom.x_to_beats(x));
        let (id, clip) = clip_edit::node_clips(snapshot.node(node)?)
            .find(|(_, c)| c.start() <= beat && beat < c.end())?;
        let at = self.snap.round(beat);
        (at > clip.start() && at < clip.end()).then_some((id, at, top, height))
    }

    /// While the scissors are out: a layer over the lanes that cuts the
    /// clip under a click at the snapped position (every selected clip
    /// there, if that clip is selected), with a cut line under the pointer.
    fn scissors_layer(
        &self,
        rows: &[(NodeId, f32, f32)],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if self.arrange_tool != ArrangeTool::Scissors {
            return None;
        }
        let zoom = self.arrange_view.zoom;
        let line = self
            .scissors_at
            .and_then(|pointer| self.scissors_target(pointer, rows))
            .map(|(_, at, top, height)| {
                let x = zoom.beat_to_x(at.as_beats());
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(top))
                    .w(px(1.))
                    .h(px(height))
                    .bg(theme.accent)
                    .child(
                        div()
                            .absolute()
                            .left(px(4.))
                            .top(px(-18.))
                            .whitespace_nowrap()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(theme.accent)
                            .text_color(theme.on_accent)
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(clip_edit::bar_beat(at)),
                    )
            });
        let rows_for_click = rows.to_vec();
        Some(
            div()
                .id("scissors")
                .absolute()
                .size_full()
                .occlude()
                .cursor_crosshair()
                // The cut line follows the pointer.
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    this.scissors_at = Some(event.position);
                    this.hover_changed(cx);
                }))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !hovered {
                        this.scissors_at = None;
                        this.hover_changed(cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, _| {
                        this.cut_with_scissors(event.position, &rows_for_click);
                    }),
                )
                .children(line)
                .into_any_element(),
        )
    }

    /// A scissors click: cuts the clip under the pointer, or every selected
    /// clip there if it is selected, as one undo step.
    fn cut_with_scissors(
        &mut self,
        pointer: gpui::Point<gpui::Pixels>,
        rows: &[(NodeId, f32, f32)],
    ) {
        let Some((id, at, _, _)) = self.scissors_target(pointer, rows) else {
            return;
        };
        let ids: Vec<daw_engine::ClipId> = if self.selected_clips.contains(&id) {
            self.selected_clips.iter().copied().collect()
        } else {
            vec![id]
        };
        let Some((edit, pieces)) = clip_edit::split_at(self.session.snapshot(), &ids, at) else {
            return;
        };
        let keep_selected = self.selected_clips.contains(&id);
        self.session.apply(Command::Edit(edit));
        // A cut selection stays selected, both halves of it.
        if keep_selected {
            self.selected_clips.extend(pieces);
        }
    }

    /// Arrange's tools, left of the snap control: the pointer and the
    /// scissors (keys 1 and 3, as in the piano roll).
    pub(crate) fn render_arrange_tools(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let button = |id: &'static str, kind: Icon, tool: ArrangeTool, hint: &'static str| {
            let on = self.arrange_tool == tool;
            small_button(id, theme)
                .when(on, |d| d.bg(theme.control))
                .child(icon(
                    kind,
                    if on {
                        theme.accent
                    } else {
                        theme.text_secondary
                    },
                    14.,
                ))
                .tooltip(tooltip::text(hint))
                .on_click(cx.listener(move |this, _, _, _| {
                    this.arrange_tool = tool;
                    this.scissors_at = None;
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(button(
                "arrange-pointer",
                Icon::Pointer,
                ArrangeTool::Pointer,
                "Pointer (1): select, move and resize clips",
            ))
            .child(button(
                "arrange-scissors",
                Icon::Scissors,
                ArrangeTool::Scissors,
                "Scissors (3): click a clip to cut it at the grid; a selected clip cuts the whole selection there",
            ))
    }

    /// Where files dragged over the lanes would go, if any are.
    fn file_target(&self, rows: &[(NodeId, f32, f32)]) -> Option<(FileDrop, f32, usize)> {
        let (pointer, count) = self.file_hover?;
        let bounds = self.arrange_lanes_bounds.get();
        let x = (pointer.x - bounds.left()) / px(1.);
        let y = (pointer.y - bounds.top()) / px(1.);
        Some((file_drop(self.session.snapshot(), rows, y), x, count))
    }

    /// Files dropped on the lanes: into an audio node, or onto new audio
    /// nodes between rows, at the playhead.
    fn drop_files(&mut self, files: &[std::path::PathBuf], rows: &[(NodeId, f32, f32)]) {
        let target = self.file_target(rows).map(|(target, _, _)| target);
        self.file_hover = None;
        let target = match target {
            Some(FileDrop::Into { node, .. }) => crate::session::ImportTarget::Into(node),
            Some(FileDrop::NewNodes { parent, index, .. }) => {
                crate::session::ImportTarget::NewNodes { parent, index }
            }
            Some(FileDrop::Refused { .. }) | None => return,
        };
        self.import_error = self.session.import_audio(files, target).err();
    }

    /// The preview while files are dragged over the lanes: the receiving
    /// row lit (red where audio cannot go) or a line where new nodes would
    /// go, and a label by the pointer.
    fn file_drop_overlay(
        &self,
        rows: &[(NodeId, f32, f32)],
        theme: &Theme,
    ) -> Vec<gpui::AnyElement> {
        let Some((target, x, count)) = self.file_target(rows) else {
            return Vec::new();
        };
        let snapshot = self.session.snapshot();
        let row_height = self.arrange_view.zoom.row_height;
        let top_of = |row: usize| {
            rows.get(row).map_or_else(
                || rows.last().map_or(0., |(_, top, height)| top + height),
                |(_, top, _)| *top,
            )
        };
        let height_of = |row: usize| rows.get(row).map_or(row_height, |(_, _, h)| *h);
        let files = if count == 1 {
            "1 file".to_owned()
        } else {
            format!("{count} files")
        };
        let (mark, text, ok) = match target {
            FileDrop::Into { node, row } => {
                let mut tint = theme.accent;
                tint.a = 0.1;
                let name = snapshot.node(node).map_or("", |n| n.name.as_str());
                let mark = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_of(row)))
                    .h(px(height_of(row)))
                    .bg(tint)
                    .border_1()
                    .border_color(theme.accent);
                (mark, format!("{files} into {name}"), true)
            }
            FileDrop::NewNodes { row, .. } => {
                let mark = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_of(row) - 1.))
                    .h(px(2.))
                    .bg(theme.accent);
                let nodes = if count == 1 {
                    "New audio node".to_owned()
                } else {
                    format!("{count} new audio nodes")
                };
                (mark, nodes, true)
            }
            FileDrop::Refused { row } => {
                let mut tint = theme.record;
                tint.a = 0.08;
                let mark = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_of(row)))
                    .h(px(height_of(row)))
                    .bg(tint);
                (mark, "Can't go here".to_owned(), false)
            }
        };
        let label_row = match target {
            FileDrop::Into { row, .. } | FileDrop::Refused { row } => top_of(row),
            FileDrop::NewNodes { row, .. } => top_of(row),
        };
        let at = clip_edit::bar_beat(daw_engine::Ticks::from_beats_f64(
            self.session.position_beats().max(0.),
        ));
        let label = div()
            .absolute()
            .left(px(x + 12.))
            .top(px((label_row - 20.).max(0.)))
            .px(px(5.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(if ok { theme.accent } else { theme.record })
            .text_color(theme.on_accent)
            .text_size(px(10.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(if ok { format!("{text} · {at}") } else { text });
        vec![mark.into_any_element(), label.into_any_element()]
    }

    fn group_node(&mut self, id: NodeId) {
        if let Some(edit) = tree::group_edit(self.session.snapshot(), id) {
            self.session.apply(Command::Edit(edit));
        }
    }

    fn duplicate_node(&mut self, id: NodeId) {
        if let Some((copy, edit)) = tree::duplicate_edit(self.session.snapshot(), id) {
            self.session.apply(Command::Edit(edit));
            self.selected_node = Some(copy);
        }
    }

    /// Deletes a node and everything under it (undo brings it back).
    pub(crate) fn delete_node(&mut self, id: NodeId) {
        if id == NodeId::ROOT {
            return;
        }
        self.session.apply(Command::Edit(Edit::RemoveNode { id }));
        self.selected_node = None;
    }

    fn move_node(&mut self, id: NodeId, drop: Drop) {
        if let Some(edit) = tree::move_edit(self.session.snapshot(), id, drop) {
            self.session.apply(Command::Edit(edit));
        }
    }

    /// The node's signal button: publish as a signal, and the signals it
    /// publishes, each with a go-to icon. Tinted in the first published
    /// signal's colour once it publishes any.
    fn publish_button(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let snapshot = self.session.snapshot();
        let id = node.id;
        let color = published_signals(snapshot, id)
            .next()
            .map_or(theme.text_secondary, |s| theme.content(s.color).detail);
        let open = self.open_menu == Some(OpenMenu::NodeSignals(id));
        div()
            .relative()
            .flex()
            .flex_col()
            .child(
                small_button(("node-signals", id.0), theme)
                    .w(px(20.))
                    .h(px(18.))
                    .when(open, |d| d.bg(theme.control))
                    .child(icon(Icon::Signal, color, 13.))
                    // Hidden while the menu is open, so it never covers it.
                    .when(!open, |d| d.tooltip(tooltip::text("Publish as signal")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, _| {
                            this.toggle_menu(OpenMenu::NodeSignals(id));
                        }),
                    ),
            )
            .when(open, |d| d.child(self.publish_menu(node, theme, cx)))
    }

    /// Publishing stays on Arrange with the menu open, so the new signal
    /// appears in it with its go-to icon.
    fn publish_menu(
        &self,
        node: &NodeSnapshot,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let node_id = node.id;
        let unavailable = |id: &'static str, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .flex()
                .justify_between()
                .gap(px(24.))
                .px(px(10.))
                .py(px(5.))
                .text_color(theme.text_tertiary)
                .child(label)
                .child(hint)
        };
        let notes = if node_has_notes(node) {
            menu::item(theme, "publish-notes", "Publish notes as motion signal", "")
                .on_click(cx.listener(move |this, _, _, _| this.publish_notes(node_id)))
        } else {
            unavailable(
                "publish-notes",
                "Publish notes as motion signal",
                "no notes",
            )
        };
        let hover = theme.hover;
        let published: Vec<_> = published_signals(snapshot, node_id)
            .map(|signal| {
                let id = signal.id;
                div()
                    .id(("published-signal", id.0))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(5.))
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .size(px(8.))
                            .rounded(px(4.))
                            .bg(theme.content(signal.color).detail),
                    )
                    .child(div().flex_1().child(signal.name.clone()))
                    .child(icon(Icon::GoTo, theme.text_secondary, 12.))
                    .tooltip(tooltip::text("Go to this signal"))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.open_menu = None;
                        this.go_to_signal(id);
                    }))
            })
            .collect();
        menu::float(
            menu::panel(theme)
                .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                .child(notes)
                .child(unavailable(
                    "publish-sound-motion",
                    "Publish sound as motion signal",
                    "coming later",
                ))
                .child(unavailable(
                    "publish-sound-audio",
                    "Publish sound as audio signal",
                    "coming later",
                ))
                .when(!published.is_empty(), |d| {
                    d.child(menu::separator(theme)).children(published)
                }),
        )
    }

    /// The lanes: sideways-scrolling, as wide as the song.
    fn render_timeline(
        &self,
        rows: &[(&NodeSnapshot, &RowLayout)],
        theme: &Theme,
        bars: u32,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zoom = self.arrange_view.zoom;
        // Where each row starts, for the drag preview.
        let mut row_tops = Vec::with_capacity(rows.len());
        let mut y = 0.;
        for (_, layout) in rows.iter().filter(|(_, l)| l.ghost.is_none()) {
            row_tops.push(y);
            y += layout.height;
        }
        let mut overlay = self.clip_drag_overlay(&row_tops, theme);
        // Rows on screen with their tops and heights, for dropping files.
        let drop_rows: Vec<(NodeId, f32, f32)> = rows
            .iter()
            .filter(|(_, l)| l.ghost.is_none())
            .zip(&row_tops)
            .map(|((node, layout), top)| (node.id, *top, layout.height))
            .collect();
        overlay.extend(self.file_drop_overlay(&drop_rows, theme));
        let scissor_rows = drop_rows.clone();
        // Only what is near the screen gets elements (`paint::visible_x`).
        let near_x = crate::paint::visible_x(&self.arrange_view.x);
        let near_y = crate::paint::visible_y(&self.arrange_view.y);
        let near = move |start: f64, end: f64| {
            near_x.is_none_or(|(left, right)| {
                zoom.beat_to_x(end) >= left && zoom.beat_to_x(start) <= right
            })
        };
        // Every clip as shown this frame, for the parents' combined blocks.
        let shown_clips = self.shown_clips();
        let mut row_top = 0.;
        let lanes: Vec<_> = rows
            .iter()
            .map(|(node, layout)| {
                let top = row_top;
                row_top += layout.height;
                let row_near = near_y
                    .is_none_or(|(above, below)| top + layout.height >= above && top <= below);
                // Rows far off screen are empty boxes of the same height.
                if !row_near {
                    return div().h(px(layout.height)).flex_none().into_any_element();
                }
                let color = if node.muted {
                    theme.muted_content
                } else {
                    theme.content(node.color)
                };
                let id = node.id;
                let takes_clips = node.takes_midi();
                let audio = node.kind == NodeKind::Audio && node.parent.is_some();
                // The lane's clips near the screen, painted in one drawing.
                let painted: Vec<crate::clip_paint::Painted> = crate::clip_edit::node_clips(node)
                    .filter(|_| row_near)
                    .filter(|(_, clip)| {
                        near(
                            clip.start().as_beats(),
                            (clip.start() + clip.length()).as_beats(),
                        )
                    })
                    .map(|(cid, clip)| {
                        // A clip moves with its block while dragged.
                        let shown = match &clip {
                            crate::clip_edit::AnyClip::Midi(midi) => self
                                .combined_drag
                                .as_ref()
                                .and_then(|d| d.shifted(cid, midi))
                                .map_or(clip, crate::clip_edit::AnyClip::Midi),
                            crate::clip_edit::AnyClip::Audio(_) => clip,
                        };
                        self.painted_clip(cid, &shown, &node.name, color, theme)
                    })
                    .collect();
                // Where each clip is, for presses and the cursor.
                let spans: Vec<(daw_engine::ClipId, f32, f32)> =
                    painted.iter().map(|c| (c.id, c.left, c.width)).collect();
                // What a press here would do, or what the drag is doing: the
                // ends resize, the loop zone drags out repeats.
                let clip_height = zoom.row_height - 2. * crate::clip_paint::CLIP_INSET;
                let lanes = self.arrange_lanes_bounds.get();
                let dragging = self.clip_drag.as_ref().filter(|d| d.started());
                let part = match dragging {
                    Some(drag) => drag
                        .held()
                        .iter()
                        .any(|h| h.node == id)
                        .then_some((drag.grab(), drag.pointer())),
                    None => self
                        .arrange_hover
                        .filter(|(n, _, _)| *n == id && self.arrange_tool == ArrangeTool::Pointer)
                        .map(|(_, _, grab)| (grab, self.arrange_pointer.unwrap_or_default())),
                };
                let resize_cursor = matches!(part, Some((Grab::Start | Grab::End, _)));
                // GPUI has no loop cursor: hide it and draw the icon there.
                let loop_cursor = part.filter(|(grab, _)| *grab == Grab::Loop).map(|(_, at)| {
                    let x = (at.x - lanes.left()) / px(1.);
                    let y = (at.y - lanes.top()) / px(1.) - top;
                    div()
                        .absolute()
                        .left(px(x - LOOP_CURSOR / 2.))
                        .top(px(y - LOOP_CURSOR / 2.))
                        .child(icon(Icon::Loop, theme.accent, LOOP_CURSOR))
                });
                let press_spans = spans.clone();
                let lane = div()
                    .debug_selector(|| format!("arrange-lane-{}", id.0))
                    .relative()
                    .h(px(zoom.row_height))
                    .flex_none()
                    .border_b_1()
                    .border_color(theme.hairline)
                    // A press on a clip picks it up (or an end, to resize;
                    // a double-click opens it). A press on empty lane clears
                    // the clip selection; a double-click on a MIDI node's
                    // lane adds a one-bar clip there and opens the editor
                    // on it.
                    .when(resize_cursor, |d| d.cursor_ew_resize())
                    .when_some(loop_cursor, |d, cursor| {
                        d.cursor_none(gpui::CursorStyle::Arrow).child(cursor)
                    })
                    // Which clip and part the pointer is on: redraw when that
                    // changes, and with every move over a loop zone (its
                    // drawn cursor follows the pointer).
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            let lanes = this.arrange_lanes_bounds.get();
                            let x = (event.position.x - lanes.left()) / px(1.);
                            let y = (event.position.y - lanes.top()) / px(1.)
                                - top
                                - crate::clip_paint::CLIP_INSET;
                            let hover = crate::clip_paint::clip_at(&spans, x, y, clip_height)
                                .map(|(clip, grab)| (id, clip, grab));
                            let on_loop = matches!(hover, Some((_, _, Grab::Loop)));
                            let was_on_loop = this.arrange_pointer.is_some();
                            this.arrange_pointer = on_loop.then_some(event.position);
                            if hover != this.arrange_hover || on_loop || was_on_loop {
                                this.arrange_hover = hover;
                                this.hover_changed(cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            let lanes = this.arrange_lanes_bounds.get();
                            let x = (event.position.x - lanes.left()) / px(1.);
                            let y = (event.position.y - lanes.top()) / px(1.)
                                - top
                                - crate::clip_paint::CLIP_INSET;
                            if let Some((clip, grab)) =
                                crate::clip_paint::clip_at(&press_spans, x, y, clip_height)
                            {
                                this.press_painted_clip(clip, grab, audio, event);
                                return;
                            }
                            this.selected_clips.clear();
                            this.selected_combined = None;
                            if event.click_count >= 2 && takes_clips {
                                this.add_clip_at(id, event.position);
                            } else if event.click_count >= 2 && audio {
                                this.open_audio_editor(id, 0.);
                            }
                        }),
                    )
                    // A parent shows the clips below it combined, under its
                    // own clips.
                    .children(if node.children.is_empty() || !row_near {
                        Vec::new()
                    } else {
                        self.combined_elements(id, &shown_clips, theme, cx)
                    })
                    .child(
                        crate::clip_paint::clips_canvas(painted)
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                    );
                animated(layout, zoom.row_height, lane).into_any_element()
            })
            .collect();

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

        scroll::axis_locked(div())
            .id("timeline")
            .flex_1()
            .min_w_0()
            // A row, so the content stretches to the full height.
            .flex()
            .overflow_x_scroll()
            .track_scroll(&self.arrange_view.x)
            .child(
                div()
                    .id("arrange-lanes")
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .child({
                        let cell = self.arrange_lanes_bounds.clone();
                        canvas(move |bounds, _, _| cell.set(bounds), |_, (), _, _| {})
                            .absolute()
                            .size_full()
                    })
                    // Leaving the lanes leaves any clip.
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !hovered && this.arrange_hover.take().is_some() {
                            this.arrange_pointer = None;
                            this.hover_changed(cx);
                        }
                    }))
                    // Audio files dragged in from outside the app.
                    .on_drag_move::<ExternalPaths>(cx.listener(
                        |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                            let count = event.drag(cx).paths().len();
                            this.file_hover = Some((event.event.position, count));
                        },
                    ))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, _| {
                        this.drop_files(paths.paths(), &drop_rows);
                    }))
                    .children(grid_lines(zoom, bars, theme))
                    .children(lanes)
                    .children(overlay)
                    .children(self.scissors_layer(&scissor_rows, theme, cx))
                    .child(playhead),
            )
    }

    pub(crate) fn render_ruler(
        &self,
        theme: &Theme,
        bars: u32,
        pixels_per_beat: f32,
        // The page's sideways scroll, to label only what is near the screen.
        scroll: &gpui::ScrollHandle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bounds_cell = self.ruler_bounds.clone();
        // The loop strip along the top half; bar numbers below it.
        let width = bars as f32 * BEATS_PER_BAR as f32 * pixels_per_beat;
        let loop_strip = self.render_loop_strip(
            crate::loop_strip::LoopRuler::Main,
            pixels_per_beat,
            width,
            theme,
            cx,
        );

        let step = ruler_label_step(pixels_per_beat);
        // Only labels near the screen (`paint::visible_x`).
        let near_x = crate::paint::visible_x(scroll);
        let bar_width = BEATS_PER_BAR as f32 * pixels_per_beat;
        let labels = (0..bars).step_by(step as usize).filter(move |bar| {
            let x = *bar as f32 * bar_width;
            near_x.is_none_or(|(left, right)| x + bar_width >= left && x <= right)
        });
        let labels = labels.map(|bar| {
            div()
                .absolute()
                .left(px(bar as f32 * BEATS_PER_BAR as f32 * pixels_per_beat + 4.))
                .bottom(px(1.))
                .text_size(px(10.))
                .text_color(theme.text_tertiary)
                .child((bar + 1).to_string())
        });

        div()
            .relative()
            .h(px(RULER_HEIGHT))
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.hairline)
            .cursor_pointer()
            .child(
                // Records where the ruler is on screen, to map clicks to beats.
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(labels)
            .child(loop_strip)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    let origin = this.ruler_bounds.get().origin.x;
                    let x = (event.position.x - origin) / px(1.);
                    this.session.apply(Command::Locate {
                        position: x_to_snapped_position(x, pixels_per_beat),
                    });
                }),
            )
    }
}

/// A row (or its lane) as it is drawn this frame: as tall as it has grown,
/// shifted while it glides, and faded in or out. Its content stays full
/// height, anchored at the top edge, so an opening row slides out from
/// under the row above it (its folder) and a closing one slides back up.
fn animated(layout: &RowLayout, row_height: f32, content: impl IntoElement) -> gpui::Div {
    div()
        .relative()
        .flex_none()
        .h(px(layout.height))
        .top(px(layout.offset))
        .opacity(layout.opacity)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top_0()
                .h(px(row_height))
                .child(content),
        )
}

/// Width of the nesting map: none without folders; up to four columns
/// show in full, and past that four and a half (the rest scroll sideways
/// inside it).
fn map_width(columns: usize) -> f32 {
    if (columns as f32) <= MAP_VISIBLE_COLUMNS {
        full_map_width(columns)
    } else {
        MAP_PAD + MAP_VISIBLE_COLUMNS * MAP_COLUMN
    }
}

/// Width of the nesting map showing all `columns`.
fn full_map_width(columns: usize) -> f32 {
    if columns == 0 {
        0.
    } else {
        columns as f32 * MAP_COLUMN + 2. * MAP_PAD
    }
}

/// A node being dragged to a new place in the tree.
#[derive(Debug, Clone)]
struct DraggedNode {
    id: NodeId,
    name: String,
    /// A branch is one kind: it only drops where that kind goes.
    kind: NodeKind,
}

impl Render for DraggedNode {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        div()
            .px(px(8.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(theme.group)
            .border_1()
            .border_color(theme.edge)
            .shadow_md()
            .text_size(px(12.))
            .text_color(theme.text)
            .child(self.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_map_shows_four_columns_then_half_of_the_next() {
        assert_eq!(map_width(0), 0., "no folders, no map");
        assert_eq!(map_width(1), MAP_COLUMN + 2. * MAP_PAD);
        assert_eq!(map_width(4), 4. * MAP_COLUMN + 2. * MAP_PAD);
        assert_eq!(
            map_width(5),
            MAP_PAD + 4.5 * MAP_COLUMN,
            "half a column peeks"
        );
        assert_eq!(map_width(9), map_width(5));
        assert_eq!(
            full_map_width(9),
            9. * MAP_COLUMN + 2. * MAP_PAD,
            "while dragging"
        );
    }
}
