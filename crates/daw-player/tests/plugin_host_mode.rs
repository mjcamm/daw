//! The executable itself serves plugins when started in plugin-host mode
//! (the app does the same through the same function).

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::block::{Context, Layout, Outcome};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::{PluginSpec, TestPlugin};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

#[test]
fn the_player_runs_as_a_plugin_host() {
    let mut host = HostProcess::spawn(&HostCommand {
        program: env!("CARGO_BIN_EXE_daw-player").into(),
        args: vec!["--plugin-host".into()],
    })
    .unwrap();
    let id = next_instance();
    host.create(id, PluginSpec::Test(TestPlugin::Gain), None)
        .unwrap();
    let layout = Layout {
        max_frames: 256,
        inputs: 2,
        outputs: 2,
    };
    let mut block = host.activate(id, layout, 48_000.).unwrap();
    let input = vec![0.5; 128];
    let (mut l, mut r) = (vec![0.; 128], vec![0.; 128]);
    let outcome = block.process(
        &[&input, &input],
        &[],
        Context::default(),
        &mut [&mut l, &mut r],
        Some(Instant::now() + Duration::from_secs(1)),
    );
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(l[0], 0.5);
}
