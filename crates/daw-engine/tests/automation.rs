//! Layered automation driving the instrument, an effect and the channel.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::devices::{FILTER_PARAMS, channel, filter, instrument};
use daw_engine::{
    AudioProcessor, BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Effect,
    EffectKind, Engine, EngineConfig, FIRST_INSTRUMENT, FIRST_NODE, Layer, LayerId, LoopRange,
    MidiClip, MidiNote, ParamId, Shape, ShapePoint, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, assert_reference, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const FILTER: DeviceId = DeviceId(1);

const VOLUME: ParamId = ParamId {
    node: FIRST_NODE,
    device: DeviceRef::Channel,
    index: channel::VOLUME,
};
const SYNTH_CUTOFF: ParamId = ParamId {
    node: FIRST_NODE,
    device: DeviceRef::Instrument(FIRST_INSTRUMENT),
    index: instrument::CUTOFF,
};
const FILTER_CUTOFF: ParamId = ParamId {
    node: FIRST_NODE,
    device: DeviceRef::Effect(FILTER),
    index: filter::CUTOFF,
};

fn beats(n: f64) -> Ticks {
    Ticks::from_beats_f64(n)
}

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// An engine playing one held note (middle C) for eight beats.
fn held_note() -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: beats(8.0),
                notes: vec![MidiNote {
                    start: Ticks::ZERO,
                    length: beats(8.0),
                    pitch: 60,
                    velocity: 100,
                    bend: None,
                }],
                looping: None,
            },
        },
    );
    (engine, processor)
}

fn add_filter(engine: &mut Engine) {
    apply(
        engine,
        Edit::AddEffect {
            node: FIRST_NODE,
            id: FILTER,
            index: 0,
            effect: Effect::new(EffectKind::Filter),
        },
    );
}

/// Adds a layer with one block, using the layer id for the block id too.
fn add_layer(engine: &mut Engine, param: ParamId, id: u64, mode: BlendMode, block: Block) {
    let mut layer = Layer::new(mode);
    layer.blocks.push((BlockId(id), block));
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

fn block(start: f64, length: f64, shape: Shape) -> Block {
    Block::new(beats(start), beats(length), shape)
}

fn play(engine: &mut Engine, processor: &mut AudioProcessor, beats: f64) -> Rendered {
    engine.apply(Command::Play).unwrap();
    let frames = (beats * FRAMES_PER_BEAT as f64) as usize;
    render(engine, processor, frames, VARIED_BLOCKS)
}

/// The left channel between two beat positions.
fn window(rendered: &Rendered, from: f64, to: f64) -> Vec<f32> {
    let channels = usize::from(rendered.channels);
    let start = (from * FRAMES_PER_BEAT as f64) as usize;
    let end = (to * FRAMES_PER_BEAT as f64) as usize;
    rendered.samples[start * channels..end * channels]
        .iter()
        .step_by(channels)
        .copied()
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Rough brightness: how much the signal changes sample to sample, relative
/// to its level. Higher means more high-frequency content.
fn brightness(samples: &[f32]) -> f32 {
    let diffs: Vec<f32> = samples.windows(2).map(|w| w[1] - w[0]).collect();
    rms(&diffs) / rms(samples)
}

#[test]
fn volume_follows_a_drawn_rise() {
    let (mut engine, mut processor) = held_note();
    // Multiply by a rising ramp: silent at the start, unchanged (the centre
    // line) at the end.
    add_layer(
        &mut engine,
        VOLUME,
        1,
        BlendMode::Multiply,
        block(0.0, 4.0, Shape::ramp(0.0, 0.5)),
    );
    let rendered = play(&mut engine, &mut processor, 4.0);
    let levels: Vec<f32> = (0..4)
        .map(|beat| rms(&window(&rendered, f64::from(beat), f64::from(beat) + 1.0)))
        .collect();
    assert!(
        levels.windows(2).all(|w| w[1] > w[0] * 1.2),
        "levels should rise beat by beat: {levels:?}"
    );
}

#[test]
fn gaps_snap_back_to_the_default_state() {
    let (mut engine, mut processor) = held_note();
    // Silence beats 1..2 only: multiply by zero.
    add_layer(
        &mut engine,
        VOLUME,
        1,
        BlendMode::Multiply,
        block(1.0, 1.0, Shape::constant(0.0)),
    );
    let rendered = play(&mut engine, &mut processor, 3.0);
    let before = rms(&window(&rendered, 0.5, 1.0));
    let during = rms(&window(&rendered, 1.01, 1.99));
    let after = rms(&window(&rendered, 2.01, 2.5));
    assert_eq!(during, 0.0);
    assert!(before > 0.01);
    assert!((after / before - 1.0).abs() < 0.05, "{before} vs {after}");
}

#[test]
fn a_layer_on_the_synth_cutoff_brightens_the_sound() {
    let (mut engine, mut processor) = held_note();
    add_layer(
        &mut engine,
        SYNTH_CUTOFF,
        1,
        BlendMode::Add,
        block(2.0, 2.0, Shape::constant(0.9)),
    );
    let rendered = play(&mut engine, &mut processor, 4.0);
    let dark = brightness(&window(&rendered, 1.0, 2.0));
    let bright = brightness(&window(&rendered, 2.5, 3.5));
    assert!(bright > dark * 2.0, "{dark} -> {bright}");
}

#[test]
fn a_layer_on_the_filter_effect_cuts_the_sound() {
    let (mut engine, mut processor) = held_note();
    add_filter(&mut engine);
    // Pull the filter cutoff down from fully open to about 80 Hz, below the
    // note.
    add_layer(
        &mut engine,
        FILTER_CUTOFF,
        1,
        BlendMode::Add,
        block(2.0, 2.0, Shape::constant(0.1)),
    );
    let rendered = play(&mut engine, &mut processor, 4.0);
    let open = rms(&window(&rendered, 1.0, 2.0));
    let closed = rms(&window(&rendered, 2.5, 3.5));
    assert!(closed < open * 0.2, "{open} -> {closed}");
}

#[test]
fn moving_the_filter_mode_knob_changes_the_sound() {
    let (mut engine, mut processor) = held_note();
    add_filter(&mut engine);
    let high_pass = FILTER_PARAMS[filter::MODE as usize]
        .scale
        .to_normalized(1.0);
    apply(
        &mut engine,
        Edit::SetParam {
            param: ParamId {
                node: FIRST_NODE,
                device: DeviceRef::Effect(FILTER),
                index: filter::MODE,
            },
            value: high_pass,
        },
    );
    apply(
        &mut engine,
        Edit::SetParam {
            param: FILTER_CUTOFF,
            value: 0.6,
        },
    );
    let (mut plain, mut plain_processor) = held_note();
    let filtered = play(&mut engine, &mut processor, 1.0);
    let unfiltered = play(&mut plain, &mut plain_processor, 1.0);
    let ratio = rms(&window(&filtered, 0.5, 1.0)) / rms(&window(&unfiltered, 0.5, 1.0));
    assert!(ratio < 0.5, "{ratio}");
}

#[test]
fn a_repeating_multiply_layer_ducks_every_beat() {
    let (mut engine, mut processor) = held_note();
    // From silence up to the centre line (no change), every beat.
    let mut duck = block(0.0, 4.0, Shape::ramp(0.0, 0.5));
    duck.cycle = Some(beats(1.0));
    add_layer(&mut engine, VOLUME, 1, BlendMode::Multiply, duck);
    let rendered = play(&mut engine, &mut processor, 4.0);
    for beat in 1..4 {
        let start = f64::from(beat);
        let ducked = rms(&window(&rendered, start, start + 0.1));
        let recovered = rms(&window(&rendered, start + 0.8, start + 0.95));
        assert!(
            ducked < recovered * 0.3,
            "beat {beat}: {ducked} vs {recovered}"
        );
    }
}

#[test]
fn layers_stack_bottom_to_top() {
    // Multiply pulls the volume right down, then Add on top brings it back.
    let render_with = |add_on_top: bool| {
        let (mut engine, mut processor) = held_note();
        add_layer(
            &mut engine,
            VOLUME,
            1,
            BlendMode::Multiply,
            block(0.0, 2.0, Shape::constant(0.05)),
        );
        if add_on_top {
            let mut layer = Layer::new(BlendMode::Add);
            layer
                .blocks
                .push((BlockId(2), block(0.0, 2.0, Shape::constant(0.85))));
            apply(
                &mut engine,
                Edit::AddLayer {
                    param: VOLUME,
                    id: LayerId(2),
                    index: 1,
                    layer,
                },
            );
        }
        rms(&window(&play(&mut engine, &mut processor, 2.0), 1.0, 2.0))
    };
    assert!(render_with(true) > render_with(false) * 10.0);
}

#[test]
fn disabling_or_undoing_a_layer_restores_the_original_sound_exactly() {
    let (mut engine, mut processor) = held_note();
    let original = play(&mut engine, &mut processor, 2.0);

    let with_layer = |change: Option<Edit>, undo: bool| {
        let (mut engine, mut processor) = held_note();
        add_layer(
            &mut engine,
            SYNTH_CUTOFF,
            1,
            BlendMode::Add,
            block(0.0, 2.0, Shape::ramp(0.0, 1.0)),
        );
        if let Some(edit) = change {
            apply(&mut engine, edit);
        }
        if undo {
            engine.apply(Command::Undo).unwrap();
        }
        play(&mut engine, &mut processor, 2.0)
    };

    assert!(
        with_layer(None, false) != original,
        "the layer should be heard"
    );
    let disabled = Edit::SetLayerOptions {
        id: LayerId(1),
        mode: BlendMode::Add,
        enabled: false,
    };
    assert!(with_layer(Some(disabled), false) == original);
    assert!(with_layer(None, true) == original);
}

#[test]
fn edits_mid_render_keep_the_effect_running_smoothly() {
    // An edit that changes nothing audible swaps in a new render state; the
    // filter must keep its memory, so the output is unchanged.
    let render_with_edit = |edit: bool| {
        let (mut engine, mut processor) = held_note();
        add_filter(&mut engine);
        apply(
            &mut engine,
            Edit::SetParam {
                param: FILTER_CUTOFF,
                value: 0.5,
            },
        );
        engine.apply(Command::Play).unwrap();
        let mut rendered = render(&mut engine, &mut processor, 12_000, &[480]);
        if edit {
            apply(&mut engine, Edit::SetTempo { bpm: 120.0 });
        }
        rendered
            .samples
            .extend(render(&mut engine, &mut processor, 12_000, &[480]).samples);
        rendered
    };
    assert!(render_with_edit(true) == render_with_edit(false));
}

#[test]
fn snapshot_shows_the_value_at_the_playhead() {
    let (mut engine, mut processor) = held_note();
    add_layer(
        &mut engine,
        SYNTH_CUTOFF,
        1,
        BlendMode::Add,
        block(1.0, 2.0, Shape::ramp(0.0, 1.0)),
    );
    engine
        .apply(Command::Locate {
            position: beats(2.5),
        })
        .unwrap();
    render(&mut engine, &mut processor, 1, &[1]);
    let snapshot = engine.snapshot();
    let cutoff = &snapshot.node(FIRST_NODE).unwrap().devices[0].params[instrument::CUTOFF as usize];
    assert_eq!(cutoff.name, "Cutoff");
    // Three quarters of the way up the ramp: a quarter of the range above
    // the centre line, so a push of +0.5.
    let pushed = (cutoff.base + 0.5).min(1.0);
    assert!((cutoff.value - pushed).abs() < 1e-9, "{}", cutoff.value);
    assert_eq!(cutoff.layers.len(), 1);
}

/// Automation on every kind of device at once, with repeating shapes, curves,
/// fades and a loop.
fn render_automated_pattern(blocks: &[usize], frames: usize) -> Rendered {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    let notes = (0..16)
        .map(|step| MidiNote {
            start: beats(f64::from(step) * 0.5),
            length: beats(0.4),
            pitch: [48, 55, 60, 63][step as usize % 4],
            velocity: 100,
            bend: None,
        })
        .collect();
    apply(
        &mut engine,
        Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: beats(8.0),
                notes,
                looping: None,
            },
        },
    );
    add_filter(&mut engine);

    // A sweep from 0.3 to 0.95, pushed from the filter's resting value:
    // `(target - rest + 1) / 2` on the centred lane. (Recorded before blend
    // modes were centred, as a Replace layer from 0.3 to 0.95: the same
    // sound.)
    let rest = FILTER_PARAMS[filter::CUTOFF as usize].default_normalized();
    let towards = |target: f64| (target - rest + 1.0) / 2.0;
    let mut sweep = block(0.0, 8.0, Shape::ramp(towards(0.3), towards(0.95)));
    sweep.fade_in = beats(0.5);
    add_layer(&mut engine, FILTER_CUTOFF, 1, BlendMode::Add, sweep);

    let mut wobble = block(
        2.0,
        4.0,
        Shape {
            points: vec![
                ShapePoint::new(0.0, 0.0).with_segment(daw_engine::Segment::Curve(0.5)),
                ShapePoint::new(0.5, 1.0),
                ShapePoint::new(1.0, 0.0),
            ],
        },
    );
    wobble.cycle = Some(beats(1.0 / 3.0));
    wobble.depth = 0.3;
    wobble.fade_out = beats(1.0);
    add_layer(&mut engine, SYNTH_CUTOFF, 2, BlendMode::Add, wobble);

    // Multiply from x0.2 back to x1 (the centre line) every beat.
    let mut duck = block(0.0, 8.0, Shape::ramp(0.1, 0.5));
    duck.cycle = Some(beats(1.0));
    add_layer(&mut engine, VOLUME, 3, BlendMode::Multiply, duck);

    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: beats(1.0),
            end: beats(7.0),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, frames, blocks)
}

#[test]
fn automated_output_is_identical_for_any_block_size() {
    let reference = render_automated_pattern(&[480], 400_000);
    assert!(!reference.is_silent());
    for blocks in [&[1][..], VARIED_BLOCKS, &[4096], &[31, 33]] {
        assert!(
            render_automated_pattern(blocks, 400_000) == reference,
            "blocks {blocks:?} changed the output"
        );
    }
}

#[test]
fn automated_pattern_matches_reference_recording() {
    assert_reference!(
        "automated-pattern",
        &render_automated_pattern(VARIED_BLOCKS, 200_000)
    );
}

#[test]
fn constant_layer_editing_during_playback_stays_realtime_safe() {
    // The realtime guard aborts this test if the audio side allocates or frees.
    let (mut engine, mut processor) = held_note();
    engine.apply(Command::Play).unwrap();
    for i in 0..300_u64 {
        let device = DeviceId(100 + i);
        apply(
            &mut engine,
            Edit::AddEffect {
                node: FIRST_NODE,
                id: device,
                index: 0,
                effect: Effect::new(EffectKind::Filter),
            },
        );
        add_layer(
            &mut engine,
            ParamId {
                node: FIRST_NODE,
                device: DeviceRef::Effect(device),
                index: filter::CUTOFF,
            },
            i,
            BlendMode::Add,
            block((i % 8) as f64, 1.0, Shape::ramp(0.0, 1.0)),
        );
        if i % 2 == 0 {
            apply(&mut engine, Edit::RemoveEffect { id: device });
        }
        if i % 7 == 0 {
            engine.apply(Command::Undo).unwrap();
        }
        let synth = engine.project().node(FIRST_NODE).unwrap();
        if synth.effects.len() > 12 {
            let id = synth.effects[0].0;
            apply(&mut engine, Edit::RemoveEffect { id });
        }
        render(&mut engine, &mut processor, 97, &[13, 31]);
    }
}
