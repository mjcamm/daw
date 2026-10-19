//! Project folders on disk and the list of recently opened projects.
//!
//! A project is a folder holding `project.json`. Imported samples (and
//! later recordings) live in an `audio/` folder beside it.

use daw_engine::{LoadError, ProjectFile};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROJECT_FILE: &str = "project.json";
/// Where a project keeps its audio files, inside its folder.
pub const AUDIO_DIR: &str = "audio";
/// Background backup written while there are unsaved changes, so work can
/// be recovered after a crash. Removed by a normal save.
pub const AUTOSAVE_FILE: &str = "project.autosave.json";
/// Most projects kept in the recent list.
const MAX_RECENT: usize = 20;

/// Where new projects go by default: a folder in the user's Documents.
pub fn default_projects_dir() -> PathBuf {
    dirs::document_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("DAW Projects")
}

/// Where the app keeps its own settings, such as the recent list.
pub fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("daw")
}

/// Creates a new, empty project folder for a song called `name` inside
/// `parent`, adding a number if the name is taken. Returns the folder.
pub fn create_folder(parent: &Path, name: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(parent)?;
    let base = folder_name(name);
    for n in 1.. {
        let candidate = if n == 1 {
            parent.join(&base)
        } else {
            parent.join(format!("{base} {n}"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("ran out of folder names")
}

/// A folder name for a song name, without characters that are invalid in
/// file names on any platform.
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Copies an audio file into the project's `audio/` folder (adding a
/// number if the name is taken) and returns how the project refers to it:
/// the file's name without its extension, and its path inside the folder.
pub fn import_sample(folder: &Path, source: &Path) -> io::Result<daw_engine::SampleRef> {
    let stem = source
        .file_stem()
        .map_or_else(|| "Sample".to_owned(), |s| s.to_string_lossy().into_owned());
    let extension = source
        .extension()
        .map_or_else(|| "wav".to_owned(), |e| e.to_string_lossy().to_lowercase());
    let base = folder_name(&stem);
    let dir = folder.join(AUDIO_DIR);
    std::fs::create_dir_all(&dir)?;
    for n in 1.. {
        let name = if n == 1 {
            format!("{base}.{extension}")
        } else {
            format!("{base} {n}.{extension}")
        };
        let target = dir.join(&name);
        // Create the file first, so two imports never pick the same name.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(_) => {
                std::fs::copy(source, &target)?;
                return Ok(daw_engine::SampleRef {
                    name: stem,
                    file: format!("{AUDIO_DIR}/{name}"),
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("ran out of file names")
}

/// How long a WAV file plays, in seconds, from its header alone.
pub fn wav_seconds(path: &Path) -> io::Result<f64> {
    let reader = hound::WavReader::open(path).map_err(io::Error::other)?;
    let rate = reader.spec().sample_rate.max(1);
    Ok(f64::from(reader.duration()) / f64::from(rate))
}

/// Saves a project into its folder. The file is written beside the old one
/// and then swapped in, so a crash mid-save never damages the existing file.
pub fn save(folder: &Path, file: &ProjectFile) -> io::Result<()> {
    write_atomically(folder, PROJECT_FILE, file)?;
    match std::fs::remove_file(folder.join(AUTOSAVE_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Writes the crash-recovery backup, leaving the saved project untouched.
pub fn autosave(folder: &Path, file: &ProjectFile) -> io::Result<()> {
    write_atomically(folder, AUTOSAVE_FILE, file)
}

fn write_atomically(folder: &Path, name: &str, file: &ProjectFile) -> io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let temporary = folder.join(format!("{name}.saving"));
    std::fs::write(&temporary, file.to_json())?;
    std::fs::rename(&temporary, folder.join(name))
}

/// Why a project folder could not be opened.
#[derive(Debug)]
pub enum OpenError {
    Io(io::Error),
    Load(LoadError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read the project: {e}"),
            Self::Load(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for OpenError {}

pub fn open(folder: &Path) -> Result<ProjectFile, OpenError> {
    let json = std::fs::read_to_string(folder.join(PROJECT_FILE)).map_err(OpenError::Io)?;
    ProjectFile::from_json(&json).map_err(OpenError::Load)
}

/// A project in the recent list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentProject {
    pub folder: PathBuf,
    pub name: String,
    /// When it was last opened or saved, in seconds since 1970.
    pub last_opened: u64,
}

/// The list of recently opened projects, most recent first, stored as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentProjects {
    path: PathBuf,
    entries: Vec<RecentProject>,
}

impl RecentProjects {
    /// The app's recent list.
    pub fn load_default() -> Self {
        Self::load(app_config_dir().join("recent.json"))
    }

    /// Reads the list stored at `path`. A missing or unreadable list is
    /// simply empty: it is a convenience, not data worth failing over.
    pub fn load(path: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self { path, entries }
    }

    /// Projects whose folders still hold a project file.
    pub fn existing(&self) -> impl Iterator<Item = &RecentProject> {
        self.entries
            .iter()
            .filter(|p| p.folder.join(PROJECT_FILE).is_file())
    }

    /// Moves a project to the top of the list and saves the list.
    pub fn touch(&mut self, folder: &Path, name: &str) -> io::Result<()> {
        self.entries.retain(|p| p.folder != folder);
        self.entries.insert(
            0,
            RecentProject {
                folder: folder.to_owned(),
                name: name.to_owned(),
                last_opened: now_seconds(),
            },
        );
        self.entries.truncate(MAX_RECENT);
        self.save()
    }

    /// Drops a project from the list and saves the list.
    pub fn forget(&mut self, folder: &Path) -> io::Result<()> {
        self.entries.retain(|p| p.folder != folder);
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.entries).unwrap_or_default();
        std::fs::write(&self.path, json)
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Command, Edit, Engine, EngineConfig};

    /// A fresh, empty directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("daw-host-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn song(name: &str) -> ProjectFile {
        let (mut engine, _processor) = Engine::new(EngineConfig::default());
        engine
            .apply(Command::Edit(Edit::RenameSong {
                name: name.to_owned(),
            }))
            .unwrap();
        engine.to_file()
    }

    #[test]
    fn folder_names_are_safe_on_every_platform() {
        assert_eq!(folder_name("Night Drive"), "Night Drive");
        assert_eq!(folder_name("AC/DC: Live?"), "AC-DC- Live-");
        assert_eq!(folder_name("  ends with dots... "), "ends with dots");
        assert_eq!(folder_name("   "), "Untitled");
    }

    #[test]
    fn new_folders_never_overwrite_existing_ones() {
        let dir = scratch("create");
        let first = create_folder(&dir, "Song").unwrap();
        let second = create_folder(&dir, "Song").unwrap();
        assert_eq!(first, dir.join("Song"));
        assert_eq!(second, dir.join("Song 2"));
    }

    #[test]
    fn saved_projects_open_again() {
        let dir = scratch("save");
        let file = song("Round Trip");
        save(&dir, &file).unwrap();
        assert_eq!(open(&dir).unwrap(), file);
        // Saving again replaces the file cleanly.
        save(&dir, &song("Second Save")).unwrap();
        assert_eq!(open(&dir).unwrap().name, "Second Save");
        assert!(!dir.join("project.json.saving").exists());
    }

    #[test]
    fn autosave_leaves_the_saved_project_alone_until_the_next_save() {
        let dir = scratch("autosave");
        save(&dir, &song("Saved")).unwrap();
        autosave(&dir, &song("Unsaved changes")).unwrap();
        assert_eq!(open(&dir).unwrap().name, "Saved");
        assert!(dir.join(AUTOSAVE_FILE).is_file());
        save(&dir, &song("Saved again")).unwrap();
        assert!(!dir.join(AUTOSAVE_FILE).exists());
    }

    #[test]
    fn opening_a_missing_or_broken_project_fails_clearly() {
        let dir = scratch("broken");
        assert!(matches!(open(&dir), Err(OpenError::Io(_))));
        std::fs::write(dir.join(PROJECT_FILE), "{ nope").unwrap();
        assert!(matches!(open(&dir), Err(OpenError::Load(_))));
    }

    #[test]
    fn recent_list_keeps_most_recent_first_without_duplicates() {
        let dir = scratch("recent");
        let list_path = dir.join("recent.json");
        let mut recent = RecentProjects::load(list_path.clone());
        for name in ["A", "B", "C"] {
            let folder = dir.join(name);
            save(&folder, &song(name)).unwrap();
            recent.touch(&folder, name).unwrap();
        }
        recent.touch(&dir.join("A"), "A renamed").unwrap();

        let reloaded = RecentProjects::load(list_path);
        let names: Vec<_> = reloaded.existing().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A renamed", "C", "B"]);
    }

    #[test]
    fn recent_list_hides_deleted_projects_and_can_forget() {
        let dir = scratch("recent-missing");
        let mut recent = RecentProjects::load(dir.join("recent.json"));
        let kept = dir.join("Kept");
        save(&kept, &song("Kept")).unwrap();
        recent.touch(&kept, "Kept").unwrap();
        recent.touch(&dir.join("Deleted"), "Deleted").unwrap();
        assert_eq!(recent.existing().count(), 1);

        recent.forget(&kept).unwrap();
        assert_eq!(recent.existing().count(), 0);
    }

    #[test]
    fn a_missing_recent_list_is_empty() {
        let dir = scratch("recent-none");
        assert_eq!(
            RecentProjects::load(dir.join("nothing.json"))
                .existing()
                .count(),
            0
        );
    }

    #[test]
    fn imported_samples_are_copied_into_the_project() {
        let dir = scratch("import");
        let source = dir.join("Big Kick.WAV");
        std::fs::write(&source, b"RIFF").unwrap();
        let project = dir.join("Song");
        let first = import_sample(&project, &source).unwrap();
        assert_eq!(first.name, "Big Kick");
        assert_eq!(first.file, "audio/Big Kick.wav");
        assert_eq!(
            std::fs::read(project.join("audio/Big Kick.wav")).unwrap(),
            b"RIFF"
        );
        let second = import_sample(&project, &source).unwrap();
        assert_eq!(second.file, "audio/Big Kick 2.wav", "never overwrites");
    }

    #[test]
    fn a_wav_files_length_comes_from_its_header() {
        let path = scratch("wav-length").join("half.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..2 * 22_050 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
        assert_eq!(wav_seconds(&path).unwrap(), 0.5);
        assert!(wav_seconds(&path.with_file_name("none.wav")).is_err());
    }
}
