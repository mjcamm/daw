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

/// The resize zone inside each end of a note, in pixels: it grows with the
/// note's width on screen (so long or zoomed-in notes are easy to catch),
/// but never takes more than a quarter of the note, so the middle always
/// moves it.
pub fn edge_inside(width: f64) -> f64 {
    (width * 0.15).clamp(EDGE_MIN, EDGE_MAX).min(width / 4.)
}

/// How far an end's resize zone hangs outside the note, in pixels, with
/// `gap` pixels free beyond it on its row: up to `OVERHANG`, and never past
/// half the gap, so two notes close together share it fairly.
pub fn edge_outside(gap: f64) -> f64 {
    (gap / 2.).clamp(0., OVERHANG)
}

pub const EDGE_MIN: f64 = 4.;
pub const EDGE_MAX: f64 = 10.;
pub const OVERHANG: f64 = 5.;

/// A note's resize zones at `pixels_per_beat`: (inside each end, outside
/// its start, outside its end), from the free space on its row.
pub fn edge_zones(notes: &[MidiNote], index: usize, pixels_per_beat: f64) -> (f64, f64, f64) {
    let n = &notes[index];
    let (start, stop) = (n.start, end(n));
    let row = notes
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != index && m.pitch == n.pitch);
    let before = row
        .clone()
        .filter(|(_, m)| end(m) <= start)
        .map(|(_, m)| end(m))
        .max()
        .unwrap_or(Ticks::ZERO);
    let after = row
        .filter(|(_, m)| m.start >= stop)
        .map(|(_, m)| m.start)
        .min();
    let px = |t: Ticks| t.as_beats() * pixels_per_beat;
    let width = px(stop - start);
    // Nothing after: the zone hangs out in full. Before the clip start
    // there is nowhere to go.
    let gap_after = after.map_or(f64::INFINITY, |a| px(a - stop));
    (
        edge_inside(width),
        edge_outside(px(start - before)),
        edge_outside(gap_after),
    )
}

/// The topmost note at `beat` (relative to the clip) and `pitch`, and which
/// part, at `pixels_per_beat`: inside a note, its ends' zones resize it
/// (`edge_inside`); just outside a note, in free space, its ends' zones
/// hang out to resize it too (`edge_outside`). Later notes are drawn on
/// top, so they win.
pub fn hit(
    notes: &[MidiNote],
    beat: f64,
    pitch: u8,
    pixels_per_beat: f64,
) -> Option<(usize, Part)> {
    let x = beat * pixels_per_beat;
    let px = |t: Ticks| t.as_beats() * pixels_per_beat;
    let inside = notes.iter().enumerate().rev().find_map(|(i, n)| {
        let (start, stop) = (px(n.start), px(end(n)));
        if n.pitch != pitch || x < start || x >= stop {
            return None;
        }
        let edge = edge_inside(stop - start);
        let part = if stop - x <= edge {
            Part::End
        } else if x - start <= edge {
            Part::Start
        } else {
            Part::Body
        };
        Some((i, part))
    });
    inside.or_else(|| {
        notes.iter().enumerate().rev().find_map(|(i, n)| {
            if n.pitch != pitch {
                return None;
            }
            let (_, before, after) = edge_zones(notes, i, pixels_per_beat);
            let (start, stop) = (px(n.start), px(end(n)));
            if x >= stop && x < stop + after {
                Some((i, Part::End))
            } else if x < start && x >= start - before {
                Some((i, Part::Start))
            } else {
                None
            }
        })
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

/// The notes with the selected ones' starts on the nearest `grid` line
/// (every note when nothing is selected); lengths stay as they were.
pub fn quantized(notes: &[MidiNote], selection: &Selection, grid: Ticks) -> Vec<MidiNote> {
    let g = grid.0.max(1);
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            if selection.is_empty() || selection.contains(&i) {
                MidiNote {
                    start: Ticks((n.start.0 + g / 2).div_euclid(g) * g),
                    ..*n
                }
            } else {
                *n
            }
        })
        .collect()
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
        // 100 px per beat.
        let notes = [note(0., 2., 60), note(1., 1., 60)];
        assert_eq!(hit(&notes, 0.5, 60, 100.), Some((0, Part::Body)));
        assert_eq!(
            hit(&notes, 1.5, 60, 100.),
            Some((1, Part::Body)),
            "the later note is on top"
        );
        assert_eq!(hit(&notes, 0.05, 60, 100.), Some((0, Part::Start)));
        assert_eq!(hit(&notes, 1.95, 60, 100.), Some((1, Part::End)));
        assert_eq!(hit(&notes, 0.5, 61, 100.), None);
        assert_eq!(hit(&notes, 2.5, 60, 100.), None);
    }

    #[test]
    fn resize_zones_grow_with_the_note_but_leave_its_middle() {
        assert_eq!(edge_inside(20.), 4., "at least 4 px");
        assert_eq!(edge_inside(40.), 6.);
        assert_eq!(edge_inside(400.), 10., "at most 10 px");
        assert_eq!(edge_inside(8.), 2., "a tiny note keeps half for moving");
    }

    #[test]
    fn resize_zones_hang_into_free_space() {
        // 100 px per beat; a short note, then a gap of 4 px, then another.
        let notes = [note(1., 0.08, 60), note(1.12, 0.5, 60), note(3., 1., 60)];
        let (_, before, after) = edge_zones(&notes, 0, 100.);
        assert_eq!(before, 5., "free space before: the full overhang");
        assert!((after - 2.).abs() < 0.1, "half the 4 px gap");
        let (_, _, after) = edge_zones(&notes, 2, 100.);
        assert_eq!(after, 5., "nothing after");
        // Just past the first note's end resizes it; past the shared gap's
        // middle belongs to the next note's start.
        assert_eq!(hit(&notes, 1.09, 60, 100.), Some((0, Part::End)));
        assert_eq!(hit(&notes, 1.11, 60, 100.), Some((1, Part::Start)));
        assert_eq!(hit(&notes, 4.04, 60, 100.), Some((2, Part::End)));
        assert_eq!(hit(&notes, 4.06, 60, 100.), None, "beyond the overhang");
        // A note right at the clip start has nothing before it to hang into.
        let first = [note(0., 1., 60)];
        assert_eq!(edge_zones(&first, 0, 100.).1, 0.);
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

    #[test]
    fn quantize_moves_starts_to_the_grid_and_keeps_lengths() {
        let notes = [note(0.1, 0.5, 60), note(1.4, 1., 62), note(2.6, 0.3, 64)];
        let q = quantized(&notes, &set(&[0, 2]), beats(1.));
        assert_eq!(q[0].start, beats(0.));
        assert_eq!(q[0].length, beats(0.5), "length kept");
        assert_eq!(q[1], notes[1], "not selected");
        assert_eq!(q[2].start, beats(3.));
        let all = quantized(&notes, &Selection::new(), beats(1.));
        assert_eq!(all[1].start, beats(1.), "nothing selected: every note");
    }
}
