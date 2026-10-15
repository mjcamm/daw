//! Compiling the project into the flat, time-sorted form the audio side plays.

use crate::project::Project;
use daw_automation::Automation;
use daw_protocol::{DeviceId, EffectKind, ParamState};

/// Everything the audio thread needs to render, compiled from the project by
/// the control side. Immutable once sent.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderState {
    pub tempo_bpm: f64,
    /// Every note on the timeline, sorted by start.
    pub notes: Vec<ScheduledNote>,
    /// One automation per instrument parameter, in index order.
    pub instrument: Vec<Automation>,
    /// The effect chain in signal order.
    pub effects: Vec<EffectState>,
    /// One automation per channel parameter, in index order.
    pub channel: Vec<Automation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectState {
    pub id: DeviceId,
    pub kind: EffectKind,
    /// One automation per parameter, in index order.
    pub params: Vec<Automation>,
}

/// A note at an absolute song position, in beats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduledNote {
    pub start_beats: f64,
    pub end_beats: f64,
    pub pitch: u8,
    pub velocity: u8,
}

fn compile_params(params: &[ParamState]) -> Vec<Automation> {
    params.iter().map(Automation::compile).collect()
}

impl RenderState {
    pub fn compile(project: &Project) -> Self {
        let node = &project.node;
        let mut notes: Vec<ScheduledNote> = node
            .clips
            .values()
            .flat_map(|clip| {
                clip.notes.iter().filter_map(move |note| {
                    // Only the part of a note inside the clip plays.
                    let start = note.start.max(daw_protocol::Ticks::ZERO);
                    let end = (note.start + note.length).min(clip.length);
                    (start < end).then(|| ScheduledNote {
                        start_beats: (clip.start + start).as_beats(),
                        end_beats: (clip.start + end).as_beats(),
                        pitch: note.pitch,
                        velocity: note.velocity,
                    })
                })
            })
            .collect();
        // Stable sort keeps clip order for simultaneous notes: deterministic.
        notes.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
        Self {
            tempo_bpm: project.tempo_bpm,
            notes,
            instrument: compile_params(&node.instrument),
            effects: node
                .effects
                .iter()
                .map(|(id, effect)| EffectState {
                    id: *id,
                    kind: effect.kind,
                    params: compile_params(&effect.params),
                })
                .collect(),
            channel: compile_params(&node.channel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_protocol::{ClipId, MidiClip, MidiNote, Ticks};

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: Ticks(start),
            length: Ticks(length),
            pitch,
            velocity: 100,
        }
    }

    fn project_with(clips: Vec<MidiClip>) -> Project {
        let mut project = Project::default();
        for (i, clip) in clips.into_iter().enumerate() {
            project.node.clips.insert(ClipId(i as u64), clip);
        }
        project
    }

    fn starts(state: &RenderState) -> Vec<(f64, u8)> {
        state
            .notes
            .iter()
            .map(|n| (n.start_beats, n.pitch))
            .collect()
    }

    #[test]
    fn notes_are_placed_at_clip_start_plus_offset_in_beats() {
        let project = project_with(vec![MidiClip {
            start: Ticks::from_beats(4),
            length: Ticks::from_beats(4),
            notes: vec![note(480, 960, 60)],
        }]);
        let state = RenderState::compile(&project);
        assert_eq!(
            state.notes,
            [ScheduledNote {
                start_beats: 4.5,
                end_beats: 5.5,
                pitch: 60,
                velocity: 100
            }]
        );
    }

    #[test]
    fn notes_from_all_clips_are_merged_in_time_order() {
        let project = project_with(vec![
            MidiClip {
                start: Ticks::from_beats(2),
                length: Ticks::from_beats(2),
                notes: vec![note(0, 10, 62), note(960, 10, 64)],
            },
            MidiClip {
                start: Ticks::ZERO,
                length: Ticks::from_beats(4),
                notes: vec![note(2880, 10, 65), note(0, 10, 60)],
            },
        ]);
        let state = RenderState::compile(&project);
        assert_eq!(starts(&state), [(0.0, 60), (2.0, 62), (3.0, 64), (3.0, 65)]);
    }

    #[test]
    fn notes_are_trimmed_to_the_clip() {
        let project = project_with(vec![MidiClip {
            start: Ticks::ZERO,
            length: Ticks::from_beats(1),
            notes: vec![
                note(480, 960, 60),  // overhangs the end: trimmed
                note(960, 100, 62),  // starts at the end: dropped
                note(-480, 960, 64), // starts before the clip: trimmed
            ],
        }]);
        let state = RenderState::compile(&project);
        let spans: Vec<_> = state
            .notes
            .iter()
            .map(|n| (n.pitch, n.start_beats, n.end_beats))
            .collect();
        assert_eq!(spans, [(64, 0.0, 0.5), (60, 0.5, 1.0)]);
    }
}
