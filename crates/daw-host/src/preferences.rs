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
