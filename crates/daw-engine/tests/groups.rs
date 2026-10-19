//! Group clips: a window on a parent whose notes live in member clips below
//! it (`docs/design/note-flow.md`).

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::{
    ClipGroup, ClipId, Command, CommandError, Edit, Engine, EngineConfig, MidiClip, Node, NodeId,
    ProjectFile, Ticks,
};

const DRUMS: NodeId = NodeId(10);
const KICK: NodeId = NodeId(11);
const SNARE: NodeId = NodeId(12);
const OTHER: NodeId = NodeId(13);
const BEAT: ClipId = ClipId(100);

fn apply(engine: &mut Engine, edit: Edit) -> Result<(), CommandError> {
    engine.apply(Command::Edit(edit))
}

fn clip() -> MidiClip {
    MidiClip {
        start: Ticks::ZERO,
        length: Ticks::from_bars(1),
        notes: Vec::new(),
    }
}

fn group(members: &[u64]) -> ClipGroup {
    ClipGroup {
        name: "Beat".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::from_bars(1),
        members: members.iter().map(|&m| ClipId(m)).collect(),
    }
}

/// Song › Drums › [Kick, Snare], and Other beside Drums; a clip on each of
/// Kick (1), Snare (2) and Other (3).
fn engine() -> Engine {
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    let mut drums = Node::new("Drums", 0);
    drums.children = vec![(KICK, Node::new("Kick", 1)), (SNARE, Node::new("Snare", 2))];
    apply(
        &mut engine,
        Edit::AddNode {
            id: DRUMS,
            parent: NodeId::ROOT,
            index: 0,
            node: drums,
        },
    )
    .unwrap();
    apply(
        &mut engine,
        Edit::AddNode {
            id: OTHER,
            parent: NodeId::ROOT,
            index: 1,
            node: Node::new("Other", 3),
        },
    )
    .unwrap();
    for (node, id) in [(KICK, 1), (SNARE, 2), (OTHER, 3)] {
        apply(
            &mut engine,
            Edit::AddClip {
                node,
                id: ClipId(id),
                clip: clip(),
            },
        )
        .unwrap();
    }
    engine
}

#[test]
fn a_group_holds_members_from_below_its_node() {
    let mut engine = engine();
    apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: BEAT,
            group: group(&[1, 2]),
        },
    )
    .unwrap();
    let snapshot = engine.snapshot();
    let drums = snapshot.node(DRUMS).unwrap();
    assert_eq!(drums.groups[0].0, BEAT);
    assert_eq!(drums.groups[0].1.members, [ClipId(1), ClipId(2)]);
}

#[test]
fn members_must_be_below_and_in_one_group() {
    let mut engine = engine();
    let refused = apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: BEAT,
            group: group(&[1, 3]),
        },
    )
    .unwrap_err();
    assert_eq!(
        refused,
        CommandError::InvalidMember {
            group: BEAT,
            clip: ClipId(3)
        },
        "Other is not under Drums"
    );
    apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: BEAT,
            group: group(&[1]),
        },
    )
    .unwrap();
    let refused = apply(
        &mut engine,
        Edit::AddGroup {
            node: NodeId::ROOT,
            id: ClipId(101),
            group: group(&[1]),
        },
    )
    .unwrap_err();
    assert!(
        matches!(refused, CommandError::InvalidMember { .. }),
        "already in Beat"
    );
    let refused = apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: ClipId(1),
            group: group(&[]),
        },
    )
    .unwrap_err();
    assert!(
        matches!(refused, CommandError::DuplicateClip { .. }),
        "groups share the clip ids"
    );
}

#[test]
fn group_edits_undo() {
    let mut engine = engine();
    apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: BEAT,
            group: group(&[1, 2]),
        },
    )
    .unwrap();
    let moved = ClipGroup {
        start: Ticks::from_bars(2),
        ..group(&[1])
    };
    apply(
        &mut engine,
        Edit::SetGroup {
            id: BEAT,
            group: moved.clone(),
        },
    )
    .unwrap();
    assert_eq!(engine.snapshot().node(DRUMS).unwrap().groups[0].1, moved);
    apply(&mut engine, Edit::RemoveGroup { id: BEAT }).unwrap();
    assert!(engine.snapshot().node(DRUMS).unwrap().groups.is_empty());
    engine.apply(Command::Undo).unwrap();
    engine.apply(Command::Undo).unwrap();
    assert_eq!(
        engine.snapshot().node(DRUMS).unwrap().groups[0].1,
        group(&[1, 2])
    );
}

#[test]
fn groups_survive_saving() {
    let mut engine = engine();
    apply(
        &mut engine,
        Edit::AddGroup {
            node: DRUMS,
            id: BEAT,
            group: group(&[1, 2]),
        },
    )
    .unwrap();
    apply(
        &mut engine,
        Edit::AddGroup {
            node: NodeId::ROOT,
            id: ClipId(101),
            group: group(&[3]),
        },
    )
    .unwrap();
    let json = engine.to_file().to_json();
    let (mut reopened, _processor) = Engine::new(EngineConfig::default());
    reopened
        .load(ProjectFile::from_json(&json).unwrap())
        .unwrap();
    let snapshot = reopened.snapshot();
    assert_eq!(snapshot.node(DRUMS).unwrap().groups[0].1, group(&[1, 2]));
    assert_eq!(
        snapshot.node(NodeId::ROOT).unwrap().groups[0].1,
        group(&[3])
    );
}
