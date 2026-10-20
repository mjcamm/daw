//! The SingleSampler playing audio samples, and the project's sample table.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_dsp::pan::equal_power;
use daw_engine::samples::SampleData;
use daw_engine::{
    AudioProcessor, ClipId, Command, CommandError, DeviceId, Edit, Engine, EngineConfig,
    FIRST_INSTRUMENT, FIRST_NODE, Instrument, InstrumentKind, MidiClip, MidiNote, SampleId,
    SampleRef, SampleStatus, Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

/// 120 BPM at 48 kHz.
const FRAMES_PER_BEAT: usize = 24_000;
const KICK: SampleId = SampleId(1);
const KICK_FILE: &str = "audio/kick.wav";
const SAMPLER: DeviceId = DeviceId(10);

fn kick_ref() -> SampleRef {
    SampleRef {
        name: "Kick".to_owned(),
        file: KICK_FILE.to_owned(),
    }
}

/// A short, recognisable sound: a falling ramp.
fn kick_audio() -> Vec<f32> {
    (0..1000).map(|i| 1.0 - i as f32 / 1000.0).collect()
}

/// The first node with its synth replaced by a SingleSampler playing the
/// kick, and the kick's audio in the pool.
fn sampler_engine() -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    let edits = vec![
        Edit::AddSample {
            id: KICK,
            sample: kick_ref(),
        },
        Edit::RemoveInstrument {
            id: FIRST_INSTRUMENT,
        },
        Edit::AddInstrument {
            node: FIRST_NODE,
            id: SAMPLER,
            index: 0,
            instrument: Instrument {
                sample: Some(KICK),
                ..Instrument::new(InstrumentKind::SingleSampler)
            },
        },
    ];
    engine.apply(Command::Edit(Edit::Batch(edits))).unwrap();
    engine.insert_sample_data(KICK, KICK_FILE, SampleData::mono(48_000, kick_audio()));
    (engine, processor)
}

fn add_hits(engine: &mut Engine, beats: &[f64]) {
    let clip = MidiClip {
        start: Ticks::ZERO,
        length: Ticks::from_beats(8),
        notes: beats
            .iter()
            .map(|&b| MidiNote {
                start: Ticks::from_beats_f64(b),
                // Shorter than the sample: a one-shot plays on regardless.
                length: Ticks::from_beats_f64(0.001),
                pitch: 36,
                velocity: 127,
            })
            .collect(),
        looping: None,
    };
    engine
        .apply(Command::Edit(Edit::AddClip {
            node: FIRST_NODE,
            id: ClipId(1),
            clip,
        }))
        .unwrap();
}

fn left(rendered: &Rendered, frame: usize) -> f32 {
    rendered.samples[frame * usize::from(rendered.channels)]
}

#[test]
fn a_hit_plays_the_sample_from_its_exact_frame_to_its_end() {
    let (mut engine, mut processor) = sampler_engine();
    add_hits(&mut engine, &[1.0]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(
        &mut engine,
        &mut processor,
        2 * FRAMES_PER_BEAT,
        VARIED_BLOCKS,
    );
    // An instrument is panned as a mono source: equal power at the centre.
    let (gain, _) = equal_power(0.0);
    let start = FRAMES_PER_BEAT;
    assert_eq!(left(&rendered, start - 1), 0.0, "silent before the hit");
    for (i, want) in kick_audio().iter().enumerate() {
        let got = left(&rendered, start + i);
        assert!(
            (got - want * gain).abs() < 1e-6,
            "frame {i}: {got} vs {}",
            want * gain
        );
    }
    assert_eq!(
        left(&rendered, start + 1000),
        0.0,
        "silent after the sample ends"
    );
}

#[test]
fn sampler_output_is_identical_for_any_block_size() {
    let render_with = |blocks: &[usize]| {
        let (mut engine, mut processor) = sampler_engine();
        add_hits(&mut engine, &[0.0, 0.25, 0.26, 1.5, 3.0]);
        engine.apply(Command::Play).unwrap();
        render(&mut engine, &mut processor, 4 * FRAMES_PER_BEAT, blocks)
    };
    let reference = render_with(&[480]);
    assert!(!reference.is_silent());
    for blocks in [&[1][..], VARIED_BLOCKS, &[4096]] {
        assert!(
            render_with(blocks) == reference,
            "blocks {blocks:?} changed the output"
        );
    }
}

#[test]
fn a_sample_recorded_at_another_rate_plays_at_its_own_speed() {
    let (mut engine, mut processor) = sampler_engine();
    // The same ramp at 24 kHz lasts twice as long at the engine's 48 kHz.
    engine.insert_sample_data(KICK, KICK_FILE, SampleData::mono(24_000, kick_audio()));
    add_hits(&mut engine, &[0.0]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 4000, VARIED_BLOCKS);
    assert!(left(&rendered, 1990) > 0.0, "still sounding");
    assert_eq!(left(&rendered, 2000), 0.0, "done after 2000 frames");
}

#[test]
fn without_its_audio_the_sampler_is_silent_and_the_sample_shows_as_missing() {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    engine
        .apply(Command::Edit(Edit::Batch(vec![
            Edit::AddSample {
                id: KICK,
                sample: kick_ref(),
            },
            Edit::AddInstrument {
                node: FIRST_NODE,
                id: SAMPLER,
                index: 1,
                instrument: Instrument {
                    sample: Some(KICK),
                    ..Instrument::new(InstrumentKind::SingleSampler)
                },
            },
            Edit::SetDeviceEnabled {
                id: FIRST_INSTRUMENT,
                enabled: false,
            },
        ])))
        .unwrap();
    add_hits(&mut engine, &[0.0]);
    engine.apply(Command::Play).unwrap();
    let rendered = render(&mut engine, &mut processor, 2000, VARIED_BLOCKS);
    assert!(rendered.is_silent());
    let snapshot = engine.snapshot();
    assert!(matches!(
        snapshot.samples[0].status,
        SampleStatus::Missing { .. }
    ));
}

#[test]
fn choosing_a_sample_is_undoable() {
    let (mut engine, mut processor) = sampler_engine();
    add_hits(&mut engine, &[0.0]);
    engine
        .apply(Command::Edit(Edit::SetInstrumentSample {
            id: SAMPLER,
            sample: None,
        }))
        .unwrap();
    engine.apply(Command::Play).unwrap();
    assert!(render(&mut engine, &mut processor, 2000, VARIED_BLOCKS).is_silent());
    engine.apply(Command::Undo).unwrap();
    engine.apply(Command::Stop).unwrap();
    engine
        .apply(Command::Locate {
            position: Ticks::ZERO,
        })
        .unwrap();
    engine.apply(Command::Play).unwrap();
    assert!(!render(&mut engine, &mut processor, 2000, VARIED_BLOCKS).is_silent());
}

#[test]
fn the_sample_table_guards_its_samples() {
    let (mut engine, _processor) = sampler_engine();
    let refused = |engine: &mut Engine, edit: Edit| engine.apply(Command::Edit(edit)).unwrap_err();
    assert_eq!(
        refused(&mut engine, Edit::RemoveSample { id: KICK }),
        CommandError::SampleInUse { id: KICK }
    );
    for file in [
        "",
        "../outside.wav",
        "/abs.wav",
        "C:/x.wav",
        "audio\\kick.wav",
        "audio//x.wav",
    ] {
        assert!(
            matches!(
                refused(
                    &mut engine,
                    Edit::AddSample {
                        id: SampleId(9),
                        sample: SampleRef {
                            name: "x".to_owned(),
                            file: file.to_owned(),
                        },
                    }
                ),
                CommandError::InvalidSampleFile { .. }
            ),
            "{file:?} accepted"
        );
    }
    assert_eq!(
        refused(
            &mut engine,
            Edit::SetInstrumentSample {
                id: SAMPLER,
                sample: Some(SampleId(5)),
            }
        ),
        CommandError::UnknownSample { id: SampleId(5) }
    );
    // A synth plays no sample.
    let synth = DeviceId(11);
    engine
        .apply(Command::Edit(Edit::AddInstrument {
            node: FIRST_NODE,
            id: synth,
            index: 1,
            instrument: Instrument::new(InstrumentKind::Synth),
        }))
        .unwrap();
    assert_eq!(
        refused(
            &mut engine,
            Edit::SetInstrumentSample {
                id: synth,
                sample: Some(KICK),
            }
        ),
        CommandError::NotASampler { id: synth }
    );
    // Once nothing plays it, it can go, and come back with undo.
    engine
        .apply(Command::Edit(Edit::SetInstrumentSample {
            id: SAMPLER,
            sample: None,
        }))
        .unwrap();
    engine
        .apply(Command::Edit(Edit::RemoveSample { id: KICK }))
        .unwrap();
    assert!(engine.snapshot().samples.is_empty());
    engine.apply(Command::Undo).unwrap();
    assert_eq!(engine.snapshot().samples[0].name, "Kick");
}

#[test]
fn a_reopened_project_keeps_its_samples_and_sounds_identical() {
    let play = |engine: &mut Engine, processor: &mut AudioProcessor| {
        engine.apply(Command::Play).unwrap();
        render(engine, processor, FRAMES_PER_BEAT, VARIED_BLOCKS)
    };
    let (mut engine, mut processor) = sampler_engine();
    add_hits(&mut engine, &[0.0, 0.5]);
    let json = engine.to_file().to_json();
    assert!(json.contains("audio/kick.wav"));
    let original = play(&mut engine, &mut processor);

    let (mut reopened, mut processor) = Engine::new(EngineConfig::default());
    reopened
        .load(daw_engine::ProjectFile::from_json(&json).unwrap())
        .unwrap();
    // The folder would supply the file; here the pool gets the same audio.
    reopened.insert_sample_data(KICK, KICK_FILE, SampleData::mono(48_000, kick_audio()));
    let snapshot = reopened.snapshot();
    assert_eq!(snapshot.samples[0].name, "Kick");
    let sampler = snapshot
        .node(FIRST_NODE)
        .unwrap()
        .devices
        .iter()
        .find(|d| d.instrument == Some(InstrumentKind::SingleSampler))
        .unwrap();
    assert_eq!(sampler.sample, Some(KICK));
    assert!(play(&mut reopened, &mut processor) == original);
}
