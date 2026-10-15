//! Motion signals: a kick pattern publishes a ducking signal that several
//! parameters link to, and every use stays in sync.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::{channel, filter, instrument};
use daw_engine::{
    AudioProcessor, BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Effect,
    EffectKind, Engine, EngineConfig, Layer, LayerId, LoopRange, MidiClip, MidiNote, NotesSource,
    ParamId, Shape, Signal, SignalId, SignalSource, SignalUse, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const FILTER: DeviceId = DeviceId(1);
const DUCK: SignalId = SignalId(1);
const KICK: u8 = 36;

const VOLUME: ParamId = ParamId {
    device: DeviceRef::Channel,
    index: channel::VOLUME,
};
const SYNTH_CUTOFF: ParamId = ParamId {
    device: DeviceRef::Instrument,
    index: instrument::CUTOFF,
};
const FILTER_CUTOFF: ParamId = ParamId {
    device: DeviceRef::Effect(FILTER),
    index: filter::CUTOFF,
};

fn beats(n: f64) -> Ticks {
    Ticks::from_beats_f64(n)
}

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

fn kick_envelope(length: f64) -> NotesSource {
    NotesSource {
        lowest: KICK,
        highest: KICK,
        length: beats(length),
        // Fully ducked at the kick, back to full over the envelope.
        shape: Shape::ramp(0.0, 1.0),
    }
}

/// A chord note and a kick on every beat for eight beats, a filter
/// effect, a ducking signal from the kicks, and a Multiply layer linked to
/// it on the volume, the synth cutoff and the filter cutoff.
fn kick_ducking() -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    // A chord note on every beat (not one held note: a loop jump does not
    // restart notes that began before the loop), plus the kick.
    let notes = (0..8)
        .flat_map(|beat| {
            let start = beats(f64::from(beat));
            [
                MidiNote {
                    start,
                    length: beats(0.95),
                    pitch: 60,
                    velocity: 100,
                },
                MidiNote {
                    start,
                    length: beats(0.1),
                    pitch: KICK,
                    velocity: 100,
                },
            ]
        })
        .collect();
    apply(
        &mut engine,
        Edit::AddClip {
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: beats(8.0),
                notes,
            },
        },
    );
    apply(
        &mut engine,
        Edit::AddEffect {
            id: FILTER,
            index: 0,
            effect: Effect::new(EffectKind::Filter),
        },
    );
    apply(
        &mut engine,
        Edit::AddSignal {
            id: DUCK,
            signal: Signal {
                name: "Kick duck".to_owned(),
                color: 0,
                source: SignalSource::Notes(kick_envelope(0.5)),
            },
        },
    );
    for (id, param) in [(1, VOLUME), (2, SYNTH_CUTOFF), (3, FILTER_CUTOFF)] {
        let mut layer = Layer::new(BlendMode::Multiply);
        let block = Block::new(Ticks::ZERO, beats(8.0), Shape::constant(1.0)).linked(DUCK);
        layer.blocks.push((BlockId(id), block));
        apply(
            &mut engine,
            Edit::AddLayer {
                param,
                id: LayerId(id),
                index: 0,
                layer,
            },
        );
    }
    (engine, processor)
}

/// Each linked parameter's value and resting value at song position `beat`.
fn values_at(
    engine: &mut Engine,
    processor: &mut AudioProcessor,
    beat: f64,
) -> Vec<(ParamId, f64, f64)> {
    engine
        .apply(Command::Locate {
            position: beats(beat),
        })
        .unwrap();
    render(engine, processor, 1, &[1]);
    let snapshot = engine.snapshot();
    [VOLUME, SYNTH_CUTOFF, FILTER_CUTOFF]
        .into_iter()
        .map(|id| {
            let param = snapshot
                .devices
                .iter()
                .flat_map(|d| &d.params)
                .find(|p| p.id == id)
                .unwrap();
            (id, param.value, param.base)
        })
        .collect()
}

#[test]
fn every_linked_parameter_ducks_at_every_kick() {
    let (mut engine, mut processor) = kick_ducking();
    for kick in 0..8 {
        let kick = f64::from(kick);
        for (id, value, _) in values_at(&mut engine, &mut processor, kick) {
            assert_eq!(value, 0.0, "{id:?} at beat {kick}");
        }
        for (id, value, base) in values_at(&mut engine, &mut processor, kick + 0.25) {
            assert!((value - base * 0.5).abs() < 1e-9, "{id:?} half way back");
        }
        for (id, value, base) in values_at(&mut engine, &mut processor, kick + 0.75) {
            assert!((value - base).abs() < 1e-9, "{id:?} recovered");
        }
    }
}

#[test]
fn one_change_to_the_signal_moves_every_use() {
    let (mut engine, mut processor) = kick_ducking();
    apply(
        &mut engine,
        Edit::SetNotesSource {
            id: DUCK,
            source: kick_envelope(0.9),
        },
    );
    // With the longer envelope, three quarters of a beat after a kick every
    // parameter is still recovering.
    for (id, value, base) in values_at(&mut engine, &mut processor, 2.75) {
        assert!(value < base * 0.9, "{id:?}: {value} vs {base}");
    }
}

fn play(engine: &mut Engine, processor: &mut AudioProcessor, beats: f64) -> Rendered {
    engine.apply(Command::Play).unwrap();
    let frames = (beats * FRAMES_PER_BEAT as f64) as usize;
    render(engine, processor, frames, VARIED_BLOCKS)
}

/// The left channel's level between two times, in beats of playing.
fn level(rendered: &Rendered, from: f64, to: f64) -> f32 {
    let channels = usize::from(rendered.channels);
    let start = (from * FRAMES_PER_BEAT as f64) as usize;
    let end = (to * FRAMES_PER_BEAT as f64) as usize;
    let samples: Vec<f32> = rendered.samples[start * channels..end * channels]
        .iter()
        .step_by(channels)
        .copied()
        .collect();
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
fn the_sound_ducks_on_every_kick_including_after_a_loop_jump() {
    let (mut engine, mut processor) = kick_ducking();
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: beats(1.0),
            end: beats(3.0),
        })))
        .unwrap();
    // Plays beats 0..3, then jumps back to 1: the kick at song beat 1 is
    // heard again at three beats of playing, and ducks again.
    let rendered = play(&mut engine, &mut processor, 5.0);
    for time in [1.0, 2.0, 3.0, 4.0] {
        let ducked = level(&rendered, time, time + 0.05);
        let recovered = level(&rendered, time + 0.6, time + 0.95);
        assert!(
            ducked < recovered * 0.3,
            "at {time} beats: {ducked} vs {recovered}"
        );
    }
}

fn render_kick_ducking(blocks: &[usize]) -> Rendered {
    let (mut engine, mut processor) = kick_ducking();
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: beats(1.0),
            end: beats(5.0),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 200_000, blocks)
}

#[test]
fn linked_output_is_identical_for_any_block_size() {
    let reference = render_kick_ducking(&[480]);
    assert!(!reference.is_silent());
    for blocks in [&[1][..], VARIED_BLOCKS, &[4096], &[31, 33]] {
        assert!(
            render_kick_ducking(blocks) == reference,
            "blocks {blocks:?} changed the output"
        );
    }
}

#[test]
fn the_snapshot_lists_every_use_of_a_signal() {
    let (engine, _processor) = kick_ducking();
    let snapshot = engine.snapshot();
    let [signal] = snapshot.signals.as_slice() else {
        panic!("expected one signal");
    };
    assert_eq!((signal.id, signal.name.as_str()), (DUCK, "Kick duck"));
    let uses: Vec<SignalUse> = signal.used_by.clone();
    assert_eq!(
        uses,
        [
            SignalUse {
                param: SYNTH_CUTOFF,
                layer: LayerId(2),
                block: BlockId(2)
            },
            SignalUse {
                param: FILTER_CUTOFF,
                layer: LayerId(3),
                block: BlockId(3)
            },
            SignalUse {
                param: VOLUME,
                layer: LayerId(1),
                block: BlockId(1)
            },
        ],
        "in signal order: instrument, effects, channel"
    );
}

#[test]
fn undo_takes_a_signal_change_back_everywhere() {
    let (mut engine, mut processor) = kick_ducking();
    let before = values_at(&mut engine, &mut processor, 2.6);
    apply(
        &mut engine,
        Edit::SetNotesSource {
            id: DUCK,
            source: kick_envelope(0.9),
        },
    );
    assert_ne!(values_at(&mut engine, &mut processor, 2.6), before);
    engine.apply(Command::Undo).unwrap();
    assert_eq!(values_at(&mut engine, &mut processor, 2.6), before);
}
