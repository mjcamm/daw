//! Authoritative project state and the edits that change it.

use daw_protocol::{CommandError, Edit};

pub const MIN_TEMPO_BPM: f64 = 20.0;
pub const MAX_TEMPO_BPM: f64 = 999.0;
pub const DEFAULT_TEMPO_BPM: f64 = 120.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub tempo_bpm: f64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            tempo_bpm: DEFAULT_TEMPO_BPM,
        }
    }
}

impl Project {
    /// Applies `edit` and returns the edit that reverts it.
    ///
    /// A rejected edit leaves the project unchanged.
    pub fn apply(&mut self, edit: Edit) -> Result<Edit, CommandError> {
        match edit {
            Edit::SetTempo { bpm } => {
                if !(MIN_TEMPO_BPM..=MAX_TEMPO_BPM).contains(&bpm) {
                    return Err(CommandError::TempoOutOfRange {
                        bpm,
                        min: MIN_TEMPO_BPM,
                        max: MAX_TEMPO_BPM,
                    });
                }
                let previous = std::mem::replace(&mut self.tempo_bpm, bpm);
                Ok(Edit::SetTempo { bpm: previous })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_tempo_returns_its_inverse() {
        let mut project = Project::default();
        let inverse = project.apply(Edit::SetTempo { bpm: 140.0 }).unwrap();
        assert_eq!(project.tempo_bpm, 140.0);
        project.apply(inverse).unwrap();
        assert_eq!(project, Project::default());
    }

    #[test]
    fn out_of_range_tempo_is_rejected_without_change() {
        for bpm in [0.0, 19.9, 1000.0, f64::NAN, f64::INFINITY] {
            let mut project = Project::default();
            let result = project.apply(Edit::SetTempo { bpm });
            assert!(
                matches!(result, Err(CommandError::TempoOutOfRange { .. })),
                "{bpm}"
            );
            assert_eq!(project, Project::default());
        }
    }
}
