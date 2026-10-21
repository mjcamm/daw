//! The loop strip along the top of every ruler (`docs/design/loop-strip.md`).
//! Any drag on it draws a fresh loop, except from a handle at either end,
//! which moves that end. Snapped to the page's grid; Alt flips it.
//!
//! The rules here are pure; `loop_view.rs` draws the strip and turns presses
//! into calls here.

use crate::block_edit::Snap;
use daw_engine::{LoopRange, MIN_LOOP_LENGTH, Ticks};

/// The strip's height, at the top of each ruler.
pub const STRIP_HEIGHT: f32 = 14.;
/// The start and end handles' width, inside the loop's ends.
pub const HANDLE_WIDTH: f32 = 6.;
/// How far outside an end a press still grabs its handle.
const OUTSIDE: f32 = 3.;

/// Which ruler a loop drag began in: they snap to different grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopRuler {
    /// Arrange, Parameters and Signals (the header's snap).
    Main,
    /// The piano roll (its own grid).
    Notes,
    /// The audio panel (the header's snap).
    Audio,
}

/// What a press on the strip takes hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    Start,
    End,
    /// Anywhere else: a fresh loop from the press.
    New,
}

/// A loop being drawn or one of its ends being moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopDrag {
    pub ruler: LoopRuler,
    pub grab: Grab,
    /// Where the press landed, unsnapped.
    pub press: Ticks,
    /// The ruler's zoom when pressed.
    pub pixels_per_beat: f32,
    /// The loop before the drag.
    pub original: Option<LoopRange>,
    /// What releasing now would set; `None` while a fresh loop is still
    /// shorter than one grid step.
    pub preview: Option<LoopRange>,
}

impl LoopDrag {
    /// The loop to draw while dragging.
    pub fn shown(&self) -> Option<LoopRange> {
        self.preview.or(self.original)
    }
}

/// What a press at `x` (pixels from the song start) takes hold of.
pub fn grab_at(range: Option<LoopRange>, x: f32, pixels_per_beat: f32) -> Grab {
    let Some(range) = range else {
        return Grab::New;
    };
    let start = range.start.as_beats() as f32 * pixels_per_beat;
    let end = range.end.as_beats() as f32 * pixels_per_beat;
    // On a short loop the handles share it, half each.
    let handle = HANDLE_WIDTH.min((end - start) / 2.);
    let on_start = x >= start - OUTSIDE && x <= start + handle;
    let on_end = x >= end - handle && x <= end + OUTSIDE;
    match (on_start, on_end) {
        (true, true) if (x - start).abs() <= (end - x).abs() => Grab::Start,
        (true, _) => Grab::Start,
        (_, true) => Grab::End,
        _ => Grab::New,
    }
}

/// The loop a drag makes with the pointer at `at` (unsnapped).
pub fn dragged(
    grab: Grab,
    press: Ticks,
    at: Ticks,
    original: Option<LoopRange>,
    snap: Snap,
) -> Option<LoopRange> {
    let shortest = snap.min_length().max(MIN_LOOP_LENGTH);
    let at = snap.round(at.max(Ticks::ZERO));
    match grab {
        Grab::New => {
            let anchor = snap.round(press.max(Ticks::ZERO));
            let (start, end) = if at >= anchor {
                (anchor, at)
            } else {
                (at, anchor)
            };
            (end - start >= shortest).then_some(LoopRange { start, end })
        }
        Grab::Start => original.map(|r| LoopRange {
            start: at.min(r.end - shortest).max(Ticks::ZERO),
            end: r.end,
        }),
        Grab::End => original.map(|r| LoopRange {
            start: r.start,
            end: at.max(r.start + shortest),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_edit::Grid;

    fn bars(start: i64, end: i64) -> Option<LoopRange> {
        Some(LoopRange {
            start: Ticks::from_bars(start),
            end: Ticks::from_bars(end),
        })
    }

    fn beats(b: f64) -> Ticks {
        Ticks::from_beats_f64(b)
    }

    const QUARTER: Snap = Snap {
        enabled: true,
        grid: Grid::Quarter,
    };
    const BAR: Snap = Snap {
        enabled: true,
        grid: Grid::Bar,
    };
    const SLIDE: Snap = Snap {
        enabled: false,
        grid: Grid::Quarter,
    };

    #[test]
    fn a_press_grabs_the_handle_it_is_on_else_starts_a_new_loop() {
        // Bars 2 to 4 at 10 px a beat: x 40 to 120.
        let range = bars(1, 3);
        assert_eq!(grab_at(range, 40., 10.), Grab::Start);
        assert_eq!(grab_at(range, 45., 10.), Grab::Start, "inside its handle");
        assert_eq!(grab_at(range, 38., 10.), Grab::Start, "just outside");
        assert_eq!(grab_at(range, 120., 10.), Grab::End);
        assert_eq!(grab_at(range, 115., 10.), Grab::End);
        assert_eq!(grab_at(range, 80., 10.), Grab::New, "the middle draws anew");
        assert_eq!(grab_at(range, 10., 10.), Grab::New);
        assert_eq!(grab_at(None, 40., 10.), Grab::New);
    }

    #[test]
    fn on_a_short_loop_the_nearer_end_wins() {
        // One beat at 4 px a beat: x 0 to 4.
        let range = Some(LoopRange {
            start: Ticks::ZERO,
            end: beats(1.),
        });
        assert_eq!(grab_at(range, 1., 4.), Grab::Start);
        assert_eq!(grab_at(range, 3., 4.), Grab::End);
    }

    #[test]
    fn a_new_loop_snaps_both_ends_either_way_round() {
        let made = dragged(Grab::New, beats(4.1), beats(11.8), bars(0, 1), QUARTER);
        assert_eq!(
            made,
            Some(LoopRange {
                start: beats(4.),
                end: beats(12.),
            })
        );
        let back = dragged(Grab::New, beats(12.2), beats(3.9), None, QUARTER);
        assert_eq!(back, made, "dragged leftwards");
        assert_eq!(
            dragged(Grab::New, beats(5.), beats(13.), None, BAR),
            bars(1, 3),
            "the bar grid"
        );
    }

    #[test]
    fn a_new_loop_needs_a_grid_step() {
        assert_eq!(
            dragged(Grab::New, beats(4.), beats(4.2), bars(0, 1), QUARTER),
            None
        );
        let tiny = dragged(Grab::New, beats(4.), beats(4.2), None, SLIDE);
        assert_eq!(
            tiny,
            Some(LoopRange {
                start: beats(4.),
                end: beats(4.2),
            }),
            "sliding: any length from the engine's shortest"
        );
        assert_eq!(
            dragged(Grab::New, beats(4.), beats(4.01), None, SLIDE),
            None
        );
    }

    #[test]
    fn handles_move_one_end_and_keep_a_step_between_them() {
        let range = bars(1, 3);
        assert_eq!(
            dragged(Grab::End, beats(12.), beats(19.9), range, QUARTER),
            bars(1, 5)
        );
        assert_eq!(
            dragged(Grab::Start, beats(4.), beats(0.2), range, QUARTER),
            bars(0, 3)
        );
        assert_eq!(
            dragged(Grab::Start, beats(4.), beats(30.), range, QUARTER),
            Some(LoopRange {
                start: beats(11.),
                end: beats(12.),
            }),
            "the start stops a step before the end"
        );
        assert_eq!(
            dragged(Grab::End, beats(12.), beats(-3.), range, QUARTER),
            Some(LoopRange {
                start: beats(4.),
                end: beats(5.),
            }),
            "the end stops a step after the start"
        );
        assert_eq!(
            dragged(Grab::Start, beats(4.), beats(-8.), range, QUARTER),
            bars(0, 3),
            "never before the song start"
        );
    }

    #[test]
    fn sliding_leaves_ends_where_the_pointer_is() {
        let moved = dragged(Grab::End, beats(12.), beats(13.37), bars(1, 3), SLIDE);
        assert_eq!(moved.map(|r| r.end), Some(beats(13.37)));
    }
}
