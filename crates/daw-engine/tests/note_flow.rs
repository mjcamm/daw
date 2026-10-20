//! Notes flow up the tree, and each instrument plays only its own notes
//! (`docs/design/note-flow.md`).

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::samples::SampleData;
use daw_engine::{
    ClipId, Command, CommandError, DeviceId, Edit, Engine, EngineConfig, FIRST_INSTRUMENT,
    FIRST_NODE, Instrument, InstrumentKind, MidiClip, MidiNote, Node, NodeId, SampleId, SampleRef,
    Ticks,
};
use daw_testkit::{Rendered, VARIED_BLOCKS, render};

const CHILD: NodeId = NodeId(20);

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

fn clip(engine: &mut Engine, node: NodeId, id: u64, pitches: &[u8]) {
    let notes = pitches
        .iter()
        .map(|&pitch| MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_beats(1),
            pitch,
            velocity: 100,
        })
        .collect();
    apply(
        engine,
        Edit::AddClip {
            node,
            id: ClipId(id),
            clip: MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_beats(4),
                notes,
                looping: None,
            },
        },
    );
}

fn play(engine: &mut Engine, processor: &mut daw_engine::AudioProcessor) -> Rendered {
    engine.apply(Command::Play).unwrap();
    render(engine, processor, 12_000, VARIED_BLOCKS)
}

/// The first node with a SingleSampler (on its default C1) playing a click,
/// in place of its synth.
fn sampler_engine() -> (Engine, daw_engine::AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    apply(
        &mut engine,
        Edit::Batch(vec![
            Edit::AddSample {
                id: SampleId(1),
                sample: SampleRef {
                    name: "Click".to_owned(),
                    file: "audio/click.wav".to_owned(),
                },
            },
            Edit::RemoveInstrument {
                id: FIRST_INSTRUMENT,
            },
            Edit::AddInstrument {
                node: FIRST_NODE,
                id: DeviceId(10),
                index: 0,
                instrument: Instrument {
                    sample: Some(SampleId(1)),
                    ..Instrument::new(InstrumentKind::SingleSampler)
                },
            },
        ]),
    );
    engine.insert_sample_data(
        SampleId(1),
        "audio/click.wav",
        SampleData::mono(48_000, vec![0.5; 2000]),
    );
    (engine, processor)
}

#[test]
fn a_sampler_plays_only_its_trigger_note() {
    let (mut engine, mut processor) = sampler_engine();
    clip(&mut engine, FIRST_NODE, 1, &[38]);
    assert!(
        play(&mut engine, &mut processor).is_silent(),
        "D1 is not its note"
    );

    let (mut engine, mut processor) = sampler_engine();
    clip(&mut engine, FIRST_NODE, 1, &[36]);
    assert!(!play(&mut engine, &mut processor).is_silent(), "C1 is");

    let (mut engine, mut processor) = sampler_engine();
    clip(&mut engine, FIRST_NODE, 1, &[38]);
    apply(
        &mut engine,
        Edit::SetInstrumentNote {
            id: DeviceId(10),
            note: 38,
        },
    );
    assert!(
        !play(&mut engine, &mut processor).is_silent(),
        "moved to D1"
    );
    let snapshot = engine.snapshot();
    let device = &snapshot.node(FIRST_NODE).unwrap().devices[0];
    assert_eq!(device.trigger, Some(38));
    assert!(device.plays.contains(38) && device.plays.len() == 1);
}

#[test]
fn trigger_notes_belong_to_samplers_and_stay_in_range() {
    let (mut engine, _processor) = Engine::new(EngineConfig::default());
    let refused = engine
        .apply(Command::Edit(Edit::SetInstrumentNote {
            id: FIRST_INSTRUMENT,
            note: 40,
        }))
        .unwrap_err();
    assert_eq!(
        refused,
        CommandError::NotASampler {
            id: FIRST_INSTRUMENT
        }
    );
    let (mut engine, _processor) = sampler_engine();
    let refused = engine
        .apply(Command::Edit(Edit::SetInstrumentNote {
            id: DeviceId(10),
            note: 200,
        }))
        .unwrap_err();
    assert_eq!(refused, CommandError::NoteOutOfRange { note: 200 });
}

/// The first node (with its synth) and an empty MIDI child under it.
fn with_child(child_has_synth: bool) -> (Engine, daw_engine::AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    let node = Node::new("Child", 1);
    let node = if child_has_synth {
        node.with_instrument(DeviceId(30))
    } else {
        node
    };
    apply(
        &mut engine,
        Edit::AddNode {
            id: CHILD,
            parent: FIRST_NODE,
            index: 0,
            node,
        },
    );
    (engine, processor)
}

#[test]
fn a_childs_notes_pass_up_to_its_parents_instruments() {
    // The child has no instrument; its notes still sound, played by the
    // parent's synth.
    let (mut engine, mut processor) = with_child(false);
    clip(&mut engine, CHILD, 1, &[60]);
    assert!(!play(&mut engine, &mut processor).is_silent());
}

#[test]
fn notes_never_pass_down() {
    // The parent's synth is off: the parent's notes do not reach the
    // child's synth.
    let (mut engine, mut processor) = with_child(true);
    apply(
        &mut engine,
        Edit::SetDeviceEnabled {
            id: FIRST_INSTRUMENT,
            enabled: false,
        },
    );
    clip(&mut engine, FIRST_NODE, 1, &[60]);
    assert!(play(&mut engine, &mut processor).is_silent());
}

#[test]
fn a_parent_layers_its_childs_notes_and_the_child_plays_only_its_own() {
    // Child accents: the child's synth plays its note; the parent's synth
    // plays its own note and the child's. Louder than the child alone.
    let (mut engine, mut processor) = with_child(true);
    clip(&mut engine, CHILD, 1, &[64]);
    let layered = play(&mut engine, &mut processor);

    let (mut engine, mut processor) = with_child(true);
    apply(
        &mut engine,
        Edit::SetDeviceEnabled {
            id: FIRST_INSTRUMENT,
            enabled: false,
        },
    );
    clip(&mut engine, CHILD, 1, &[64]);
    let child_alone = play(&mut engine, &mut processor);
    assert!(!child_alone.is_silent());
    assert!(
        layered.peak() > child_alone.peak() * 1.2,
        "the parent joins in"
    );
}
