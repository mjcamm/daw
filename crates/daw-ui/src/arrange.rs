//! The Arrange page: the node list on the left, the timeline beside it,
//! framed by scrollbars and zoom sliders (see `frame.rs`).

use crate::frame::{Timeline, grid_lines};
use crate::icons::{Icon, icon};
use crate::layout::{
    BEATS_PER_BAR, meter_height, note_rows, ruler_label_step, timeline_bars, x_to_snapped_position,
};
use crate::menu::{self, OpenMenu};
use crate::parameters::small_button;
use crate::project::{ProjectView, RenameTarget, song_clips};
use crate::scroll;
use crate::signals::{node_has_notes, published_signals};
use crate::theme::{ContentColor, Theme};
use crate::tooltip;
use crate::tree::{self, Bar, Drop, display_order};
use crate::zoom::Zoom;
use daw_engine::{ClipId, Command, Edit, MidiClip, Node, NodeId, NodeSnapshot};
use gpui::{
    ClickEvent, Context, FontWeight, Hsla, MouseButton, MouseDownEvent, canvas, div, prelude::*, px,
};

const RULER_HEIGHT: f32 = 28.;
const NODE_LIST_WIDTH: f32 = 260.;
const CLIP_INSET: f32 = 4.;
const CLIP_LABEL_HEIGHT: f32 = 16.;
const METER_HEIGHT: f32 = 32.;
/// Width of one column of the nesting map, and its padding.
const MAP_COLUMN: f32 = 18.;
const MAP_PAD: f32 = 4.;
/// Columns the map shows before it scrolls sideways.
const MAP_VISIBLE_COLUMNS: usize = 4;

impl ProjectView {
    pub(crate) fn render_arrange(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let bars = timeline_bars(song_clips(snapshot), snapshot.loop_range);
        let width = zoom.beat_to_x(f64::from(bars) * BEATS_PER_BAR);
        let rows = display_order(snapshot, &self.folded);
        let map_bars = tree::bars(&rows);
        let map_width = map_width(tree::columns(&map_bars));
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
                    .child(self.render_map(&rows, &map_bars, map_width, theme, cx))
                    .child(self.render_node_list(&rows, theme, cx))
                    .child(self.render_timeline(&rows, theme, bars, width)),
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

    fn render_node_list(
        &self,
        rows: &[&NodeSnapshot],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .w(px(NODE_LIST_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.edge)
            .children(
                rows.iter()
                    .map(|node| self.node_row(node, theme, cx))
                    .collect::<Vec<_>>(),
            )
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
        let meter = self.session.meter(id);
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
        let kind = if song {
            "Master"
        } else if node.has_instrument() {
            "MIDI · Synth"
        } else {
            "Folder"
        };
        let mut drop_highlight = theme.accent;
        drop_highlight.a = 0.18;
        let dragged = DraggedNode {
            id,
            name: node.name.clone(),
        };

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
                if dragged.id == id {
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
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(meter_bar(meter.left, meter_size, theme))
                    .child(meter_bar(meter.right, meter_size, theme)),
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
                    .on_click(cx.listener(|this, _, _, _| this.toggle_menu(OpenMenu::AddNode))),
            )
            .when(open, |d| {
                d.child(menu::float(
                    menu::panel(theme)
                        .on_mouse_down_out(cx.listener(|this, _, _, _| this.dismiss_menu()))
                        .child(
                            menu::item(theme, "add-instrument", "Instrument (Synth)", "")
                                .on_click(cx.listener(|this, _, _, _| this.add_node(true))),
                        )
                        .child(
                            menu::item(theme, "add-folder", "Folder", "")
                                .on_click(cx.listener(|this, _, _, _| this.add_node(false))),
                        ),
                ))
            });
        let group = action(
            "node-group",
            Icon::FolderPlus,
            "Group into a folder",
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

    /// The nesting map: a column per level, the Song at the far left. Each
    /// folder's bar runs beside what is under it, and an arrow points into
    /// the folder's own row. The chevron folds a folder; dropping a node on
    /// a bar puts it last in that folder.
    fn render_map(
        &self,
        rows: &[&NodeSnapshot],
        bars: &[Bar],
        width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row_height = self.arrange_view.zoom.row_height;
        let snapshot = self.session.snapshot();
        let content_width = tree::columns(bars) as f32 * MAP_COLUMN + 2. * MAP_PAD;
        let hover = theme.hover;
        let marks = bars.iter().filter_map(|bar| {
            let node = snapshot.node(bar.node)?;
            let id = bar.node;
            let colors = theme.content(node.color);
            let x = MAP_PAD + bar.column as f32 * MAP_COLUMN;
            let folded = self.folded.contains(&id);
            let foldable = id != NodeId::ROOT && !node.children.is_empty();
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
                    .on_click(cx.listener(move |this, _, _, _| this.toggle_fold(id)))
            });
            // The bar: beside what is under the folder, or (folded, empty)
            // a short pill on its own row.
            let (top, height) = if bar.first < bar.row {
                (
                    bar.first as f32 * row_height + 4.,
                    (bar.row - bar.first) as f32 * row_height - 8.,
                )
            } else {
                (bar.row as f32 * row_height + 6., 14.)
            };
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
                .drag_over::<DraggedNode>(move |style, _, _, _| style.bg(drop_fill))
                .on_drop(cx.listener(move |this, dragged: &DraggedNode, _, _| {
                    this.move_node(dragged.id, Drop::Into(id));
                }));
            // The arrow into the folder's row.
            let arrow = (bar.first < bar.row).then(|| {
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(bar.row as f32 * row_height + row_height / 2. - 8.))
                    .child(icon(Icon::IntoFolder, colors.detail, 14.))
            });
            Some([
                Some(body.into_any_element()),
                arrow.map(|a| a.into_any_element()),
            ])
        });
        let content = div()
            .relative()
            .w(px(content_width))
            .h(px(rows.len() as f32 * row_height))
            .flex_shrink_0()
            .children(marks.flatten().flatten().collect::<Vec<_>>());
        scroll::axis_locked(div())
            .id("node-map")
            .w(px(width))
            .flex_none()
            .flex()
            .overflow_x_scroll()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.hairline)
            .child(content)
    }

    fn toggle_fold(&mut self, node: NodeId) {
        if !self.folded.remove(&node) {
            self.folded.insert(node);
        }
    }

    /// Adds a node that plays notes, or a folder, after the selected node,
    /// and selects it.
    fn add_node(&mut self, instrument: bool) {
        self.open_menu = None;
        let snapshot = self.session.snapshot();
        let (parent, index) = tree::new_node_place(snapshot, self.selected_node);
        let id = tree::next_node_id(snapshot);
        let color = tree::next_color(snapshot);
        let node = if instrument {
            Node::new(&tree::next_name(snapshot, "Synth"), color).with_instrument()
        } else {
            Node::new(&tree::next_name(snapshot, "Folder"), color)
        };
        self.session.apply(Command::Edit(Edit::AddNode {
            id,
            parent,
            index,
            node,
        }));
        self.selected_node = Some(id);
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
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.toggle_menu(OpenMenu::NodeSignals(id));
                    })),
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
        rows: &[&NodeSnapshot],
        theme: &Theme,
        bars: u32,
        width: f32,
    ) -> impl IntoElement {
        let zoom = self.arrange_view.zoom;
        let lanes = rows.iter().map(|node| {
            let color = if node.muted {
                theme.muted_content
            } else {
                theme.content(node.color)
            };
            div()
                .relative()
                .h(px(zoom.row_height))
                .flex_none()
                .border_b_1()
                .border_color(theme.hairline)
                .children(
                    node.clips
                        .iter()
                        .map(|(id, clip)| clip_view(*id, clip, &node.name, color, zoom)),
                )
        });

        let playhead = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(zoom.beat_to_x(self.session.position_beats())))
            .w(px(1.))
            .bg(theme.accent);

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
                    .relative()
                    .flex_shrink_0()
                    .w(px(width))
                    .flex()
                    .flex_col()
                    .children(grid_lines(zoom, bars, theme))
                    .children(lanes)
                    .child(playhead),
            )
    }

    pub(crate) fn render_ruler(
        &self,
        theme: &Theme,
        bars: u32,
        pixels_per_beat: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bounds_cell = self.ruler_bounds.clone();
        let mut loop_fill = theme.accent;
        loop_fill.a = 0.2;

        let loop_band = self.session.snapshot().loop_range.map(|range| {
            let start = range.start.as_beats() as f32 * pixels_per_beat;
            let end = range.end.as_beats() as f32 * pixels_per_beat;
            div()
                .absolute()
                .top(px(2.))
                .h(px(6.))
                .left(px(start))
                .w(px(end - start))
                .rounded(px(2.))
                .bg(loop_fill)
        });

        let step = ruler_label_step(pixels_per_beat);
        let labels = (0..bars).step_by(step as usize).map(|bar| {
            div()
                .absolute()
                .left(px(bar as f32 * BEATS_PER_BAR as f32 * pixels_per_beat + 4.))
                .bottom(px(4.))
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
            .children(loop_band)
            .children(labels)
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

/// Width of the nesting map: up to [`MAP_VISIBLE_COLUMNS`] columns show;
/// more scroll sideways inside it.
fn map_width(columns: usize) -> f32 {
    columns.min(MAP_VISIBLE_COLUMNS) as f32 * MAP_COLUMN + 2. * MAP_PAD
}

/// A node being dragged to a new place in the tree.
#[derive(Debug, Clone)]
struct DraggedNode {
    id: NodeId,
    name: String,
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

fn meter_bar(peak: f32, size: f32, theme: &Theme) -> impl IntoElement {
    let height = meter_height(peak);
    div()
        .w(px(4.))
        .h(px(size))
        .rounded(px(1.))
        .bg(theme.control)
        .flex()
        .flex_col()
        .justify_end()
        .child(
            div()
                .w_full()
                .h(px(size * height))
                .rounded(px(1.))
                .bg(if height > 0.98 {
                    theme.meter_high
                } else {
                    theme.meter_low
                }),
        )
}

fn clip_view(
    id: ClipId,
    clip: &MidiClip,
    label: &str,
    color: ContentColor,
    zoom: Zoom,
) -> impl IntoElement {
    let left = zoom.beat_to_x(clip.start.as_beats());
    let width = zoom.beat_to_x(clip.length.as_beats());
    let note_area = (zoom.row_height - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.).max(0.);
    let notes = clip
        .notes
        .iter()
        .zip(note_rows(clip))
        .filter(|(note, _)| note.start < clip.length)
        .map(move |(note, row)| {
            let end = (note.start + note.length).min(clip.length);
            div()
                .absolute()
                .left(px(zoom.beat_to_x(note.start.as_beats())))
                .top(px(CLIP_LABEL_HEIGHT + row * note_area))
                .w(px(
                    (zoom.beat_to_x((end - note.start).as_beats()) - 1.).max(2.)
                ))
                .h(px(3.))
                .rounded(px(1.))
                .bg(color.detail)
        });
    div()
        .id(("clip", id.0))
        .absolute()
        .left(px(left))
        .top(px(CLIP_INSET))
        .w(px(width))
        .h(px(zoom.row_height - 2. * CLIP_INSET))
        .rounded(px(5.))
        .bg(color.fill)
        .border_1()
        .border_color(color.outline)
        .overflow_hidden()
        .child(
            div()
                .px(px(6.))
                .pt(px(2.))
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color.detail)
                .child(label.to_owned()),
        )
        .children(notes)
}
