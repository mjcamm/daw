//! A VST3 instrument (this repo's "Test Synth") in a host process: notes
//! in at exact frames, sound out.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

mod common;

use common::{SYNTH_CID, install};
use daw_engine::{
    AudioProcessor, ClipId, Command, Curve, DeviceId, DeviceRef, Edit, Engine, EngineConfig,
    FIRST_INSTRUMENT, FIRST_NODE, Instrument, MidiClip, MidiNote, ParamId, ParamState, PluginParam,
    PluginState, Ticks, bend_value,
};
use daw_plugins::block::{Context, Layout, NoteEvent, Outcome};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::{BendRoute, PluginSpec};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};
use std::time::{Duration, Instant};

fn host_command() -> HostCommand {
    HostCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: Vec::new(),
    }
}

#[test]
fn a_vst3_instrument_plays_notes_from_their_exact_frames() {
    let (_, bundle) = install("vst3-synth");
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    let spec = PluginSpec::Vst3 {
        bundle,
        cid: SYNTH_CID.into(),
        name: "Test Synth".into(),
        vendor: "DAW Tests".into(),
    };
    host.create(id, spec, None).unwrap();
    let layout = Layout {
        max_frames: 64,
        inputs: 0,
        outputs: 2,
    };
    let mut block = host.activate(id, layout, 48_000.).unwrap();
    let (mut l, mut r) = (vec![9.; 64], vec![9.; 64]);
    let outcome = block.process_with_notes(
        &[],
        &[],
        &[NoteEvent::start(10, 7, 69, 1.0), NoteEvent::end(40, 7, 69)],
        Context::default(),
        &mut [&mut l, &mut r],
        Some(Instant::now() + Duration::from_secs(2)),
    );
    assert_eq!(outcome, Outcome::Done);
    assert!(l[..10].iter().all(|&s| s == 0.0), "silent before the note");
    assert!(
        (l[10] - 0.2).abs() < 1e-6,
        "full velocity, cosine at its start"
    );
    assert!(l[11] != 0.0 && l[39] != 0.0, "sounds while held");
    assert!(l[40..].iter().all(|&s| s == 0.0), "stops at its end");
    assert_eq!(l, r, "both sides");
    drop(block);
    host.destroy(id).unwrap();
}

/// A note in tune and a note an octave below tuned up an octave
/// (`docs/design/note-bends.md`) sound the same, sample for sample: the
/// host passes tunings on as the plugin's per-note tuning.
#[test]
fn a_vst3_instrument_hears_a_notes_tuning() {
    let (_, bundle) = install("vst3-synth-tuning");
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    let spec = PluginSpec::Vst3 {
        bundle,
        cid: SYNTH_CID.into(),
        name: "Test Synth".into(),
        vendor: "DAW Tests".into(),
    };
    host.create(id, spec, None).unwrap();
    let (route, found) = host.bends(id).unwrap();
    assert_eq!(route, BendRoute::NoteExpression, "{found}");
    let layout = Layout {
        max_frames: 256,
        inputs: 0,
        outputs: 2,
    };
    let mut block = host.activate(id, layout, 48_000.).unwrap();
    let mut play = |notes: &[NoteEvent]| {
        let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
        let outcome = block.process_with_notes(
            &[],
            &[],
            notes,
            Context::default(),
            &mut [&mut l, &mut r],
            Some(Instant::now() + Duration::from_secs(2)),
        );
        assert_eq!(outcome, Outcome::Done);
        l
    };
    let octave = play(&[NoteEvent::start(10, 1, 69, 1.0), NoteEvent::end(200, 1, 69)]);
    let tuned = play(&[
        NoteEvent::start(10, 2, 57, 1.0),
        NoteEvent::tune(10, 2, 57, 12.0),
        NoteEvent::end(200, 2, 57),
    ]);
    assert!(octave[10..200].iter().any(|&s| s != 0.0));
    // VST3 carries tunings on a 0..1 scale, so +12 arrives as 12.00…01.
    assert!(close(&tuned, &octave), "the tuned note is the octave");
    drop(block);
    host.destroy(id).unwrap();
}

const SYNTH: DeviceId = DeviceId(901);

/// The first node playing one A4 from beat 1 to beat 2 (frames 24 000 to
/// 48 000 at 120 BPM) on Test Synth instead of the built-in synth.
fn song(bundle: &std::path::Path) -> (Engine, AudioProcessor) {
    song_with(bundle, 69, None)
}

/// As [`song`], the note at `pitch` and bent by `bend`.
fn song_with(bundle: &std::path::Path, pitch: u8, bend: Option<Curve>) -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    engine.set_plugin_host(host_command());
    let spec = PluginSpec::Vst3 {
        bundle: bundle.to_path_buf(),
        cid: SYNTH_CID.into(),
        name: "Test Synth".into(),
        vendor: "DAW Tests".into(),
    };
    for edit in [
        Edit::RemoveInstrument {
            id: FIRST_INSTRUMENT,
        },
        Edit::AddInstrument {
            node: FIRST_NODE,
            id: SYNTH,
            index: 0,
            instrument: Instrument::plugin(spec),
        },
        Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_bars(1),
                notes: vec![MidiNote {
                    start: Ticks::from_beats(1),
                    length: Ticks::from_beats(1),
                    pitch,
                    velocity: 127,
                    bend,
                }],
                looping: None,
            },
        },
    ] {
        engine.apply(Command::Edit(edit)).unwrap();
    }
    engine.apply(Command::Play).unwrap();
    (engine, processor)
}

fn left(out: &Rendered, frame: usize) -> f32 {
    out.samples[frame * 2]
}

/// Sample for sample within rounding.
fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// docs/design/note-bends.md: a song's bent note reaches a plugin
/// instrument as the note's tuning, on the control grid, the same for any
/// block size.
#[test]
fn a_songs_bent_note_bends_on_a_vst3_instrument() {
    let (_, bundle) = install("vst3-synth-bend");
    let steady = Some(Curve::line(bend_value(12.), bend_value(12.)));
    let (mut engine, mut processor) = song_with(&bundle, 57, steady);
    let bent = render(&mut engine, &mut processor, 60_000, &[512]);
    let (mut engine, mut processor) = song(&bundle);
    let octave = render(&mut engine, &mut processor, 60_000, &[512]);
    assert!(
        close(&bent.samples, &octave.samples),
        "a steady octave bend is the octave"
    );

    // A glide rises, and sounds the same for any block size.
    let glide = || Some(Curve::line(bend_value(-12.), bend_value(0.)));
    let (mut engine, mut processor) = song_with(&bundle, 69, glide());
    let one = render(&mut engine, &mut processor, 60_000, &[512]);
    let (mut engine, mut processor) = song_with(&bundle, 69, glide());
    let varied = render(&mut engine, &mut processor, 60_000, VARIED_BLOCKS);
    assert!(varied == one, "the same at any block size");
    let rises = |from: usize| {
        (from..from + 4_800)
            .filter(|&f| left(&one, f) > 0. && left(&one, f + 1) <= 0.)
            .count()
    };
    let (early, late) = (rises(24_000), rises(43_200));
    assert!(late > early * 3 / 2, "{early} then {late} cycles");
}

#[test]
fn a_vst3_instrument_plays_a_songs_notes_on_their_frames_at_any_block_size() {
    let (_, bundle) = install("vst3-synth-song");
    let (mut engine, mut processor) = song(&bundle);
    let device = engine
        .snapshot()
        .node(FIRST_NODE)
        .unwrap()
        .devices
        .iter()
        .find(|d| d.device == DeviceRef::Instrument(SYNTH))
        .cloned()
        .unwrap();
    assert_eq!(device.name, "Test Synth");
    assert_eq!(
        device.plugin.as_ref().map(|p| p.state.clone()),
        Some(PluginState::Running)
    );
    let out = render(&mut engine, &mut processor, 60_000, &[512]);
    assert!(
        (0..24_000).all(|f| left(&out, f) == 0.0),
        "silent before the note"
    );
    assert!(left(&out, 24_000) > 0.0, "starts on its frame");
    assert!(
        (48_000..60_000).all(|f| left(&out, f) == 0.0),
        "stops on its frame"
    );

    let (mut again, mut again_processor) = song(&bundle);
    let varied = render(&mut again, &mut again_processor, 60_000, VARIED_BLOCKS);
    assert!(varied == out, "the same at any block size");

    // Its parameter (Gain, resting at 0.25) halves it.
    let (mut quiet, mut quiet_processor) = song(&bundle);
    quiet
        .apply(Command::Edit(Edit::AddPluginParam {
            device: SYNTH,
            param: PluginParam {
                id: 0,
                name: "Gain".into(),
                state: ParamState {
                    base: 0.25,
                    layers: Vec::new(),
                },
            },
        }))
        .unwrap();
    let halved = render(&mut quiet, &mut quiet_processor, 60_000, &[512]);
    let gain = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Instrument(SYNTH),
        index: 0,
    };
    assert!(quiet.project().param(gain).is_some());
    assert!(
        (24_000..48_000).all(|f| (left(&halved, f) - left(&out, f) * 0.5).abs() < 1e-6),
        "a plugin instrument's parameter moves it"
    );

    // Its state is saved with the song.
    assert!(engine.to_file().plugin_states().contains_key(&SYNTH));
}
