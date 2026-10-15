//! Authoritative project state and the edits that change it.

use daw_dsp::synth::SynthParams;
use daw_protocol::{ClipId, CommandError, Edit, InvalidClip, MidiClip};
use std::collections::BTreeMap;

pub const MIN_TEMPO_BPM: f64 = 20.0;
pub const MAX_TEMPO_BPM: f64 = 999.0;
pub const DEFAULT_TEMPO_BPM: f64 = 120.0;
pub const MAX_VOLUME_DB: f32 = 12.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub tempo_bpm: f64,
    pub node: Node,
}

/// A channel: clips playing through an instrument, then volume and pan.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub volume_db: f32,
    pub pan: f32,
    pub instrument: SynthParams,
    pub clips: BTreeMap<ClipId, MidiClip>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            tempo_bpm: DEFAULT_TEMPO_BPM,
            node: Node::default(),
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            pan: 0.0,
            instrument: SynthParams::default(),
            clips: BTreeMap::new(),
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
            Edit::AddClip { id, clip } => {
                if self.node.clips.contains_key(&id) {
                    return Err(CommandError::DuplicateClip { id });
                }
                validate_clip(&clip).map_err(|reason| CommandError::InvalidClip { reason })?;
                self.node.clips.insert(id, clip);
                Ok(Edit::RemoveClip { id })
            }
            Edit::RemoveClip { id } => {
                let clip = self
                    .node
                    .clips
                    .remove(&id)
                    .ok_or(CommandError::UnknownClip { id })?;
                Ok(Edit::AddClip { id, clip })
            }
            Edit::SetVolume { db } => {
                // -inf (mute) is allowed; NaN and anything too loud are not.
                if db.is_nan() || db > MAX_VOLUME_DB {
                    return Err(CommandError::VolumeOutOfRange {
                        db,
                        max: MAX_VOLUME_DB,
                    });
                }
                let previous = std::mem::replace(&mut self.node.volume_db, db);
                Ok(Edit::SetVolume { db: previous })
            }
            Edit::SetPan { pan } => {
                if !(-1.0..=1.0).contains(&pan) {
                    return Err(CommandError::PanOutOfRange { pan });
                }
                let previous = std::mem::replace(&mut self.node.pan, pan);
                Ok(Edit::SetPan { pan: previous })
            }
        }
    }
}

fn validate_clip(clip: &MidiClip) -> Result<(), InvalidClip> {
    if clip.start.0 < 0 {
        return Err(InvalidClip::NegativeStart);
    }
    if clip.length.0 <= 0 {
        return Err(InvalidClip::EmptyLength);
    }
    for (index, note) in clip.notes.iter().enumerate() {
        if note.pitch > 127 {
            return Err(InvalidClip::NotePitch { index });
        }
        if !(1..=127).contains(&note.velocity) {
            return Err(InvalidClip::NoteVelocity { index });
        }
        if note.length.0 <= 0 {
            return Err(InvalidClip::NoteLength { index });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_protocol::{MidiNote, Ticks};

    fn clip(notes: Vec<MidiNote>) -> MidiClip {
        MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_bars(1),
            notes,
        }
    }

    fn note(pitch: u8, velocity: u8, length: i64) -> MidiNote {
        MidiNote {
            start: Ticks::ZERO,
            length: Ticks(length),
            pitch,
            velocity,
        }
    }

    /// Applies `edit`, checks it changed something, then checks its inverse
    /// restores the original project.
    fn assert_round_trip(edit: Edit) {
        let mut project = Project::default();
        let original = project.clone();
        let inverse = project.apply(edit).unwrap();
        assert_ne!(project, original);
        project.apply(inverse).unwrap();
        assert_eq!(project, original);
    }

    fn assert_rejected(project: &mut Project, edit: Edit) -> CommandError {
        let before = project.clone();
        let err = project.apply(edit).unwrap_err();
        assert_eq!(*project, before, "rejected edit changed the project");
        err
    }

    #[test]
    fn every_edit_is_reverted_by_its_inverse() {
        assert_round_trip(Edit::SetTempo { bpm: 140.0 });
        assert_round_trip(Edit::SetVolume { db: -6.0 });
        assert_round_trip(Edit::SetPan { pan: 0.5 });
        assert_round_trip(Edit::AddClip {
            id: ClipId(1),
            clip: clip(vec![note(60, 100, 240)]),
        });
    }

    #[test]
    fn removing_a_clip_is_reverted_by_adding_it_back() {
        let mut project = Project::default();
        let added = clip(vec![note(60, 100, 240)]);
        project
            .apply(Edit::AddClip {
                id: ClipId(7),
                clip: added.clone(),
            })
            .unwrap();
        let inverse = project.apply(Edit::RemoveClip { id: ClipId(7) }).unwrap();
        assert_eq!(
            inverse,
            Edit::AddClip {
                id: ClipId(7),
                clip: added
            }
        );
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let mut project = Project::default();
        for bpm in [0.0, 19.9, 1000.0, f64::NAN, f64::INFINITY] {
            assert_rejected(&mut project, Edit::SetTempo { bpm });
        }
        for db in [12.1, f32::NAN, f32::INFINITY] {
            assert_rejected(&mut project, Edit::SetVolume { db });
        }
        for pan in [-1.1, 1.1, f32::NAN] {
            assert_rejected(&mut project, Edit::SetPan { pan });
        }
    }

    #[test]
    fn muting_with_negative_infinity_is_allowed() {
        let mut project = Project::default();
        project
            .apply(Edit::SetVolume {
                db: f32::NEG_INFINITY,
            })
            .unwrap();
    }

    #[test]
    fn clip_ids_must_be_unique_and_known() {
        let mut project = Project::default();
        let add = Edit::AddClip {
            id: ClipId(1),
            clip: clip(vec![]),
        };
        project.apply(add.clone()).unwrap();
        assert_eq!(
            assert_rejected(&mut project, add),
            CommandError::DuplicateClip { id: ClipId(1) }
        );
        assert_eq!(
            assert_rejected(&mut project, Edit::RemoveClip { id: ClipId(2) }),
            CommandError::UnknownClip { id: ClipId(2) }
        );
    }

    #[test]
    fn invalid_clips_are_rejected_with_a_reason() {
        let cases = [
            (
                MidiClip {
                    start: Ticks(-1),
                    ..clip(vec![])
                },
                InvalidClip::NegativeStart,
            ),
            (
                MidiClip {
                    length: Ticks::ZERO,
                    ..clip(vec![])
                },
                InvalidClip::EmptyLength,
            ),
            (
                clip(vec![note(60, 100, 10), note(128, 100, 10)]),
                InvalidClip::NotePitch { index: 1 },
            ),
            (
                clip(vec![note(60, 0, 10)]),
                InvalidClip::NoteVelocity { index: 0 },
            ),
            (
                clip(vec![note(60, 100, 0)]),
                InvalidClip::NoteLength { index: 0 },
            ),
        ];
        for (clip, reason) in cases {
            let mut project = Project::default();
            let err = assert_rejected(
                &mut project,
                Edit::AddClip {
                    id: ClipId(1),
                    clip,
                },
            );
            assert_eq!(err, CommandError::InvalidClip { reason });
        }
    }
}
