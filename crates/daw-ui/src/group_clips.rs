//! Group clips on Arrange (`docs/design/note-flow.md`): a group lives on a
//! parent's lane and holds no notes; its member clips on the children hold
//! them. Double-click an empty spot on a folder's lane to make one. Dragging
//! a group moves its members with it, its end edge resizes them all, and
//! Delete removes it with its members, each as one undo step. A member
//! dragged on its own leaves the group.

use crate::clip_edit::Grab;
use crate::icons::{Icon, icon};
use crate::layout::note_rows;
use crate::project::ProjectView;
use crate::theme::Theme;
use crate::tooltip;
use daw_engine::{ClipGroup, ClipId, Command, Edit, MidiClip, NodeId, NodeSnapshot, Ticks};
use gpui::{
    AnyElement, Context, FontWeight, Modifiers, MouseButton, MouseDownEvent, Pixels, Point, div,
    prelude::*, px,
};

const INSET: f32 = 4.;
const LABEL_HEIGHT: f32 = 16.;
const EDGE: f32 = 6.;
/// Pixels the pointer moves before a press on a group becomes a drag.
const DRAG_THRESHOLD: f32 = 3.;

/// A group being moved or resized, previewed until release.
#[derive(Debug, Clone)]
pub(crate) struct GroupDrag {
    id: ClipId,
    grab: Grab,
    origin: Point<Pixels>,
    original: ClipGroup,
    pub preview: ClipGroup,
    started: bool,
}

impl GroupDrag {
    /// Whether this drag moves group `id` (and has begun).
    pub fn is(&self, id: ClipId) -> bool {
        self.id == id && self.started
    }

    /// Where a member clip shows while its group is dragged.
    pub fn member(&self, clip: &MidiClip) -> MidiClip {
        let shift = self.preview.start - self.original.start;
        MidiClip {
            start: clip.start + shift,
            length: if self.grab == Grab::End {
                self.preview.length
            } else {
                clip.length
            },
            ..clip.clone()
        }
    }
}

/// The group a clip is a member of, anywhere in the song.
pub(crate) fn group_of(
    snapshot: &daw_engine::Snapshot,
    clip: ClipId,
) -> Option<(NodeId, ClipId, &ClipGroup)> {
    snapshot.nodes.iter().find_map(|node| {
        node.groups
            .iter()
            .find(|(_, g)| g.members.contains(&clip))
            .map(|(id, g)| (node.id, *id, g))
    })
}

/// A group's member clips with the node each is on.
pub(crate) fn members<'a>(
    snapshot: &'a daw_engine::Snapshot,
    group: &ClipGroup,
) -> Vec<(&'a NodeSnapshot, ClipId, &'a MidiClip)> {
    group
        .members
        .iter()
        .filter_map(|id| {
            snapshot.nodes.iter().find_map(|node| {
                node.clips
                    .iter()
                    .find(|(c, _)| c == id)
                    .map(|(c, clip)| (node, *c, clip))
            })
        })
        .collect()
}

/// A name for a new group: "Pattern n", the first n not taken.
fn next_group_name(snapshot: &daw_engine::Snapshot) -> String {
    let taken: Vec<&str> = snapshot
        .nodes
        .iter()
        .flat_map(|n| n.groups.iter().map(|(_, g)| g.name.as_str()))
        .collect();
    (1..)
        .map(|n| format!("Pattern {n}"))
        .find(|name| !taken.contains(&name.as_str()))
        .unwrap_or_default()
}

impl ProjectView {
    /// Makes a group clip where an empty spot on a folder's lane was
    /// double-clicked: a bar long from the grid line at or before the
    /// pointer, selected and open in the editor.
    pub(crate) fn add_group_at(&mut self, node: NodeId, position: Point<Pixels>) {
        let view = &self.arrange_view;
        let bounds = view.x.bounds();
        let x = (position.x - bounds.left() - view.x.offset().x) / px(1.);
        let at = Ticks::from_beats_f64(view.zoom.x_to_beats(x).max(0.));
        let snapshot = self.session.snapshot();
        let id = crate::clip_edit::next_clip_id(snapshot);
        let group = ClipGroup {
            name: next_group_name(snapshot),
            start: self.snap.floor(at),
            length: Ticks::from_beats(4),
            members: Vec::new(),
        };
        self.session
            .apply(Command::Edit(Edit::AddGroup { node, id, group }));
        self.selected_clips.clear();
        self.selected_group = Some(id);
        self.open_group_editor(id, node, node);
    }

    fn press_group(&mut self, id: ClipId, grab: Grab, event: &MouseDownEvent) {
        self.selected_clips.clear();
        self.selected_group = Some(id);
        let snapshot = self.session.snapshot();
        let Some(group) = snapshot
            .nodes
            .iter()
            .find_map(|n| n.groups.iter().find(|(g, _)| *g == id).map(|(_, g)| g))
        else {
            return;
        };
        self.group_drag = Some(GroupDrag {
            id,
            grab,
            origin: event.position,
            original: group.clone(),
            preview: group.clone(),
            started: false,
        });
    }

    /// Follows a group drag; false if none is running.
    pub(crate) fn drag_group(&mut self, position: Point<Pixels>, modifiers: Modifiers) -> bool {
        let snap = if modifiers.alt {
            self.snap.inverted()
        } else {
            self.snap
        };
        let ppb = self.arrange_view.zoom.pixels_per_beat;
        let grid = self.snap.grid.ticks();
        let Some(drag) = &mut self.group_drag else {
            return false;
        };
        let dx = (position.x - drag.origin.x) / px(1.);
        if !drag.started && dx.abs() < DRAG_THRESHOLD {
            return true;
        }
        drag.started = true;
        let moved = Ticks::from_beats_f64(f64::from(dx / ppb));
        let original = &drag.original;
        drag.preview = match drag.grab {
            Grab::End => {
                let end = snap.round(original.start + original.length + moved);
                ClipGroup {
                    length: (end - original.start).max(grid),
                    ..original.clone()
                }
            }
            _ => ClipGroup {
                start: snap.round(original.start + moved).max(Ticks::ZERO),
                ..original.clone()
            },
        };
        true
    }

    /// Ends a group drag: the group and its members move or resize as one
    /// step.
    pub(crate) fn finish_group_drag(&mut self) {
        let Some(drag) = self.group_drag.take() else {
            return;
        };
        if !drag.started || drag.preview == drag.original {
            return;
        }
        let snapshot = self.session.snapshot();
        let mut edits = vec![Edit::SetGroup {
            id: drag.id,
            group: drag.preview.clone(),
        }];
        for (node, id, clip) in members(snapshot, &drag.original) {
            edits.push(Edit::SetClip {
                id,
                node: node.id,
                clip: drag.member(clip),
            });
        }
        self.session.apply(Command::Edit(Edit::Batch(edits)));
    }

    /// Deletes the selected group with its members; false if none is
    /// selected.
    pub(crate) fn delete_group(&mut self) -> bool {
        let Some(id) = self.selected_group.take() else {
            return false;
        };
        let snapshot = self.session.snapshot();
        let Some(group) = snapshot
            .nodes
            .iter()
            .find_map(|n| n.groups.iter().find(|(g, _)| *g == id).map(|(_, g)| g))
        else {
            return false;
        };
        let mut edits: Vec<Edit> = group
            .members
            .iter()
            .filter(|m| {
                snapshot
                    .nodes
                    .iter()
                    .any(|n| n.clips.iter().any(|(c, _)| c == *m))
            })
            .map(|m| Edit::RemoveClip { id: *m })
            .collect();
        edits.push(Edit::RemoveGroup { id });
        if self
            .note_editor
            .as_ref()
            .is_some_and(|e| e.group.as_ref().is_some_and(|g| g.id == id))
        {
            self.close_note_editor();
        }
        self.session.apply(Command::Edit(Edit::Batch(edits)));
        true
    }

    /// The edits that take clips moved on their own out of their groups.
    pub(crate) fn unlink_edits(&self, moved: &[ClipId]) -> Vec<Edit> {
        let snapshot = self.session.snapshot();
        snapshot
            .nodes
            .iter()
            .flat_map(|n| n.groups.iter())
            .filter(|(_, g)| g.members.iter().any(|m| moved.contains(m)))
            .map(|(id, g)| Edit::SetGroup {
                id: *id,
                group: ClipGroup {
                    members: g
                        .members
                        .iter()
                        .copied()
                        .filter(|m| !moved.contains(m))
                        .collect(),
                    ..g.clone()
                },
            })
            .collect()
    }

    /// A group on its parent's lane: its name, and its members' notes in
    /// their nodes' colours.
    pub(crate) fn group_element(
        &self,
        id: ClipId,
        group: &ClipGroup,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let zoom = self.arrange_view.zoom;
        let snapshot = self.session.snapshot();
        let dragging = self.group_drag.as_ref().filter(|d| d.id == id && d.started);
        let shown = dragging.map_or(group, |d| &d.preview);
        let selected = self.selected_group == Some(id);
        let left = zoom.beat_to_x(shown.start.as_beats());
        let width = zoom.beat_to_x(shown.length.as_beats());
        let note_area = (zoom.row_height - 2. * INSET - LABEL_HEIGHT - 6.).max(0.);
        let notes: Vec<AnyElement> = members(snapshot, group)
            .into_iter()
            .flat_map(|(node, _, clip)| {
                let color = theme.content(node.color).detail;
                let rows = note_rows(clip);
                clip.notes
                    .iter()
                    .zip(rows)
                    .filter(|(n, _)| n.start < clip.length)
                    .map(|(note, row)| {
                        let end = (note.start + note.length).min(clip.length);
                        div()
                            .absolute()
                            .left(px(zoom.beat_to_x(note.start.max(Ticks::ZERO).as_beats())))
                            .top(px(LABEL_HEIGHT + row * note_area))
                            .w(px(
                                (zoom.beat_to_x((end - note.start).as_beats()) - 1.).max(2.)
                            ))
                            .h(px(3.))
                            .rounded(px(1.))
                            .bg(color)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut fill = theme.control;
        fill.a *= 0.8;
        let end_edge = div()
            .id(("group-end", id.0))
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(EDGE))
            .cursor_ew_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.press_group(id, Grab::End, event);
                }),
            );
        let parent = snapshot
            .nodes
            .iter()
            .find(|n| n.groups.iter().any(|(g, _)| *g == id))
            .map_or(NodeId::ROOT, |n| n.id);
        div()
            .id(("group", id.0))
            .absolute()
            .left(px(left))
            .top(px(INSET))
            .w(px(width))
            .h(px(zoom.row_height - 2. * INSET))
            .rounded(px(5.))
            .bg(fill)
            .border_1()
            .border_color(if selected { theme.accent } else { theme.edge })
            .when(selected, |d| d.border_2())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .pt(px(2.))
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text_secondary)
                    .child(icon(Icon::Layers, theme.text_secondary, 10.))
                    .child(shown.name.clone()),
            )
            .children(notes)
            .child(end_edge)
            .tooltip(tooltip::text(
                "Group clip: its notes live on the nodes below. Double-click to edit them together.",
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if event.click_count >= 2 {
                        this.group_drag = None;
                        this.open_group_editor(id, parent, parent);
                        return;
                    }
                    this.press_group(id, Grab::Body, event);
                }),
            )
            .into_any_element()
    }
}
