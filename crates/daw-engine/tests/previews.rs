//! Previews: a drag's change heard as it moves, kept out of the undo
//! history until the drag lands as one edit.

use daw_engine::{Command, CommandError, Edit, Engine, EngineConfig, Node, NodeId};
use daw_testkit::{VARIED_BLOCKS, render};

fn engine() -> (Engine, daw_engine::AudioProcessor) {
    Engine::new(EngineConfig::default())
}

fn tempo(bpm: f64) -> Edit {
    Edit::SetTempo { bpm }
}

#[test]
fn a_preview_is_heard_at_once() {
    let (mut engine, mut processor) = engine();
    engine.apply(Command::Play).unwrap();
    engine.apply(Command::Preview(tempo(240.0))).unwrap();
    render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    // 240 BPM at 48 kHz is 12,000 frames per beat.
    assert_eq!(engine.snapshot().position_beats, 1.0);
}

#[test]
fn a_preview_is_not_an_edit() {
    let (mut engine, _processor) = engine();
    let revision = engine.revision();
    let saved = engine.content_revision();
    engine.apply(Command::Preview(tempo(130.0))).unwrap();
    assert_eq!(engine.project().tempo_bpm, 130.0);
    // Front ends keep showing their own preview: no new snapshot, nothing
    // to save, nothing to undo.
    assert_eq!(engine.revision(), revision);
    assert_eq!(engine.content_revision(), saved);
    assert!(!engine.snapshot().can_undo);
}

#[test]
fn each_preview_replaces_the_last() {
    let (mut engine, _processor) = engine();
    let id = NodeId(50);
    let add = |name: &str| {
        Command::Preview(Edit::AddNode {
            id,
            parent: NodeId::ROOT,
            index: 0,
            node: Node::new(name, 0),
        })
    };
    engine.apply(add("First")).unwrap();
    // The same id again: fine, as the first preview is gone.
    engine.apply(add("Second")).unwrap();
    assert_eq!(engine.project().node(id).unwrap().name, "Second");
}

#[test]
fn the_edit_that_ends_a_drag_is_one_undo_step() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(130.0))).unwrap();
    engine.apply(Command::Preview(tempo(135.0))).unwrap();
    engine.apply(Command::Edit(tempo(140.0))).unwrap();
    assert_eq!(engine.project().tempo_bpm, 140.0);
    engine.apply(Command::Undo).unwrap();
    assert_eq!(engine.project().tempo_bpm, 120.0);
    assert!(!engine.snapshot().can_undo);
}

#[test]
fn a_cancelled_preview_leaves_the_song_as_it_was() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(130.0))).unwrap();
    engine.apply(Command::CancelPreview).unwrap();
    assert_eq!(engine.project().tempo_bpm, 120.0);
    // Nothing to cancel is fine too.
    engine.apply(Command::CancelPreview).unwrap();
    assert!(!engine.snapshot().can_undo);
}

#[test]
fn a_rejected_preview_keeps_the_one_before() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(130.0))).unwrap();
    assert!(matches!(
        engine.apply(Command::Preview(tempo(5.0))),
        Err(CommandError::TempoOutOfRange { .. })
    ));
    assert_eq!(engine.project().tempo_bpm, 130.0);
}

#[test]
fn undo_during_a_drag_drops_the_preview_first() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Edit(tempo(128.0))).unwrap();
    engine.apply(Command::Preview(tempo(150.0))).unwrap();
    engine.apply(Command::Undo).unwrap();
    assert_eq!(engine.project().tempo_bpm, 120.0);
    engine.apply(Command::Redo).unwrap();
    assert_eq!(engine.project().tempo_bpm, 128.0);
}

#[test]
fn saving_during_a_drag_saves_the_song_without_the_preview() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(150.0))).unwrap();
    assert_eq!(engine.to_file().tempo_bpm, 120.0);
    // The drag still sounds.
    assert_eq!(engine.project().tempo_bpm, 150.0);
}

#[test]
fn a_rejected_edit_still_ends_the_drag() {
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(130.0))).unwrap();
    assert!(engine.apply(Command::Edit(tempo(5.0))).is_err());
    assert_eq!(engine.project().tempo_bpm, 120.0);
    assert!(!engine.snapshot().can_undo);
}

#[test]
fn front_ends_see_the_song_without_the_preview_whatever_else_happens() {
    // A command during a drag (a note heard, play, locate) brings a new
    // snapshot: it must still show the song as it was before the drag,
    // or the drag's release finds nothing to change.
    let (mut engine, _processor) = engine();
    engine.apply(Command::Preview(tempo(150.0))).unwrap();
    engine.apply(Command::Play).unwrap();
    assert_eq!(engine.snapshot_without_preview().tempo_bpm, 120.0);
    assert!(engine.snapshot_without_preview().playing);
    // The drag still sounds, and lands as usual.
    assert_eq!(engine.project().tempo_bpm, 150.0);
    engine.apply(Command::Edit(tempo(150.0))).unwrap();
    assert_eq!(engine.snapshot_without_preview().tempo_bpm, 150.0);
    assert!(engine.snapshot().can_undo);
}
