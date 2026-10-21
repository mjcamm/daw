//! Installed commercial plugins in a host process: opt-in, since it needs
//! them on the machine (`cargo test -p daw-plugins --test installed_plugins
//! -- --ignored --nocapture`). Each listed effect that is installed runs
//! two seconds of noise; every block must finish in time with sound that
//! is finite.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::block::{Context, Layout, Outcome};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::PluginSpec;
use daw_plugins::scan::{Catalog, Progress, ScanCommand, standard_folders};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

/// Effects to try, by name, if installed.
const EFFECTS: &[&str] = &[
    "OTT",
    "ValhallaRoom",
    "ValhallaDelay",
    "Serum 2 FX",
    "Filtron",
    "smartEQ3",
    "Cassette",
];

#[test]
#[ignore = "needs the user's installed plugins"]
fn installed_effects_process_audio_in_a_host_process() {
    let scanner = ScanCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: vec![daw_plugins::scan::SCAN_FLAG.into()],
        env: Vec::new(),
    };
    let mut catalog = Catalog::default();
    catalog.update(&standard_folders(), &scanner, &Progress::default());
    let layout = Layout {
        max_frames: 512,
        inputs: 2,
        outputs: 2,
    };
    let mut failures = Vec::new();
    for name in EFFECTS {
        let Some(plugin) = catalog
            .plugins()
            .into_iter()
            .find(|p| p.name == *name && !p.is_instrument())
            .cloned()
        else {
            println!("{name}: not installed");
            continue;
        };
        let mut host = HostProcess::spawn(&HostCommand {
            program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
            args: Vec::new(),
        })
        .unwrap();
        let id = next_instance();
        let started = Instant::now();
        let spec = PluginSpec::Vst3 {
            bundle: plugin.bundle.clone(),
            cid: plugin.cid.clone(),
            name: plugin.name.clone(),
            vendor: plugin.vendor.clone(),
        };
        if let Err(e) = host.create(id, spec) {
            failures.push(format!("{name}: create: {e}"));
            continue;
        }
        let mut block = match host.activate(id, layout, 48_000.) {
            Ok(block) => block,
            Err(e) => {
                failures.push(format!("{name}: activate: {e}"));
                continue;
            }
        };
        let load = started.elapsed();
        // Two seconds of noise in 512-frame blocks.
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let (mut late, mut peak, mut finite, mut slowest) = (0, 0f32, true, Duration::ZERO);
        for _ in 0..(96_000 / 512) {
            let input: Vec<f32> = (0..512).map(|_| noise() * 0.5).collect();
            let (mut l, mut r) = (vec![0.; 512], vec![0.; 512]);
            let t = Instant::now();
            let outcome = block.process(
                &[&input, &input],
                &[],
                Context {
                    tempo_bpm: 120.,
                    position_beats: 0.,
                    playing: true,
                    offline: false,
                },
                &mut [&mut l, &mut r],
                Some(Instant::now() + Duration::from_millis(500)),
            );
            slowest = slowest.max(t.elapsed());
            if outcome != Outcome::Done {
                late += 1;
            }
            finite &= l.iter().chain(&r).all(|s| s.is_finite());
            peak = l.iter().chain(&r).fold(peak, |p, s| p.max(s.abs()));
        }
        println!(
            "{name} ({}): loaded in {:.0} ms, slowest block {:.2} ms, late {late}, peak {peak:.3}",
            plugin.vendor,
            load.as_secs_f64() * 1000.,
            slowest.as_secs_f64() * 1000.
        );
        if late > 2 || !finite {
            failures.push(format!("{name}: {late} late blocks, finite {finite}"));
        }
        drop(block);
        let _ = host.destroy(id);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
