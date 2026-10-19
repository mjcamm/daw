//! The tree and note map behind the piano roll (`docs/design/note-flow.md`):
//! which nodes sit in a branch, which notes each one's instruments play,
//! and the names of named rows (a drum's trigger note).

use crate::piano_roll::NoteEditor;
use crate::project::ProjectView;
use daw_engine::{NodeId, NodeSnapshot, NoteSet, Snapshot};

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

/// The nodes from `root` down whose own instruments play `pitch`.
#[cfg_attr(not(test), allow(dead_code, reason = "used by the node editor next"))]
pub fn owners(snapshot: &Snapshot, root: NodeId, pitch: u8) -> Vec<NodeId> {
    tree_nodes(snapshot, root)
        .into_iter()
        .filter(|(n, _)| own_notes(n).contains(pitch))
        .map(|(n, _)| n.id)
        .collect()
}

impl ProjectView {
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
}
