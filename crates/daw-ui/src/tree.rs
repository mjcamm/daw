//! The node tree as Arrange and the mixer show it: flat, aligned rows (or
//! strips) with each parent before its children, the Song first, and a
//! nesting map beside them. Pure functions over the snapshot, so they are
//! tested without a window.

use daw_engine::{
    BlockId, ClipId, DeviceId, DeviceRef, Edit, Effect, Instrument, Layer, LayerId, Node, NodeId,
    NodeSnapshot, ParamState, SignalSource, Snapshot,
};
use std::collections::{BTreeMap, HashSet};

/// Whether a node gets a bar in the nesting map: a node with children (a
/// folder), and the Song.
pub fn is_folder(node: &NodeSnapshot) -> bool {
    node.parent.is_none() || !node.children.is_empty()
}

/// The rows in display order: each parent before its children, the Song
/// first. The children of folded nodes are left out.
pub fn display_order<'a>(
    snapshot: &'a Snapshot,
    folded: &HashSet<NodeId>,
) -> Vec<&'a NodeSnapshot> {
    fn visit<'a>(
        snapshot: &'a Snapshot,
        node: &'a NodeSnapshot,
        folded: &HashSet<NodeId>,
        out: &mut Vec<&'a NodeSnapshot>,
    ) {
        out.push(node);
        if !folded.contains(&node.id) {
            for child in node.children.iter().filter_map(|id| snapshot.node(*id)) {
                visit(snapshot, child, folded, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(snapshot, snapshot.root(), folded, &mut out);
    out
}

/// A folder's bar in the nesting map: from its own row down beside what
/// is under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    pub node: NodeId,
    /// Its column: folders at the top level are column 0, at the far left.
    /// The Song has no bar: everything is under it, so it says nothing.
    pub column: usize,
    /// The folder's own row, where the bar starts with its chevron.
    pub row: usize,
    /// The last row under it; equal to `row` when nothing under it shows
    /// (folded).
    pub last: usize,
}

/// Every folder's bar, for rows in display order (not the Song's).
pub fn bars(rows: &[&NodeSnapshot]) -> Vec<Bar> {
    let mut bars = Vec::new();
    for (row, node) in rows.iter().enumerate() {
        if !is_folder(node) || node.id == NodeId::ROOT {
            continue;
        }
        // Its descendants follow it, deeper than it.
        let last = rows[row + 1..]
            .iter()
            .position(|r| r.depth <= node.depth)
            .map_or(rows.len() - 1, |i| row + i);
        bars.push(Bar {
            node: node.id,
            column: node.depth - 1,
            row,
            last,
        });
    }
    bars
}

/// How many columns the map needs.
pub fn columns(bars: &[Bar]) -> usize {
    bars.iter().map(|b| b.column + 1).max().unwrap_or(0)
}

/// The trigger note for a new SingleSampler on `node`: the lowest note from
/// C1 up that no SingleSampler among it and its siblings plays yet, so the
/// drums in a folder never clash.
pub fn next_trigger_note(snapshot: &Snapshot, node: NodeId) -> u8 {
    let siblings: Vec<NodeId> = snapshot
        .node(node)
        .and_then(|n| n.parent)
        .and_then(|p| snapshot.node(p))
        .map_or_else(|| vec![node], |p| p.children.clone());
    let taken: Vec<u8> = siblings
        .iter()
        .filter_map(|id| snapshot.node(*id))
        .flat_map(|n| n.devices.iter().filter_map(|d| d.trigger))
        .collect();
    (daw_engine::DEFAULT_TRIGGER_NOTE..=127)
        .find(|n| !taken.contains(n))
        .unwrap_or(daw_engine::DEFAULT_TRIGGER_NOTE)
}

/// A device id (instrument or effect) no device uses yet.
pub fn next_device_id(snapshot: &Snapshot) -> DeviceId {
    let highest = snapshot
        .nodes
        .iter()
        .flat_map(|n| &n.devices)
        .filter_map(|d| match d.device {
            DeviceRef::Effect(id) | DeviceRef::Instrument(id) => Some(id.0),
            DeviceRef::Channel => None,
        })
        .max();
    DeviceId(highest.map_or(0, |h| h + 1))
}

/// An id no node uses yet.
pub fn next_node_id(snapshot: &Snapshot) -> NodeId {
    NodeId(snapshot.nodes.iter().map(|n| n.id.0).max().unwrap_or(0) + 1)
}

/// The first palette colour no node uses yet (the Song's grey aside); once
/// all are used, the palette goes round again.
pub fn next_color(snapshot: &Snapshot) -> u8 {
    const PALETTE: u8 = 8;
    let used = |c: u8| {
        snapshot
            .nodes
            .iter()
            .any(|n| n.parent.is_some() && n.color == c)
    };
    (0..PALETTE)
        .find(|c| !used(*c))
        .unwrap_or((snapshot.nodes.len() % usize::from(PALETTE)) as u8)
}

/// `base`, or "`base` 2", "`base` 3"... whichever no node uses yet.
pub fn next_name(snapshot: &Snapshot, base: &str) -> String {
    let taken = |name: &str| snapshot.nodes.iter().any(|n| n.name == name);
    if !taken(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .unwrap_or_default()
}

/// Where a new node of `kind` goes: just after the selected node, under the
/// same parent, if that parent takes its kind (a branch is one kind); else
/// (or with nothing, or the Song, selected) at the end of the Song.
pub fn new_node_place(
    snapshot: &Snapshot,
    selected: Option<NodeId>,
    kind: daw_engine::NodeKind,
) -> (NodeId, usize) {
    let after = selected
        .and_then(|id| snapshot.node(id))
        .and_then(|node| place_of(snapshot, node.id))
        .filter(|(parent, _)| {
            *parent == NodeId::ROOT || snapshot.node(*parent).is_some_and(|p| p.kind == kind)
        });
    after.map_or(
        (NodeId::ROOT, snapshot.root().children.len()),
        |(parent, index)| (parent, index + 1),
    )
}

/// A node's parent and position among its siblings.
fn place_of(snapshot: &Snapshot, id: NodeId) -> Option<(NodeId, usize)> {
    let parent = snapshot.node(snapshot.node(id)?.parent?)?;
    let index = parent.children.iter().position(|c| *c == id)?;
    Some((parent.id, index))
}

/// Whether `node` is `ancestor` or anywhere under it.
fn is_within(snapshot: &Snapshot, node: NodeId, ancestor: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = snapshot.node(id).and_then(|n| n.parent);
    }
    false
}

/// Where a dragged node is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    /// On a row: just after that node, in its folder. On the Song's row:
    /// last in the Song.
    After(NodeId),
    /// On a folder's bar: last in that folder.
    Into(NodeId),
}

/// The edit that moves `dragged` to `drop`, or `None` if it would not move
/// or cannot go there.
pub fn move_edit(snapshot: &Snapshot, dragged: NodeId, drop: Drop) -> Option<Edit> {
    let (from_parent, from_index) = place_of(snapshot, dragged)?;
    let (parent, index) = match drop {
        Drop::After(target) if target == NodeId::ROOT => (NodeId::ROOT, usize::MAX),
        Drop::After(target) => {
            let (parent, index) = place_of(snapshot, target)?;
            (parent, index + 1)
        }
        Drop::Into(folder) => (folder, usize::MAX),
    };
    if is_within(snapshot, parent, dragged) {
        return None;
    }
    let siblings = snapshot.node(parent)?.children.len();
    // Positions count after the node is taken out.
    let same = parent == from_parent;
    let mut index = index.min(siblings);
    if same && from_index < index {
        index -= 1;
    }
    let len = siblings - usize::from(same);
    let index = index.min(len);
    if same && index == from_index {
        return None;
    }
    Some(Edit::MoveNode {
        id: dragged,
        parent,
        index,
    })
}

/// The edit that wraps a node in a new parent of its kind, in its place,
/// as one step.
pub fn group_edit(snapshot: &Snapshot, id: NodeId) -> Option<Edit> {
    let (parent, index) = place_of(snapshot, id)?;
    let node = snapshot.node(id)?;
    let folder = next_node_id(snapshot);
    Some(Edit::Batch(vec![
        Edit::AddNode {
            id: folder,
            parent,
            index,
            node: Node::new(&next_name(snapshot, "Group"), node.color).with_kind(node.kind),
        },
        Edit::MoveNode {
            id,
            parent: folder,
            index: 0,
        },
    ]))
}

/// Fresh ids for a copy: one past the highest in use, for each kind.
struct FreshIds {
    node: u64,
    clip: u64,
    device: u64,
    layer: u64,
    block: u64,
}

impl FreshIds {
    fn new(snapshot: &Snapshot) -> Self {
        let mut ids = Self {
            node: 0,
            clip: 0,
            device: 0,
            layer: 0,
            block: 0,
        };
        for node in &snapshot.nodes {
            ids.node = ids.node.max(node.id.0);
            for (clip, _) in &node.clips {
                ids.clip = ids.clip.max(clip.0);
            }
            for device in &node.devices {
                if let DeviceRef::Effect(id) | DeviceRef::Instrument(id) = device.device {
                    ids.device = ids.device.max(id.0);
                }
                for (layer, l) in device.params.iter().flat_map(|p| &p.layers) {
                    ids.layer = ids.layer.max(layer.0);
                    for (block, _) in &l.blocks {
                        ids.block = ids.block.max(block.0);
                    }
                }
            }
        }
        for signal in &snapshot.signals {
            if let SignalSource::Drawn(blocks) = &signal.source {
                for (block, _) in blocks {
                    ids.block = ids.block.max(block.0);
                }
            }
        }
        ids
    }

    fn next(counter: &mut u64) -> u64 {
        *counter += 1;
        *counter
    }
}

fn copy_params(params: &[daw_engine::ParamSnapshot], ids: &mut FreshIds) -> Vec<ParamState> {
    params
        .iter()
        .map(|p| ParamState {
            base: p.base,
            layers: p
                .layers
                .iter()
                .map(|(_, layer)| {
                    let layer = Layer {
                        blocks: layer
                            .blocks
                            .iter()
                            .map(|(_, b)| (BlockId(FreshIds::next(&mut ids.block)), b.clone()))
                            .collect(),
                        ..layer.clone()
                    };
                    (LayerId(FreshIds::next(&mut ids.layer)), layer)
                })
                .collect(),
        })
        .collect()
}

fn copy_node(snapshot: &Snapshot, node: &NodeSnapshot, ids: &mut FreshIds) -> Node {
    let mut copy = Node::new(&node.name, node.color).with_kind(node.kind);
    copy.muted = node.muted;
    copy.soloed = node.soloed;
    for device in &node.devices {
        match device.device {
            DeviceRef::Instrument(_) => {
                if let Some(kind) = device.instrument {
                    let instrument = Instrument {
                        kind,
                        enabled: device.enabled,
                        params: copy_params(&device.params, ids),
                        sample: device.sample,
                        note: device.trigger,
                    };
                    copy.instruments
                        .push((DeviceId(FreshIds::next(&mut ids.device)), instrument));
                }
            }
            DeviceRef::Channel => copy.channel = copy_params(&device.params, ids),
            DeviceRef::Effect(_) => {
                if let Some(kind) = device.effect {
                    let effect = Effect {
                        kind,
                        enabled: device.enabled,
                        params: copy_params(&device.params, ids),
                    };
                    copy.effects
                        .push((DeviceId(FreshIds::next(&mut ids.device)), effect));
                }
            }
        }
    }
    copy.clips = node
        .clips
        .iter()
        .map(|(_, clip)| (ClipId(FreshIds::next(&mut ids.clip)), clip.clone()))
        .collect::<BTreeMap<_, _>>();
    copy.children = node
        .children
        .iter()
        .filter_map(|id| snapshot.node(*id))
        .map(|child| {
            let child_copy = copy_node(snapshot, child, ids);
            (NodeId(FreshIds::next(&mut ids.node)), child_copy)
        })
        .collect();
    copy
}

/// The edit that copies a node, with everything in and under it, just after
/// it; and the copy's id. Layers in the copy listen to the same signals.
pub fn duplicate_edit(snapshot: &Snapshot, id: NodeId) -> Option<(NodeId, Edit)> {
    let (parent, index) = place_of(snapshot, id)?;
    let node = snapshot.node(id)?;
    let mut ids = FreshIds::new(snapshot);
    let mut copy = copy_node(snapshot, node, &mut ids);
    copy.name = next_name(snapshot, &node.name);
    let copy_id = NodeId(FreshIds::next(&mut ids.node));
    Some((
        copy_id,
        Edit::AddNode {
            id: copy_id,
            parent,
            index: index + 1,
            node: copy,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{
        BlendMode, Block, Command, Engine, EngineConfig, FIRST_NODE, MidiClip, ParamId, Shape,
        Ticks,
    };

    const DRUMS: NodeId = NodeId(2);
    const KICK: NodeId = NodeId(3);
    const HATS: NodeId = NodeId(4);

    /// Song › [Synth, Drums › [Kick, Hats]].
    fn engine() -> Engine {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        let mut drums = Node::new("Drums", 0);
        drums.children = vec![
            (KICK, Node::new("Kick", 0).with_instrument(DeviceId(201))),
            (HATS, Node::new("Hats", 1).with_instrument(DeviceId(202))),
        ];
        engine
            .apply(Command::Edit(Edit::AddNode {
                id: DRUMS,
                parent: NodeId::ROOT,
                index: 1,
                node: drums,
            }))
            .unwrap();
        engine
    }

    fn names(rows: &[&NodeSnapshot]) -> Vec<String> {
        rows.iter().map(|n| n.name.clone()).collect()
    }

    fn apply(engine: &mut Engine, edit: Edit) -> Snapshot {
        engine.apply(Command::Edit(edit)).unwrap();
        engine.snapshot()
    }

    #[test]
    fn parents_come_before_their_children_and_the_song_first() {
        let snapshot = engine().snapshot();
        let rows = display_order(&snapshot, &HashSet::new());
        assert_eq!(names(&rows), ["Song", "Synth", "Drums", "Kick", "Hats"]);
        let folded = HashSet::from([DRUMS]);
        let rows = display_order(&snapshot, &folded);
        assert_eq!(names(&rows), ["Song", "Synth", "Drums"]);
    }

    #[test]
    fn bars_run_from_each_folder_down_beside_its_children() {
        let snapshot = engine().snapshot();
        let rows = display_order(&snapshot, &HashSet::new());
        let bars = bars(&rows);
        assert_eq!(
            bars,
            [Bar {
                node: DRUMS,
                column: 0,
                row: 2,
                last: 4
            }],
            "the Song has no bar"
        );
        assert_eq!(columns(&bars), 1);

        let folded = HashSet::from([DRUMS]);
        let rows = display_order(&snapshot, &folded);
        assert_eq!(
            super::bars(&rows)[0],
            Bar {
                node: DRUMS,
                column: 0,
                row: 2,
                last: 2
            },
            "a folded folder's bar is just its row"
        );
    }

    #[test]
    fn new_nodes_go_after_the_selection_or_at_the_end() {
        let snapshot = engine().snapshot();
        let midi = daw_engine::NodeKind::Midi;
        assert_eq!(new_node_place(&snapshot, None, midi), (NodeId::ROOT, 2));
        assert_eq!(new_node_place(&snapshot, Some(KICK), midi), (DRUMS, 1));
        assert_eq!(
            new_node_place(&snapshot, Some(NodeId::ROOT), midi),
            (NodeId::ROOT, 2)
        );
        assert_eq!(
            new_node_place(&snapshot, Some(KICK), daw_engine::NodeKind::Audio),
            (NodeId::ROOT, 2),
            "an Audio node never goes into a MIDI branch"
        );
        assert_eq!(next_node_id(&snapshot), NodeId(5));
        assert_eq!(next_name(&snapshot, "Group"), "Group");
        assert_eq!(next_name(&snapshot, "Drums"), "Drums 2");
        // Drums and Kick are red (0), Hats orange (1), the Synth purple (6).
        assert_eq!(next_color(&snapshot), 2);
    }

    #[test]
    fn dropping_moves_after_a_row_or_into_a_folder() {
        let mut engine = engine();
        let snapshot = engine.snapshot();
        assert_eq!(
            move_edit(&snapshot, FIRST_NODE, Drop::After(KICK)),
            Some(Edit::MoveNode {
                id: FIRST_NODE,
                parent: DRUMS,
                index: 1
            })
        );
        assert_eq!(move_edit(&snapshot, KICK, Drop::After(KICK)), None);
        assert_eq!(move_edit(&snapshot, DRUMS, Drop::Into(DRUMS)), None);
        assert_eq!(
            move_edit(&snapshot, KICK, Drop::Into(DRUMS)),
            Some(Edit::MoveNode {
                id: KICK,
                parent: DRUMS,
                index: 1
            }),
            "to the end of its own folder"
        );
        assert_eq!(
            move_edit(&snapshot, HATS, Drop::Into(DRUMS)),
            None,
            "already last"
        );

        let edit = move_edit(&snapshot, KICK, Drop::After(NodeId::ROOT)).unwrap();
        let moved = apply(&mut engine, edit);
        let rows = display_order(&moved, &HashSet::new());
        assert_eq!(names(&rows), ["Song", "Synth", "Drums", "Hats", "Kick"]);

        let edit = move_edit(&moved, KICK, Drop::After(FIRST_NODE)).unwrap();
        let moved = apply(&mut engine, edit);
        let rows = display_order(&moved, &HashSet::new());
        assert_eq!(names(&rows), ["Song", "Synth", "Kick", "Drums", "Hats"]);
    }

    #[test]
    fn grouping_wraps_a_node_in_a_folder_in_its_place() {
        let mut engine = engine();
        let edit = group_edit(&engine.snapshot(), KICK).unwrap();
        let grouped = apply(&mut engine, edit);
        let rows = display_order(&grouped, &HashSet::new());
        assert_eq!(
            names(&rows),
            ["Song", "Synth", "Drums", "Group", "Kick", "Hats"]
        );
        engine.apply(Command::Undo).unwrap();
        assert_eq!(engine.snapshot().nodes.len(), 5, "one undo step");
    }

    #[test]
    fn duplicates_copy_everything_with_fresh_ids() {
        let mut engine = engine();
        let clip = MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(4),
            notes: Vec::new(),
        };
        let mut layer = Layer::named("Wobble", BlendMode::Add);
        layer.blocks.push((
            BlockId(1),
            Block::new(Ticks::ZERO, Ticks::from_beats(1), Shape::constant(0.7)),
        ));
        for edit in [
            Edit::AddClip {
                node: KICK,
                id: ClipId(1),
                clip,
            },
            Edit::AddEffect {
                node: KICK,
                id: DeviceId(1),
                index: 0,
                effect: Effect::new(daw_engine::EffectKind::Filter),
            },
            Edit::AddLayer {
                param: ParamId {
                    node: KICK,
                    device: DeviceRef::Channel,
                    index: 0,
                },
                id: LayerId(1),
                index: 0,
                layer,
            },
        ] {
            engine.apply(Command::Edit(edit)).unwrap();
        }
        let (copy, edit) = duplicate_edit(&engine.snapshot(), DRUMS).unwrap();
        let snapshot = apply(&mut engine, edit);
        let copy = snapshot.node(copy).unwrap();
        assert_eq!(copy.name, "Drums 2");
        let kick_copy = snapshot.node(copy.children[0]).unwrap();
        assert_eq!(kick_copy.name, "Kick");
        assert_eq!(kick_copy.clips.len(), 1);
        assert_ne!(kick_copy.clips[0].0, ClipId(1));
        assert_eq!(kick_copy.devices.len(), 3, "instrument, filter, channel");
        let layers = &kick_copy.devices[2].params[0].layers;
        assert_eq!(layers[0].1.name, "Wobble");
        assert_ne!(layers[0].0, LayerId(1));
        let rows = display_order(&snapshot, &HashSet::new());
        assert_eq!(
            names(&rows),
            [
                "Song", "Synth", "Drums", "Kick", "Hats", "Drums 2", "Kick", "Hats"
            ]
        );
    }

    #[test]
    fn new_samplers_take_the_next_free_note_among_their_siblings() {
        let mut engine = engine();
        let snapshot = engine.snapshot();
        assert_eq!(next_trigger_note(&snapshot, KICK), 36, "C1 first");
        let sampler = |note: u8| daw_engine::Instrument {
            note: Some(note),
            ..daw_engine::Instrument::new(daw_engine::InstrumentKind::SingleSampler)
        };
        apply(
            &mut engine,
            Edit::AddInstrument {
                node: KICK,
                id: DeviceId(301),
                index: 1,
                instrument: sampler(36),
            },
        );
        let snapshot = apply(
            &mut engine,
            Edit::AddInstrument {
                node: HATS,
                id: DeviceId(302),
                index: 1,
                instrument: sampler(38),
            },
        );
        assert_eq!(next_trigger_note(&snapshot, HATS), 37, "the gap first");
        assert_eq!(
            next_trigger_note(&snapshot, FIRST_NODE),
            36,
            "notes are counted among siblings only"
        );
    }
}
