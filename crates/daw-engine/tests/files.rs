//! Saving and opening projects through the engine.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::{channel, filter};
use daw_engine::{
    AudioProcessor, BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Effect,
    EffectKind, Engine, EngineConfig, Layer, LayerId, LoopRange, MidiClip, MidiNote, ParamId,
    ProjectFile, Shape, Ticks,
};
use daw_testkit::{VARIED_BLOCKS, render};

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// An engine with notes, an effect, layers and a loop.
fn busy_engine() -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::RenameSong {
            name: "Saved Song".to_owned(),
        },
    );
    apply(
        &mut engine,
        Edit::AddClip {
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_bars(2),
                notes: (0..16)
                    .map(|i| MidiNote {
                        start: Ticks::from_beats_f64(f64::from(i) * 0.5),
                        length: Ticks(400),
                        pitch: 48 + (i * 5 % 24) as u8,
                        velocity: 100,
                    })
                    .collect(),
            },
        },
    );
    apply(
        &mut engine,
        Edit::AddEffect {
            id: DeviceId(1),
            index: 0,
            effect: Effect::new(EffectKind::Filter),
        },
    );
    let mut sweep = Layer::new(BlendMode::Replace);
    sweep.blocks.push((
        BlockId(1),
        Block::new(Ticks::ZERO, Ticks::from_bars(2), Shape::ramp(0.3, 0.9)),
    ));
    apply(
        &mut engine,
        Edit::AddLayer {
            param: ParamId {
                device: DeviceRef::Effect(DeviceId(1)),
                index: filter::CUTOFF,
            },
            id: LayerId(1),
            index: 0,
            layer: sweep,
        },
    );
    apply(
        &mut engine,
        Edit::SetParam {
            param: ParamId {
                device: DeviceRef::Channel,
                index: channel::PAN,
            },
            value: 0.3,
        },
    );
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: Ticks::ZERO,
            end: Ticks::from_bars(2),
        })))
        .unwrap();
    (engine, processor)
}

fn play_and_render(engine: &mut Engine, processor: &mut AudioProcessor) -> Vec<f32> {
    engine.apply(Command::Play).unwrap();
    render(engine, processor, 250_000, VARIED_BLOCKS).samples
}

#[test]
fn a_reopened_project_sounds_identical() {
    let (mut original, mut original_processor) = busy_engine();
    let json = original.to_file().to_json();
    let expected = play_and_render(&mut original, &mut original_processor);

    let (mut reopened, mut reopened_processor) = Engine::new(EngineConfig::default());
    reopened
        .load(ProjectFile::from_json(&json).unwrap())
        .unwrap();
    let rendered = play_and_render(&mut reopened, &mut reopened_processor);

    assert!(!expected.iter().all(|&s| s == 0.0));
    assert!(rendered == expected);
}

#[test]
fn opening_a_project_resets_undo_and_stops() {
    let (source, _) = busy_engine();
    let file = source.to_file();

    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    apply(&mut engine, Edit::SetTempo { bpm: 90.0 });
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 1_000, &[100]);

    engine.load(file).unwrap();
    render(&mut engine, &mut processor, 100, &[100]);
    let snapshot = engine.snapshot();
    assert_eq!(snapshot.song_name, "Saved Song");
    assert!(!snapshot.playing);
    assert!(!snapshot.can_undo && !snapshot.can_redo);
    assert_eq!(snapshot.position_beats, 0.0);
    assert!(snapshot.loop_range.is_some());
}

#[test]
fn a_file_that_fails_to_open_changes_nothing() {
    let (mut engine, _processor) = busy_engine();
    let before = engine.snapshot();
    let mut broken = engine.to_file();
    broken.tempo_bpm = 0.0;
    assert!(engine.load(broken).is_err());
    assert_eq!(engine.snapshot(), before);
}

#[test]
fn only_saved_content_counts_as_a_change() {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    let start = engine.content_revision();
    engine.apply(Command::Play).unwrap();
    engine
        .apply(Command::Locate {
            position: Ticks::from_beats(2),
        })
        .unwrap();
    render(&mut engine, &mut processor, 100, &[100]);
    engine.apply(Command::Stop).unwrap();
    assert_eq!(engine.content_revision(), start, "transport is not content");

    apply(&mut engine, Edit::SetTempo { bpm: 100.0 });
    let edited = engine.content_revision();
    assert_ne!(edited, start);
    engine.apply(Command::Undo).unwrap();
    assert_ne!(engine.content_revision(), edited);
}
