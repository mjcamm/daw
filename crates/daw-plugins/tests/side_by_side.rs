//! Blocks handed to several instances at once ([`DawBlock::send`], then
//! [`DawBlock::finish`] for each) run side by side in their host process,
//! so a block costs the slowest instance, not the sum.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::block::{Context, DawBlock, Layout, Outcome};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
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

fn instance(host: &mut HostProcess, device: TestPlugin) -> DawBlock {
    let id = next_instance();
    host.create(id, PluginSpec::Test(device), None).unwrap();
    host.activate(id, LAYOUT, 48_000.).unwrap()
}

fn deadline(ms: u64) -> Option<Instant> {
    Some(Instant::now() + Duration::from_millis(ms))
}

/// Sends a block of `value`s.
fn send(block: &mut DawBlock, value: f32) -> Result<(), Outcome> {
    let input = [value; 256];
    block.send(256, &[&input, &input], &[], &[], Context::default())
}

/// Finishes the block sent: the outcome and the left output's first sample.
fn finish(block: &mut DawBlock, ms: u64) -> (Outcome, f32) {
    let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
    let outcome = block.finish(&mut [&mut l, &mut r], deadline(ms));
    (outcome, l[0])
}

#[test]
fn blocks_sent_together_run_side_by_side() {
    let mut host = host();
    let mut blocks: Vec<DawBlock> = (0..3)
        .map(|_| instance(&mut host, TestPlugin::Slow { millis: 30 }))
        .collect();
    // One block each first, so the host's audio threads are up and warm.
    for block in &mut blocks {
        send(block, 0.0).unwrap();
        assert_eq!(finish(block, 1_000).0, Outcome::Done);
    }
    let started = Instant::now();
    for (i, block) in blocks.iter_mut().enumerate() {
        send(block, i as f32 + 1.0).unwrap();
    }
    for (i, block) in blocks.iter_mut().enumerate() {
        assert_eq!(finish(block, 1_000), (Outcome::Done, i as f32 + 1.0));
    }
    let took = started.elapsed();
    // One after another they would take 90 ms.
    assert!(took < Duration::from_millis(75), "{took:?}");
}

#[test]
fn a_late_instance_does_not_hold_up_the_others() {
    let mut host = host();
    let mut slow = instance(&mut host, TestPlugin::Slow { millis: 200 });
    let mut quick = instance(&mut host, TestPlugin::Gain);
    let started = Instant::now();
    send(&mut slow, 1.0).unwrap();
    send(&mut quick, 0.5).unwrap();
    assert_eq!(finish(&mut quick, 1_000), (Outcome::Done, 0.5));
    assert_eq!(finish(&mut slow, 10).0, Outcome::Late);
    assert!(started.elapsed() < Duration::from_millis(100));
    // Still on that block: the next is refused, without waiting.
    assert_eq!(send(&mut slow, 1.0), Err(Outcome::Busy));
    assert_eq!(slow.stats().missed_total(), 2);
    assert_eq!(quick.stats().done_total(), 1);
}

#[test]
fn finishing_with_nothing_sent_is_busy() {
    let mut host = host();
    let mut block = instance(&mut host, TestPlugin::Gain);
    assert_eq!(finish(&mut block, 10).0, Outcome::Busy);
    send(&mut block, 1.0).unwrap();
    assert_eq!(finish(&mut block, 1_000), (Outcome::Done, 1.0));
    assert_eq!(finish(&mut block, 10).0, Outcome::Busy, "finished once");
}

#[test]
fn nothing_is_sent_to_a_dead_host() {
    let mut host = host();
    let mut block = instance(&mut host, TestPlugin::Gain);
    host.kill();
    assert_eq!(send(&mut block, 1.0), Err(Outcome::Dead));
}
