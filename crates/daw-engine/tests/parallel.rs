//! Rendering on several threads gives exactly the sound of rendering on one.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::{
    ClipId, Command, DeviceId, Edit, Effect, EffectKind, Engine, EngineConfig, LoopRange, MidiClip,
    MidiNote, Node, NodeId, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// A song with folders three deep, twelve players and effects, looping
/// across a tempo change.
fn render_song(threads: usize, blocks: &[usize]) -> Rendered {
    let (mut engine, mut processor) = Engine::new(EngineConfig {
        threads,
        ..EngineConfig::default()
    });
    let mut next_device = 100;
    let mut device = || {
        next_device += 1;
        DeviceId(next_device)
    };
    for folder in 0..3u64 {
        let folder_id = NodeId(10 + folder);
        apply(
            &mut engine,
            Edit::AddNode {
                id: folder_id,
                parent: NodeId::ROOT,
                index: 0,
                node: Node::new("Folder", 0),
            },
        );
        let inner = NodeId(20 + folder);
        apply(
            &mut engine,
            Edit::AddNode {
                id: inner,
                parent: folder_id,
                index: 0,
                node: Node::new("Inner", 1),
            },
        );
        apply(
            &mut engine,
            Edit::AddEffect {
                node: inner,
                id: device(),
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            },
        );
        for player in 0..4u64 {
            let id = NodeId(100 + folder * 10 + player);
            let parent = if player % 2 == 0 { folder_id } else { inner };
            apply(
                &mut engine,
                Edit::AddNode {
                    id,
                    parent,
                    index: 0,
                    node: Node::new("Player", 2).with_instrument(device()),
                },
            );
            let notes = (0..16)
                .map(|step| MidiNote {
                    start: Ticks::from_beats_f64(f64::from(step) * 0.25 + player as f64 * 0.03),
                    length: Ticks::from_beats_f64(0.2),
                    pitch: 40 + (step * 5 + player as i32 * 3) as u8 % 40,
                    velocity: 90,
                })
                .collect();
            apply(
                &mut engine,
                Edit::AddClip {
                    node: id,
                    id: ClipId(id.0),
                    clip: MidiClip {
                        start: Ticks::ZERO,
                        length: Ticks::from_beats(4),
                        notes,
                    },
                },
            );
        }
    }
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: Ticks::from_beats(1),
            end: Ticks::from_beats_f64(3.5),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    let mut first = render(&mut engine, &mut processor, 60_000, blocks);
    apply(&mut engine, Edit::SetTempo { bpm: 133.0 });
    let second = render(&mut engine, &mut processor, 60_000, blocks);
    first.samples.extend(second.samples);
    first
}

#[test]
fn any_number_of_threads_renders_the_same_sound() {
    let reference = render_song(1, &[512]);
    assert!(!reference.is_silent());
    for threads in [2, 4, 8] {
        for blocks in [&[512][..], VARIED_BLOCKS, &[64]] {
            assert!(
                render_song(threads, blocks) == reference,
                "{threads} threads with blocks {blocks:?} changed the sound"
            );
        }
    }
}

#[test]
fn the_processor_reports_its_threads() {
    let (_, processor) = Engine::new(EngineConfig {
        threads: 4,
        ..EngineConfig::default()
    });
    assert_eq!(processor.threads(), 4);
    assert!(daw_engine::auto_threads() >= 1);
}

#[test]
fn reconnecting_keeps_the_song_playing_from_where_it_was() {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::AddClip {
            node: daw_engine::FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_beats(8),
                notes: vec![MidiNote {
                    start: Ticks::from_beats(2),
                    length: Ticks::from_beats(1),
                    pitch: 60,
                    velocity: 100,
                }],
            },
        },
    );
    engine.apply(Command::Play).unwrap();
    // One beat at 120 BPM and 48 kHz, then on at 96 kHz with four threads.
    assert!(render(&mut engine, &mut processor, 24_000, VARIED_BLOCKS).is_silent());
    let mut processor = engine.reconnect(EngineConfig {
        sample_rate: 96_000,
        channels: 2,
        threads: 4,
    });
    assert_eq!(processor.threads(), 4);
    assert!((engine.position_beats() - 1.0).abs() < 1e-9);
    // The note at beat 2 is one beat (48 000 frames at 96 kHz) away.
    let rendered = render(&mut engine, &mut processor, 60_000, VARIED_BLOCKS);
    let first = rendered.samples.iter().position(|&s| s != 0.0).unwrap() / 2;
    assert!((48_000..=48_001).contains(&first), "note at frame {first}");
    let undo = engine.apply(Command::Undo);
    assert!(undo.is_ok(), "undo history survives");
}
