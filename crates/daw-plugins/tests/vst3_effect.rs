//! A real VST3 plugin (this repo's test plugin) running in a host process,
//! alone and in a song.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

mod common;

use common::{GAIN_CID, install};
use daw_engine::{
    ClipId, Command, DeviceId, Edit, Effect, Engine, EngineConfig, FIRST_NODE, MidiClip, MidiNote,
    PluginSpec, Ticks,
};
use daw_plugins::block::{Context, Layout, Outcome, ParamChange};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_testkit::render;
use std::path::Path;
use std::time::{Duration, Instant};

// The realtime guard comes with `daw_testkit`.

fn host_command() -> HostCommand {
    HostCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: Vec::new(),
    }
}

fn gain(bundle: &Path) -> PluginSpec {
    PluginSpec::Vst3 {
        bundle: bundle.to_path_buf(),
        cid: GAIN_CID.into(),
        name: "Test Gain".into(),
        vendor: "DAW Tests".into(),
    }
}

#[test]
fn a_vst3_effect_processes_audio_and_parameters_in_a_host_process() {
    let (_, bundle) = install("vst3-alone");
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    host.create(id, gain(&bundle)).unwrap();
    let layout = Layout {
        max_frames: 512,
        inputs: 2,
        outputs: 2,
    };
    let mut block = host.activate(id, layout, 48_000.).unwrap();
    let input = vec![0.5; 256];
    let mut run = |params: &[ParamChange]| {
        let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
        let outcome = block.process(
            &[&input, &input],
            params,
            Context::default(),
            &mut [&mut l, &mut r],
            Some(Instant::now() + Duration::from_secs(2)),
        );
        (outcome, l[100], r[200])
    };
    assert_eq!(run(&[]), (Outcome::Done, 0.5, 0.5), "unity by default");
    let quieter = [ParamChange {
        id: 0,
        offset: 0,
        value: 0.25,
    }];
    assert_eq!(run(&quieter), (Outcome::Done, 0.25, 0.25), "gain 0.5");
    drop(block);
    host.deactivate(id).unwrap();
    host.destroy(id).unwrap();
}

#[test]
fn a_song_through_a_vst3_effect_sounds_exactly_as_without() {
    let (_, bundle) = install("vst3-song");
    let song = |effect: Option<PluginSpec>| {
        let (mut engine, mut processor) = Engine::new(EngineConfig::default());
        engine.set_plugin_host(host_command());
        let notes = [60, 67]
            .map(|pitch| MidiNote {
                start: Ticks::ZERO,
                length: Ticks::from_bars(2),
                pitch,
                velocity: 100,
            })
            .to_vec();
        engine
            .apply(Command::Edit(Edit::AddClip {
                node: FIRST_NODE,
                id: ClipId(1),
                clip: MidiClip {
                    start: Ticks::ZERO,
                    length: Ticks::from_bars(2),
                    notes,
                    looping: None,
                },
            }))
            .unwrap();
        if let Some(spec) = effect {
            engine
                .apply(Command::Edit(Edit::AddEffect {
                    node: FIRST_NODE,
                    id: DeviceId(900),
                    index: 0,
                    effect: Effect::plugin(spec),
                }))
                .unwrap();
        }
        engine.apply(Command::Play).unwrap();
        render(&mut engine, &mut processor, 24_000, &[480])
    };
    let plain = song(None);
    let through = song(Some(gain(&bundle)));
    assert!(!plain.is_silent());
    assert!(through == plain, "a unity VST3 gain changes nothing");
}
