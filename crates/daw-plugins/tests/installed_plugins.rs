//! Installed commercial plugins in a host process: opt-in, since it needs
//! them on the machine (`cargo test -p daw-plugins --test installed_plugins
//! -- --ignored --nocapture`). Each listed effect that is installed runs
//! two seconds of noise; every block must finish in time with sound that
//! is finite.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::block::{Context, Layout, NoteEvent, Outcome, ParamChange};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::{BendRoute, PluginSpec};
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
        if let Err(e) = host.create(id, spec, None) {
            failures.push(format!("{name}: create: {e}"));
            continue;
        }
        // Their automatable parameters, as "+ Add Parameter" lists them.
        match host.params(id) {
            Ok(params) => {
                let groups: std::collections::BTreeSet<&str> =
                    params.iter().map(|p| p.group.as_str()).collect();
                let first: Vec<String> = params
                    .iter()
                    .take(4)
                    .map(|p| format!("{} [{}] {:.2}", p.name, p.group, p.value))
                    .collect();
                println!(
                    "{name}: {} parameters in {} groups, first {first:?}",
                    params.len(),
                    groups.len()
                );
            }
            Err(e) => failures.push(format!("{name}: params: {e}")),
        }
        let mut block = match host.activate(id, layout, 48_000.) {
            Ok(block) => block,
            Err(e) => {
                failures.push(format!("{name}: activate: {e}"));
                continue;
            }
        };
        let load = started.elapsed();
        // Its window opens (briefly, on screen) and closes.
        let opened = Instant::now();
        match host.open_editor(id, &format!("{name} — test"), None) {
            Ok((w, h)) => println!(
                "{name}: window {w} x {h}, opened in {:.0} ms",
                opened.elapsed().as_secs_f64() * 1000.
            ),
            Err(e) => failures.push(format!("{name}: window: {e}")),
        }
        if let Err(e) = host.close_editor(id) {
            failures.push(format!("{name}: closing its window: {e}"));
        }
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

/// Instruments to try, by name, if installed.
const INSTRUMENTS: &[&str] = &["Serum 2"];

/// Synths to survey for how they take bends.
const BEND_SURVEY: &[&str] = &[
    "Serum 2", "Serum", "Pigments", "DS Thorn", "Myth", "Synplant", "Diva", "Hive", "Zebra2",
    "Repro-5",
];

/// How installed synths take bent notes (`docs/design/note-bends.md`): the
/// route the host picks, parameters that look like a bend range, and a
/// measured full bend. Reports only; nothing fails on a synth's answers.
#[test]
#[ignore = "needs the user's installed plugins"]
fn installed_instruments_report_how_they_take_bends() {
    let scanner = ScanCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: vec![daw_plugins::scan::SCAN_FLAG.into()],
        env: Vec::new(),
    };
    let mut catalog = Catalog::default();
    catalog.update(&standard_folders(), &scanner, &Progress::default());
    let layout = Layout {
        max_frames: 512,
        inputs: 0,
        outputs: 2,
    };
    for name in BEND_SURVEY {
        let Some(plugin) = catalog
            .plugins()
            .into_iter()
            .find(|p| p.name == *name && p.is_instrument())
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
        let spec = PluginSpec::Vst3 {
            bundle: plugin.bundle.clone(),
            cid: plugin.cid.clone(),
            name: plugin.name.clone(),
            vendor: plugin.vendor.clone(),
        };
        if let Err(e) = host.create(id, spec, None) {
            println!("{name}: could not start: {e}");
            continue;
        }
        let (route, found) = host.bends(id).unwrap_or((BendRoute::None, String::new()));
        println!("{name}: route {}", route_name(&route));
        println!("{name}: found {}", short(&found));
        for p in host.params(id).unwrap_or_default() {
            let n = p.name.to_lowercase();
            let bendish = ["bend", "pb", "mpe", "range", "pitch"]
                .iter()
                .any(|w| n.contains(w));
            if bendish {
                println!(
                    "{name}:   param \"{}\" [{}] {:.3} unit \"{}\" steps {}",
                    p.name, p.group, p.value, p.unit, p.steps
                );
            }
        }
        let Ok(mut block) = host.activate(id, layout, 48_000.) else {
            println!("{name}: could not activate");
            continue;
        };
        // A C, then a C with a full upward bend: by tuning (per-note), by
        // the host's MPE channel at full travel (its 48), or the pitch bend
        // parameter at its top.
        let bend_param = match &route {
            BendRoute::Channel { param } => Some(*param),
            _ => None,
        };
        let mut rises = |note: i32, bent: bool| {
            let mut sound = Vec::new();
            for n in 0..(48_000 / 512) {
                let mut notes = Vec::new();
                let mut params = Vec::new();
                if n == 0 {
                    notes.push(NoteEvent::start(0, note, 60, 0.8));
                    if let Some(param) = bend_param {
                        let value = if bent { 1.0 } else { 0.5 };
                        params.push(ParamChange {
                            id: param,
                            offset: 0,
                            value,
                        });
                    } else if bent {
                        notes.push(NoteEvent::tune(0, note, 60, 48.));
                    }
                }
                if n == 46 {
                    notes.push(NoteEvent::end(0, note, 60));
                }
                let (mut l, mut r) = (vec![0.; 512], vec![0.; 512]);
                block.process_with_notes(
                    &[],
                    &params,
                    &notes,
                    Context {
                        tempo_bpm: 120.,
                        position_beats: 0.,
                        playing: true,
                        offline: false,
                    },
                    &mut [&mut l, &mut r],
                    Some(Instant::now() + Duration::from_millis(500)),
                );
                if (10..38).contains(&n) {
                    sound.extend(l);
                }
            }
            sound.windows(2).filter(|w| w[0] <= 0. && w[1] > 0.).count()
        };
        let (plain, bent) = (rises(200, false), rises(201, true));
        if plain == 0 {
            println!("{name}: its first patch made no sound to measure");
        } else {
            let semitones = 12. * (bent as f64 / plain as f64).log2();
            println!("{name}: C rises {plain}, fully bent {bent}: about {semitones:+.1} semitones");
        }
        drop(block);
        let _ = host.destroy(id);
    }
}

fn route_name(route: &BendRoute) -> String {
    match route {
        BendRoute::NoteExpression => "per-note tuning (note expression)".into(),
        BendRoute::Mpe { .. } => "MPE (a pitch bend per channel)".into(),
        BendRoute::Channel { param } => format!("one pitch bend (parameter {param})"),
        BendRoute::None => "none".into(),
    }
}

/// The host's findings, with a long run of the same pitch bend parameter
/// shortened.
fn short(found: &str) -> String {
    found.chars().take(400).collect()
}

#[test]
#[ignore = "needs the user's installed plugins"]
fn installed_instruments_play_notes_in_a_host_process() {
    let scanner = ScanCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: vec![daw_plugins::scan::SCAN_FLAG.into()],
        env: Vec::new(),
    };
    let mut catalog = Catalog::default();
    catalog.update(&standard_folders(), &scanner, &Progress::default());
    let layout = Layout {
        max_frames: 512,
        inputs: 0,
        outputs: 2,
    };
    let mut failures = Vec::new();
    for name in INSTRUMENTS {
        let Some(plugin) = catalog
            .plugins()
            .into_iter()
            .find(|p| p.name == *name && p.is_instrument())
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
        if let Err(e) = host.create(id, spec, None) {
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
        match host.bends(id) {
            Ok((route, found)) => println!("{name}: bends by {route:?}; {found}"),
            Err(e) => failures.push(format!("{name}: bends: {e}")),
        }
        match host.open_editor(id, &format!("{name} — test"), None) {
            Ok((w, h)) => println!("{name}: window {w} x {h}"),
            Err(e) => failures.push(format!("{name}: window: {e}")),
        }
        let _ = host.close_editor(id);
        // A chord for one second, then a second of release.
        let (mut late, mut peak, mut finite) = (0, 0f32, true);
        for n in 0..(96_000 / 512) {
            let notes: Vec<NoteEvent> = [60u16, 64, 67]
                .iter()
                .enumerate()
                .filter_map(|(i, &pitch)| match n {
                    0 => Some(NoteEvent::start(0, i as i32, pitch, 0.8)),
                    93 => Some(NoteEvent::end(0, i as i32, pitch)),
                    _ => None,
                })
                .collect();
            let (mut l, mut r) = (vec![0.; 512], vec![0.; 512]);
            let outcome = block.process_with_notes(
                &[],
                &[],
                &notes,
                Context {
                    tempo_bpm: 120.,
                    position_beats: 0.,
                    playing: true,
                    offline: false,
                },
                &mut [&mut l, &mut r],
                Some(Instant::now() + Duration::from_millis(500)),
            );
            if outcome != Outcome::Done {
                late += 1;
            }
            finite &= l.iter().chain(&r).all(|s| s.is_finite());
            peak = l.iter().chain(&r).fold(peak, |p, s| p.max(s.abs()));
        }
        println!(
            "{name} ({}): loaded in {:.0} ms, late {late}, peak {peak:.3}",
            plugin.vendor,
            load.as_secs_f64() * 1000.
        );
        if late > 2 || !finite || peak == 0.0 {
            failures.push(format!("{name}: {late} late, finite {finite}, peak {peak}"));
        }
        // Bent notes (docs/design/note-bends.md): a C held for half a
        // second, then a C tuned up an octave; the second rises through
        // zero about twice as often.
        let mut cycles = |id: i32, tuning: Option<f32>| {
            let mut sound = Vec::new();
            for n in 0..(48_000 / 512) {
                let mut notes = Vec::new();
                if n == 0 {
                    notes.push(NoteEvent::start(0, id, 60, 0.8));
                    notes.extend(tuning.map(|t| NoteEvent::tune(0, id, 60, t)));
                }
                if n == 46 {
                    notes.push(NoteEvent::end(0, id, 60));
                }
                let (mut l, mut r) = (vec![0.; 512], vec![0.; 512]);
                block.process_with_notes(
                    &[],
                    &[],
                    &notes,
                    Context {
                        tempo_bpm: 120.,
                        position_beats: 0.,
                        playing: true,
                        offline: false,
                    },
                    &mut [&mut l, &mut r],
                    Some(Instant::now() + Duration::from_millis(500)),
                );
                // A tenth of a second in, past the attack, for 0.3 s.
                if (10..38).contains(&n) {
                    sound.extend(l);
                }
            }
            sound.windows(2).filter(|w| w[0] <= 0. && w[1] > 0.).count()
        };
        let (plain, octave) = (cycles(100, None), cycles(101, Some(12.)));
        let ratio = octave as f64 / plain.max(1) as f64;
        println!("{name}: a C rises {plain} times, tuned up an octave {octave} ({ratio:.2}x)");
        if !(1.8..2.2).contains(&ratio) {
            failures.push(format!(
                "{name}: an octave's tuning gave {ratio:.2}x the cycles"
            ));
        }
        drop(block);
        let _ = host.destroy(id);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
