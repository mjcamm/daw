//! Saved stamps: curves the user keeps for every project, in `stamps.json`
//! beside the preferences (`docs/design/curve-editor.md`). Songs never
//! depend on them: stamping copies the points.

use crate::projects::app_config_dir;
use daw_engine::Curve;
use std::io;
use std::path::PathBuf;

/// A saved stamp: a name and its shape.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub name: String,
    pub curve: Curve,
}

/// The saved stamps and where they are kept.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredStamps {
    path: PathBuf,
    pub stamps: Vec<Stamp>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct File {
    stamps: Vec<Stamp>,
}

impl StoredStamps {
    /// The user's stamps.
    pub fn load_default() -> Self {
        Self::load(app_config_dir().join("stamps.json"))
    }

    /// Reads the stamps stored at `path`. Missing or unreadable stamps are
    /// none: not worth failing to start over. Stamps whose curves are not
    /// valid are dropped.
    pub fn load(path: PathBuf) -> Self {
        let stamps = std::fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str::<File>(&json).ok())
            .map(|f| f.stamps)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| daw_engine::validate_curve(&s.curve).is_ok())
            .collect();
        Self { path, stamps }
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File {
            stamps: self.stamps.clone(),
        };
        let json = serde_json::to_string_pretty(&file).unwrap_or_default();
        std::fs::write(&self.path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("daw-stamps-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("stamps.json")
    }

    #[test]
    fn stamps_round_trip() {
        let path = scratch("round-trip");
        let mut stored = StoredStamps::load(path.clone());
        assert!(stored.stamps.is_empty(), "none at first");
        stored.stamps.push(Stamp {
            name: "Pump".to_owned(),
            curve: Curve::line(0.0, 0.5),
        });
        stored.save().unwrap();
        assert_eq!(StoredStamps::load(path), stored);
    }

    #[test]
    fn unreadable_or_invalid_stamps_are_dropped() {
        let path = scratch("invalid");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(StoredStamps::load(path.clone()).stamps.is_empty());
        let mut bad = Curve::line(0.0, 0.5);
        bad.points[1].at = 0.5;
        let file = File {
            stamps: vec![
                Stamp {
                    name: "Bad".to_owned(),
                    curve: bad,
                },
                Stamp {
                    name: "Good".to_owned(),
                    curve: Curve::line(1.0, 0.0),
                },
            ],
        };
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        let names: Vec<String> = StoredStamps::load(path)
            .stamps
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["Good"]);
    }
}
