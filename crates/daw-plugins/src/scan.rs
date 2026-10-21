//! Finding VST3 plugins: bundles in the plugin folders, their classes from
//! `moduleinfo.json` when they ship one, else from loading them in a
//! scanner process (the host binary with `--plugin-scan <bundle>`), with a
//! timeout. A [`Catalog`] caches the results by each bundle's binary size
//! and modified time, so only new or changed plugins are scanned again.

#![allow(
    clippy::disallowed_types,
    reason = "control side only: scanner output is read on a thread with a deadline"
)]

use crate::sys::Job;
use crate::vst3::{Module, cid_string, parse_cid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// The argument that makes the host binary scan one bundle.
pub const SCAN_FLAG: &str = "--plugin-scan";

/// How long one bundle may take to load (copy protection can be slow).
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Bundles scanned at once.
const PARALLEL: usize = 4;

/// One plugin (an audio processor class) a bundle offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub bundle: PathBuf,
    /// Steinberg's text form ([`cid_string`]).
    pub cid: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// e.g. ["Instrument", "Synth"] or ["Fx", "Delay"].
    pub subcategories: Vec<String>,
}

impl PluginInfo {
    pub fn is_instrument(&self) -> bool {
        self.subcategories.iter().any(|s| s == "Instrument")
    }
}

/// How to start the scanner: `program args... <bundle>`, with `env` added
/// to its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The standard VST3 folders on this system.
pub fn standard_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            folders.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("VST3"),
            );
        }
        let common = std::env::var_os("CommonProgramFiles").map_or_else(
            || PathBuf::from(r"C:\Program Files\Common Files"),
            PathBuf::from,
        );
        folders.push(common.join("VST3"));
    } else if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            folders.push(PathBuf::from(home).join("Library/Audio/Plug-ins/VST3"));
        }
        folders.push("/Library/Audio/Plug-ins/VST3".into());
    } else {
        if let Some(home) = std::env::var_os("HOME") {
            folders.push(PathBuf::from(home).join(".vst3"));
        }
        folders.extend(["/usr/lib/vst3", "/usr/local/lib/vst3"].map(PathBuf::from));
    }
    folders
}

fn is_bundle_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
}

/// Every `.vst3` bundle (or legacy single file) under `folders`, searched
/// recursively but never inside a bundle. Sorted, without duplicates.
pub fn find_bundles(folders: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending: Vec<PathBuf> = folders.to_vec();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if is_bundle_name(&path) {
                found.push(path);
            } else if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(path);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The bundle's binary for this system: inside a bundle folder, or the
/// file itself for a legacy single-file plugin.
pub fn binary_path(bundle: &Path) -> Option<PathBuf> {
    if bundle.is_file() {
        return Some(bundle.to_path_buf());
    }
    let stem = bundle.file_stem()?;
    let (folder, file) = if cfg!(windows) {
        ("x86_64-win", Path::new(stem).with_extension("vst3"))
    } else if cfg!(target_os = "macos") {
        ("MacOS", PathBuf::from(stem))
    } else {
        ("x86_64-linux", Path::new(stem).with_extension("so"))
    };
    let path = bundle.join("Contents").join(folder).join(file);
    path.is_file().then_some(path)
}

/// Plugins from a `PluginInfo` list of classes: audio processors only.
fn plugins(
    bundle: &Path,
    classes: impl IntoIterator<Item = (String, String, String, String, String, String)>,
) -> Vec<PluginInfo> {
    classes
        .into_iter()
        .filter(|(_, category, ..)| category == "Audio Module Class")
        .map(
            |(cid, _, name, vendor, version, subcategories)| PluginInfo {
                bundle: bundle.to_path_buf(),
                cid,
                name,
                vendor,
                version,
                subcategories: subcategories
                    .split('|')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            },
        )
        .collect()
}

#[derive(Deserialize)]
struct ModuleInfo {
    #[serde(rename = "Factory Info", default)]
    factory: Option<FactoryInfo>,
    #[serde(rename = "Classes")]
    classes: Vec<ModuleClass>,
}

#[derive(Deserialize)]
struct FactoryInfo {
    #[serde(rename = "Vendor", default)]
    vendor: String,
}

#[derive(Deserialize)]
struct ModuleClass {
    #[serde(rename = "CID")]
    cid: String,
    #[serde(rename = "Category")]
    category: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Vendor", default)]
    vendor: String,
    #[serde(rename = "Version", default)]
    version: String,
    #[serde(rename = "Sub Categories", default)]
    subcategories: Vec<String>,
}

/// The plugins `moduleinfo.json` lists, without loading any code; `None`
/// if the bundle has none or it does not parse (it may be JSON5: then the
/// bundle is loaded instead).
pub fn read_module_info(bundle: &Path) -> Option<Vec<PluginInfo>> {
    let text = std::fs::read_to_string(bundle.join("Contents/Resources/moduleinfo.json")).ok()?;
    let info: ModuleInfo = serde_json::from_str(&text).ok()?;
    let factory_vendor = info.factory.map(|f| f.vendor).unwrap_or_default();
    let classes = info
        .classes
        .into_iter()
        .filter(|c| parse_cid(&c.cid).is_some())
        .map(|c| {
            let vendor = if c.vendor.is_empty() {
                factory_vendor.clone()
            } else {
                c.vendor
            };
            (
                c.cid.trim().to_uppercase(),
                c.category,
                c.name,
                vendor,
                c.version,
                c.subcategories.join("|"),
            )
        });
    Some(plugins(bundle, classes))
}

/// Scanner process: loads `bundle` and prints its plugins as JSON.
pub fn run_scan(bundle: &Path) -> ExitCode {
    crate::vst3::quiet_errors();
    let Some(binary) = binary_path(bundle) else {
        eprintln!("no binary for this system in {}", bundle.display());
        return ExitCode::FAILURE;
    };
    let module = match Module::load(&binary) {
        Ok(module) => module,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let classes = module.classes().into_iter().map(|c| {
        (
            cid_string(&c.cid),
            c.category,
            c.name,
            c.vendor,
            c.version,
            c.subcategories,
        )
    });
    let found = plugins(bundle, classes);
    drop(module);
    match serde_json::to_string(&found) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Loads `bundle` in a scanner process and returns its plugins, or why it
/// could not: it crashed, hung past `timeout`, or reported an error.
pub fn scan_bundle(
    command: &ScanCommand,
    bundle: &Path,
    timeout: Duration,
    job: &Job,
) -> Result<Vec<PluginInfo>, String> {
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .arg(bundle)
        .envs(command.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the scanner: {e}"))?;
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        return Err(format!("could not start the scanner: {e}"));
    }
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        return Err("the scanner has no output".into());
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut out, mut err) = (String::new(), String::new());
        let _ = stdout.read_to_string(&mut out);
        let _ = stderr.read_to_string(&mut err);
        let _ = sender.send((out, err));
    });
    let Ok((out, err)) = receiver.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "did not finish loading within {} s",
            timeout.as_secs()
        ));
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let why = err.lines().last().unwrap_or("").trim();
        return Err(if why.is_empty() {
            format!("crashed while loading ({status})")
        } else {
            why.to_owned()
        });
    }
    serde_json::from_str(out.trim()).map_err(|e| format!("the scanner's report did not read: {e}"))
}

/// How far an update has got, readable from another thread.
#[derive(Debug, Default)]
pub struct Progress {
    /// Bundles finished so far.
    pub done: AtomicUsize,
    /// Bundles this update scans (new or changed ones).
    pub total: AtomicUsize,
}

/// What identifies a bundle's version on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    size: u64,
    modified_secs: u64,
    modified_nanos: u32,
}

fn fingerprint(bundle: &Path) -> Option<Fingerprint> {
    let meta = std::fs::metadata(binary_path(bundle)?).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?;
    Some(Fingerprint {
        size: meta.len(),
        modified_secs: modified.as_secs(),
        modified_nanos: modified.subsec_nanos(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    fingerprint: Option<Fingerprint>,
    result: Result<Vec<PluginInfo>, String>,
}

/// Every plugin found, and every bundle that failed with why. Saved
/// between runs so unchanged bundles are never scanned again.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    version: u32,
    bundles: BTreeMap<PathBuf, Entry>,
}

/// Bump when a scan would now find something different for the same bundle.
const CATALOG_VERSION: u32 = 1;

impl Catalog {
    /// The saved catalog, or an empty one if there is none or it is from
    /// another version.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .filter(|c| c.version == CATALOG_VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Plugins of every bundle that scanned, by vendor then name.
    pub fn plugins(&self) -> Vec<&PluginInfo> {
        let mut all: Vec<&PluginInfo> = self
            .bundles
            .values()
            .filter_map(|e| e.result.as_ref().ok())
            .flatten()
            .collect();
        all.sort_by(|a, b| {
            (a.vendor.to_lowercase(), a.name.to_lowercase())
                .cmp(&(b.vendor.to_lowercase(), b.name.to_lowercase()))
        });
        all
    }

    /// Bundles that failed, with why.
    pub fn failures(&self) -> Vec<(&Path, &str)> {
        self.bundles
            .iter()
            .filter_map(|(path, e)| {
                e.result
                    .as_ref()
                    .err()
                    .map(|why| (path.as_path(), why.as_str()))
            })
            .collect()
    }

    /// Forgets a bundle's result, so the next update scans it again.
    pub fn retry(&mut self, bundle: &Path) {
        self.bundles.remove(bundle);
    }

    /// Bundles under `folders` that the next [`Catalog::update`] scans.
    pub fn stale(&self, folders: &[PathBuf]) -> Vec<PathBuf> {
        find_bundles(folders)
            .into_iter()
            .filter(|b| {
                self.bundles
                    .get(b)
                    .is_none_or(|e| e.fingerprint != fingerprint(b))
            })
            .collect()
    }

    /// Brings the catalog up to date with `folders`: drops bundles that are
    /// gone, scans new and changed ones (`moduleinfo.json` first, else a
    /// scanner process each, a few at a time), counting in `progress`.
    pub fn update(&mut self, folders: &[PathBuf], scanner: &ScanCommand, progress: &Progress) {
        self.version = CATALOG_VERSION;
        let present = find_bundles(folders);
        self.bundles
            .retain(|path, _| present.binary_search(path).is_ok());
        let stale = self.stale(folders);
        progress.done.store(0, Ordering::Relaxed);
        progress.total.store(stale.len(), Ordering::Relaxed);
        let job = Job::new().ok();
        let next = AtomicUsize::new(0);
        let results: Vec<(PathBuf, Entry)> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..PARALLEL.min(stale.len()))
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(bundle) = stale.get(i) else {
                                break;
                            };
                            let result = read_module_info(bundle).map_or_else(
                                || match &job {
                                    Some(job) => scan_bundle(scanner, bundle, SCAN_TIMEOUT, job),
                                    None => Err("could not start the scanner".into()),
                                },
                                Ok,
                            );
                            out.push((
                                bundle.clone(),
                                Entry {
                                    fingerprint: fingerprint(bundle),
                                    result,
                                },
                            ));
                            progress.done.fetch_add(1, Ordering::Relaxed);
                        }
                        out
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|w| w.join().unwrap_or_default())
                .collect()
        });
        self.bundles.extend(results);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests fail on error")]

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daw-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A bundle folder holding a (fake) Windows binary.
    fn bundle(root: &Path, name: &str) -> PathBuf {
        let bundle = root.join(format!("{name}.vst3"));
        let bin = bundle.join("Contents").join("x86_64-win");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(format!("{name}.vst3")), b"binary").unwrap();
        bundle
    }

    #[test]
    fn bundles_are_found_recursively_but_never_inside_a_bundle() {
        let root = temp("find");
        let a = bundle(&root, "Alpha");
        let b = bundle(&root.join("Vendor").join("Sub"), "Beta");
        // A bundle's own resources are not searched.
        bundle(&a.join("Contents").join("Resources"), "Inner");
        let legacy = root.join("Legacy.vst3");
        std::fs::write(&legacy, b"dll").unwrap();
        let found = find_bundles(&[root.clone(), root.join("missing")]);
        assert_eq!(found, {
            let mut v = vec![a.clone(), b, legacy.clone()];
            v.sort();
            v
        });
        if cfg!(windows) {
            assert_eq!(
                binary_path(&a),
                Some(a.join("Contents/x86_64-win/Alpha.vst3"))
            );
            assert_eq!(binary_path(&legacy), Some(legacy));
        }
    }

    #[test]
    fn moduleinfo_lists_audio_processors_with_the_factory_vendor() {
        let root = temp("moduleinfo");
        let b = bundle(&root, "Gamma");
        let resources = b.join("Contents").join("Resources");
        std::fs::create_dir_all(&resources).unwrap();
        std::fs::write(
            resources.join("moduleinfo.json"),
            r#"{
              "Name": "Gamma", "Version": "1.2",
              "Factory Info": { "Vendor": "Acme", "URL": "", "E-Mail": "" },
              "Classes": [
                { "CID": "444157544741494E0102030401020304", "Category": "Audio Module Class",
                  "Name": "Gamma Verb", "Version": "1.2", "Sub Categories": ["Fx", "Reverb"] },
                { "CID": "444157544741494E0102030401020305", "Category": "Component Controller Class",
                  "Name": "Gamma Verb Controller" }
              ]
            }"#,
        )
        .unwrap();
        let found = read_module_info(&b).unwrap();
        assert_eq!(found.len(), 1, "only the processor");
        assert_eq!(found[0].name, "Gamma Verb");
        assert_eq!(found[0].vendor, "Acme");
        assert_eq!(found[0].subcategories, ["Fx", "Reverb"]);
        assert!(!found[0].is_instrument());

        std::fs::write(resources.join("moduleinfo.json"), "{ // JSON5 comment\n }").unwrap();
        assert_eq!(read_module_info(&b), None, "unreadable: load it instead");
    }

    #[test]
    fn a_catalog_keeps_results_and_forgets_bundles_that_are_gone() {
        let root = temp("catalog");
        let b = bundle(&root, "Delta");
        let mut catalog = Catalog::default();
        catalog.bundles.insert(
            b.clone(),
            Entry {
                fingerprint: fingerprint(&b),
                result: Err("broken".into()),
            },
        );
        assert!(
            catalog.stale(std::slice::from_ref(&root)).is_empty(),
            "unchanged"
        );
        std::fs::write(b.join("Contents/x86_64-win/Delta.vst3"), b"a newer binary").unwrap();
        assert_eq!(
            catalog.stale(std::slice::from_ref(&root)),
            std::slice::from_ref(&b),
            "changed"
        );

        let file = root.join("catalog.json");
        catalog.version = CATALOG_VERSION;
        catalog.save(&file).unwrap();
        assert_eq!(Catalog::load(&file), catalog);
        assert_eq!(catalog.failures(), [(b.as_path(), "broken")]);
        catalog.retry(&b);
        assert!(catalog.failures().is_empty());
    }
}
