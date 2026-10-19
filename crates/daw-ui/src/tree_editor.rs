//! The piano roll on a group clip (`docs/design/note-flow.md`): a tree of
//! the group's node and everything below it on the right; clicking a name
//! focuses that node (the roll shows and edits its member clip), clicking
//! a pen makes it where new notes go. With the group's own node focused,
//! every member's notes show in their nodes' colours: clicking one focuses
//! its node, and a new note goes to the one instrument that plays its row,
//! or to the pen's node when several do.

use crate::group_clips::members;
use crate::icons::{Icon, icon};
use crate::piano_roll::NoteEditor;
use crate::project::ProjectView;
use crate::theme::Theme;
use crate::tooltip;
use daw_engine::{
    ClipGroup, ClipId, Command, Edit, MidiClip, MidiNote, NodeId, NodeSnapshot, NoteSet, Snapshot,
};
use gpui::{AnyElement, Context, FontWeight, MouseButton, div, prelude::*, px};
use std::time::{Duration, Instant};

pub const TREE_WIDTH: f32 = 190.;
/// How long a tree row flashes after its node gains a note or the pen.
const FLASH: Duration = Duration::from_millis(500);

/// An editor opened on a group clip.
#[derive(Debug, Clone)]
pub struct GroupMode {
    pub id: ClipId,
    /// The node holding the group: the top of the tree.
    pub root: NodeId,
    /// Where new notes go when a row has several owners.
    pub target: NodeId,
    /// A node to flash in the tree, and since when.
    pub flash: Option<(NodeId, Instant)>,
}

/// The group's node and everything below it, with each one's depth under
/// it, in tree order.
pub fn tree_nodes(snapshot: &Snapshot, root: NodeId) -> Vec<(&NodeSnapshot, usize)> {
    fn visit<'a>(
        snapshot: &'a Snapshot,
        id: NodeId,
        depth: usize,
        out: &mut Vec<(&'a NodeSnapshot, usize)>,
    ) {
        if let Some(node) = snapshot.node(id) {
            out.push((node, depth));
            for child in &node.children {
                visit(snapshot, *child, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(snapshot, root, 0, &mut out);
    out
}

/// The notes a node's own instruments play.
pub fn own_notes(node: &NodeSnapshot) -> NoteSet {
    node.devices
        .iter()
        .filter(|d| d.enabled)
        .fold(NoteSet::NONE, |set, d| set.union(d.plays))
}

/// The nodes in the tree whose own instruments play `pitch`.
pub fn owners(snapshot: &Snapshot, root: NodeId, pitch: u8) -> Vec<NodeId> {
    tree_nodes(snapshot, root)
        .into_iter()
        .filter(|(n, _)| own_notes(n).contains(pitch))
        .map(|(n, _)| n.id)
        .collect()
}

/// Where a new note on `pitch` goes from the group's node: the one node
/// that plays it, else the pen's node.
pub fn route(snapshot: &Snapshot, mode: &GroupMode, pitch: u8) -> NodeId {
    match owners(snapshot, mode.root, pitch).as_slice() {
        [only] => *only,
        _ => mode.target,
    }
}

impl ProjectView {
    fn group(&self, id: ClipId) -> Option<&ClipGroup> {
        self.session
            .snapshot()
            .nodes
            .iter()
            .find_map(|n| n.groups.iter().find(|(g, _)| *g == id).map(|(_, g)| g))
    }

    /// A node's member clip in a group: its id and clip, if it has one.
    fn member_of(&self, group: &ClipGroup, node: NodeId) -> Option<(ClipId, MidiClip)> {
        members(self.session.snapshot(), group)
            .into_iter()
            .find(|(n, _, _)| n.id == node)
            .map(|(_, id, clip)| (id, clip.clone()))
    }

    /// Opens the editor on a group, focused on `focus`.
    pub(crate) fn open_group_editor(&mut self, group: ClipId, root: NodeId, focus: NodeId) {
        let height = self
            .note_editor
            .as_ref()
            .map_or(crate::piano_roll::DEFAULT_HEIGHT, |e| e.height);
        let opened = self.note_editor.as_ref().map(|e| e.opened());
        let mut editor = NoteEditor::new(focus, ClipId(0), height);
        if let Some(opened) = opened {
            editor.set_opened(opened);
        }
        // The pen starts on the first node in the tree that plays notes.
        let target = tree_nodes(self.session.snapshot(), root)
            .into_iter()
            .find(|(n, _)| !own_notes(n).is_empty())
            .map_or(focus, |(n, _)| n.id);
        editor.group = Some(GroupMode {
            id: group,
            root,
            target,
            flash: None,
        });
        self.note_editor = Some(editor);
        self.focus_group_node(focus);
    }

    /// Focuses a node in the open group: the roll edits its member clip
    /// (made with its first note if it has none). A child takes the pen.
    pub(crate) fn focus_group_node(&mut self, node: NodeId) {
        let Some(mode) = self.note_editor.as_ref().and_then(|e| e.group.clone()) else {
            return;
        };
        let Some(group) = self.group(mode.id).cloned() else {
            return;
        };
        let clip = self
            .member_of(&group, node)
            .map_or_else(|| self.fresh_clip_id(), |(id, _)| id);
        if let Some(editor) = &mut self.note_editor {
            editor.node = node;
            editor.clip = clip;
            editor.selection.clear();
            if let Some(mode) = &mut editor.group
                && node != mode.root
            {
                mode.target = node;
            }
        }
    }

    /// A clip id for a member not made yet, unused by any clip or group.
    fn fresh_clip_id(&self) -> ClipId {
        crate::clip_edit::next_clip_id(self.session.snapshot())
    }

    pub(crate) fn set_group_target(&mut self, node: NodeId) {
        if let Some(mode) = self.note_editor.as_mut().and_then(|e| e.group.as_mut()) {
            mode.target = node;
            mode.flash = Some((node, Instant::now()));
        }
    }

    /// The open clip as the editor sees it: a group member not made yet
    /// is an empty clip spanning the group.
    pub(crate) fn virtual_member(&self, editor: &NoteEditor) -> Option<MidiClip> {
        let mode = editor.group.as_ref()?;
        let group = self.group(mode.id)?;
        Some(MidiClip {
            start: group.start,
            length: group.length,
            notes: Vec::new(),
        })
    }

    /// The edit that sets a node's member clip in the open group to
    /// `notes`, making the member if needed.
    pub(crate) fn member_edit(
        &self,
        node: NodeId,
        clip: ClipId,
        notes: Vec<MidiNote>,
    ) -> Option<Edit> {
        let mode = self.note_editor.as_ref()?.group.as_ref()?;
        let group = self.group(mode.id)?;
        if let Some((id, existing)) = self.member_of(group, node) {
            return Some(Edit::SetClip {
                id,
                node,
                clip: MidiClip { notes, ..existing },
            });
        }
        Some(Edit::Batch(vec![
            Edit::AddClip {
                node,
                id: clip,
                clip: MidiClip {
                    start: group.start,
                    length: group.length,
                    notes,
                },
            },
            Edit::SetGroup {
                id: mode.id,
                group: ClipGroup {
                    members: group.members.iter().copied().chain([clip]).collect(),
                    ..group.clone()
                },
            },
        ]))
    }

    /// With the group's own node focused, a new note on a row another node
    /// plays goes to that node (or the pen's); true if it was sent there.
    pub(crate) fn add_routed_note(&mut self, note: MidiNote) -> bool {
        let Some(editor) = self.note_editor.as_ref() else {
            return false;
        };
        let Some(mode) = editor.group.clone() else {
            return false;
        };
        if editor.node != mode.root {
            return false;
        }
        let owner = route(self.session.snapshot(), &mode, note.pitch);
        if owner == editor.node {
            return false;
        }
        let Some(group) = self.group(mode.id).cloned() else {
            return false;
        };
        let (clip, mut notes) = self.member_of(&group, owner).map_or_else(
            || (self.fresh_clip_id(), Vec::new()),
            |(id, c)| (id, c.notes),
        );
        notes.push(note);
        if let Some(edit) = self.member_edit(owner, clip, notes) {
            self.session.apply(Command::Edit(edit));
        }
        self.preview_routed(owner, note);
        if let Some(mode) = self.note_editor.as_mut().and_then(|e| e.group.as_mut()) {
            mode.flash = Some((owner, Instant::now()));
        }
        true
    }

    /// With the group's own node focused, the other members' notes, in
    /// their nodes' colours: clicking one focuses its node with it selected.
    pub(crate) fn member_ghosts(
        &self,
        editor: &NoteEditor,
        ppb: f32,
        key: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(mode) = editor.group.as_ref() else {
            return Vec::new();
        };
        if editor.node != mode.root {
            return Vec::new();
        }
        let Some(group) = self.group(mode.id) else {
            return Vec::new();
        };
        let snapshot = self.session.snapshot();
        members(snapshot, group)
            .into_iter()
            .filter(|(node, _, _)| node.id != editor.node)
            .flat_map(|(node, clip_id, clip)| {
                let colors = theme.content(node.color);
                let owner = node.id;
                let plays = own_notes(node);
                clip.notes
                    .iter()
                    .enumerate()
                    .map(|(index, n)| {
                        let mut body = colors.detail;
                        // A note its node does not play is silent: faded.
                        body.a *= if plays.contains(n.pitch) { 0.85 } else { 0.3 };
                        div()
                            .id(("member-note", (clip_id.0 as usize) << 16 | index))
                            .absolute()
                            .left(px(n.start.as_beats() as f32 * ppb))
                            .top(px(
                                editor.rows.row_of(n.pitch).unwrap_or(0) as f32 * key + 1.
                            ))
                            .w(px((n.length.as_beats() as f32 * ppb - 1.).max(3.)))
                            .h(px(key - 2.))
                            .rounded(px(2.))
                            .bg(body)
                            .border_1()
                            .border_color(colors.outline)
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.focus_group_node(owner);
                                    if let Some(editor) = &mut this.note_editor {
                                        editor.selection = [index].into();
                                    }
                                }),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// For the focused node: the notes it and everything below it play,
    /// the names of named rows (a drum's trigger note), and how many nodes
    /// play a pitch.
    pub(crate) fn row_names(
        &self,
        editor: &NoteEditor,
    ) -> (NoteSet, Vec<(u8, String)>, impl Fn(u8) -> usize + use<>) {
        let snapshot = self.session.snapshot();
        let nodes: Vec<(NodeSnapshot, NoteSet)> = tree_nodes(snapshot, editor.node)
            .into_iter()
            .map(|(n, _)| (n.clone(), own_notes(n)))
            .collect();
        let plays = nodes
            .iter()
            .fold(NoteSet::NONE, |set, (_, p)| set.union(*p));
        let names = nodes
            .iter()
            .flat_map(|(n, _)| {
                n.devices
                    .iter()
                    .filter_map(|d| d.trigger)
                    .map(|t| (t, n.name.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let sets: Vec<NoteSet> = nodes.iter().map(|(_, p)| *p).collect();
        let owners = move |pitch: u8| sets.iter().filter(|s| s.contains(pitch)).count();
        (plays, names, owners)
    }

    /// Pitches of the other members' notes shown with the group's node
    /// focused, so folded rows keep them.
    pub(crate) fn ghost_pitches(&self, editor: &NoteEditor) -> Vec<u8> {
        let Some(mode) = editor.group.as_ref() else {
            return Vec::new();
        };
        if editor.node != mode.root {
            return Vec::new();
        }
        self.group(mode.id).map_or_else(Vec::new, |group| {
            members(self.session.snapshot(), group)
                .into_iter()
                .flat_map(|(_, _, c)| c.notes.iter().map(|n| n.pitch).collect::<Vec<_>>())
                .collect()
        })
    }

    /// The node a new note on `pitch` would go to.
    pub(crate) fn new_note_owner(&self, editor: &NoteEditor, pitch: u8) -> NodeId {
        match &editor.group {
            Some(mode) if editor.node == mode.root => route(self.session.snapshot(), mode, pitch),
            _ => editor.node,
        }
    }

    /// "Drawing to ● Name" for the header, in the target's colour.
    pub(crate) fn drawing_to(&self, editor: &NoteEditor, theme: &Theme) -> Option<AnyElement> {
        let mode = editor.group.as_ref()?;
        let target = if editor.node == mode.root {
            mode.target
        } else {
            editor.node
        };
        let node = self.session.snapshot().node(target)?;
        let color = theme.content(node.color).detail;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(11.))
                .text_color(theme.text_secondary)
                .child("Drawing to")
                .child(div().size(px(8.)).rounded(px(4.)).bg(color))
                .child(div().text_color(color).child(node.name.clone()))
                .into_any_element(),
        )
    }

    /// The tree on the right: the group's node and everything below it,
    /// each with its colour, name and notes in the group; a click focuses,
    /// the pen column sets where new notes go.
    pub(crate) fn render_group_tree(
        &self,
        editor: &NoteEditor,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = editor.group.as_ref()?;
        let group = self.group(mode.id)?;
        let snapshot = self.session.snapshot();
        let counts: Vec<(NodeId, usize)> = members(snapshot, group)
            .into_iter()
            .map(|(n, _, c)| (n.id, c.notes.len()))
            .collect();
        let pen_on = if editor.node == mode.root {
            mode.target
        } else {
            editor.node
        };
        let hover = theme.hover;
        let mut selected = theme.accent;
        selected.a = 0.16;
        let rows = tree_nodes(snapshot, mode.root)
            .into_iter()
            .map(|(node, depth)| {
                let id = node.id;
                let focused = editor.node == id;
                let colors = theme.content(node.color);
                let count = counts.iter().find(|(n, _)| *n == id).map_or(0, |(_, c)| *c);
                let flashing = mode
                    .flash
                    .is_some_and(|(n, at)| n == id && at.elapsed() < FLASH);
                let plays = !own_notes(node).is_empty();
                let pen = plays.then(|| {
                    let lit = pen_on == id;
                    div()
                        .id(("tree-pen", id.0))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .hover(move |s| s.bg(hover))
                        .child(icon(
                            Icon::Pencil,
                            if lit {
                                colors.detail
                            } else {
                                theme.text_tertiary
                            },
                            12.,
                        ))
                        .tooltip(tooltip::text(if lit {
                            "New notes go here"
                        } else {
                            "Draw new notes here"
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_group_target(id);
                        }))
                });
                div()
                    .id(("tree-row", id.0))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(8. + depth as f32 * 12.))
                    .pr(px(4.))
                    .rounded(px(5.))
                    .when(focused, |d| d.bg(selected))
                    .when(flashing, |d| d.bg(colors.fill))
                    .hover(move |s| s.bg(hover))
                    .child(div().size(px(8.)).rounded(px(4.)).bg(colors.detail))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .when(focused, |d| d.font_weight(FontWeight::SEMIBOLD))
                            .child(node.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.text_tertiary)
                            .child(count.to_string()),
                    )
                    .children(pen)
                    .on_click(cx.listener(move |this, _, _, _| this.focus_group_node(id)))
            });
        Some(
            div()
                .w(px(TREE_WIDTH))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(6.))
                .bg(theme.sidebar)
                .border_l_1()
                .border_color(theme.edge)
                .child(
                    div()
                        .px(px(6.))
                        .pb(px(4.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_tertiary)
                        .child(group.name.to_uppercase()),
                )
                .children(rows.collect::<Vec<_>>())
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{DeviceId, Engine, EngineConfig, Instrument, InstrumentKind, Node, NodeId};

    /// Drums › [Kick (C1), Snare (D1), Pad (a synth: every note)].
    fn snapshot() -> Snapshot {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        let sampler = |note: u8| Instrument {
            note: Some(note),
            ..Instrument::new(InstrumentKind::SingleSampler)
        };
        let mut drums = Node::new("Drums", 0);
        let mut kick = Node::new("Kick", 1);
        kick.instruments.push((DeviceId(301), sampler(36)));
        let mut snare = Node::new("Snare", 2);
        snare.instruments.push((DeviceId(302), sampler(38)));
        drums.children = vec![
            (NodeId(11), kick),
            (NodeId(12), snare),
            (
                NodeId(13),
                Node::new("Pad", 3).with_instrument(DeviceId(303)),
            ),
        ];
        engine
            .apply(Command::Edit(Edit::AddNode {
                id: NodeId(10),
                parent: NodeId::ROOT,
                index: 0,
                node: drums,
            }))
            .unwrap();
        engine.snapshot()
    }

    #[test]
    fn a_row_with_one_owner_goes_to_it_and_shared_rows_to_the_pen() {
        let snapshot = snapshot();
        let mode = GroupMode {
            id: ClipId(1),
            root: NodeId(10),
            target: NodeId(13),
            flash: None,
        };
        assert_eq!(owners(&snapshot, NodeId(10), 36), [NodeId(11), NodeId(13)]);
        assert_eq!(route(&snapshot, &mode, 50), NodeId(13), "only the pad");
        let mode = GroupMode {
            target: NodeId(12),
            ..mode
        };
        assert_eq!(route(&snapshot, &mode, 50), NodeId(13), "one owner wins");
        assert_eq!(route(&snapshot, &mode, 36), NodeId(12), "shared: the pen");
        let names: Vec<&str> = tree_nodes(&snapshot, NodeId(10))
            .iter()
            .map(|(n, _)| n.name.as_str())
            .collect();
        assert_eq!(names, ["Drums", "Kick", "Snare", "Pad"]);
    }
}
