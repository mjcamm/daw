//! Bent notes played through the built-in Synth (`docs/design/note-bends.md`).

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::{
    ClipId, Command, Curve, Edit, Engine, EngineConfig, FIRST_NODE, LoopRange, MidiClip, MidiNote,
    Ticks,
};
use daw_protocol::{CurvePoint, Handle, PointKind, bend_value};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;

fn beats(n: f64) -> Ticks {
    Ticks::from_beats_f64(n)
}

fn note(start: f64, length: f64, pitch: u8, bend: Option<Curve>) -> MidiNote {
    MidiNote {
        start: beats(start),
        length: beats(length),
        pitch,
        velocity: 100,
        bend,
    }
}

/// Straight from `from` to `to` semitones away from the note's pitch.
fn glide(from: f64, to: f64) -> Option<Curve> {
    Some(Curve::line(bend_value(from), bend_value(to)))
}

fn clip(notes: Vec<MidiNote>) -> MidiClip {
    MidiClip {
        start: Ticks::ZERO,
        length: beats(8.0),
        notes,
        looping: None,
    }
}

fn set_clip(engine: &mut Engine, edit: fn(ClipId, MidiClip) -> Edit, notes: Vec<MidiNote>) {
    engine
        .apply(Command::Edit(edit(ClipId(1), clip(notes))))
        .unwrap();
}

fn add(id: ClipId, clip: MidiClip) -> Edit {
    Edit::AddClip {
        node: FIRST_NODE,
        id,
        clip,
    }
}

fn set(id: ClipId, clip: MidiClip) -> Edit {
    Edit::SetClip {
        node: FIRST_NODE,
        id,
        clip,
    }
}

fn play(notes: Vec<MidiNote>, frames: usize, blocks: &[usize]) -> Rendered {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    set_clip(&mut engine, add, notes);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, frames, blocks)
}

/// The left channel from `from` to `to`.
fn left(rendered: &Rendered, from: usize, to: usize) -> Vec<f32> {
    let channels = usize::from(rendered.channels);
    rendered.samples[from * channels..to * channels]
        .iter()
        .step_by(channels)
        .copied()
        .collect()
}

/// Cycles of the saw: one drop through zero each.
fn cycles(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|w| w[0] > 0.0 && w[1] <= 0.0)
        .count()
}

#[test]
fn a_note_bent_a_steady_octave_up_sounds_like_the_octave() {
    let bent = play(
        vec![note(0.0, 1.0, 60, glide(12.0, 12.0))],
        FRAMES_PER_BEAT * 2,
        VARIED_BLOCKS,
    );
    let octave = play(
        vec![note(0.0, 1.0, 72, None)],
        FRAMES_PER_BEAT * 2,
        VARIED_BLOCKS,
    );
    assert!(!octave.is_silent());
    assert!(bent == octave);
}

#[test]
fn a_glide_from_below_rises_into_the_note() {
    // A C whose front is an octave below: it starts low and slides up over
    // four beats.
    let length = 4 * FRAMES_PER_BEAT;
    let rendered = play(
        vec![note(0.0, 4.0, 60, glide(-12.0, 0.0))],
        length,
        VARIED_BLOCKS,
    );
    let tenth = length / 10;
    let early = cycles(&left(&rendered, 0, tenth));
    let late = cycles(&left(&rendered, length - tenth, length));
    // Around 48.6 then 59.4: 2^(10.8 / 12) ≈ 1.87 times as many cycles.
    let ratio = late as f32 / early as f32;
    assert!((1.75..2.0).contains(&ratio), "{early} then {late} cycles");
}

fn busy_bends(blocks: &[usize]) -> Rendered {
    let s_curve = Curve {
        points: vec![
            CurvePoint::corner(0.0, bend_value(0.0)).with_handles(
                PointKind::Smooth,
                Handle::NONE,
                Handle::new(0.4, 0.0),
            ),
            CurvePoint::corner(1.0, bend_value(7.0)).with_handles(
                PointKind::Smooth,
                Handle::new(-0.4, 0.0),
                Handle::NONE,
            ),
        ],
    };
    let mut notes = vec![
        note(0.0, 1.5, 60, Some(s_curve)),
        note(0.25, 0.7, 64, glide(5.0, -5.0)),
        note(1.0 / 3.0, 1.0 / 7.0, 72, glide(0.0, 24.0)),
        note(2.0, 3.0, 55, glide(-2.0, 2.0)),
    ];
    // Straight notes alongside.
    notes.extend((0..8).map(|i| note(f64::from(i) * 0.5, 0.2, 48 + i as u8, None)));
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    set_clip(&mut engine, add, notes);
    engine
        .apply(Command::SetLoop(Some(LoopRange {
            start: beats(0.5),
            end: beats(3.5),
        })))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 200_000, blocks)
}

#[test]
fn bends_sound_the_same_for_any_block_size() {
    let reference = busy_bends(&[480]);
    assert!(!reference.is_silent());
    for blocks in [&[1][..], VARIED_BLOCKS, &[4096], &[1023, 1025]] {
        assert!(busy_bends(blocks) == reference, "blocks {blocks:?}");
    }
}

#[test]
fn a_bend_changed_while_the_note_plays_is_heard_at_once() {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    set_clip(&mut engine, add, vec![note(0.0, 4.0, 60, None)]);
    engine.apply(Command::Play).unwrap();
    let before = render(&mut engine, &mut processor, FRAMES_PER_BEAT, VARIED_BLOCKS);
    // As if dragging its end dot up an octave, all the way along.
    set_clip(
        &mut engine,
        set,
        vec![note(0.0, 4.0, 60, glide(12.0, 12.0))],
    );
    let after = render(&mut engine, &mut processor, FRAMES_PER_BEAT, VARIED_BLOCKS);
    let ratio = cycles(&left(&after, 0, FRAMES_PER_BEAT)) as f32
        / cycles(&left(&before, 0, FRAMES_PER_BEAT)) as f32;
    assert!((ratio - 2.0).abs() < 0.05, "{ratio}");
}
