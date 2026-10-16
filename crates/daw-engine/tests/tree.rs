//! Nodes in a tree: sound flows up through folders into the Song.

#![allow(clippy::unwrap_used, reason = "test helpers fail the test on error")]

use daw_engine::{
    AudioProcessor, ClipId, Command, DeviceId, Edit, Engine, EngineConfig, FIRST_NODE, MidiClip,
    MidiNote, Node, NodeId, Ticks,
};
use daw_testkit::{VARIED_BLOCKS, render};

/// One beat at 120 BPM and 48 kHz.
const FRAMES: usize = 24_000;
const FOLDER: NodeId = NodeId(10);
const LEAD: NodeId = NodeId(11);

fn new_engine() -> (Engine, AudioProcessor) {
    Engine::new(EngineConfig::default())
}

fn apply(engine: &mut Engine, edit: Edit) {
    engine.apply(Command::Edit(edit)).unwrap();
}

/// A held note at `pitch` on `node`, filling the first beat.
fn hold(engine: &mut Engine, node: NodeId, id: u64, pitch: u8) {
    let clip = MidiClip {
        start: Ticks::ZERO,
        length: Ticks::from_beats(4),
        notes: vec![MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_beats(2),
            pitch,
            velocity: 100,
        }],
    };
    apply(
        engine,
        Edit::AddClip {
            node,
            id: ClipId(id),
            clip,
        },
    );
}

fn add_folder(engine: &mut Engine) {
    apply(
        engine,
        Edit::AddNode {
            id: FOLDER,
            parent: NodeId::ROOT,
            index: 0,
            node: Node::new("Folder", 0),
        },
    );
}

/// A second node that plays notes, under `parent`.
fn add_lead(engine: &mut Engine, parent: NodeId) {
    apply(
        engine,
        Edit::AddNode {
            id: LEAD,
            parent,
            index: 0,
            node: Node::new("Lead", 1).with_instrument(DeviceId(104)),
        },
    );
}

fn play(mut engine: Engine, mut processor: AudioProcessor, blocks: &[usize]) -> Vec<f32> {
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, FRAMES, blocks).samples
}

fn assert_close(a: &[f32], b: &[f32]) {
    assert_eq!(a.len(), b.len());
    let worst = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-6, "differs by up to {worst}");
}

#[test]
fn a_folder_at_its_defaults_passes_sound_unchanged() {
    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    let direct = play(engine, processor, VARIED_BLOCKS);

    let (mut engine, processor) = new_engine();
    add_folder(&mut engine);
    apply(
        &mut engine,
        Edit::MoveNode {
            id: FIRST_NODE,
            parent: FOLDER,
            index: 0,
        },
    );
    hold(&mut engine, FIRST_NODE, 1, 60);
    let nested = play(engine, processor, VARIED_BLOCKS);

    assert!(direct.iter().any(|&s| s != 0.0));
    assert_eq!(direct, nested, "nesting costs nothing");
}

#[test]
fn children_sum_into_their_parent() {
    let only = |synth: bool, lead: bool| {
        let (mut engine, processor) = new_engine();
        add_lead(&mut engine, NodeId::ROOT);
        if synth {
            hold(&mut engine, FIRST_NODE, 1, 60);
        }
        if lead {
            hold(&mut engine, LEAD, 2, 67);
        }
        play(engine, processor, VARIED_BLOCKS)
    };
    let (synth, lead, both) = (only(true, false), only(false, true), only(true, true));
    let sum: Vec<f32> = synth.iter().zip(&lead).map(|(a, b)| a + b).collect();
    assert_close(&both, &sum);
}

#[test]
fn muting_a_folder_silences_everything_under_it() {
    let (mut engine, processor) = new_engine();
    add_folder(&mut engine);
    add_lead(&mut engine, FOLDER);
    hold(&mut engine, LEAD, 1, 67);
    apply(
        &mut engine,
        Edit::SetMute {
            node: FOLDER,
            muted: true,
        },
    );
    let muted = play(engine, processor, VARIED_BLOCKS);
    // Muting ramps the gain down over one control period, so it never
    // clicks; silence follows.
    let ramp = 2 * daw_engine::CONTROL_FRAMES as usize * 2;
    assert!(
        muted[..ramp].iter().any(|&s| s != 0.0),
        "the lead was playing"
    );
    assert!(muted[ramp..].iter().all(|&s| s == 0.0));
}

#[test]
fn solo_hears_only_the_soloed_branch() {
    let setup = |solo: bool| {
        let (mut engine, processor) = new_engine();
        add_folder(&mut engine);
        add_lead(&mut engine, FOLDER);
        hold(&mut engine, FIRST_NODE, 1, 60);
        hold(&mut engine, LEAD, 2, 67);
        if solo {
            apply(
                &mut engine,
                Edit::SetSolo {
                    node: FOLDER,
                    soloed: true,
                },
            );
        } else {
            apply(&mut engine, Edit::RemoveClip { id: ClipId(1) });
        }
        play(engine, processor, VARIED_BLOCKS)
    };
    let (soloed, lead_alone) = (setup(true), setup(false));
    assert!(lead_alone.iter().any(|&s| s != 0.0));
    assert_eq!(soloed, lead_alone);
}

#[test]
fn undoing_a_removed_node_brings_its_sound_back() {
    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    let before = play(engine, processor, VARIED_BLOCKS);

    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    apply(&mut engine, Edit::RemoveNode { id: FIRST_NODE });
    engine.apply(Command::Undo).unwrap();
    assert_eq!(play(engine, processor, VARIED_BLOCKS), before);
}

#[test]
fn tree_output_is_identical_for_any_block_size() {
    let render_with = |blocks: &[usize]| {
        let (mut engine, processor) = new_engine();
        add_folder(&mut engine);
        add_lead(&mut engine, FOLDER);
        hold(&mut engine, FIRST_NODE, 1, 60);
        hold(&mut engine, LEAD, 2, 67);
        play(engine, processor, blocks)
    };
    let reference = render_with(&[FRAMES]);
    for blocks in [&[1][..], &[64][..], &[1000][..], VARIED_BLOCKS] {
        assert_eq!(render_with(blocks), reference, "blocks {blocks:?}");
    }
}

#[test]
fn an_effect_switched_off_passes_sound_untouched() {
    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    let dry = play(engine, processor, VARIED_BLOCKS);

    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    let filter = daw_engine::DeviceId(5);
    apply(
        &mut engine,
        Edit::AddEffect {
            node: FIRST_NODE,
            id: filter,
            index: 0,
            effect: daw_engine::Effect::new(daw_engine::EffectKind::Filter),
        },
    );
    apply(
        &mut engine,
        Edit::SetDeviceEnabled {
            id: filter,
            enabled: false,
        },
    );
    assert_eq!(play(engine, processor, VARIED_BLOCKS), dry);
}

#[test]
fn layered_instruments_all_play_every_note() {
    let (mut engine, processor) = new_engine();
    hold(&mut engine, FIRST_NODE, 1, 60);
    let one = play(engine, processor, VARIED_BLOCKS);

    let layered = |enabled: bool| {
        let (mut engine, processor) = new_engine();
        hold(&mut engine, FIRST_NODE, 1, 60);
        apply(
            &mut engine,
            Edit::AddInstrument {
                node: FIRST_NODE,
                id: DeviceId(9),
                index: 1,
                instrument: daw_engine::Instrument::new(daw_engine::InstrumentKind::Synth),
            },
        );
        if !enabled {
            apply(
                &mut engine,
                Edit::SetDeviceEnabled {
                    id: DeviceId(9),
                    enabled: false,
                },
            );
        }
        play(engine, processor, VARIED_BLOCKS)
    };
    let doubled: Vec<f32> = one.iter().map(|s| s * 2.).collect();
    assert_close(&layered(true), &doubled);
    assert_eq!(layered(false), one, "a layer switched off is silent");
}

#[test]
fn previewed_notes_sound_while_stopped_and_stop_when_released() {
    let (mut engine, mut processor) = new_engine();
    let preview = |on: bool| Command::PreviewNote {
        node: FIRST_NODE,
        pitch: 60,
        velocity: 100,
        on,
    };
    engine.apply(preview(true)).unwrap();
    let held = render(&mut engine, &mut processor, 4800, VARIED_BLOCKS);
    assert!(
        held.samples.iter().any(|&s| s != 0.0),
        "heard while stopped"
    );
    engine.apply(preview(false)).unwrap();
    // The release fades out; well after it, silence.
    render(&mut engine, &mut processor, 48_000, VARIED_BLOCKS);
    let after = render(&mut engine, &mut processor, 4800, VARIED_BLOCKS);
    assert!(after.samples.iter().all(|&s| s == 0.0));
    assert!(
        engine
            .apply(Command::PreviewNote {
                node: NodeId(99),
                pitch: 60,
                velocity: 100,
                on: true
            })
            .is_err()
    );
}

#[test]
fn meters_report_each_node() {
    let (mut engine, mut processor) = new_engine();
    add_folder(&mut engine);
    add_lead(&mut engine, FOLDER);
    hold(&mut engine, LEAD, 1, 67);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, FRAMES, VARIED_BLOCKS);
    let meters = engine.take_meters();
    assert!(meters[&LEAD].left > 0.0);
    assert!(meters[&FOLDER].left > 0.0, "carries the lead");
    assert_eq!(meters[&FIRST_NODE].left, 0.0, "plays nothing");
    assert!(meters[&NodeId::ROOT].left > 0.0);
}
