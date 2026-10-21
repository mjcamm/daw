//! The app and the player are their own plugin hosts and scanners: started
//! with [`HOST_FLAG`] the executable serves plugins, with
//! [`SCAN_FLAG`]` <bundle>` it scans one plugin, instead of opening as usual
//! (`docs/design/vst3-hosting.md`).

use crate::projects::app_config_dir;
use daw_plugins::client::HostCommand;
use daw_plugins::scan::{Catalog, Progress, ScanCommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The first argument that starts this executable as a plugin host.
pub const HOST_FLAG: &str = "--plugin-host";
pub use daw_plugins::scan::SCAN_FLAG;

/// How to start a plugin host: this executable, in plugin-host mode.
pub fn host_command() -> Option<HostCommand> {
    Some(HostCommand {
        program: std::env::current_exe().ok()?,
        args: vec![HOST_FLAG.to_owned()],
    })
}

/// How to scan a plugin: this executable, in scan mode.
pub fn scan_command() -> Option<ScanCommand> {
    Some(ScanCommand {
        program: std::env::current_exe().ok()?,
        args: vec![SCAN_FLAG.to_owned()],
        env: Vec::new(),
    })
}

/// Serves plugins or scans one if this process was started to, returning
/// its exit code; `None` otherwise. Call first thing in `main`.
pub fn run_if_host() -> Option<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).take(2).collect();
    match args.first().map(String::as_str) {
        Some(HOST_FLAG) => Some(daw_plugins::host::run()),
        Some(SCAN_FLAG) => Some(match args.get(1) {
            Some(bundle) => daw_plugins::scan::run_scan(bundle.as_ref()),
            None => ExitCode::FAILURE,
        }),
        _ => None,
    }
}

/// Where the plugin catalog is kept between runs.
pub fn catalog_path() -> PathBuf {
    app_config_dir().join("plugins.json")
}

/// The saved catalog brought up to date with the standard folders and
/// `extra` ones, then saved, counting in `progress`.
pub fn update_catalog(extra: &[PathBuf], progress: &Progress) -> Result<Catalog, String> {
    let scanner = scan_command().ok_or("could not find this program to scan with")?;
    let mut folders = daw_plugins::scan::standard_folders();
    folders.extend_from_slice(extra);
    let path = catalog_path();
    let mut catalog = Catalog::load(&path);
    catalog.update(&folders, &scanner, progress);
    catalog.save(&path).map_err(|e| e.to_string())?;
    Ok(catalog)
}

/// Forgets a bundle's saved result, so the next update scans it again.
pub fn forget_bundle(bundle: &Path) -> std::io::Result<()> {
    let path = catalog_path();
    let mut catalog = Catalog::load(&path);
    catalog.retry(bundle);
    catalog.save(&path)
}
