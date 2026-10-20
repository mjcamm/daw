//! The tree and note map behind the piano roll (`docs/design/note-flow.md`):
//! which nodes sit in a branch, which notes each one's instruments play,
//! and the names of named rows (a drum's trigger note).

use crate::icons::{Icon, icon};
use crate::piano_roll::NoteEditor;
use crate::project::ProjectView;
use crate::theme::Theme;
use crate::timeline::notes_of;
use crate::tooltip;
use daw_engine::{MidiNote, NodeId, NodeSnapshot, NoteSet, Snapshot};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, FontWeight, MouseButton, div, ease_out_quint,
    prelude::*, px,
};
use std::time::Duration;

/// Width of the tree beside the roll.
pub const TREE_WIDTH: f32 = 190.;
/// How long a tree row flashes after it gains a note or the pen.
const FLASH: Duration = Duration::from_millis(500);
/// How long "Drawing to" flashes when the edited node changes.
const BING: Duration = Duration::from_millis(700);

/// The top of a node's branch: its ancestor just under the Song (the Song
/// itself for the Song).
pub fn branch_top(snapshot: &Snapshot, node: NodeId) -> NodeId {
    let mut at = node;
    while let Some(parent) = snapshot.node(at).and_then(|n| n.parent) {
        if parent == NodeId::ROOT {
            return at;
        }
        at = parent;
    }
    at
}

/// `root` and everything below it, with each one's depth under it, in
/// tree order.
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

/// The notes a node's own instruments play (switched-off ones aside).
pub fn own_notes(node: &NodeSnapshot) -> NoteSet {
    node.devices
        .iter()
        .filter(|d| d.enabled)
        .fold(NoteSet::NONE, |set, d| set.union(d.plays))
}

/// The notes (song time) of every node in `view`'s branch but `edited`,
/// the ones shown softer, with their nodes; nodes with none are left out.
pub fn other_notes(
    snapshot: &Snapshot,
    view: NodeId,
    edited: NodeId,
) -> Vec<(NodeSnapshot, Vec<MidiNote>)> {
    tree_nodes(snapshot, view)
        .into_iter()
        .filter(|(n, _)| n.id != edited)
        .map(|(n, _)| (n.clone(), notes_of(&n.clips)))
        .filter(|(_, notes)| !notes.is_empty())
        .collect()
}

/// The nodes from `root` down whose own instruments play `pitch`.
pub fn owners(snapshot: &Snapshot, root: NodeId, pitch: u8) -> Vec<NodeId> {
    tree_nodes(snapshot, root)
        .into_iter()
        .filter(|(n, _)| own_notes(n).contains(pitch))
        .map(|(n, _)| n.id)
        .collect()
}

impl ProjectView {
    /// The node a new note on `pitch` would go to from the editor's node:
    /// the one node at or below it that plays the pitch, else the pen's.
    pub(crate) fn new_note_owner(&self, editor: &NoteEditor, pitch: u8) -> NodeId {
        let snapshot = self.session.snapshot();
        let below = snapshot
            .node(editor.node)
            .is_some_and(|n| !n.children.is_empty());
        if !below {
            return editor.node;
        }
        match owners(snapshot, editor.node, pitch).as_slice() {
            [only] => *only,
            _ => editor.target,
        }
    }

    /// The notes of the branch in view other than the edited node's, with
    /// their nodes.
    fn inherited(&self, editor: &NoteEditor) -> Vec<(NodeSnapshot, Vec<MidiNote>)> {
        other_notes(self.session.snapshot(), editor.node, editor.target)
    }

    /// Pitches of the notes below, so folded rows keep them.
    pub(crate) fn inherited_pitches(&self, editor: &NoteEditor) -> Vec<u8> {
        self.inherited(editor)
            .into_iter()
            .flat_map(|(_, notes)| notes.into_iter().map(|n| n.pitch))
            .collect()
    }

    /// The rest of the branch's notes, softer, in their nodes' colours.
    /// Pressing one makes its node the edited one; the same press then
    /// reaches the grid, which picks the note up as usual.
    pub(crate) fn inherited_notes(
        &self,
        editor: &NoteEditor,
        ppb: f32,
        key: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Only notes near the screen get elements (`paint::visible_x`).
        let near_x = crate::paint::visible_x(&self.notes_view.x);
        let near = move |n: &daw_engine::MidiNote| {
            near_x.is_none_or(|(left, right)| {
                ((n.start + n.length).as_beats() as f32) * ppb >= left
                    && (n.start.as_beats() as f32) * ppb <= right
            })
        };
        self.inherited(editor)
            .into_iter()
            .flat_map(|(node, notes)| {
                let colors = theme.content(node.color);
                let plays = own_notes(&node);
                let owner = node.id;
                let name = node.name.clone();
                notes
                    .into_iter()
                    .enumerate()
                    .filter(|(_, n)| near(n))
                    .filter_map(|(index, n)| {
                        let row = editor.rows.row_of(n.pitch)?;
                        let mut body = colors.detail;
                        // Inherited notes are softer; ones nothing plays,
                        // fainter still.
                        body.a *= if plays.contains(n.pitch) { 0.4 } else { 0.18 };
                        let mut edge = colors.outline;
                        edge.a *= 0.6;
                        Some(
                            div()
                                .id(("inherited-note", (owner.0 as usize) << 20 | index))
                                .absolute()
                                .left(px(n.start.as_beats() as f32 * ppb))
                                .top(px(row as f32 * key + 1.))
                                .w(px((n.length.as_beats() as f32 * ppb - 1.).max(3.)))
                                .h(px(key - 2.))
                                .rounded(px(2.))
                                .bg(body)
                                .border_1()
                                .border_color(edge)
                                .cursor_pointer()
                                .tooltip(tooltip::text(format!("{name}: press to edit")))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, _| this.set_note_target(owner)),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn set_note_target(&mut self, node: NodeId) {
        if let Some(editor) = &mut self.note_editor {
            editor.retarget(node);
        }
    }

    /// "Drawing to ● Name" in the header: the node being edited. When it
    /// changes, the name flashes in its colour.
    pub(crate) fn drawing_to(&self, editor: &NoteEditor, theme: &Theme) -> Option<AnyElement> {
        let node = self.session.snapshot().node(editor.target)?;
        let color = theme.content(node.color).detail;
        let name = div()
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(6.))
            .h(px(20.))
            .rounded(px(10.))
            .border_1()
            .border_color(gpui::transparent_black())
            .child(div().size(px(8.)).rounded(px(4.)).bg(color))
            .child(div().text_color(color).child(node.name.clone()));
        let fresh = editor
            .flash()
            .is_some_and(|(n, at)| n == editor.target && at.elapsed() < BING);
        if fresh {
            self.animate();
        }
        let name = if fresh {
            name.with_animation(
                ("drawing-to", editor.flashes()),
                Animation::new(BING).with_easing(ease_out_quint()),
                move |el, t| {
                    // A bright pop that settles.
                    let mut glow = color;
                    glow.a = 0.45 * (1. - t);
                    let mut ring = color;
                    ring.a = 1. - t;
                    el.bg(glow).border_color(ring)
                },
            )
            .into_any_element()
        } else {
            name.into_any_element()
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(3.))
                .text_size(px(11.))
                .text_color(theme.text_secondary)
                .child("Drawing to")
                .child(name)
                .into_any_element(),
        )
    }

    /// The tree beside the roll: the editor's node's whole branch, each
    /// with colour, name and note count; a name selects that node, the pen
    /// column sets where new notes go.
    pub(crate) fn render_tree(
        &self,
        editor: &NoteEditor,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let snapshot = self.session.snapshot();
        let top = branch_top(snapshot, editor.node);
        let hover = theme.hover;
        let mut selected = theme.accent;
        selected.a = 0.16;
        let pen_on = editor.target;
        let flash = editor.flash();
        let rows = tree_nodes(snapshot, top).into_iter().map(|(node, depth)| {
            let id = node.id;
            let focused = editor.node == id;
            let colors = theme.content(node.color);
            let count = notes_of(&node.clips).len();
            let flashing = flash.is_some_and(|(n, at)| n == id && at.elapsed() < FLASH);
            if flashing {
                self.animate();
            }
            let pen = (!own_notes(node).is_empty()).then(|| {
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
                        this.set_note_target(id);
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
                .on_click(cx.listener(move |this, _, _, _| this.selected_node = Some(id)))
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
                .children(rows.collect::<Vec<_>>())
                .into_any_element(),
        )
    }

    /// For the editor's node: the notes it and everything below it play,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{
        Command, DeviceId, Edit, Engine, EngineConfig, Instrument, InstrumentKind, Node,
    };

    #[test]
    fn owners_are_the_nodes_whose_instruments_play_a_note() {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        let sampler = |note: u8| Instrument {
            note: Some(note),
            ..Instrument::new(InstrumentKind::SingleSampler)
        };
        let mut drums = Node::new("Drums", 0);
        let mut kick = Node::new("Kick", 1);
        kick.instruments.push((DeviceId(301), sampler(36)));
        drums.children = vec![
            (NodeId(11), kick),
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
        let snapshot = engine.snapshot();
        assert_eq!(owners(&snapshot, NodeId(10), 36), [NodeId(11), NodeId(13)]);
        assert_eq!(owners(&snapshot, NodeId(10), 50), [NodeId(13)]);
        let names: Vec<&str> = tree_nodes(&snapshot, NodeId(10))
            .iter()
            .map(|(n, _)| n.name.as_str())
            .collect();
        assert_eq!(names, ["Drums", "Kick", "Pad"]);
    }

    #[test]
    fn the_branch_shows_softer_all_but_the_edited_nodes_notes() {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        let mut parent = Node::new("Parent", 0);
        parent.children = vec![
            (NodeId(11), Node::new("A", 1)),
            (NodeId(12), Node::new("B", 2)),
            (NodeId(13), Node::new("Empty", 3)),
        ];
        let apply = |engine: &mut Engine, edit| engine.apply(Command::Edit(edit)).unwrap();
        apply(
            &mut engine,
            Edit::AddNode {
                id: NodeId(10),
                parent: NodeId::ROOT,
                index: 0,
                node: parent,
            },
        );
        let note = |pitch: u8| MidiNote {
            start: daw_engine::Ticks::ZERO,
            length: daw_engine::Ticks::from_beats(1),
            pitch,
            velocity: 100,
        };
        for (node, id, pitch) in [(10, 1, 40), (11, 2, 50), (12, 3, 60)] {
            apply(
                &mut engine,
                Edit::AddClip {
                    node: NodeId(node),
                    id: daw_engine::ClipId(id),
                    clip: daw_engine::MidiClip {
                        start: daw_engine::Ticks::from_beats(4),
                        length: daw_engine::Ticks::from_beats(4),
                        notes: vec![note(pitch)],
                        looping: None,
                    },
                },
            );
        }
        let snapshot = engine.snapshot();
        let shown = |edited: u64| -> Vec<(u64, Vec<u8>)> {
            other_notes(&snapshot, NodeId(10), NodeId(edited))
                .into_iter()
                .map(|(n, notes)| (n.id.0, notes.iter().map(|n| n.pitch).collect()))
                .collect()
        };
        assert_eq!(
            shown(11),
            [(10, vec![40]), (12, vec![60])],
            "editing A: the parent's own and B's notes are softer; Empty has none"
        );
        assert_eq!(shown(12), [(10, vec![40]), (11, vec![50])]);
        assert_eq!(
            other_notes(&snapshot, NodeId(10), NodeId(10))[0].1[0].start,
            daw_engine::Ticks::from_beats(4),
            "notes are in song time"
        );
    }
}
