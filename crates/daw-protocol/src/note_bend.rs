//! Bent notes: a note's pitch over its length as a curve of semitones away
//! from its own pitch (`docs/design/note-bends.md`).
//!
//! A bend is a [`Curve`](crate::Curve) across the note (time `0..=1` from its start to its
//! end, so it stretches with the note), read in semitones: value `0.5` is
//! the note's pitch and each `1 / (2 * BEND_RANGE)` a semitone.

use crate::{MidiNote, Ticks, curve_value};

/// How far a bend reaches each way, in semitones.
pub const BEND_RANGE: f64 = 48.0;

/// A bend curve's value for `semitones` away from the note's pitch.
pub fn bend_value(semitones: f64) -> f64 {
    0.5 + semitones / (2.0 * BEND_RANGE)
}

/// Semitones away from the note's pitch for a bend curve's `value`.
pub fn bend_semitones(value: f64) -> f64 {
    (value - 0.5) * 2.0 * BEND_RANGE
}

impl MidiNote {
    /// Semitones away from its pitch at `phase` (`0..=1` across the note);
    /// 0 when it is straight.
    pub fn bend_at(&self, phase: f64) -> f64 {
        self.bend
            .as_ref()
            .map_or(0.0, |bend| bend_semitones(curve_value(&bend.points, phase)))
    }

    /// The part of the note from `start` to `end` (the clip times its own
    /// `start` is in, inside it), keeping that part of its bend: the piece
    /// that plays where a clip's end or a loop's seam cuts it.
    pub fn cut(&self, start: Ticks, end: Ticks) -> MidiNote {
        let bend = match &self.bend {
            Some(bend) if (start, end) != (self.start, self.start + self.length) => {
                let phase = |at: Ticks| (at - self.start).0 as f64 / self.length.0 as f64;
                Some(bend.slice(phase(start), phase(end)))
            }
            other => other.clone(),
        };
        MidiNote {
            start,
            length: end - start,
            bend,
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Curve;

    fn note(start: i64, length: i64, bend: Option<Curve>) -> MidiNote {
        MidiNote {
            start: Ticks(start),
            length: Ticks(length),
            pitch: 60,
            velocity: 100,
            bend,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn values_and_semitones_convert_both_ways() {
        assert_eq!(bend_value(0.0), 0.5);
        assert_eq!(bend_value(BEND_RANGE), 1.0);
        assert_eq!(bend_value(-BEND_RANGE), 0.0);
        assert!(close(bend_semitones(bend_value(7.0)), 7.0));
    }

    #[test]
    fn a_straight_note_does_not_bend() {
        assert_eq!(note(0, 960, None).bend_at(0.5), 0.0);
    }

    #[test]
    fn a_glide_from_below_slides_up_into_the_note() {
        // A C whose front is dragged down to an A: three below, then home.
        let glide = Curve::line(bend_value(-3.0), bend_value(0.0));
        let c = note(0, 960, Some(glide));
        assert!(close(c.bend_at(0.0), -3.0));
        assert!(close(c.bend_at(0.5), -1.5));
        assert!(close(c.bend_at(1.0), 0.0));
    }

    #[test]
    fn a_cut_note_keeps_its_part_of_the_bend() {
        let glide = Curve::line(bend_value(0.0), bend_value(12.0));
        let whole = note(0, 960, Some(glide));
        // The second half: from six up to twelve.
        let half = whole.cut(Ticks(480), Ticks(960));
        assert_eq!((half.start, half.length), (Ticks(480), Ticks(480)));
        assert!(close(half.bend_at(0.0), 6.0));
        assert!(close(half.bend_at(1.0), 12.0));
    }

    #[test]
    fn an_uncut_note_keeps_its_bend_as_it_is() {
        let glide = Curve::line(bend_value(0.0), bend_value(12.0));
        let whole = note(0, 960, Some(glide));
        assert_eq!(whole.cut(Ticks(0), Ticks(960)), whole);
        let straight = note(0, 960, None);
        assert_eq!(straight.cut(Ticks(0), Ticks(480)).bend, None);
    }
}
