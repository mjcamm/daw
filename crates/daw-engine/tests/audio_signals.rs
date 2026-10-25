//! Audio signals: an audio node's loudness, followed before its effects as
//! the song plays, moving parameters anywhere (`docs/design/signals.md`).

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::channel;
use daw_engine::samples::SampleData;
use daw_engine::{
    AudioClip, AudioProcessor, AudioSource, BlendMode, Block, BlockId, ClipId, Command,
    CommandError, DeviceRef, Edit, Engine, EngineConfig, FIRST_NODE, InvalidSignal, Layer, LayerId,
    Node, NodeId, NodeKind, ParamId, SampleId, SampleRef, Shape, Signal, SignalId, SignalSource,
    Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
/// The followed node: full scale through beat 1, silent otherwise.
const DRUMS: NodeId = NodeId(50);
/// A steady sound whose volume listens.
const PAD: NodeId = NodeId(51);
const HIT: SampleId = SampleId(1);
const TONE: SampleId = SampleId(2);
const SIGNAL: SignalId = SignalId(1);
/// How far into a change the duck has surely settled: a control period to
/// be read, a period for the gain to ramp, and some room.
const SETTLE: usize = 200;

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

fn volume(node: NodeId) -> ParamId {
    ParamId {
        node,
        device: DeviceRef::Channel,
        index: channel::VOLUME,
    }
}

fn steady(level: f32) -> SampleData {
    SampleData::mono(48_000, vec![level; 4 * FRAMES_PER_BEAT])
}

fn audio_node(engine: &mut Engine, id: NodeId, parent: NodeId, name: &str) {
    apply(
        engine,
        Edit::AddNode {
            id,
            parent,
            index: 0,
            node: Node::new(name, 0).with_kind(NodeKind::Audio),
        },
    );
}

/// A clip of `sample` on `node` from beat `start`, `seconds` long (a beat
/// is half a second).
fn clip(engine: &mut Engine, node: NodeId, id: u64, sample: SampleId, start: i64, seconds: f64) {
    apply(
        engine,
        Edit::AddAudioClip {
            node,
            id: ClipId(id),
            clip: AudioClip::whole_file(sample, Ticks::from_beats(start), seconds, 120.0),
        },
    );
}

fn samples(engine: &mut Engine) {
    for (id, file, level) in [(HIT, "hit.wav", 1.0), (TONE, "tone.wav", 0.25)] {
        apply(
            engine,
            Edit::AddSample {
                id,
                sample: SampleRef {
                    name: file.to_owned(),
                    file: file.to_owned(),
                },
            },
        );
        engine.insert_sample_data(id, file, steady(level));
    }
}

/// Follows `node` with no attack or release, so the signal is at the top
/// exactly while it is at full scale.
fn publish(engine: &mut Engine, node: NodeId) {
    apply(
        engine,
        Edit::AddSignal {
            id: SIGNAL,
            signal: Signal {
                name: "Drums".to_owned(),
                color: 0,
                source: SignalSource::Audio(AudioSource {
                    attack_ms: 0.0,
                    release_ms: 0.0,
                    ..AudioSource::new(node)
                }),
            },
            listeners: Vec::new(),
        },
    );
}

/// A Multiply layer on `param` listening through an inverted window over
/// four beats: a full-scale source silences it.
fn duck(engine: &mut Engine, param: ParamId, id: u64) {
    let mut layer = Layer::new(BlendMode::Multiply).listening(SIGNAL);
    let mut window = Block::new(Ticks::ZERO, Ticks::from_beats(4), Shape::constant(0.5));
    window.invert = true;
    layer.blocks.push((BlockId(id), window));
    apply(
        engine,
        Edit::AddLayer {
            param,
            id: LayerId(id),
            index: 0,
            layer,
        },
    );
}

/// The pad (listed first, so it comes before the drums in the tree) and
/// the drums, muted: only the pad is heard, ducked by the drums' hit.
fn pad_ducked_by_drums() -> (Engine, AudioProcessor) {
    pad_ducked_by_drums_with(EngineConfig::default())
}

fn pad_ducked_by_drums_with(config: EngineConfig) -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(config);
    apply(
        &mut engine,
        Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        },
    );
    samples(&mut engine);
    audio_node(&mut engine, DRUMS, NodeId::ROOT, "Drums");
    audio_node(&mut engine, PAD, NodeId::ROOT, "Pad");
    clip(&mut engine, DRUMS, 1, HIT, 1, 0.5);
    clip(&mut engine, PAD, 2, TONE, 0, 2.0);
    apply(
        &mut engine,
        Edit::SetMute {
            node: DRUMS,
            muted: true,
        },
    );
    publish(&mut engine, DRUMS);
    duck(&mut engine, volume(PAD), 10);
    (engine, processor)
}

fn left(rendered: &Rendered, frame: usize) -> f32 {
    rendered.samples[frame * usize::from(rendered.channels)]
}

fn play(engine: &mut Engine, processor: &mut AudioProcessor, blocks: &[usize]) -> Rendered {
    engine.apply(Command::Play).unwrap();
    render(engine, processor, 3 * FRAMES_PER_BEAT, blocks)
}

#[test]
fn a_loud_source_ducks_a_listener_and_silence_lets_it_go() {
    let (mut engine, mut processor) = pad_ducked_by_drums();
    let rendered = play(&mut engine, &mut processor, VARIED_BLOCKS);
    let before = left(&rendered, FRAMES_PER_BEAT / 2);
    assert!(before > 0.1, "the pad plays before the hit: {before}");
    for frame in [FRAMES_PER_BEAT + SETTLE, FRAMES_PER_BEAT * 3 / 2] {
        assert_eq!(left(&rendered, frame), 0.0, "ducked at frame {frame}");
    }
    let after = left(&rendered, 2 * FRAMES_PER_BEAT + SETTLE);
    assert_eq!(after, before, "back once the hit ends");
}

#[test]
fn the_sound_is_the_same_for_any_block_size_and_any_threads() {
    let reference = {
        let (mut engine, mut processor) = pad_ducked_by_drums();
        play(&mut engine, &mut processor, &[512])
    };
    let (mut engine, mut processor) = pad_ducked_by_drums();
    assert_eq!(
        play(&mut engine, &mut processor, VARIED_BLOCKS).samples,
        reference.samples,
        "varied blocks"
    );
    for threads in [2, 4] {
        let (mut engine, mut processor) = pad_ducked_by_drums_with(EngineConfig {
            threads,
            ..EngineConfig::default()
        });
        assert_eq!(
            play(&mut engine, &mut processor, VARIED_BLOCKS).samples,
            reference.samples,
            "{threads} threads"
        );
    }
}

#[test]
fn a_node_can_listen_to_its_own_audio_signal() {
    // Followed before its own fader, so turning itself down never feeds
    // back: it is silent exactly while it is loud.
    let (mut engine, mut processor) = pad_ducked_by_drums();
    apply(
        &mut engine,
        Edit::SetMute {
            node: DRUMS,
            muted: false,
        },
    );
    apply(
        &mut engine,
        Edit::SetMute {
            node: PAD,
            muted: true,
        },
    );
    duck(&mut engine, volume(DRUMS), 11);
    let rendered = play(&mut engine, &mut processor, VARIED_BLOCKS);
    assert_eq!(left(&rendered, FRAMES_PER_BEAT * 3 / 2), 0.0);
}

#[test]
fn a_child_cannot_listen_to_its_parent_but_a_parent_can_listen_to_its_child() {
    // The group carries the pad's sound, so the pad can't follow it: the
    // layer hears nothing and the pad plays on.
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        },
    );
    samples(&mut engine);
    let group = NodeId(60);
    audio_node(&mut engine, group, NodeId::ROOT, "Group");
    audio_node(&mut engine, PAD, group, "Pad");
    clip(&mut engine, PAD, 2, TONE, 0, 2.0);
    publish(&mut engine, group);
    duck(&mut engine, volume(PAD), 10);
    let rendered = play(&mut engine, &mut processor, VARIED_BLOCKS);
    assert!(
        left(&rendered, FRAMES_PER_BEAT / 2) > 0.1,
        "the pad is not ducked by its own group"
    );

    // The other way round works: the group follows the pad, and the
    // group's volume listens.
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::SetMute {
            node: FIRST_NODE,
            muted: true,
        },
    );
    samples(&mut engine);
    audio_node(&mut engine, group, NodeId::ROOT, "Group");
    audio_node(&mut engine, DRUMS, group, "Drums");
    clip(&mut engine, DRUMS, 1, HIT, 1, 0.5);
    publish(&mut engine, DRUMS);
    duck(&mut engine, volume(group), 10);
    let rendered = play(&mut engine, &mut processor, VARIED_BLOCKS);
    assert_eq!(left(&rendered, FRAMES_PER_BEAT * 3 / 2), 0.0);
}

#[test]
fn a_midi_node_or_the_song_cannot_publish_an_audio_signal() {
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    for node in [FIRST_NODE, NodeId::ROOT] {
        let result = engine.apply(Command::Edit(Edit::AddSignal {
            id: SIGNAL,
            signal: Signal {
                name: "Nope".to_owned(),
                color: 0,
                source: SignalSource::Audio(AudioSource::new(node)),
            },
            listeners: Vec::new(),
        }));
        assert_eq!(
            result.unwrap_err(),
            CommandError::InvalidSignal {
                id: SIGNAL,
                reason: InvalidSignal::NotAudioNode
            },
            "{node:?}"
        );
    }
}

#[test]
fn the_follower_settings_change_in_one_undoable_edit() {
    let (mut engine, _processor) = pad_ducked_by_drums();
    let slower = AudioSource {
        attack_ms: 20.0,
        ..AudioSource::new(DRUMS)
    };
    apply(
        &mut engine,
        Edit::SetAudioSource {
            id: SIGNAL,
            source: slower.clone(),
        },
    );
    let source = |engine: &Engine| match &engine.snapshot().signals[0].source {
        SignalSource::Audio(source) => source.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(source(&engine), slower);
    engine.apply(Command::Undo).unwrap();
    assert_eq!(source(&engine).attack_ms, 0.0);
    let bad = AudioSource {
        floor_db: 0.0,
        ceiling_db: -10.0,
        ..AudioSource::new(DRUMS)
    };
    assert!(
        engine
            .apply(Command::Edit(Edit::SetAudioSource {
                id: SIGNAL,
                source: bad
            }))
            .is_err()
    );
}

#[test]
fn a_reopened_song_follows_and_ducks_the_same() {
    let (mut original, mut processor) = pad_ducked_by_drums();
    let json = original.to_file().to_json();
    let heard = play(&mut original, &mut processor, &[512]);

    let (mut reopened, mut processor) = Engine::new(EngineConfig::default());
    reopened
        .load(daw_engine::ProjectFile::from_json(&json).unwrap())
        .unwrap();
    for (id, file, level) in [(HIT, "hit.wav", 1.0), (TONE, "tone.wav", 0.25)] {
        reopened.insert_sample_data(id, file, steady(level));
    }
    assert_eq!(
        play(&mut reopened, &mut processor, &[512]).samples,
        heard.samples
    );
}

#[test]
fn a_render_records_each_followed_nodes_loudness_per_control_period() {
    let (mut engine, mut processor) = pad_ducked_by_drums();
    engine.apply(Command::Play).unwrap();
    let mut peaks = Vec::new();
    let mut out = vec![0.0; 1024 * 2];
    for _ in 0..(3 * FRAMES_PER_BEAT / 1024) {
        engine.maintain();
        processor.process(&mut out);
        for (node, chunk) in processor.followed_peaks() {
            assert_eq!(node, DRUMS, "only the followed node");
            peaks.extend(chunk);
        }
    }
    let period = daw_engine::CONTROL_FRAMES as usize;
    assert_eq!(peaks.len(), 3 * FRAMES_PER_BEAT / 1024 * 1024 / period);
    // Point n closes the period that ends at frame 32 n.
    let at = |frame: usize| peaks[frame / period];
    assert_eq!(at(FRAMES_PER_BEAT), 0.0, "silent up to the hit");
    assert_eq!(at(FRAMES_PER_BEAT + period), 1.0, "the hit's first period");
    assert_eq!(at(2 * FRAMES_PER_BEAT), 1.0, "its last");
    assert_eq!(at(2 * FRAMES_PER_BEAT + period), 0.0, "silent after");
}
