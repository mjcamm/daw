//! Drives the engine headlessly through commands, as a front end would.

use daw_engine::{Command, CommandError, Edit, Engine, EngineConfig};
use daw_testkit::{VARIED_BLOCKS, assert_reference, render};

fn engine() -> (Engine, daw_engine::AudioProcessor) {
    Engine::new(EngineConfig::default())
}

fn set_tempo(bpm: f64) -> Command {
    Command::Edit(Edit::SetTempo { bpm })
}

#[test]
fn empty_engine_renders_silence() {
    let (mut engine, mut processor) = engine();
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 4800, VARIED_BLOCKS);
    assert!(rendered.is_silent());
    assert_reference!("empty-engine-silence", &rendered);
}

#[test]
fn transport_position_follows_play_and_stop() {
    let (mut engine, mut processor) = engine();
    render(&mut engine, &mut processor, 100, &[64]);
    assert_eq!(
        engine.snapshot().position_beats,
        0.0,
        "stopped engine moved"
    );

    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    // 120 BPM at 48 kHz is 24,000 frames per beat.
    assert_eq!(engine.snapshot().position_beats, 0.5);

    engine.apply(Command::Stop).unwrap();
    render(&mut engine, &mut processor, 12_000, VARIED_BLOCKS);
    let snapshot = engine.snapshot();
    assert!(!snapshot.playing);
    assert_eq!(snapshot.position_beats, 0.5);
}

#[test]
fn snapshots_reflect_edits_undo_and_redo() {
    let (mut engine, _processor) = engine();
    let initial = engine.snapshot();
    assert_eq!(initial.tempo_bpm, 120.0);
    assert!(!initial.can_undo && !initial.can_redo);

    engine.apply(set_tempo(128.0)).unwrap();
    let edited = engine.snapshot();
    assert_eq!(edited.tempo_bpm, 128.0);
    assert!(edited.can_undo);
    assert!(edited.revision > initial.revision);

    engine.apply(Command::Undo).unwrap();
    assert_eq!(engine.snapshot().tempo_bpm, 120.0);
    engine.apply(Command::Redo).unwrap();
    assert_eq!(engine.snapshot().tempo_bpm, 128.0);
}

#[test]
fn rejected_commands_change_nothing() {
    let (mut engine, _processor) = engine();
    let before = engine.snapshot();
    assert_eq!(
        engine.apply(Command::Undo),
        Err(CommandError::NothingToUndo)
    );
    assert!(matches!(
        engine.apply(set_tempo(5.0)),
        Err(CommandError::TempoOutOfRange { .. })
    ));
    assert_eq!(engine.snapshot(), before);
}

#[test]
fn many_edits_while_rendering_never_touch_the_allocator_on_the_audio_side() {
    // The realtime guard aborts this test if the audio side allocates or frees.
    let (mut engine, mut processor) = engine();
    engine.apply(Command::Play).unwrap();
    for i in 0..2000 {
        engine.apply(set_tempo(60.0 + f64::from(i % 100))).unwrap();
        if i % 3 == 0 {
            render(&mut engine, &mut processor, 7, &[3]);
        }
    }
    render(&mut engine, &mut processor, 64, &[64]);
    assert_eq!(engine.snapshot().tempo_bpm, 159.0);
}

#[test]
fn edits_queued_without_rendering_are_delivered_later() {
    let (mut engine, mut processor) = engine();
    // Far more commands than the queue holds, with no audio callback running.
    for i in 0..5000 {
        engine
            .apply(if i % 2 == 0 {
                Command::Play
            } else {
                Command::Stop
            })
            .unwrap();
    }
    engine.apply(Command::Play).unwrap();
    for _ in 0..10 {
        render(&mut engine, &mut processor, 10, &[10]);
    }
    let before = engine.snapshot().position_beats;
    render(&mut engine, &mut processor, 2_400, &[2_400]);
    let moved = engine.snapshot().position_beats - before;
    assert!((moved - 0.1).abs() < 1e-12, "{moved}");
}
