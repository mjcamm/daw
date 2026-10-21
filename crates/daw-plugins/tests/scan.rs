//! Scanning a real VST3 binary (this repo's test plugin) in a scanner
//! process, including one that crashes or hangs while loading.

#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use daw_plugins::scan::{Catalog, Progress, ScanCommand, scan_bundle};
use daw_plugins::sys::Job;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

fn scanner() -> ScanCommand {
    ScanCommand {
        program: env!("CARGO_BIN_EXE_daw-plugin-host").into(),
        args: vec![daw_plugins::scan::SCAN_FLAG.into()],
        env: Vec::new(),
    }
}

/// A scanner whose test plugin crashes or hangs while loading.
fn misbehaving(mode: &str) -> ScanCommand {
    ScanCommand {
        env: vec![("DAW_TEST_PLUGIN_SCAN".into(), mode.into())],
        ..scanner()
    }
}

/// The test plugin's DLL, built once into its own target folder (a nested
/// build there never waits on the outer one's lock).
fn test_plugin_dll() -> &'static Path {
    static DLL: OnceLock<PathBuf> = OnceLock::new();
    DLL.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = workspace.join("target").join("test-plugin");
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "-p", "daw-test-plugin", "--target-dir"])
            .arg(&target)
            .current_dir(&workspace)
            .status()
            .unwrap();
        assert!(status.success(), "building the test plugin failed");
        target.join("debug").join("daw_test_plugin.dll")
    })
}

/// A bundle folder holding the test plugin, in a fresh folder.
fn install(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("daw-scan-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundle = root.join("Test Plugin.vst3");
    let bin = bundle.join("Contents").join("x86_64-win");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(test_plugin_dll(), bin.join("Test Plugin.vst3")).unwrap();
    (root, bundle)
}

#[test]
fn a_plugin_is_loaded_in_a_scanner_process_and_its_classes_read() {
    let (_, bundle) = install("ok");
    let found = scan_bundle(
        &scanner(),
        &bundle,
        Duration::from_secs(30),
        &Job::new().unwrap(),
    )
    .unwrap();
    let names: Vec<(&str, bool)> = found
        .iter()
        .map(|p| (p.name.as_str(), p.is_instrument()))
        .collect();
    assert_eq!(names, [("Test Gain", false), ("Test Synth", true)]);
    assert!(found.iter().all(|p| p.vendor == "DAW Tests"));
    assert_eq!(found[0].subcategories, ["Fx", "Dynamics"]);
    assert_eq!(found[0].cid, "444157544741494E0000000000000001");
}

#[test]
fn a_plugin_that_crashes_while_loading_is_a_failure_not_a_crash() {
    let (_, bundle) = install("crash");
    let result = scan_bundle(
        &misbehaving("crash"),
        &bundle,
        Duration::from_secs(30),
        &Job::new().unwrap(),
    );
    assert!(result.unwrap_err().contains("crashed"));
}

#[test]
fn a_plugin_that_hangs_while_loading_times_out() {
    let (_, bundle) = install("hang");
    let started = Instant::now();
    let result = scan_bundle(
        &misbehaving("hang"),
        &bundle,
        Duration::from_secs(2),
        &Job::new().unwrap(),
    );
    assert!(result.unwrap_err().contains("did not finish"));
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_catalog_scans_new_bundles_once() {
    let (root, _) = install("catalog");
    let mut catalog = Catalog::default();
    let progress = Progress::default();
    catalog.update(std::slice::from_ref(&root), &scanner(), &progress);
    assert_eq!(catalog.plugins().len(), 2);
    assert_eq!(progress.done.into_inner(), 1, "one bundle scanned");
    assert_eq!(progress.total.into_inner(), 1);
    assert!(
        catalog.stale(std::slice::from_ref(&root)).is_empty(),
        "nothing left to scan"
    );
}
