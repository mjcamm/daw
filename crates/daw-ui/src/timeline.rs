//! A node's notes as one timeline (`docs/design/note-flow.md`): the editor
//! shows every note of a node's clips in song time, and after an edit
//! puts each note back into the clip its start falls in. A note outside
//! every clip gets a new clip a bar long (snapped to the bar), trimmed so
//! it never overlaps a neighbour.

use daw_engine::{ClipId, Edit, MidiClip, MidiNote, NodeId, Ticks};

/// A node's notes in song time, clip by clip in time order, keeping only
/// the parts of clips that play (a note must start inside its clip). A
/// looped clip's notes show once each, at their first place in the clip
/// (`docs/design/clip-loops.md`); its repeats are [`repeats_of`].
pub fn notes_of(clips: &[(ClipId, MidiClip)]) -> Vec<MidiNote> {
    let mut sorted: Vec<&(ClipId, MidiClip)> = clips.iter().collect();
    sorted.sort_by_key(|(id, c)| (c.start, *id));
    sorted
        .into_iter()
        .flat_map(|(_, clip)| {
            let mut notes: Vec<MidiNote> = clip
                .notes
                .iter()
                .filter_map(|n| {
                    first_place(clip, n).map(|at| MidiNote {
                        start: clip.start + at,
                        ..n.clone()
                    })
                })
                .collect();
            notes.sort_by_key(|n| n.start);
            notes
        })
        .collect()
}

/// Where in `clip` (clip time) a note of it first plays, if it does: its
/// own start, or in a looped clip the first time its cycle time comes round.
fn first_place(clip: &MidiClip, note: &MidiNote) -> Option<Ticks> {
    let at = match clip.looping {
        None => note.start,
        Some(looping) => {
            if note.start < Ticks::ZERO || note.start >= looping.cycle {
                return None;
            }
            Ticks((note.start - looping.phase).0.rem_euclid(looping.cycle.0))
        }
    };
    (at >= Ticks::ZERO && at < clip.length).then_some(at)
}

/// The notes a node's looped clips play after their first cycle, in song
/// time: what the editor shows faded behind the notes it edits.
pub fn repeats_of(clips: &[(ClipId, MidiClip)]) -> Vec<MidiNote> {
    clips
        .iter()
        .filter_map(|(_, clip)| clip.looping.map(|l| (clip, l)))
        .flat_map(|(clip, looping)| {
            clip.played_notes()
                .into_iter()
                .filter(move |n| n.start >= looping.cycle)
                .map(move |n| MidiNote {
                    start: clip.start + n.start,
                    ..n
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
            Some(index) => {
                let clip = &clips[index].1;
                // In a looped clip a note goes into the cycle where it lands.
                let start = match clip.looping {
                    Some(looping) => looping.cycle_time(note.start - clip.start),
                    None => note.start - clip.start,
                };
                held[index].push(MidiNote {
                    start,
                    ..note.clone()
                });
            }
            None => loose.push(note.clone()),
        }
    }
    let mut edits = Vec::new();
    for ((id, clip), notes) in clips.iter().zip(held) {
        // Notes that were never shown (past the clip's end, outside a
        // loop's cycle) are kept.
        let mut kept: Vec<MidiNote> = clip
            .notes
            .iter()
            .filter(|n| first_place(clip, n).is_none())
            .cloned()
            .collect();
        kept.extend(notes);
        // Order does not matter: the same notes are no change.
        let key = |n: &MidiNote| (n.start, n.pitch, n.length, n.velocity);
        let (mut a, mut b) = (kept.clone(), clip.notes.clone());
        a.sort_by_key(key);
        b.sort_by_key(key);
        if a != b {
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
            looping: None,
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
    use daw_engine::FIRST_NODE;

    fn beats(n: i64) -> Ticks {
        Ticks::from_beats(n)
    }

    fn note(start: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: beats(start),
            length: beats(1),
            pitch,
            velocity: 100,
            bend: None,
        }
    }

    fn clip(start: i64, length: i64, notes: Vec<MidiNote>) -> MidiClip {
        MidiClip {
            start: beats(start),
            length: beats(length),
            notes,
            looping: None,
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

    /// A two-beat loop from beat 4, starting a beat into its cycle (as the
    /// right half of a cut), six beats long.
    fn looped() -> MidiClip {
        MidiClip {
            start: beats(4),
            length: beats(6),
            notes: vec![note(0, 60), note(1, 64)],
            looping: Some(daw_engine::ClipLoop::new(beats(2)).shifted(beats(1))),
        }
    }

    #[test]
    fn each_note_of_a_loop_shows_once_at_its_first_place() {
        // Cycle time 1 plays first (clip time 0), cycle time 0 a beat later.
        let notes = notes_of(&[(ClipId(1), looped())]);
        assert_eq!(notes, [note(4, 64), note(5, 60)]);
    }

    #[test]
    fn a_note_placed_in_a_loop_goes_into_the_cycle() {
        let clips = [(ClipId(1), looped())];
        // Its notes as shown, plus one dropped on a repeat at beat 8.5.
        let mut notes = notes_of(&clips);
        notes.push(MidiNote {
            start: Ticks::from_beats_f64(8.5),
            ..note(0, 67)
        });
        let edits = place_notes(FIRST_NODE, &clips, &notes, beats(4), ClipId(9));
        let [Edit::SetClip { clip, .. }] = edits.as_slice() else {
            panic!("one edit to the looped clip: {edits:?}")
        };
        assert!(clip.looping.is_some(), "still looped");
        // Beat 8.5 is cycle time 1.5 (clip time 4.5, a beat into the cycle).
        let mut cycle: Vec<(Ticks, u8)> = clip.notes.iter().map(|n| (n.start, n.pitch)).collect();
        cycle.sort();
        assert_eq!(
            cycle,
            vec![
                (beats(0), 60),
                (beats(1), 64),
                (Ticks::from_beats_f64(1.5), 67)
            ]
        );
    }

    #[test]
    fn shown_notes_put_back_unchanged_change_nothing() {
        let clips = [(ClipId(1), looped())];
        let notes = notes_of(&clips);
        assert!(place_notes(FIRST_NODE, &clips, &notes, beats(4), ClipId(9)).is_empty());
    }
}
