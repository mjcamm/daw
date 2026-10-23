//! A real VST3 plugin (this repo's test plugin) running in a host process,
//! alone and in a song.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

mod common;

use common::{GAIN_CID, install};
use daw_engine::AudioProcessor;
use daw_engine::{
    ClipId, Command, DeviceId, DeviceRef, Edit, Effect, Engine, EngineConfig, FIRST_NODE, MidiClip,
    MidiNote, ParamId, PluginData, PluginSpec, PluginWindow, ProjectFile, Ticks,
};
use daw_plugins::block::{Context, Layout, Outcome, ParamChange};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_testkit::{Rendered, render};
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
    host.create(id, gain(&bundle), None).unwrap();
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

/// The test plugin's state: its gain parameter, 8 bytes little-endian.
fn gain_state(value: f64) -> PluginData {
    PluginData {
        component: value.to_le_bytes().to_vec(),
        controller: Vec::new(),
    }
}

#[test]
fn a_vst3_effect_starts_from_saved_state_and_reports_it_while_running() {
    let (_, bundle) = install("vst3-state");
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    host.create(id, gain(&bundle), Some(gain_state(0.25)))
        .unwrap();
    let layout = Layout {
        max_frames: 512,
        inputs: 2,
        outputs: 2,
    };
    let mut block = host.activate(id, layout, 48_000.).unwrap();
    let input = vec![0.5; 64];
    let mut run = |params: &[ParamChange]| {
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        block.process(
            &[&input, &input],
            params,
            Context::default(),
            &mut [&mut l, &mut r],
            Some(Instant::now() + Duration::from_secs(2)),
        );
        l[10]
    };
    assert_eq!(run(&[]), 0.25, "gain 0.5 from the saved state");
    assert_eq!(host.state(id).unwrap(), Some(gain_state(0.25)));
    let change = [ParamChange {
        id: 0,
        offset: 0,
        value: 0.375,
    }];
    run(&change);
    assert_eq!(
        host.state(id).unwrap(),
        Some(gain_state(0.375)),
        "read while its audio thread runs"
    );
    drop(block);
    host.destroy(id).unwrap();
}

#[test]
fn a_vst3_effect_lists_its_automatable_parameters_with_their_groups() {
    let (_, bundle) = install("vst3-params");
    let mut host = HostProcess::spawn(&host_command()).unwrap();
    let id = next_instance();
    host.create(id, gain(&bundle), Some(gain_state(0.25)))
        .unwrap();
    let params = host.params(id).unwrap();
    let listed: Vec<_> = params
        .iter()
        .map(|p| {
            (
                p.id,
                p.name.as_str(),
                p.group.as_str(),
                p.unit.as_str(),
                p.value,
            )
        })
        .collect();
    // The read-only Meter is not offered.
    assert_eq!(
        listed,
        [(0, "Gain", "", "x", 0.25), (7, "Tilt", "Tone", "dB", 0.0)]
    );
    host.destroy(id).unwrap();
}

/// A song of two notes, through `effect` (as effect 900) if given.
fn song(effect: Option<PluginSpec>) -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
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
                id: EFFECT,
                index: 0,
                effect: Effect::plugin(spec),
            }))
            .unwrap();
    }
    (engine, processor)
}

const EFFECT: DeviceId = DeviceId(900);

/// The song from the start.
fn play(engine: &mut Engine, processor: &mut AudioProcessor) -> Rendered {
    engine
        .apply(Command::Locate {
            position: Ticks::ZERO,
        })
        .unwrap();
    engine.apply(Command::Play).unwrap();
    render(engine, processor, 24_000, &[480])
}

#[test]
fn a_song_through_a_vst3_effect_sounds_exactly_as_without() {
    let (_, bundle) = install("vst3-song");
    let (mut engine, mut processor) = song(None);
    let plain = play(&mut engine, &mut processor);
    let (mut engine, mut processor) = song(Some(gain(&bundle)));
    let through = play(&mut engine, &mut processor);
    assert!(!plain.is_silent());
    assert!(through == plain, "a unity VST3 gain changes nothing");
}

#[test]
fn a_plugins_state_is_saved_in_the_song_and_survives_undo() {
    let (_, bundle) = install("vst3-song-state");
    let (mut engine, mut processor) = song(None);
    let plain = play(&mut engine, &mut processor);
    // Played again, the first notes' tails carry over.
    let plain_again = play(&mut engine, &mut processor);
    let half = |r: &Rendered| r.samples.iter().map(|s| s * 0.5).collect::<Vec<_>>();

    let (mut engine, _) = song(Some(gain(&bundle)));
    let named = engine.snapshot().node(FIRST_NODE).and_then(|n| {
        n.devices
            .iter()
            .find(|d| d.device == DeviceRef::Effect(EFFECT))
            .map(|d| d.name.clone())
    });
    assert_eq!(named.as_deref(), Some("Test Gain"), "shown by its own name");
    let mut file = engine.to_file();
    assert_eq!(
        file.plugin_states().get(&EFFECT),
        Some(&gain_state(0.5)),
        "asked of the running plugin"
    );
    file.set_plugin_states(&[(EFFECT, gain_state(0.25))].into());
    let json = file.to_json();

    let (mut loaded, mut processor) = Engine::new(EngineConfig::default());
    loaded.set_plugin_host(host_command());
    loaded.load(ProjectFile::from_json(&json).unwrap()).unwrap();
    assert_eq!(
        play(&mut loaded, &mut processor).samples,
        half(&plain),
        "the plugin starts from its saved state"
    );
    assert_eq!(
        loaded.to_file().plugin_states().get(&EFFECT),
        Some(&gain_state(0.25))
    );

    loaded
        .apply(Command::Edit(Edit::RemoveEffect { id: EFFECT }))
        .unwrap();
    loaded.apply(Command::Undo).unwrap();
    assert_eq!(
        play(&mut loaded, &mut processor).samples,
        half(&plain_again),
        "an effect brought back by undo keeps its state"
    );
}

/// Gain and Tilt as the song has them: (id, resting value, layers).
fn song_params(engine: &Engine) -> Vec<(u32, f64, usize)> {
    let snapshot = engine.snapshot();
    let device = snapshot
        .node(FIRST_NODE)
        .and_then(|n| {
            n.devices
                .iter()
                .find(|d| d.device == DeviceRef::Effect(EFFECT))
        })
        .cloned()
        .unwrap();
    device
        .params
        .iter()
        .map(|p| (p.id.index, p.base, p.layers.len()))
        .collect()
}

fn plugin_of(engine: &Engine) -> daw_engine::PluginSnapshot {
    engine
        .snapshot()
        .node(FIRST_NODE)
        .and_then(|n| {
            n.devices
                .iter()
                .find(|d| d.device == DeviceRef::Effect(EFFECT))
        })
        .and_then(|d| d.plugin.clone())
        .unwrap()
}

#[test]
fn learn_and_knob_turns_in_a_plugins_window_change_the_song() {
    use common::windows::{click, plugin_windows, view_of, wait_for};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_CLOSE, WM_LBUTTONDOWN, WM_MBUTTONDOWN};
    let (_, bundle) = install("vst3-learn");
    let (mut engine, _processor) = song(Some(gain(&bundle)));
    engine
        .apply(Command::OpenPluginWindow { effect: EFFECT })
        .unwrap();
    // Asked without waiting: it says Open once the host has opened it.
    wait_for("the window to open", || {
        engine.maintain();
        plugin_of(&engine).window == PluginWindow::Open
    });
    let node = engine.snapshot().node(FIRST_NODE).unwrap().name.clone();
    let title = format!("Test Gain — {node}");
    let found = plugin_windows()
        .into_iter()
        .filter(|(_, t)| *t == title)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "titled with the plugin, then its node");
    let view = view_of(found[0].0);

    // Renaming its node renames the window.
    engine
        .apply(Command::Edit(Edit::RenameNode {
            node: FIRST_NODE,
            name: "Lead".into(),
        }))
        .unwrap();
    wait_for("the new title", || {
        plugin_windows()
            .iter()
            .any(|(_, t)| t == "Test Gain — Lead")
    });
    // Where it is goes into the song, to reopen it there.
    wait_for("its place", || {
        engine.maintain();
        engine.to_file().window_places().contains_key(&EFFECT)
    });

    // Learn: the first knob turned (a gesture) joins the song with a layer.
    engine
        .apply(Command::LearnPluginParam {
            effect: Some(EFFECT),
        })
        .unwrap();
    assert!(plugin_of(&engine).learning);
    click(view, WM_LBUTTONDOWN);
    wait_for("Gain to be learned", || {
        engine.maintain();
        song_params(&engine) == [(0, 0.75, 1)]
    });
    assert!(!plugin_of(&engine).learning, "learning stops");

    // Turning it again moves its resting value: one undo.
    let gain_param = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Effect(EFFECT),
        index: 0,
    };
    engine
        .apply(Command::Edit(Edit::SetParam {
            param: gain_param,
            value: 0.2,
        }))
        .unwrap();
    click(view, WM_LBUTTONDOWN);
    wait_for("the turn", || {
        engine.maintain();
        song_params(&engine) == [(0, 0.75, 1)]
    });
    engine.apply(Command::Undo).unwrap();
    assert_eq!(song_params(&engine), [(0, 0.2, 1)], "one undo for the turn");

    // A move without begin and end (a mouse wheel) is learned once it
    // pauses.
    engine
        .apply(Command::LearnPluginParam {
            effect: Some(EFFECT),
        })
        .unwrap();
    click(view, WM_MBUTTONDOWN);
    wait_for("Tilt to be learned", || {
        engine.maintain();
        song_params(&engine).len() == 2
    });
    assert_eq!(song_params(&engine)[1], (7, 0.3, 1));

    // The user closes the window.
    click(found[0].0, WM_CLOSE);
    wait_for("the window to close", || {
        engine.maintain();
        plugin_of(&engine).window == PluginWindow::Closed
    });
}
