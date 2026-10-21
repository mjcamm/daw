//! The test plugin (`daw-test-plugin`), built once per test binary and
//! installed as a bundle in a fresh folder.

#![allow(dead_code, reason = "each test binary uses some of these")]
#![allow(clippy::unwrap_used, reason = "tests fail on error")]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The test plugin's effect ("Test Gain"), in Steinberg's text form.
pub const GAIN_CID: &str = "444157544741494E0000000000000001";

/// The test plugin's DLL, built into its own target folder (a nested build
/// there never waits on the outer one's lock).
pub fn test_plugin_dll() -> &'static Path {
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

/// A bundle folder holding the test plugin, in a fresh folder: (the
/// folder, the bundle).
pub fn install(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("daw-plugin-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundle = root.join("Test Plugin.vst3");
    let bin = bundle.join("Contents").join("x86_64-win");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(test_plugin_dll(), bin.join("Test Plugin.vst3")).unwrap();
    (root, bundle)
}
