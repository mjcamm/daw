//! Editing a clip's notes in the piano roll: pure functions over the note
//! list (positions relative to the clip start), tested without a window.
//!
//! Notes are addressed by their index in the clip's list; edits keep the
//! order of untouched notes, append new ones, and return the indices of
//! the notes they produced, so a selection can follow them.

use daw_engine::{MidiNote, TICKS_PER_BEAT, Ticks};
use std::collections::BTreeSet;

/// Shortest a note can be: a 64th.
pub const MIN_LENGTH: Ticks = Ticks(TICKS_PER_BEAT / 16);

/// The part of a note under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Body,
    Start,
    End,
}

pub type Selection = BTreeSet<usize>;

fn end(note: &MidiNote) -> Ticks {
    note.start + note.length
}

/// The topmost note at `beat` (relative to the clip) and `pitch`, and which
/// part: within `edge` beats of either end is that end (the end wins on
/// short notes). Later notes are drawn on top, so they win.
pub fn hit(notes: &[MidiNote], beat: f64, pitch: u8, edge: f64) -> Option<(usize, Part)> {
    notes.iter().enumerate().rev().find_map(|(i, n)| {
        let (start, stop) = (n.start.as_beats(), end(n).as_beats());
        if n.pitch != pitch || beat < start || beat >= stop {
            return None;
        }
        let edge = edge.min((stop - start) / 3.);
        let part = if stop - beat <= edge {
            Part::End
        } else if beat - start <= edge {
            Part::Start
        } else {
            Part::Body
        };
        Some((i, part))
    })
}

/// The notes with one added; its index.
pub fn added(notes: &[MidiNote], note: MidiNote) -> (Vec<MidiNote>, usize) {
    let mut out = notes.to_vec();
    out.push(note);
    (out, notes.len())
}

/// The notes without the selected ones.
pub fn deleted(notes: &[MidiNote], selection: &Selection) -> Vec<MidiNote> {
    notes
        .iter()
        .enumerate()
        .filter(|(i, _)| !selection.contains(i))
        .map(|(_, n)| *n)
        .collect()
}

/// The selected notes moved `dt` in time and `dpitch` semitones, as a
/// group: never before the clip start, never outside 0..=127.
fn shifted(notes: &[MidiNote], selection: &Selection, dt: Ticks, dpitch: i32) -> Vec<MidiNote> {
    let chosen: Vec<&MidiNote> = selection.iter().filter_map(|&i| notes.get(i)).collect();
    let earliest = chosen.iter().map(|n| n.start).min().unwrap_or(Ticks::ZERO);
    let dt = if earliest + dt < Ticks::ZERO {
        Ticks::ZERO - earliest
    } else {
        dt
    };
    let lowest = chosen.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(0);
    let highest = chosen
        .iter()
        .map(|n| i32::from(n.pitch))
        .max()
        .unwrap_or(127);
    let dpitch = dpitch.clamp(-lowest, 127 - highest);
    chosen
        .into_iter()
        .map(|n| MidiNote {
            start: n.start + dt,
            pitch: (i32::from(n.pitch) + dpitch) as u8,
            ..*n
        })
        .collect()
}

/// The notes with the selected ones moved.
pub fn moved(notes: &[MidiNote], selection: &Selection, dt: Ticks, dpitch: i32) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    for (&i, note) in selection.iter().zip(shifted(notes, selection, dt, dpitch)) {
        out[i] = note;
    }
    out
}

/// The notes with copies of the selected ones, moved; the copies' indices.
pub fn copied(
    notes: &[MidiNote],
    selection: &Selection,
    dt: Ticks,
    dpitch: i32,
) -> (Vec<MidiNote>, Selection) {
    let copies = shifted(notes, selection, dt, dpitch);
    let mut out = notes.to_vec();
    let start = out.len();
    out.extend(copies);
    let stop = out.len();
    (out, (start..stop).collect())
}

/// The selected notes' ends moved `delta` (never shorter than a 64th).
pub fn resized_end(notes: &[MidiNote], selection: &Selection, delta: Ticks) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    for &i in selection {
        if let Some(n) = out.get_mut(i) {
            n.length = (n.length + delta).max(MIN_LENGTH);
        }
    }
    out
}

/// The selected notes' starts moved `delta`, keeping their ends (never
/// shorter than a 64th, never before the clip start).
pub fn resized_start(notes: &[MidiNote], selection: &Selection, delta: Ticks) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    for &i in selection {
        if let Some(n) = out.get_mut(i) {
            let stop = end(n);
            let start = (n.start + delta).max(Ticks::ZERO).min(stop - MIN_LENGTH);
            n.start = start;
            n.length = stop - start;
        }
    }
    out
}

/// The note at `index` split in two at `at` (relative to the clip), if
/// `at` falls inside it with room on both sides; the second half's index.
pub fn split(notes: &[MidiNote], index: usize, at: Ticks) -> Option<(Vec<MidiNote>, usize)> {
    let note = *notes.get(index)?;
    if at - note.start < MIN_LENGTH || end(&note) - at < MIN_LENGTH {
        return None;
    }
    let mut out = notes.to_vec();
    out[index].length = at - note.start;
    out.push(MidiNote {
        start: at,
        length: end(&note) - at,
        ..note
    });
    Some((out, notes.len()))
}

/// The selected notes' velocities moved by `delta`, each from its own
/// starting value, within 1..=127.
pub fn with_velocity(
    notes: &[MidiNote],
    selection: &Selection,
    original: &[MidiNote],
    delta: i32,
) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    for &i in selection {
        if let (Some(n), Some(o)) = (out.get_mut(i), original.get(i)) {
            n.velocity = (i32::from(o.velocity) + delta).clamp(1, 127) as u8;
        }
    }
    out
}

/// Notes touching the box from `t0` to `t1` (beats) and pitch `low` to
/// `high`.
pub fn in_box(notes: &[MidiNote], t0: f64, t1: f64, low: u8, high: u8) -> Selection {
    let (t0, t1) = (t0.min(t1), t0.max(t1));
    let (low, high) = (low.min(high), low.max(high));
    notes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            (low..=high).contains(&n.pitch) && n.start.as_beats() < t1 && end(n).as_beats() > t0
        })
        .map(|(i, _)| i)
        .collect()
}

/// The selected notes, moved so the earliest starts at 0: what a copy puts
/// on the clipboard.
pub fn clipboard(notes: &[MidiNote], selection: &Selection) -> Vec<MidiNote> {
    let chosen: Vec<MidiNote> = selection
        .iter()
        .filter_map(|&i| notes.get(i).copied())
        .collect();
    let earliest = chosen.iter().map(|n| n.start).min().unwrap_or(Ticks::ZERO);
    chosen
        .into_iter()
        .map(|n| MidiNote {
            start: n.start - earliest,
            ..n
        })
        .collect()
}

/// The notes with the clipboard pasted at `at`; the pasted notes' indices.
pub fn pasted(notes: &[MidiNote], clipboard: &[MidiNote], at: Ticks) -> (Vec<MidiNote>, Selection) {
    let mut out = notes.to_vec();
    let first = out.len();
    out.extend(clipboard.iter().map(|n| MidiNote {
        start: n.start + at,
        ..*n
    }));
    let stop = out.len();
    (out, (first..stop).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beats(n: f64) -> Ticks {
        Ticks::from_beats_f64(n)
    }

    fn note(start: f64, length: f64, pitch: u8) -> MidiNote {
        MidiNote {
            start: beats(start),
            length: beats(length),
            pitch,
            velocity: 100,
        }
    }

    fn set(indices: &[usize]) -> Selection {
        indices.iter().copied().collect()
    }

    #[test]
    fn hits_find_the_part_under_the_pointer() {
        let notes = [note(0., 2., 60), note(1., 1., 60)];
        assert_eq!(hit(&notes, 0.5, 60, 0.1), Some((0, Part::Body)));
        assert_eq!(
            hit(&notes, 1.5, 60, 0.1),
            Some((1, Part::Body)),
            "the later note is on top"
        );
        assert_eq!(hit(&notes, 0.05, 60, 0.1), Some((0, Part::Start)));
        assert_eq!(hit(&notes, 1.95, 60, 0.1), Some((1, Part::End)));
        assert_eq!(hit(&notes, 0.5, 61, 0.1), None);
        assert_eq!(hit(&notes, 2.5, 60, 0.1), None);
    }

    #[test]
    fn moves_keep_the_group_inside_the_clip_and_the_keyboard() {
        let notes = [note(1., 1., 60), note(2., 1., 126)];
        let all = set(&[0, 1]);
        let moved = moved(&notes, &all, beats(-5.), 5);
        assert_eq!(moved[0].start, Ticks::ZERO, "stops at the clip start");
        assert_eq!(moved[1].start, beats(1.), "keeps its offset");
        assert_eq!(moved[1].pitch, 127, "stops at the top");
        assert_eq!(moved[0].pitch, 61);
    }

    #[test]
    fn copies_leave_the_originals() {
        let notes = [note(0., 1., 60)];
        let (out, copies) = copied(&notes, &set(&[0]), beats(2.), 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], notes[0]);
        assert_eq!(out[1].start, beats(2.));
        assert_eq!(copies, set(&[1]));
    }

    #[test]
    fn resizing_keeps_a_minimum_and_the_other_edge() {
        let notes = [note(1., 1., 60)];
        let shorter = resized_end(&notes, &set(&[0]), beats(-5.));
        assert_eq!(shorter[0].length, MIN_LENGTH);
        let later = resized_start(&notes, &set(&[0]), beats(0.5));
        assert_eq!((later[0].start, later[0].length), (beats(1.5), beats(0.5)));
        let early = resized_start(&notes, &set(&[0]), beats(-5.));
        assert_eq!((early[0].start, early[0].length), (Ticks::ZERO, beats(2.)));
    }

    #[test]
    fn splits_need_room_on_both_sides() {
        let notes = [note(0., 2., 60)];
        let (out, second) = split(&notes, 0, beats(0.5)).unwrap();
        assert_eq!(
            (out[0].length, out[second].start, out[second].length),
            (beats(0.5), beats(0.5), beats(1.5))
        );
        assert!(split(&notes, 0, beats(0.01)).is_none());
        assert!(split(&notes, 0, beats(3.)).is_none());
    }

    #[test]
    fn velocities_move_from_each_notes_own_value() {
        let mut notes = vec![note(0., 1., 60), note(1., 1., 62)];
        notes[1].velocity = 20;
        let out = with_velocity(&notes, &set(&[0, 1]), &notes, 40);
        assert_eq!((out[0].velocity, out[1].velocity), (127, 60));
        let out = with_velocity(&notes, &set(&[1]), &notes, -100);
        assert_eq!(out[1].velocity, 1);
    }

    #[test]
    fn boxes_select_what_they_touch() {
        let notes = [note(0., 1., 60), note(2., 1., 64), note(0.5, 1., 72)];
        assert_eq!(in_box(&notes, 0.8, 2.2, 58, 65), set(&[0, 1]));
        assert_eq!(
            in_box(&notes, 2.2, 0.8, 65, 58),
            set(&[0, 1]),
            "either way round"
        );
    }

    #[test]
    fn copy_and_paste_land_at_the_playhead() {
        let notes = [note(1., 1., 60), note(2., 1., 64)];
        let board = clipboard(&notes, &set(&[0, 1]));
        assert_eq!(board[0].start, Ticks::ZERO);
        let (out, pasted) = pasted(&notes, &board, beats(4.));
        assert_eq!(pasted, set(&[2, 3]));
        assert_eq!((out[2].start, out[3].start), (beats(4.), beats(5.)));
        let (out, _) = added(&notes, note(3., 1., 67));
        assert_eq!(deleted(&out, &set(&[0, 2])), [notes[1]]);
    }
}
