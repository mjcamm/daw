//! Clips on Arrange in hand: selecting, moving, copying and resizing them.
//!
//! A press on a clip selects it (Shift adds or removes) and starts a drag
//! of the selection; a press near either end resizes instead. While the
//! drag runs the originals stay faintly in place, ghosts show where the
//! clips would land with the bar and beat, the receiving rows light up, and
//! rows that cannot take clips turn the ghosts red. Ctrl copies, Alt flips
//! snapping, Escape cancels, and the timeline scrolls near its edges. The
//! drop is one undo step (`clip_edit::land`). The rules live in
//! `clip_edit.rs`; this file is the hand on the mouse.

use crate::clip_edit::{self, Grab, Held, Landing, Plan};
use crate::layout::note_rows;
use crate::project::ProjectView;
use crate::scroll::{self, Extent};
use crate::slider::Axis;
use crate::theme::{ContentColor, Theme};
use crate::tree::{self, display_order};
use crate::zoom::Zoom;
use daw_engine::{ClipId, Command, Edit, MidiClip, Node, NodeId, Ticks};
use gpui::{
    AnyElement, Context, FontWeight, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, div,
    prelude::*, px,
};

/// Pixels the pointer moves before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 3.;
/// Width of the resize zone at each end of a clip.
const EDGE: f32 = 6.;
const CLIP_INSET: f32 = 4.;
const CLIP_LABEL_HEIGHT: f32 = 16.;

/// A drag of the selected clips, previewed until release.
#[derive(Debug, Clone)]
pub struct ClipDrag {
    grab: Grab,
    held: Vec<Held>,
    grabbed: usize,
    origin: Point<Pixels>,
    pointer: Point<Pixels>,
    /// The timeline's scroll when the drag began, so scrolling during the
    /// drag counts as movement.
    scroll_start: Point<Pixels>,
    started: bool,
    copy: bool,
    flip_snap: bool,
    pixels_per_beat: f32,
    row_height: f32,
    /// Each row's node and whether it plays clips, in display order.
    rows: Vec<(NodeId, bool)>,
}

impl ClipDrag {
    /// Whether the drag has moved far enough to count.
    pub fn started(&self) -> bool {
        self.started
    }

    /// The clips being moved (left faint in place), unless copying.
    pub fn moving(&self) -> Vec<ClipId> {
        if self.copy && self.grab == Grab::Body {
            Vec::new()
        } else {
            self.held.iter().map(|h| h.id).collect()
        }
    }
}

/// Rows in display order: each node and whether it plays clips.
fn row_kinds(rows: &[&daw_engine::NodeSnapshot]) -> Vec<(NodeId, bool)> {
    rows.iter().map(|n| (n.id, n.takes_midi())).collect()
}

impl ProjectView {
    fn timeline_scroll(&self) -> Point<Pixels> {
        gpui::point(
            self.arrange_view.x.offset().x,
            self.arrange_view.y.offset().y,
        )
    }

    /// Starts a press on a clip: selects it and, unless Shift took it out
    /// of the selection, begins a drag of the selection.
    pub(crate) fn press_clip(&mut self, id: ClipId, grab: Grab, event: &MouseDownEvent) {
        self.selected_combined = None;
        if event.modifiers.shift {
            if !self.selected_clips.remove(&id) {
                self.selected_clips.insert(id);
            }
        } else if !self.selected_clips.contains(&id) {
            self.selected_clips.clear();
            self.selected_clips.insert(id);
        }
        if !self.selected_clips.contains(&id) {
            return;
        }
        let snapshot = self.session.snapshot();
        let shown = display_order(snapshot, &self.folded);
        let rows = row_kinds(&shown);
        let held: Vec<Held> = shown
            .iter()
            .enumerate()
            .flat_map(|(row, node)| {
                node.clips
                    .iter()
                    .filter(|(clip, _)| self.selected_clips.contains(clip))
                    .map(move |(clip, c)| Held {
                        id: *clip,
                        node: node.id,
                        row,
                        clip: c.clone(),
                    })
            })
            .collect();
        let Some(grabbed) = held.iter().position(|h| h.id == id) else {
            return;
        };
        self.clip_drag = Some(ClipDrag {
            grab,
            held,
            grabbed,
            origin: event.position,
            pointer: event.position,
            scroll_start: self.timeline_scroll(),
            started: false,
            copy: false,
            flip_snap: false,
            pixels_per_beat: self.arrange_view.zoom.pixels_per_beat,
            row_height: self.arrange_view.zoom.row_height,
            rows,
        });
    }

    /// Follows the pointer; false if no clip drag is running.
    pub(crate) fn drag_clips(&mut self, position: Point<Pixels>, modifiers: Modifiers) -> bool {
        let Some(drag) = &mut self.clip_drag else {
            return false;
        };
        drag.pointer = position;
        drag.copy = modifiers.secondary();
        drag.flip_snap = modifiers.alt;
        let moved = (position.x - drag.origin.x) / px(1.);
        let moved_y = (position.y - drag.origin.y) / px(1.);
        if moved.hypot(moved_y) >= DRAG_THRESHOLD {
            drag.started = true;
        }
        true
    }

    /// Where the clips would land now.
    pub(crate) fn clip_plan(&self) -> Option<(&ClipDrag, Plan)> {
        let drag = self.clip_drag.as_ref().filter(|d| d.started)?;
        let scrolled = drag.scroll_start - self.timeline_scroll();
        let dx = (drag.pointer.x - drag.origin.x + scrolled.x) / px(1.);
        let dy = (drag.pointer.y - drag.origin.y + scrolled.y) / px(1.);
        let delta = Ticks::from_beats_f64(f64::from(dx / drag.pixels_per_beat));
        let row_shift = if drag.grab == Grab::Body {
            (dy / drag.row_height).round() as i64
        } else {
            0
        };
        let snap = if drag.flip_snap {
            self.snap.inverted()
        } else {
            self.snap
        };
        let plan = clip_edit::plan(
            &drag.held,
            drag.grabbed,
            drag.grab,
            delta,
            row_shift,
            &drag.rows,
            |t| snap.round(t),
        );
        Some((drag, plan))
    }

    /// Ends a clip drag: lands the clips as one edit if they can go there.
    pub(crate) fn finish_clip_drag(&mut self) {
        let landing = self
            .clip_plan()
            .filter(|(_, plan)| plan.allowed)
            .map(|(drag, plan)| {
                let snapshot = self.session.snapshot();
                let copy = drag.copy && drag.grab == Grab::Body;
                let new_node = tree::next_node_id(snapshot);
                let mut extra = Vec::new();
                if plan.places.iter().any(|(node, _)| node.is_none()) {
                    let source = snapshot.node(drag.held[0].node);
                    let name = source.map_or("Synth", |n| n.name.as_str());
                    extra.push(Edit::AddNode {
                        id: new_node,
                        parent: NodeId::ROOT,
                        index: snapshot.root().children.len(),
                        node: Node::new(
                            &tree::next_name(snapshot, name),
                            tree::next_color(snapshot),
                        )
                        .with_instrument(tree::next_device_id(snapshot)),
                    });
                }
                let landings: Vec<Landing> = drag
                    .held
                    .iter()
                    .zip(&plan.places)
                    .map(|(held, (node, clip))| Landing {
                        id: (!copy).then_some(held.id),
                        node: node.unwrap_or(new_node),
                        clip: clip.clone(),
                    })
                    .collect();
                let moving = drag.moving();
                clip_edit::land(snapshot, &landings, &moving, extra)
            });
        self.clip_drag = None;
        if let Some((edit, landed)) = landing {
            self.session.apply(Command::Edit(edit));
            // The clips just placed (copies included) are the selection.
            self.selected_clips = landed.into_iter().collect();
        }
    }

    /// Escape: drops a drag without changing anything.
    pub(crate) fn cancel_clip_drag(&mut self) -> bool {
        self.clip_drag.take().is_some()
    }

    /// Deletes the selected clips as one step; false if none are selected.
    pub(crate) fn delete_clips(&mut self) -> bool {
        if self.selected_clips.is_empty() {
            return false;
        }
        let edits = self
            .selected_clips
            .iter()
            .map(|id| Edit::RemoveClip { id: *id })
            .collect();
        self.selected_clips.clear();
        self.session.apply(Command::Edit(Edit::Batch(edits)));
        true
    }

    /// Opens the editor on a node at the time where its lane was
    /// double-clicked (drawing there makes the clips).
    pub(crate) fn open_editor_at(&mut self, node: NodeId, position: Point<Pixels>) {
        let view = &self.arrange_view;
        let bounds = view.x.bounds();
        let x = (position.x - bounds.left() - view.x.offset().x) / px(1.);
        let beat = view.zoom.x_to_beats(x).max(0.);
        // Open a little before the spot, on the bar.
        let start = (beat / 4.).floor() * 4.;
        self.open_node_editor(node, start);
    }

    /// While dragging near the timeline's edges, scrolls it. Called every
    /// frame.
    pub(crate) fn autoscroll_clip_drag(&self) {
        let Some(drag) = self.clip_drag.as_ref().filter(|d| d.started) else {
            return;
        };
        for (handle, axis) in [
            (&self.arrange_view.x, Axis::Horizontal),
            (&self.arrange_view.y, Axis::Vertical),
        ] {
            let bounds = handle.bounds();
            let (pointer, low, high) = match axis {
                Axis::Horizontal => (drag.pointer.x, bounds.left(), bounds.right()),
                Axis::Vertical => (drag.pointer.y, bounds.top(), bounds.bottom()),
            };
            let step = scroll::edge_step(pointer / px(1.), low / px(1.), high / px(1.));
            if step != 0. {
                let extent = Extent::of(handle, axis);
                scroll::scroll_to(handle, axis, extent.scrolled + step);
            }
        }
    }

    /// A clip on its lane: selectable, draggable, with resize zones at both
    /// ends. Moving clips stay faint in place while dragged.
    pub(crate) fn clip_element(
        &self,
        id: ClipId,
        clip: &MidiClip,
        label: &str,
        color: ContentColor,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = self.arrange_view.zoom;
        let selected = self.selected_clips.contains(&id);
        let moving = self
            .clip_drag
            .as_ref()
            .is_some_and(|d| d.started() && d.moving().contains(&id));
        let width = zoom.beat_to_x(clip.length.as_beats());
        let edge = |grab: Grab| {
            div()
                .id((
                    if grab == Grab::Start {
                        "clip-start"
                    } else {
                        "clip-end"
                    },
                    id.0,
                ))
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(EDGE))
                .when(grab == Grab::Start, |d| d.left_0())
                .when(grab == Grab::End, |d| d.right_0())
                .cursor_ew_resize()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.press_clip(id, grab, event);
                    }),
                )
        };
        let edges = (width >= 3. * EDGE).then(|| [edge(Grab::Start), edge(Grab::End)]);
        clip_body(clip, label, color, zoom, selected, theme)
            .id(("clip", id.0))
            .when(moving, |d| d.opacity(0.3))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count >= 2 {
                        // Double-click opens the clip in the piano roll.
                        this.clip_drag = None;
                        this.open_note_editor(id);
                        return;
                    }
                    this.press_clip(id, Grab::Body, event);
                }),
            )
            .children(edges.into_iter().flatten())
            .into_any_element()
    }

    /// The drag's preview over the lanes: receiving rows lit, ghosts where
    /// the clips would land, the grabbed one labelled with its bar and beat.
    /// `row_tops` gives each row's top; one past the last is the new-node
    /// row.
    pub(crate) fn clip_drag_overlay(&self, row_tops: &[f32], theme: &Theme) -> Vec<AnyElement> {
        let Some((drag, plan)) = self.clip_plan() else {
            return Vec::new();
        };
        let snapshot = self.session.snapshot();
        let zoom = self.arrange_view.zoom;
        let row_of = |node: Option<NodeId>| match node {
            None => drag.rows.len(),
            Some(id) => drag.rows.iter().position(|(n, _)| *n == id).unwrap_or(0),
        };
        let top_of = |row: usize| {
            row_tops
                .get(row)
                .copied()
                .unwrap_or_else(|| row_tops.last().copied().unwrap_or(0.) + zoom.row_height)
        };
        let warn = theme.record;
        let mut tint = if plan.allowed { theme.accent } else { warn };
        tint.a = 0.07;
        let mut out: Vec<AnyElement> = plan
            .rows
            .iter()
            .map(|&row| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top_of(row)))
                    .h(px(zoom.row_height))
                    .bg(tint)
                    .when(row == drag.rows.len(), |d| {
                        // The new node a drop here makes.
                        d.border_t_1().border_b_1().border_color(theme.accent)
                    })
                    .into_any_element()
            })
            .collect();
        for (index, ((node, clip), held)) in plan.places.iter().zip(&drag.held).enumerate() {
            let source = snapshot.node(held.node);
            let colors = source.map_or(theme.content(0), |n| theme.content(n.color));
            let label = source.map_or("", |n| n.name.as_str());
            let mut ghost = clip_body(clip, label, colors, zoom, true, theme);
            if !plan.allowed {
                ghost = ghost.border_color(warn);
            }
            let top = top_of(row_of(*node));
            // A solid backing, so the ghost reads as replacing what it
            // covers rather than mixing with it.
            let backing = div()
                .absolute()
                .left(px(zoom.beat_to_x(clip.start.as_beats())))
                .top(px(CLIP_INSET))
                .w(px(zoom.beat_to_x(clip.length.as_beats())))
                .h(px(zoom.row_height - 2. * CLIP_INSET))
                .rounded(px(5.))
                .bg(theme.window);
            out.push(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(top))
                    .h(px(zoom.row_height))
                    .child(backing)
                    .child(ghost)
                    .into_any_element(),
            );
            if index == drag.grabbed {
                let (at, text) = match drag.grab {
                    Grab::Body | Grab::Start => (clip.start, clip_edit::bar_beat(clip.start)),
                    Grab::End => (
                        clip_edit::end(clip),
                        clip_edit::bar_beat(clip_edit::end(clip)),
                    ),
                };
                let new_node = node.is_none();
                let text = if !plan.allowed {
                    "Can't go here".to_owned()
                } else if new_node {
                    format!("New node · {text}")
                } else if drag.copy && drag.grab == Grab::Body {
                    format!("Copy · {text}")
                } else {
                    text
                };
                out.push(
                    div()
                        .absolute()
                        .left(px(zoom.beat_to_x(at.as_beats())))
                        .top(px((top - 18.).max(0.)))
                        .px(px(5.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(if plan.allowed { theme.accent } else { warn })
                        .text_color(theme.on_accent)
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(text)
                        .into_any_element(),
                );
            }
        }
        out
    }
}

/// A clip's look: soft fill, outline (accent when selected), name, notes.
fn clip_body(
    clip: &MidiClip,
    label: &str,
    color: ContentColor,
    zoom: Zoom,
    selected: bool,
    theme: &Theme,
) -> gpui::Div {
    let left = zoom.beat_to_x(clip.start.as_beats());
    let width = zoom.beat_to_x(clip.length.as_beats());
    let note_area = (zoom.row_height - 2. * CLIP_INSET - CLIP_LABEL_HEIGHT - 6.).max(0.);
    let notes = clip
        .notes
        .iter()
        .zip(note_rows(clip))
        .filter(|(note, _)| note.start < clip.length && note.start + note.length > Ticks::ZERO)
        .map(move |(note, row)| {
            let start = note.start.max(Ticks::ZERO);
            let end = (note.start + note.length).min(clip.length);
            div()
                .absolute()
                .left(px(zoom.beat_to_x(start.as_beats())))
                .top(px(CLIP_LABEL_HEIGHT + row * note_area))
                .w(px((zoom.beat_to_x((end - start).as_beats()) - 1.).max(2.)))
                .h(px(3.))
                .rounded(px(1.))
                .bg(color.detail)
        });
    div()
        .absolute()
        .left(px(left))
        .top(px(CLIP_INSET))
        .w(px(width))
        .h(px(zoom.row_height - 2. * CLIP_INSET))
        .rounded(px(5.))
        .bg(color.fill)
        .border_1()
        .border_color(if selected {
            theme.accent
        } else {
            color.outline
        })
        .when(selected, |d| d.border_2())
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
