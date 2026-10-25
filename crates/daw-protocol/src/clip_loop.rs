//! Looping clips (`docs/design/clip-loops.md`): a clip whose content
//! repeats. MIDI and audio clips loop the same way.
//!
//! A looped clip holds one **cycle** of content and plays it again and again
//! across its length. `phase` says where in the cycle the clip begins, so a
//! clip trimmed from the left, or the right half of a cut, carries on the
//! loop exactly where it was in song time. Repeats are linked: there is one
//! cycle of content, played at every repeat.

use crate::{AudioClip, MidiClip, MidiNote, Ticks};

/// How a clip repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClipLoop {
    /// The length of one cycle. More than zero.
    pub cycle: Ticks,
    /// Where in the cycle the clip's start falls, `0..cycle`.
    pub phase: Ticks,
}

/// A stretch of a looped clip that plays one run of the cycle without
/// wrapping: clip time `from..to` plays cycle time `cycle_from..`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    pub from: Ticks,
    pub to: Ticks,
    pub cycle_from: Ticks,
}

/// Seams inside a looped audio clip fade over this long each side, so the
/// jump back to the cycle's start never clicks (about 4 ms at 120 BPM).
pub const SEAM_FADE: Ticks = Ticks(8);

impl ClipLoop {
    /// A loop of `cycle` starting at its beginning.
    pub fn new(cycle: Ticks) -> Self {
        Self {
            cycle: cycle.max(Ticks(1)),
            phase: Ticks::ZERO,
        }
    }

    /// The cycle time playing at clip time `t`.
    pub fn cycle_time(&self, t: Ticks) -> Ticks {
        Ticks((self.phase.0 + t.0).rem_euclid(self.cycle.0))
    }

    /// The same loop for a clip whose start moved by `by` (content stays put
    /// in song time).
    pub fn shifted(&self, by: Ticks) -> Self {
        Self {
            phase: self.cycle_time(by),
            ..*self
        }
    }

    /// The runs of a clip `length` long, in order: each plays the cycle
    /// from `cycle_from` without wrapping. A seam lies between each two.
    pub fn pieces(&self, length: Ticks) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let mut from = Ticks::ZERO;
        while from < length {
            let cycle_from = self.cycle_time(from);
            let to = (from + (self.cycle - cycle_from)).min(length);
            pieces.push(Piece {
                from,
                to,
                cycle_from,
            });
            from = to;
        }
        pieces
    }

    /// Where each cycle ends inside a clip `length` long (clip time): the
    /// seams, for drawing.
    pub fn seams(&self, length: Ticks) -> Vec<Ticks> {
        let pieces = self.pieces(length);
        pieces.iter().skip(1).map(|p| p.from).collect()
    }
}

impl MidiClip {
    /// The notes that play, relative to the clip's start: only the parts
    /// inside the clip, and in a looped clip the cycle's notes at every
    /// repeat (each cut where its cycle ends), in time order.
    pub fn played_notes(&self) -> Vec<MidiNote> {
        let Some(looping) = self.looping else {
            return self
                .notes
                .iter()
                .filter_map(|n| {
                    let start = n.start.max(Ticks::ZERO);
                    let end = (n.start + n.length).min(self.length);
                    (start < end).then(|| n.cut(start, end))
                })
                .collect();
        };
        let mut played = Vec::new();
        for piece in looping.pieces(self.length) {
            let cycle_to = piece.cycle_from + (piece.to - piece.from);
            for n in &self.notes {
                // A note plays the part of it inside the cycle and the piece.
                let start = n.start.max(piece.cycle_from);
                let end = (n.start + n.length).min(cycle_to).min(looping.cycle);
                if start < end {
                    let mut note = n.cut(start, end);
                    note.start = piece.from + (start - piece.cycle_from);
                    played.push(note);
                }
            }
        }
        played.sort_by_key(|n| n.start);
        played
    }
}

impl AudioClip {
    /// The clip as plain windows onto its file, back to back: itself when it
    /// does not loop, else one per run of the cycle. The clip's fades stay at
    /// its ends; seams inside get a short fade each side (`SEAM_FADE`).
    pub fn pieces(&self, song_bpm: f64) -> Vec<AudioClip> {
        let Some(looping) = self.looping else {
            return vec![self.clone()];
        };
        let pieces = looping.pieces(self.length);
        let last = pieces.len().saturating_sub(1);
        pieces
            .iter()
            .enumerate()
            .map(|(i, piece)| {
                let length = piece.to - piece.from;
                let fade_in = if i == 0 { self.fade_in } else { SEAM_FADE };
                let fade_out = if i == last { self.fade_out } else { SEAM_FADE };
                let fade_in = fade_in.min(length);
                AudioClip {
                    start: self.start + piece.from,
                    length,
                    offset: self.offset
                        + piece.cycle_from.as_beats() * self.seconds_per_beat(song_bpm),
                    fade_in,
                    fade_out: fade_out.min(length - fade_in),
                    looping: None,
                    ..self.clone()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleId;

    fn beats(n: i64) -> Ticks {
        Ticks::from_beats(n)
    }

    fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: beats(start),
            length: beats(length),
            pitch,
            velocity: 100,
            bend: None,
        }
    }

    #[test]
    fn a_bent_note_cut_by_the_clip_or_a_seam_plays_its_part_of_the_bend() {
        // An octave glide over four beats.
        let glide = MidiNote {
            bend: Some(crate::Curve::line(
                crate::bend_value(0.0),
                crate::bend_value(12.0),
            )),
            ..note(2, 4, 60)
        };
        let cut = |looping| {
            MidiClip {
                start: beats(0),
                length: beats(4),
                notes: vec![glide.clone()],
                looping,
            }
            .played_notes()
        };
        // The clip ends halfway through it, as does the loop's cycle.
        for played in [cut(None), cut(Some(ClipLoop::new(beats(4))))] {
            let first = &played[0];
            assert_eq!(first.length, beats(2));
            assert!(
                (first.bend_at(1.0) - 6.0).abs() < 1e-6,
                "{}",
                first.bend_at(1.0)
            );
        }
    }

    #[test]
    fn a_loop_runs_in_pieces_that_never_wrap() {
        let looping = ClipLoop::new(beats(4));
        assert_eq!(
            looping.pieces(beats(10)),
            vec![
                Piece {
                    from: beats(0),
                    to: beats(4),
                    cycle_from: beats(0)
                },
                Piece {
                    from: beats(4),
                    to: beats(8),
                    cycle_from: beats(0)
                },
                Piece {
                    from: beats(8),
                    to: beats(10),
                    cycle_from: beats(0)
                },
            ]
        );
        // Starting a beat into the cycle (a clip trimmed from the left).
        let shifted = looping.shifted(beats(1));
        assert_eq!(shifted.phase, beats(1));
        assert_eq!(
            shifted.pieces(beats(5)),
            vec![
                Piece {
                    from: beats(0),
                    to: beats(3),
                    cycle_from: beats(1)
                },
                Piece {
                    from: beats(3),
                    to: beats(5),
                    cycle_from: beats(0)
                },
            ]
        );
        assert_eq!(shifted.seams(beats(5)), vec![beats(3)]);
    }

    #[test]
    fn a_looped_midi_clip_plays_its_cycle_at_every_repeat() {
        let clip = MidiClip {
            start: beats(8),
            length: beats(10),
            // The second note runs past the cycle's end: it is cut there.
            notes: vec![note(0, 1, 60), note(3, 3, 62)],
            looping: Some(ClipLoop::new(beats(4))),
        };
        let starts: Vec<(i64, i64, u8)> = clip
            .played_notes()
            .iter()
            .map(|n| (n.start.0 / 960, n.length.0 / 960, n.pitch))
            .collect();
        assert_eq!(
            starts,
            vec![(0, 1, 60), (3, 1, 62), (4, 1, 60), (7, 1, 62), (8, 1, 60)]
        );
    }

    #[test]
    fn a_phase_carries_the_loop_on_in_song_time() {
        // The right half of a clip cut a beat in: the same notes in the
        // same song places.
        let whole = MidiClip {
            start: beats(0),
            length: beats(8),
            notes: vec![note(0, 1, 60), note(2, 1, 64)],
            looping: Some(ClipLoop::new(beats(4))),
        };
        let right = MidiClip {
            start: beats(1),
            length: beats(7),
            looping: whole.looping.map(|l| l.shifted(beats(1))),
            ..whole.clone()
        };
        let song = |c: &MidiClip| -> Vec<(Ticks, u8)> {
            c.played_notes()
                .iter()
                .map(|n| (c.start + n.start, n.pitch))
                .collect()
        };
        let after_cut: Vec<_> = song(&whole)
            .into_iter()
            .filter(|(t, _)| *t >= beats(1))
            .collect();
        assert_eq!(song(&right), after_cut);
    }

    #[test]
    fn a_plain_clip_plays_the_parts_of_notes_inside_it() {
        let clip = MidiClip {
            start: beats(0),
            length: beats(4),
            notes: vec![note(-1, 2, 60), note(3, 4, 62), note(5, 1, 64)],
            looping: None,
        };
        let played: Vec<(i64, i64)> = clip
            .played_notes()
            .iter()
            .map(|n| (n.start.0 / 960, n.length.0 / 960))
            .collect();
        assert_eq!(played, vec![(0, 1), (3, 1)]);
    }

    #[test]
    fn a_looped_audio_clip_is_plain_windows_back_to_back() {
        // A 120 BPM file in a 120 BPM song: half a second a beat.
        let clip = AudioClip {
            fade_in: beats(1),
            fade_out: beats(1),
            offset: 2.0,
            looping: Some(ClipLoop::new(beats(4)).shifted(beats(2))),
            length: beats(8),
            ..AudioClip::whole_file(SampleId(1), beats(4), 10.0, 120.0)
        };
        let pieces = clip.pieces(120.0);
        let spans: Vec<(i64, i64, f64)> = pieces
            .iter()
            .map(|p| (p.start.0 / 960, p.length.0 / 960, p.offset))
            .collect();
        // Two beats into the cycle (a second into it), then whole cycles.
        assert_eq!(spans, vec![(4, 2, 3.0), (6, 4, 2.0), (10, 2, 2.0)]);
        assert!(pieces.iter().all(|p| p.looping.is_none()));
        assert_eq!(pieces[0].fade_in, beats(1), "the clip's own fade in");
        assert_eq!(pieces[0].fade_out, SEAM_FADE, "a seam");
        assert_eq!(pieces[1].fade_in, SEAM_FADE);
        assert_eq!(pieces[2].fade_out, beats(1), "the clip's own fade out");
    }

    #[test]
    fn a_plain_audio_clip_is_one_piece() {
        let clip = AudioClip::whole_file(SampleId(1), beats(0), 2.0, 120.0);
        assert_eq!(clip.pieces(120.0), vec![clip]);
    }

    #[test]
    fn trimming_a_looped_audio_clip_moves_its_phase_not_its_window() {
        let clip = AudioClip {
            looping: Some(ClipLoop::new(beats(2))),
            length: beats(6),
            ..AudioClip::whole_file(SampleId(1), beats(0), 1.0, 120.0)
        };
        let trimmed = clip.windowed(beats(1), beats(6), 120.0);
        assert_eq!(trimmed.offset, clip.offset, "the cycle stays where it is");
        assert_eq!(
            trimmed.looping,
            Some(ClipLoop::new(beats(2)).shifted(beats(1)))
        );
        let song = |c: &AudioClip| -> Vec<(Ticks, u64)> {
            c.pieces(120.0)
                .iter()
                .map(|p| (p.start, (p.offset * 1000.).round() as u64))
                .collect()
        };
        // Same audio at the same song times from beat 2 on.
        assert_eq!(song(&trimmed)[1..], song(&clip)[1..]);
    }

    #[test]
    fn a_new_tempo_scales_the_loop_with_the_clip() {
        let clip = AudioClip {
            looping: Some(ClipLoop::new(beats(2)).shifted(beats(1))),
            length: beats(6),
            ..AudioClip::whole_file(SampleId(1), beats(0), 1.0, 120.0)
        };
        let faster = clip.with_bpm(240.0);
        assert_eq!(faster.length, beats(12));
        assert_eq!(
            faster.looping,
            Some(ClipLoop::new(beats(4)).shifted(beats(2)))
        );
    }
}
