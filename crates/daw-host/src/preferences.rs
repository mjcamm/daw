//! The app's preferences, stored as JSON beside the recent projects list.

use crate::projects::app_config_dir;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// Light or dark, or whatever the system uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// Every preference. Missing fields take their defaults, so older files
/// keep working as preferences are added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub appearance: Appearance,
    pub audio: AudioSettings,
    /// Plugin folders searched besides the standard ones.
    pub plugin_folders: Vec<PathBuf>,
    /// The click track: on or off, volume, sound, accent, pitch.
    pub metronome: daw_engine::Metronome,
}

/// The audio setup the user chose. Each `None` leaves it to the system (or
/// the app): the default driver, device, rate and buffer, automatic
/// threads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// The driver by name, e.g. "WASAPI" or "ASIO".
    pub driver: Option<String>,
    /// The output device, by its system id.
    pub device: Option<String>,
    pub sample_rate: Option<u32>,
    /// Frames per buffer: smaller is less delay but more work per second.
    pub buffer_frames: Option<u32>,
    /// Threads that render, the audio thread included.
    pub threads: Option<usize>,
}

/// Preferences and where they are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPreferences {
    path: PathBuf,
    pub preferences: Preferences,
}

impl StoredPreferences {
    /// The app's preferences.
    pub fn load_default() -> Self {
        Self::load(app_config_dir().join("preferences.json"))
    }

    /// Reads the preferences stored at `path`. Missing or unreadable
    /// preferences are the defaults: not worth failing to start over.
    pub fn load(path: PathBuf) -> Self {
        let preferences = std::fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        Self { path, preferences }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.preferences).unwrap_or_default();
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("daw-prefs-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("preferences.json")
    }

    #[test]
    fn missing_preferences_are_the_defaults() {
        let stored = StoredPreferences::load(scratch("missing"));
        assert_eq!(stored.preferences.appearance, Appearance::System);
    }

    #[test]
    fn preferences_survive_a_round_trip() {
        let path = scratch("round-trip");
        let mut stored = StoredPreferences::load(path.clone());
        stored.preferences.appearance = Appearance::Dark;
        stored.save().unwrap();
        assert_eq!(
            StoredPreferences::load(path).preferences.appearance,
            Appearance::Dark
        );
    }

    #[test]
    fn the_metronome_is_kept_and_older_files_take_its_defaults() {
        let path = scratch("metronome");
        let mut stored = StoredPreferences::load(path.clone());
        stored.preferences.metronome = daw_engine::Metronome {
            on: true,
            volume: 35,
            sound: daw_engine::ClickSound::Cowbell,
            accent: false,
            pitch: -5,
        };
        stored.save().unwrap();
        let loaded = StoredPreferences::load(path.clone()).preferences.metronome;
        assert_eq!(loaded, stored.preferences.metronome);
        // A file from before the metronome.
        std::fs::write(&path, r#"{"appearance":"Dark"}"#).unwrap();
        let older = StoredPreferences::load(path).preferences;
        assert_eq!(older.appearance, Appearance::Dark);
        assert_eq!(older.metronome, daw_engine::Metronome::default());
    }

    #[test]
    fn damaged_or_partial_files_fall_back_to_defaults() {
        let path = scratch("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(
            StoredPreferences::load(path.clone()).preferences,
            Preferences::default()
        );
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(
            StoredPreferences::load(path).preferences,
            Preferences::default()
        );
    }
}
