//! The engine playing plugin effects in a real host process (the
//! `daw-plugin-host` binary, which only this package's tests can find).

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_engine::{
    AudioProcessor, BlendMode, Block, BlockId, ClipId, Command, DeviceId, DeviceRef, Edit, Effect,
    Engine, EngineConfig, FIRST_NODE, Layer, LayerId, MidiClip, MidiNote, ParamId, ParamState,
    PluginParam, PluginSpec, PluginState, Shape, TestPlugin, Ticks,
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

#[test]
fn a_plugin_parameter_follows_its_resting_value_and_its_layers() {
    let (mut plain, mut plain_processor) = engine(None);
    let expected = render(&mut plain, &mut plain_processor, 96_000, &[512]);
    let (mut engine, mut processor) = engine(Some(TestPlugin::Gain));
    let listed = engine.snapshot().node(FIRST_NODE).unwrap().devices[1]
        .plugin
        .clone()
        .unwrap()
        .params;
    assert_eq!(
        listed
            .iter()
            .map(|p| (p.id, p.name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "Gain")]
    );

    // Gain = 2 × value: resting at 0.25 halves the sound; from beat 2 a
    // layer multiplies it down to nothing.
    let gain = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Effect(PLUGIN),
        index: 0,
    };
    for edit in [
        Edit::AddPluginParam {
            device: PLUGIN,
            param: PluginParam {
                id: 0,
                name: "Gain".into(),
                state: ParamState {
                    base: 0.25,
                    layers: Vec::new(),
                },
            },
        },
        Edit::AddLayer {
            param: gain,
            id: LayerId(1),
            index: 0,
            layer: Layer::new(BlendMode::Multiply),
        },
        Edit::AddBlock {
            layer: LayerId(1),
            id: BlockId(1),
            block: Block::new(
                Ticks::from_beats(2),
                Ticks::from_beats(2),
                Shape::constant(0.0),
            ),
        },
    ] {
        engine.apply(Command::Edit(edit)).unwrap();
    }
    let out = render(&mut engine, &mut processor, 96_000, &[512]);
    // Beat 2 (frame 48 000) falls inside the block from frame 47 616; the
    // test plugin takes a block's last value for the whole block.
    let halves = |range: std::ops::Range<usize>| {
        out.samples[range.clone()]
            .iter()
            .zip(&expected.samples[range])
            .all(|(o, e)| *o == e * 0.5)
    };
    assert!(halves(0..47_616 * 2), "rests at half");
    assert!(
        out.samples[48_128 * 2..].iter().all(|&s| s == 0.0),
        "the layer pulls it to nothing"
    );
    assert!(!expected.samples[48_128 * 2..].iter().all(|&s| s == 0.0));
}

/// A second node ("Copy", node 2) playing the same chord on its own synth.
fn add_copy(engine: &mut Engine) {
    let mut node = daw_engine::Node::new("Copy", 1).with_instrument(DeviceId(50));
    let notes = [60, 64, 67]
        .map(|pitch| MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_bars(4),
            pitch,
            velocity: 100,
        })
        .to_vec();
    node.clips.insert(
        ClipId(2),
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_bars(4),
            notes,
            looping: None,
        },
    );
    engine
        .apply(Command::Edit(Edit::AddNode {
            id: daw_engine::NodeId(2),
            parent: daw_engine::NodeId::ROOT,
            index: 1,
            node,
        }))
        .unwrap();
}

#[test]
fn a_delaying_plugin_lines_the_song_up_late_by_its_latency() {
    let (mut plain, mut plain_processor) = engine(None);
    add_copy(&mut plain);
    let expected = render(&mut plain, &mut plain_processor, 20_000, &[512]);
    assert_eq!(plain_processor.latency(), 0);

    let (mut delayed, mut processor) = engine(Some(TestPlugin::Delay { frames: 100 }));
    add_copy(&mut delayed);
    let out = render(&mut delayed, &mut processor, 20_000, &[512]);
    assert_eq!(processor.latency(), 100, "the song's latency");
    assert!(out.samples[..200].iter().all(|&s| s == 0.0), "late by 100");
    assert!(
        out.samples[200..] == expected.samples[..expected.samples.len() - 200],
        "both nodes line up, exactly 100 frames late"
    );

    let (mut again, mut again_processor) = engine(Some(TestPlugin::Delay { frames: 100 }));
    add_copy(&mut again);
    let varied = render(&mut again, &mut again_processor, 20_000, VARIED_BLOCKS);
    assert!(varied == out, "the same at any block size");
}

#[test]
fn automation_acts_on_the_sound_it_was_drawn_against() {
    use daw_engine::{BlendMode, Block, BlockId, DeviceRef, Layer, LayerId, ParamId, Shape};
    let (mut engine, mut processor) = engine(Some(TestPlugin::Delay { frames: 1_000 }));
    // The volume drops to nothing from beat 1 (frame 24 000).
    let volume = ParamId {
        node: FIRST_NODE,
        device: DeviceRef::Channel,
        index: 0,
    };
    for edit in [
        Edit::AddLayer {
            param: volume,
            id: LayerId(1),
            index: 0,
            layer: Layer::new(BlendMode::Multiply),
        },
        Edit::AddBlock {
            layer: LayerId(1),
            id: BlockId(1),
            block: Block::new(
                Ticks::from_beats(1),
                Ticks::from_beats(2),
                Shape::constant(0.0),
            ),
        },
    ] {
        engine.apply(Command::Edit(edit)).unwrap();
    }
    let out = render(&mut engine, &mut processor, 30_000, &[512]);
    let loud = |range: std::ops::Range<usize>| {
        out.samples[range.start * 2..range.end * 2]
            .iter()
            .any(|&s| s.abs() > 1e-3)
    };
    // Beat 1's sound comes out at frame 25 000: the cut lands there, not
    // 1 000 frames early.
    assert!(
        loud(24_100..24_900),
        "still sounding until its sound is due"
    );
    assert!(!loud(25_100..30_000), "cut from beat 1's sound on");
    // The playhead shows what is heard.
    let heard = (30_000.0 - 1_000.0) / 24_000.0;
    assert!((engine.position_beats() - heard).abs() < 1e-9);
}
