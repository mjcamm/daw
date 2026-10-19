//! Audio clips on audio nodes: placing, editing, and playing them stretched
//! to the song's tempo (`docs/design/audio-clips.md`).

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::samples::SampleData;
use daw_engine::{
    AudioClip, AudioProcessor, ClipId, Command, CommandError, Edit, Engine, EngineConfig,
    FIRST_NODE, InvalidClip, Node, NodeId, NodeKind, SampleId, SampleRef, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const AUDIO: NodeId = NodeId(50);
const LOOP: SampleId = SampleId(1);
const LOOP_FILE: &str = "audio/loop.wav";
const CLIP: ClipId = ClipId(7);

/// Recognisable audio: frame `i` holds `i / 1e6`, left; right is negated.
fn counting(frames: usize) -> SampleData {
    let left: Vec<f32> = (0..frames).map(|i| i as f32 / 1e6).collect();
    let right = left.iter().map(|s| -s).collect();
    SampleData::stereo(48_000, left, right)
}

/// Something with hits and tones, for the stretcher to work on.
fn music(frames: usize) -> SampleData {
    let left: Vec<f32> = (0..frames)
        .map(|i| {
            let tone = (i as f32 * 0.031).sin() * 0.3;
            let hit = if i % 12_000 < 300 { 0.4 } else { 0.0 };
            tone + hit
        })
        .collect();
    SampleData::mono(48_000, left)
}

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// An engine with an audio node and the loop sample in its table and pool.
fn engine_with(config: EngineConfig, data: SampleData) -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(config);
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
                id: LOOP,
                sample: SampleRef {
                    name: "Loop".to_owned(),
                    file: LOOP_FILE.to_owned(),
                },
            },
        ]),
    );
    // Only the audio node sounds.
    apply(
        &mut engine,
        Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        },
    );
    engine.insert_sample_data(LOOP, LOOP_FILE, data);
    (engine, processor)
}

/// Two beats from beat 1, playing the file from 0.5 s in.
fn clip() -> AudioClip {
    AudioClip {
        offset: 0.5,
        ..AudioClip::whole_file(LOOP, Ticks::from_beats(1), 1.0, 120.0)
    }
}

fn frame(rendered: &Rendered, frame: usize) -> (f32, f32) {
    let i = frame * usize::from(rendered.channels);
    (rendered.samples[i], rendered.samples[i + 1])
}

#[test]
fn a_clip_plays_its_window_of_the_file_from_its_exact_frame() {
    let (mut engine, mut processor) = engine_with(EngineConfig::default(), counting(200_000));
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: clip(),
        },
    );
    engine.apply(Command::Play).unwrap();
    let rendered = render(
        &mut engine,
        &mut processor,
        4 * FRAMES_PER_BEAT,
        VARIED_BLOCKS,
    );
    let start = FRAMES_PER_BEAT;
    assert_eq!(frame(&rendered, start - 1), (0.0, 0.0), "silent before");
    // The file from 0.5 s (frame 24000), both sides, at unity gain.
    for i in [0, 1, 1000, 47_999] {
        let want = (24_000 + i) as f32 / 1e6;
        assert_eq!(frame(&rendered, start + i), (want, -want), "frame {i}");
    }
    // Its two beats end at beat 3, where the window closes.
    assert_eq!(
        frame(&rendered, 3 * FRAMES_PER_BEAT),
        (0.0, 0.0),
        "silent after"
    );
}

#[test]
fn a_stretched_clip_sounds_the_same_for_any_block_size_and_thread_count() {
    let render_with = |threads: usize, blocks: &[usize]| {
        let config = EngineConfig {
            threads,
            ..EngineConfig::default()
        };
        let (mut engine, mut processor) = engine_with(config, music(400_000));
        // A 100 BPM loop in a 120 BPM song: played faster.
        apply(
            &mut engine,
            Edit::AddAudioClip {
                node: AUDIO,
                id: CLIP,
                clip: AudioClip {
                    bpm: 100.0,
                    ..clip()
                },
            },
        );
        engine.apply(Command::Play).unwrap();
        render(&mut engine, &mut processor, 4 * FRAMES_PER_BEAT, blocks)
    };
    let reference = render_with(1, &[512]);
    assert!(!reference.is_silent());
    for (threads, blocks) in [(1, VARIED_BLOCKS), (1, &[1][..]), (4, VARIED_BLOCKS)] {
        assert!(
            render_with(threads, blocks) == reference,
            "{threads} threads, blocks {blocks:?} changed the output"
        );
    }
}

#[test]
fn a_file_at_another_rate_is_converted_and_keeps_its_length_in_time() {
    let data = SampleData::mono(24_000, vec![0.25; 24_000]);
    let (mut engine, mut processor) = engine_with(EngineConfig::default(), data);
    // The whole one-second file from the song start, two beats long.
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: AudioClip {
                length: Ticks::from_beats(4),
                ..AudioClip::whole_file(LOOP, Ticks::ZERO, 1.0, 120.0)
            },
        },
    );
    engine.apply(Command::Play).unwrap();
    let rendered = render(
        &mut engine,
        &mut processor,
        3 * FRAMES_PER_BEAT,
        VARIED_BLOCKS,
    );
    assert!(
        (frame(&rendered, 24_000).0 - 0.25).abs() < 0.01,
        "half way: playing"
    );
    assert!(
        frame(&rendered, 47_000).0.abs() > 0.2,
        "still playing near its end"
    );
    assert!(
        frame(&rendered, 49_000).0.abs() < 0.01,
        "a second of file is a second"
    );
}

#[test]
fn audio_clips_go_only_on_audio_nodes_with_a_known_sample() {
    let (mut engine, _processor) = engine_with(EngineConfig::default(), counting(10));
    let add = |node, clip| {
        Command::Edit(Edit::AddAudioClip {
            node,
            id: CLIP,
            clip,
        })
    };
    assert_eq!(
        engine.apply(add(FIRST_NODE, clip())),
        Err(CommandError::NotAudio { node: FIRST_NODE })
    );
    assert_eq!(
        engine.apply(add(NodeId::ROOT, clip())),
        Err(CommandError::NotAudio { node: NodeId::ROOT })
    );
    assert_eq!(
        engine.apply(add(
            AUDIO,
            AudioClip {
                sample: SampleId(9),
                ..clip()
            }
        )),
        Err(CommandError::UnknownSample { id: SampleId(9) })
    );
    let invalid = [
        (
            AudioClip {
                offset: -1.0,
                ..clip()
            },
            InvalidClip::Offset,
        ),
        (AudioClip { bpm: 0.0, ..clip() }, InvalidClip::Bpm),
        (
            AudioClip {
                gain_db: 99.0,
                ..clip()
            },
            InvalidClip::Gain,
        ),
        (
            AudioClip {
                fade_in: Ticks::from_beats(2),
                fade_out: Ticks::from_beats(1),
                ..clip()
            },
            InvalidClip::Fades,
        ),
        (
            AudioClip {
                length: Ticks::ZERO,
                ..clip()
            },
            InvalidClip::EmptyLength,
        ),
    ];
    for (clip, reason) in invalid {
        assert_eq!(
            engine.apply(add(AUDIO, clip)),
            Err(CommandError::InvalidClip { reason })
        );
    }
    // MIDI clips cannot use the audio clip's id, nor go on the audio node.
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: clip(),
        },
    );
    assert_eq!(
        engine.apply(add(AUDIO, clip())),
        Err(CommandError::DuplicateClip { id: CLIP })
    );
}

#[test]
fn audio_clips_on_a_node_never_overlap() {
    let (mut engine, _processor) = engine_with(EngineConfig::default(), counting(10));
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: clip(),
        },
    );
    // `clip()` covers beats 1..3: one at beat 3 touches it, one at beat 2
    // would overlap it.
    let at = |beat: i64| AudioClip {
        start: Ticks::from_beats(beat),
        ..clip()
    };
    let add = |beat: i64| {
        Command::Edit(Edit::AddAudioClip {
            node: AUDIO,
            id: ClipId(8),
            clip: at(beat),
        })
    };
    assert_eq!(
        engine.apply(add(2)),
        Err(CommandError::StackedClips {
            node: AUDIO,
            clip: CLIP,
            over: ClipId(8),
        })
    );
    engine.apply(add(3)).unwrap();
}

#[test]
fn clip_edits_undo_and_a_used_sample_stays() {
    let (mut engine, _processor) = engine_with(EngineConfig::default(), counting(10));
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: clip(),
        },
    );
    assert_eq!(
        engine.apply(Command::Edit(Edit::RemoveSample { id: LOOP })),
        Err(CommandError::SampleInUse { id: LOOP })
    );
    let moved = AudioClip {
        start: Ticks::from_beats(8),
        ..clip()
    };
    apply(
        &mut engine,
        Edit::SetAudioClip {
            id: CLIP,
            node: AUDIO,
            clip: moved.clone(),
        },
    );
    let clips = |engine: &Engine| engine.snapshot().node(AUDIO).unwrap().audio_clips.clone();
    assert_eq!(clips(&engine), [(CLIP, moved)]);
    engine.apply(Command::Undo).unwrap();
    assert_eq!(clips(&engine), [(CLIP, clip())]);
    apply(&mut engine, Edit::RemoveClip { id: CLIP });
    assert!(clips(&engine).is_empty());
    engine.apply(Command::Undo).unwrap();
    assert_eq!(clips(&engine), [(CLIP, clip())]);
}

#[test]
fn the_snapshot_carries_each_samples_waveform_overview() {
    let (engine, _processor) = engine_with(EngineConfig::default(), counting(100_000));
    let snapshot = engine.snapshot();
    let peaks = snapshot.samples[0].peaks.as_ref().unwrap();
    assert_eq!(
        (peaks.frames, peaks.sample_rate, peaks.channels()),
        (100_000, 48_000, 2)
    );
    let range = peaks.range(0, 0.0, 100_000.0);
    assert!((range.max - 0.099_999).abs() < 1e-6 && range.min == 0.0);
}

#[test]
fn audio_clips_are_saved_and_loaded() {
    let (mut engine, _processor) = engine_with(EngineConfig::default(), counting(10));
    let saved = AudioClip {
        follow_tempo: false,
        gain_db: -3.0,
        fade_out: Ticks(240),
        ..clip()
    };
    apply(
        &mut engine,
        Edit::AddAudioClip {
            node: AUDIO,
            id: CLIP,
            clip: saved.clone(),
        },
    );
    let json = engine.to_file().to_json();
    let (mut loaded, _processor) = Engine::new(EngineConfig::default());
    loaded
        .load(daw_engine::ProjectFile::from_json(&json).unwrap())
        .unwrap();
    assert_eq!(
        loaded.snapshot().node(AUDIO).unwrap().audio_clips,
        [(CLIP, saved)]
    );
}
