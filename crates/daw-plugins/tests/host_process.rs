//! A real host process, started from the `daw-plugin-host` binary.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::block::{Context, Layout, Outcome, ParamChange};
use daw_plugins::client::{HostCommand, HostError, HostProcess, next_instance};
use daw_plugins::control::{PluginSpec, TestPlugin};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

const LAYOUT: Layout = Layout {
    max_frames: 512,
    inputs: 2,
    outputs: 2,
};

fn host() -> HostProcess {
    HostProcess::spawn(&HostCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: Vec::new(),
    })
    .unwrap()
}

fn deadline(ms: u64) -> Option<Instant> {
    Some(Instant::now() + Duration::from_millis(ms))
}

/// One block of ones through `block`; the outcome and the left output.
fn run(
    block: &mut daw_plugins::block::DawBlock,
    params: &[ParamChange],
    ms: u64,
) -> (Outcome, f32) {
    let input = vec![1.; 256];
    let (mut l, mut r) = (vec![0.; 256], vec![0.; 256]);
    let outcome = block.process(
        &[&input, &input],
        params,
        Context::default(),
        &mut [&mut l, &mut r],
        deadline(ms),
    );
    (outcome, l[0])
}

#[test]
fn a_device_in_a_host_process_processes_audio_and_parameters() {
    let mut host = host();
    let id = next_instance();
    host.create(id, PluginSpec::Test(TestPlugin::Gain)).unwrap();
    let mut block = host.activate(id, LAYOUT, 48_000.).unwrap();
    assert_eq!(run(&mut block, &[], 1_000), (Outcome::Done, 1.));
    let half = [ParamChange {
        id: 0,
        offset: 0,
        value: 0.25,
    }];
    assert_eq!(run(&mut block, &half, 1_000), (Outcome::Done, 0.5));
    drop(block);
    host.deactivate(id).unwrap();
    host.destroy(id).unwrap();
}

#[test]
fn a_crashing_device_takes_only_its_host_process_down() {
    let mut host = host();
    let id = next_instance();
    host.create(id, PluginSpec::Test(TestPlugin::CrashAt(3)))
        .unwrap();
    let mut block = host.activate(id, LAYOUT, 48_000.).unwrap();
    assert_eq!(run(&mut block, &[], 1_000).0, Outcome::Done);
    assert_eq!(run(&mut block, &[], 1_000).0, Outcome::Done);
    // The third block crashes the host: late or dead, never a hang.
    let started = Instant::now();
    let (crashed, _) = run(&mut block, &[], 200);
    assert!(
        matches!(crashed, Outcome::Late | Outcome::Dead),
        "{crashed:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(400));
    let until = Instant::now() + Duration::from_secs(5);
    while host.is_alive() && Instant::now() < until {
        std::hint::spin_loop();
    }
    assert!(!host.is_alive(), "the crash is noticed");
    assert_eq!(run(&mut block, &[], 200).0, Outcome::Dead);
    assert!(matches!(host.destroy(id), Err(HostError::Gone)));
}

#[test]
fn a_hung_device_costs_one_deadline_then_blocks_are_skipped_until_killed() {
    let mut host = host();
    let id = next_instance();
    host.create(id, PluginSpec::Test(TestPlugin::HangAt(2)))
        .unwrap();
    let mut block = host.activate(id, LAYOUT, 48_000.).unwrap();
    assert_eq!(run(&mut block, &[], 1_000).0, Outcome::Done);
    let started = Instant::now();
    assert_eq!(run(&mut block, &[], 5).0, Outcome::Late);
    assert!(
        started.elapsed() < Duration::from_millis(30),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        run(&mut block, &[], 5).0,
        Outcome::Busy,
        "no waiting on a hung host"
    );
    assert!(host.is_alive(), "hung, not crashed");
    host.kill();
    assert_eq!(run(&mut block, &[], 5).0, Outcome::Dead);
}

#[test]
fn calls_the_host_cannot_do_are_refused() {
    let mut host = host();
    assert!(matches!(
        host.deactivate(next_instance()),
        Err(HostError::Refused(_))
    ));
    let id = next_instance();
    host.create(id, PluginSpec::Test(TestPlugin::Gain)).unwrap();
    assert!(matches!(
        host.create(id, PluginSpec::Test(TestPlugin::Gain)),
        Err(HostError::Refused(_))
    ));
}

#[test]
fn several_instances_share_one_host_process() {
    let mut host = host();
    let (a, b) = (next_instance(), next_instance());
    host.create(a, PluginSpec::Test(TestPlugin::Gain)).unwrap();
    host.create(b, PluginSpec::Test(TestPlugin::Gain)).unwrap();
    let mut block_a = host.activate(a, LAYOUT, 48_000.).unwrap();
    let mut block_b = host.activate(b, LAYOUT, 48_000.).unwrap();
    let quiet = [ParamChange {
        id: 0,
        offset: 0,
        value: 0.1,
    }];
    assert_eq!(run(&mut block_a, &quiet, 1_000), (Outcome::Done, 0.2));
    assert_eq!(
        run(&mut block_b, &[], 1_000),
        (Outcome::Done, 1.),
        "each keeps its own state"
    );
}

#[test]
fn dropping_the_host_process_ends_it() {
    let host = host();
    let pid = host.pid();
    drop(host);
    let listed = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(!text.contains(&pid.to_string()), "{text}");
}
