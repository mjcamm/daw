//! Looping clips (`docs/design/clip-loops.md`): a looped clip sounds exactly
//! like its repeats laid out as plain clips.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::samples::SampleData;
use daw_engine::{
    AudioClip, ClipId, ClipLoop, Command, CommandError, Edit, Engine, EngineConfig, FIRST_NODE,
    InvalidClip, MidiClip, MidiNote, Node, NodeId, NodeKind, ProjectFile, SampleId, SampleRef,
    Ticks,
};
use daw_testkit::{VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const AUDIO: NodeId = NodeId(50);
const SAMPLE: SampleId = SampleId(1);
const FILE: &str = "audio/loop.wav";

fn beats(n: i64) -> Ticks {
    Ticks::from_beats(n)
}

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// Plays the song's first `beats` and returns the samples.
fn play(engine: &mut Engine, processor: &mut daw_engine::AudioProcessor, beats: usize) -> Vec<f32> {
    engine.apply(Command::Play).unwrap();
    render(engine, processor, beats * FRAMES_PER_BEAT, VARIED_BLOCKS).samples
}

/// Two notes inside a two-beat cycle.
fn cycle_notes() -> Vec<MidiNote> {
    vec![
        MidiNote {
            start: Ticks::ZERO,
            length: Ticks(480),
            pitch: 60,
            velocity: 100,
            bend: None,
        },
        MidiNote {
            start: beats(1),
            length: Ticks(480),
            pitch: 67,
            velocity: 90,
            bend: None,
        },
    ]
}

#[test]
fn a_looped_midi_clip_sounds_like_its_repeats_as_plain_clips() {
    let (mut looped, mut looped_audio) = Engine::new(EngineConfig::default());
    apply(
        &mut looped,
        Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: beats(1),
                length: beats(6),
                notes: cycle_notes(),
                looping: Some(ClipLoop::new(beats(2))),
            },
        },
    );
    let (mut plain, mut plain_audio) = Engine::new(EngineConfig::default());
    for k in 0..3 {
        apply(
            &mut plain,
            Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(1 + k),
                clip: MidiClip {
                    start: beats(1 + 2 * k as i64),
                    length: beats(2),
                    notes: cycle_notes(),
                    looping: None,
                },
            },
        );
    }
    let a = play(&mut looped, &mut looped_audio, 8);
    let b = play(&mut plain, &mut plain_audio, 8);
    assert!(a.iter().any(|s| *s != 0.0), "the loop sounds");
    assert_eq!(a, b);
}

/// Something with hits and tones.
fn music(frames: usize) -> SampleData {
    let left: Vec<f32> = (0..frames)
        .map(|i| (i as f32 * 0.031).sin() * 0.3 + if i % 12_000 < 300 { 0.4 } else { 0.0 })
        .collect();
    SampleData::mono(48_000, left)
}

/// An engine with an audio node (only it sounds) and the sample loaded.
fn audio_engine() -> (Engine, daw_engine::AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::Batch(vec![
            Edit::AddNode {
                id: AUDIO,
                parent: NodeId::ROOT,
                index: 0,
                node: Node::new("Loop", 0).with_kind(NodeKind::Audio),
            },
            Edit::AddSample {
                id: SAMPLE,
                sample: SampleRef {
                    name: "Loop".to_owned(),
                    file: FILE.to_owned(),
                },
            },
            Edit::SetMute {
                node: FIRST_NODE,
                muted: true,
            },
        ]),
    );
    engine.insert_sample_data(SAMPLE, FILE, music(400_000));
    (engine, processor)
}

#[test]
fn a_looped_audio_clip_sounds_like_its_pieces_as_plain_clips() {
    // A beat into a two-beat cycle (as after a cut), over five beats.
    let clip = AudioClip {
        start: beats(1),
        length: beats(5),
        offset: 0.5,
        fade_in: Ticks(240),
        fade_out: Ticks(240),
        looping: Some(ClipLoop::new(beats(2)).shifted(beats(1))),
        ..AudioClip::whole_file(SAMPLE, beats(1), 1.0, 120.0)
    };
    let (mut looped, mut looped_audio) = audio_engine();
    apply(
        &mut looped,
        Edit::AddAudioClip {
            node: AUDIO,
            id: ClipId(1),
            clip: clip.clone(),
        },
    );
    let (mut plain, mut plain_audio) = audio_engine();
    let pieces = clip.pieces(120.0);
    assert_eq!(pieces.len(), 3);
    for (k, piece) in pieces.into_iter().enumerate() {
        apply(
            &mut plain,
            Edit::AddAudioClip {
                node: AUDIO,
                id: ClipId(1 + k as u64),
                clip: piece,
            },
        );
    }
    let a = play(&mut looped, &mut looped_audio, 8);
    let b = play(&mut plain, &mut plain_audio, 8);
    assert!(a.iter().any(|s| *s != 0.0), "the loop sounds");
    assert_eq!(a, b);
}

#[test]
fn a_loop_with_no_cycle_or_a_phase_outside_it_is_refused() {
    let (mut engine, _audio) = Engine::new(EngineConfig::default());
    for looping in [
        ClipLoop {
            cycle: Ticks::ZERO,
            phase: Ticks::ZERO,
        },
        ClipLoop {
            cycle: beats(2),
            phase: beats(2),
        },
        ClipLoop {
            cycle: beats(2),
            phase: Ticks(-1),
        },
    ] {
        let result = engine.apply(Command::Edit(Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: beats(4),
                notes: Vec::new(),
                looping: Some(looping),
            },
        }));
        assert_eq!(
            result,
            Err(CommandError::InvalidClip {
                reason: InvalidClip::Loop
            }),
            "{looping:?}"
        );
    }
}

#[test]
fn a_looped_clip_is_saved_and_loaded() {
    let (mut engine, _audio) = Engine::new(EngineConfig::default());
    let clip = MidiClip {
        start: beats(4),
        length: beats(6),
        notes: cycle_notes(),
        looping: Some(ClipLoop::new(beats(2)).shifted(beats(1))),
    };
    apply(
        &mut engine,
        Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(9),
            clip: clip.clone(),
        },
    );
    let json = engine.to_file().to_json();
    let (mut loaded, _audio) = Engine::new(EngineConfig::default());
    loaded.load(ProjectFile::from_json(&json).unwrap()).unwrap();
    let snapshot = loaded.snapshot();
    let node = snapshot.node(FIRST_NODE).unwrap();
    assert_eq!(
        node.clips
            .iter()
            .find(|(id, _)| *id == ClipId(9))
            .unwrap()
            .1,
        clip
    );
}
