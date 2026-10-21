//! The engine playing plugin effects in a real host process (the
//! `daw-plugin-host` binary, which only this package's tests can find).

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_engine::{
    AudioProcessor, ClipId, Command, DeviceId, Edit, Effect, Engine, EngineConfig, FIRST_NODE,
    MidiClip, MidiNote, PluginSpec, PluginState, TestPlugin, Ticks,
};
use daw_plugins::client::HostCommand;
use daw_testkit::{VARIED_BLOCKS, render};
use std::time::{Duration, Instant};

const PLUGIN: DeviceId = DeviceId(900);

fn host_command() -> HostCommand {
    HostCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: Vec::new(),
    }
}

/// The default song's synth playing a held chord, with `plugin` on it.
fn engine(plugin: Option<TestPlugin>) -> (Engine, AudioProcessor) {
    let (mut engine, processor) = Engine::new(EngineConfig::default());
    engine.set_plugin_host(host_command());
    let notes = [60, 64, 67]
        .map(|pitch| MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_bars(4),
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
                length: Ticks::from_bars(4),
                notes,
                looping: None,
            },
        }))
        .unwrap();
    if let Some(plugin) = plugin {
        add_plugin(&mut engine, plugin);
    }
    engine.apply(Command::Play).unwrap();
    (engine, processor)
}

fn add_plugin(engine: &mut Engine, plugin: TestPlugin) {
    engine
        .apply(Command::Edit(Edit::AddEffect {
            node: FIRST_NODE,
            id: PLUGIN,
            index: 0,
            effect: Effect::plugin(PluginSpec::Test(plugin)),
        }))
        .unwrap();
}

fn plugin_state(engine: &Engine) -> Option<PluginState> {
    let snapshot = engine.snapshot();
    snapshot
        .node(FIRST_NODE)?
        .devices
        .iter()
        .find(|d| d.device == daw_engine::DeviceRef::Effect(PLUGIN))?
        .plugin
        .as_ref()
        .map(|p| p.state.clone())
}

/// Waits (briefly) for the engine to see what its host did.
fn settle(engine: &mut Engine, until: impl Fn(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !until(engine) && Instant::now() < deadline {
        engine.maintain();
        std::hint::spin_loop();
    }
}

#[test]
fn a_plugin_effect_returns_the_nodes_sound_exactly() {
    let (mut plain, mut plain_processor) = engine(None);
    let expected = render(&mut plain, &mut plain_processor, 48_000, &[512]);
    let (mut with, mut with_processor) = engine(Some(TestPlugin::Gain));
    let through = render(&mut with, &mut with_processor, 48_000, &[512]);
    assert!(!expected.is_silent());
    assert!(
        through == expected,
        "a unity gain in another process changes nothing"
    );
    assert_eq!(plugin_state(&with), Some(PluginState::Running));
}

#[test]
fn a_plugin_effect_sounds_the_same_at_any_block_size() {
    let (mut a, mut pa) = engine(Some(TestPlugin::Gain));
    let reference = render(&mut a, &mut pa, 30_000, &[512]);
    let (mut b, mut pb) = engine(Some(TestPlugin::Gain));
    let varied = render(&mut b, &mut pb, 30_000, VARIED_BLOCKS);
    assert!(varied == reference);
}

#[test]
fn a_crashing_plugin_goes_silent_is_reported_and_reloads() {
    let (mut engine, mut processor) = engine(Some(TestPlugin::CrashAt(20)));
    let started = Instant::now();
    let before = render(&mut engine, &mut processor, 19 * 1024, &[1024]);
    assert!(!before.is_silent(), "plays until the crash");
    let after = render(&mut engine, &mut processor, 8 * 1024, &[1024]);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "never stalls on the crash"
    );
    let tail = &after.samples[after.samples.len() / 2..];
    assert!(tail.iter().all(|&s| s == 0.), "silent once crashed");
    settle(&mut engine, |e| {
        plugin_state(e) == Some(PluginState::Crashed)
    });
    assert_eq!(plugin_state(&engine), Some(PluginState::Crashed));

    engine.apply(Command::ReloadPlugins).unwrap();
    let reloaded = render(&mut engine, &mut processor, 8 * 1024, &[1024]);
    assert!(!reloaded.is_silent(), "plays again after Reload");
    assert_eq!(plugin_state(&engine), Some(PluginState::Running));
}

#[test]
fn live_a_slow_plugin_costs_at_most_the_deadline_and_is_reported_hung() {
    let (mut engine, mut processor) = engine(Some(TestPlugin::Slow { millis: 50 }));
    processor.set_offline(false);
    let mut out = vec![0.; 480 * 2];
    let mut slowest = Duration::ZERO;
    for _ in 0..100 {
        engine.maintain();
        let started = Instant::now();
        processor.process(&mut out);
        slowest = slowest.max(started.elapsed());
    }
    // A 480-frame block lasts 10 ms; the deadline is part of that.
    assert!(
        slowest < Duration::from_millis(15),
        "slowest block {slowest:?}"
    );
    assert!(out.iter().all(|&s| s == 0.), "late blocks are silent");
    settle(&mut engine, |e| plugin_state(e) == Some(PluginState::Hung));
    assert_eq!(plugin_state(&engine), Some(PluginState::Hung));
}

#[test]
fn removing_a_plugin_effect_and_undoing_it_restarts_it() {
    let (mut engine, mut processor) = engine(Some(TestPlugin::Gain));
    render(&mut engine, &mut processor, 4_096, &[512]);
    engine.apply(Command::Undo).unwrap();
    render(&mut engine, &mut processor, 4_096, &[512]);
    assert_eq!(plugin_state(&engine), None, "gone with its effect");
    engine.apply(Command::Redo).unwrap();
    let back = render(&mut engine, &mut processor, 4_096, &[512]);
    assert!(!back.is_silent());
    assert_eq!(plugin_state(&engine), Some(PluginState::Running));
}

#[test]
fn without_a_plugin_host_a_plugin_effect_is_silent_and_says_why() {
    let (mut engine, mut processor) = Engine::new(EngineConfig::default());
    add_plugin(&mut engine, TestPlugin::Gain);
    engine.apply(Command::Play).unwrap();
    render(&mut engine, &mut processor, 4_096, &[512]);
    assert!(matches!(
        plugin_state(&engine),
        Some(PluginState::Failed(_))
    ));
}
