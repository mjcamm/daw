//! A node's notes as one timeline (`docs/design/note-flow.md`): the editor
//! shows every note of a node's clips in song time, and after an edit
//! puts each note back into the clip its start falls in. A note outside
//! every clip gets a new clip a bar long (snapped to the bar), trimmed so
//! it never overlaps a neighbour.

use daw_engine::{ClipId, Edit, MidiClip, MidiNote, NodeId, Ticks};

/// A node's notes in song time, clip by clip in time order, keeping only
/// the parts of clips that play (a note must start inside its clip).
pub fn notes_of(clips: &[(ClipId, MidiClip)]) -> Vec<MidiNote> {
    let mut sorted: Vec<&(ClipId, MidiClip)> = clips.iter().collect();
    sorted.sort_by_key(|(id, c)| (c.start, *id));
    sorted
        .into_iter()
        .flat_map(|(_, clip)| {
            clip.notes
                .iter()
                .filter(|n| n.start >= Ticks::ZERO && n.start < clip.length)
                .map(|n| MidiNote {
                    start: clip.start + n.start,
                    ..*n
                })
        })
        .collect()
}

/// The edits that make a node's clips hold `notes` (song time), and the
/// ids of any clips made. `bar` is the size of a new clip; `next_id` the
/// first free clip id. Clips whose notes do not change get no edit.
pub fn place_notes(
    node: NodeId,
    clips: &[(ClipId, MidiClip)],
    notes: &[MidiNote],
    bar: Ticks,
    next_id: ClipId,
) -> Vec<Edit> {
    let holder = |t: Ticks| {
        clips
            .iter()
            .position(|(_, c)| c.start <= t && t < c.start + c.length)
    };
    let mut held: Vec<Vec<MidiNote>> = vec![Vec::new(); clips.len()];
    let mut loose: Vec<MidiNote> = Vec::new();
    for note in notes {
        match holder(note.start) {
            Some(index) => held[index].push(MidiNote {
                start: note.start - clips[index].1.start,
                ..*note
            }),
            None => loose.push(*note),
        }
    }
    let mut edits = Vec::new();
    for ((id, clip), notes) in clips.iter().zip(held) {
        // Notes past the clip's end were never shown; keep them.
        let mut kept: Vec<MidiNote> = clip
            .notes
            .iter()
            .filter(|n| n.start < Ticks::ZERO || n.start >= clip.length)
            .copied()
            .collect();
        kept.extend(notes);
        if kept != clip.notes {
            edits.push(Edit::SetClip {
                id: *id,
                node,
                clip: MidiClip {
                    notes: kept,
                    ..clip.clone()
                },
            });
        }
    }
    // Loose notes: a new clip per bar they start in, fitted between the
    // clips around it.
    let mut new_clips: Vec<MidiClip> = Vec::new();
    let bar = Ticks(bar.0.max(1));
    for note in loose {
        let bar_start = Ticks(note.start.0.div_euclid(bar.0) * bar.0).max(Ticks::ZERO);
        if let Some(clip) = new_clips
            .iter_mut()
            .find(|c| c.start <= note.start && note.start < c.start + c.length)
        {
            clip.notes.push(MidiNote {
                start: note.start - clip.start,
                ..note
            });
            continue;
        }
        let start = clips
            .iter()
            .map(|(_, c)| c.start + c.length)
            .filter(|end| *end <= note.start)
            .fold(bar_start, Ticks::max);
        let end = clips
            .iter()
            .map(|(_, c)| c.start)
            .filter(|s| *s > note.start)
            .fold(bar_start + bar, Ticks::min);
        new_clips.push(MidiClip {
            start,
            length: end - start,
            notes: vec![MidiNote {
                start: note.start - start,
                ..note
            }],
        });
    }
    for (offset, clip) in new_clips.into_iter().enumerate() {
        edits.push(Edit::AddClip {
            node,
            id: ClipId(next_id.0 + offset as u64),
            clip,
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beats(n: i64) -> Ticks {
        Ticks::from_beats(n)
    }

    fn note(start: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: beats(start),
            length: beats(1),
            pitch,
            velocity: 100,
        }
    }

    fn clip(start: i64, length: i64, notes: Vec<MidiNote>) -> MidiClip {
        MidiClip {
            start: beats(start),
            length: beats(length),
            notes,
        }
    }

    #[test]
    fn the_timeline_reads_every_clip_in_song_time() {
        let clips = [
            (ClipId(2), clip(8, 4, vec![note(1, 62)])),
            (ClipId(1), clip(0, 4, vec![note(0, 60), note(5, 61)])),
        ];
        let notes = notes_of(&clips);
        assert_eq!(notes, [note(0, 60), note(9, 62)], "past its end is hidden");
    }

    #[test]
    fn notes_go_back_to_the_clip_they_start_in() {
        let clips = [
            (ClipId(1), clip(0, 4, vec![note(0, 60)])),
            (ClipId(2), clip(8, 4, Vec::new())),
        ];
        // The note moved from the first clip into the second.
        let edits = place_notes(NodeId(5), &clips, &[note(9, 60)], beats(4), ClipId(10));
        assert_eq!(
            edits,
            [
                Edit::SetClip {
                    id: ClipId(1),
                    node: NodeId(5),
                    clip: clip(0, 4, Vec::new()),
                },
                Edit::SetClip {
                    id: ClipId(2),
                    node: NodeId(5),
                    clip: clip(8, 4, vec![note(1, 60)]),
                },
            ]
        );
    }

    #[test]
    fn a_note_outside_every_clip_gets_a_bar_trimmed_to_its_neighbours() {
        let clips = [
            (ClipId(1), clip(0, 2, Vec::new())),
            (ClipId(2), clip(7, 4, Vec::new())),
        ];
        // Beat 5 is in the bar 4..8, but the clips leave only 4..7.
        let edits = place_notes(
            NodeId(5),
            &clips,
            &[note(5, 60), note(6, 62), note(20, 64)],
            beats(4),
            ClipId(10),
        );
        assert_eq!(
            edits,
            [
                Edit::AddClip {
                    node: NodeId(5),
                    id: ClipId(10),
                    clip: clip(4, 3, vec![note(1, 60), note(2, 62)]),
                },
                Edit::AddClip {
                    node: NodeId(5),
                    id: ClipId(11),
                    clip: clip(20, 4, vec![note(0, 64)]),
                },
            ]
        );
    }

    #[test]
    fn unchanged_clips_get_no_edit() {
        let clips = [(ClipId(1), clip(0, 4, vec![note(0, 60)]))];
        assert!(place_notes(NodeId(5), &clips, &[note(0, 60)], beats(4), ClipId(10)).is_empty());
    }
}
