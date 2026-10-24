//! Zoom for timeline pages: horizontal (pixels per beat) and vertical (row
//! height), each set by a short slim slider in the page's bottom-right
//! corner. The sliders work on a log scale, so every bit of travel zooms by
//! the same proportion across a wide range. Each page keeps its own `Zoom`.
//!
//! Shared by every timeline page: the page's frame (`frame.rs`) holds a
//! `Zoom` and draws the sliders at the ends of its scrollbars. Over the
//! frame, Ctrl+wheel zooms rows and Ctrl+Shift+wheel zooms time, about the
//! pointer.

use crate::slider;
use crate::theme::Theme;
use gpui::{Div, ElementId, Pixels, Point, Stateful, px};

const MIN_PIXELS_PER_BEAT: f32 = 2.;
const MAX_PIXELS_PER_BEAT: f32 = 400.;
/// Rows always fit two lines of text.
const MIN_ROW_HEIGHT: f32 = 44.;
const MAX_ROW_HEIGHT: f32 = 400.;
/// Length of a slider's travel.
const SLIDER_LENGTH: f32 = 80.;
/// How much one notch of Ctrl+wheel zooms.
pub const WHEEL_STEP: f32 = 1.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Time: pixels per beat.
    Horizontal,
    /// Row height.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    pub pixels_per_beat: f32,
    pub row_height: f32,
    /// The row height's range: node rows by default, keys in the piano roll.
    rows: (f32, f32),
}

impl Zoom {
    pub const fn new(pixels_per_beat: f32, row_height: f32) -> Self {
        Self {
            pixels_per_beat,
            row_height,
            rows: (MIN_ROW_HEIGHT, MAX_ROW_HEIGHT),
        }
    }

    /// This zoom with rows ranging `min..=max` high.
    pub const fn with_rows(self, min: f32, max: f32) -> Self {
        Self {
            rows: (min, max),
            ..self
        }
    }

    fn range(self, axis: Axis) -> (f32, f32) {
        match axis {
            Axis::Horizontal => (MIN_PIXELS_PER_BEAT, MAX_PIXELS_PER_BEAT),
            Axis::Vertical => self.rows,
        }
    }

    fn get(self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.pixels_per_beat,
            Axis::Vertical => self.row_height,
        }
    }

    /// Where the slider for `axis` sits, `0..=1` (0 is zoomed out).
    pub fn position(self, axis: Axis) -> f32 {
        let (min, max) = self.range(axis);
        ((self.get(axis) / min).ln() / (max / min).ln()).clamp(0., 1.)
    }

    /// This zoom with the slider for `axis` at `position`.
    pub fn with_position(self, axis: Axis, position: f32) -> Self {
        let (min, max) = self.range(axis);
        let value = min * (max / min).powf(position.clamp(0., 1.));
        match axis {
            Axis::Horizontal => Self {
                pixels_per_beat: value,
                ..self
            },
            Axis::Vertical => Self {
                row_height: value.round(),
                ..self
            },
        }
    }

    /// This zoom after dragging the `axis` slider by `delta` pixels from
    /// where it was (right or up zooms in).
    pub fn dragged(self, axis: Axis, delta: Point<f32>) -> Self {
        let travel = match axis {
            Axis::Horizontal => delta.x,
            Axis::Vertical => -delta.y,
        };
        self.with_position(axis, self.position(axis) + travel / SLIDER_LENGTH)
    }

    /// This zoom after `notches` of Ctrl+wheel on `axis` (positive zooms
    /// in): each notch by the same proportion, within the axis's range.
    pub fn wheeled(self, axis: Axis, notches: f32) -> Self {
        let (min, max) = self.range(axis);
        let value = (self.get(axis) * WHEEL_STEP.powf(notches)).clamp(min, max);
        match axis {
            Axis::Horizontal => Self {
                pixels_per_beat: value,
                ..self
            },
            Axis::Vertical => Self {
                row_height: value.round(),
                ..self
            },
        }
    }

    pub fn beat_to_x(self, beats: f64) -> f32 {
        beats as f32 * self.pixels_per_beat
    }

    pub fn x_to_beats(self, x: f32) -> f64 {
        f64::from(x / self.pixels_per_beat)
    }
}

/// The new scroll distance (how far the view is scrolled right, in pixels)
/// that keeps the beat `anchor` pixels into the view in place when the
/// horizontal zoom changes.
pub fn keep_anchor(scrolled: f32, anchor: f32, old_ppb: f32, new_ppb: f32) -> f32 {
    ((scrolled + anchor) * new_ppb / old_ppb - anchor).max(0.)
}

/// Where horizontal zoom should hold still, in pixels from the view's left
/// edge: the playhead when it is in view (so zooming at the song start keeps
/// the start in place), otherwise the middle of the view. `playhead` is the
/// playhead's position on the timeline, `scrolled` how far the view is
/// scrolled right and `view` its width, all in pixels.
pub fn zoom_anchor(playhead: f32, scrolled: f32, view: f32) -> f32 {
    let in_view = playhead - scrolled;
    if (0.0..=view).contains(&in_view) {
        in_view
    } else {
        view / 2.
    }
}

/// A zoom slider being dragged.
#[derive(Debug, Clone, Copy)]
pub struct ZoomDrag {
    pub axis: Axis,
    pub origin: Point<Pixels>,
    pub original: Zoom,
}

impl ZoomDrag {
    /// The zoom with the pointer at `position`.
    pub fn zoom_at(&self, position: Point<Pixels>) -> Zoom {
        let delta = Point {
            x: (position.x - self.origin.x) / px(1.),
            y: (position.y - self.origin.y) / px(1.),
        };
        self.original.dragged(self.axis, delta)
    }
}

/// A zoom slider: the app's standard slider (see `slider.rs`), `position`
/// along its travel. Attach `on_mouse_down` to start a drag. Timeline
/// pages place these at the ends of their scrollbars (see `frame.rs`).
pub fn slider(
    id: impl Into<ElementId>,
    axis: Axis,
    position: f32,
    active: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let orientation = match axis {
        Axis::Horizontal => slider::Axis::Horizontal,
        Axis::Vertical => slider::Axis::Vertical,
    };
    slider::slider(id, orientation, position, SLIDER_LENGTH, active, theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_notch_zooms_by_a_step_and_stays_in_range() {
        let zoom = Zoom::new(24., 56.);
        let wider = zoom.wheeled(Axis::Horizontal, 1.);
        assert!((wider.pixels_per_beat - 24. * WHEEL_STEP).abs() < 1e-4);
        assert_eq!(wider.row_height, 56., "only its own axis");
        let back = wider.wheeled(Axis::Horizontal, -1.);
        assert!(
            (back.pixels_per_beat - 24.).abs() < 1e-3,
            "a notch back undoes it"
        );
        let taller = zoom.wheeled(Axis::Vertical, 2.);
        assert_eq!(taller.row_height, (56. * WHEEL_STEP * WHEEL_STEP).round());
        assert_eq!(taller.pixels_per_beat, 24.);
        assert_eq!(
            zoom.wheeled(Axis::Horizontal, 100.).pixels_per_beat,
            MAX_PIXELS_PER_BEAT
        );
        assert_eq!(
            zoom.wheeled(Axis::Vertical, -100.).row_height,
            MIN_ROW_HEIGHT
        );
    }

    #[test]
    fn slider_positions_span_the_range_on_a_log_scale() {
        let zoom = Zoom::new(24., 56.);
        let out = zoom.with_position(Axis::Horizontal, 0.);
        let in_ = zoom.with_position(Axis::Horizontal, 1.);
        assert_eq!(out.pixels_per_beat, MIN_PIXELS_PER_BEAT);
        assert!((in_.pixels_per_beat - MAX_PIXELS_PER_BEAT).abs() < 1e-3);
        let middle = zoom.with_position(Axis::Horizontal, 0.5).pixels_per_beat;
        let geometric = (MIN_PIXELS_PER_BEAT * MAX_PIXELS_PER_BEAT).sqrt();
        assert!((middle - geometric).abs() < 1e-3);
        assert_eq!(out.row_height, 56., "the other axis is untouched");
    }

    #[test]
    fn positions_round_trip() {
        let zoom = Zoom::new(24., 56.);
        let p = zoom.position(Axis::Horizontal);
        let back = zoom.with_position(Axis::Horizontal, p).pixels_per_beat;
        assert!((back - 24.).abs() < 1e-3);
        let rows = zoom.with_position(Axis::Vertical, zoom.position(Axis::Vertical));
        assert_eq!(rows.row_height, 56.);
    }

    #[test]
    fn dragging_right_or_up_zooms_in() {
        let zoom = Zoom::new(24., 56.);
        let wider = zoom.dragged(Axis::Horizontal, Point { x: 10., y: 0. });
        assert!(wider.pixels_per_beat > 24.);
        let taller = zoom.dragged(Axis::Vertical, Point { x: 0., y: -10. });
        assert!(taller.row_height > 56.);
        let shorter = zoom.dragged(Axis::Vertical, Point { x: 0., y: 500. });
        assert_eq!(shorter.row_height, MIN_ROW_HEIGHT);
    }

    #[test]
    fn zooming_keeps_the_anchor_on_the_same_beat() {
        // 300 px scrolled at 24 px/beat with the anchor 200 px in: beat 20.8.
        let scrolled = keep_anchor(300., 200., 24., 48.);
        assert!(((scrolled + 200.) / 48. - 500. / 24.).abs() < 1e-4);
        assert_eq!(
            keep_anchor(0., 200., 24., 12.),
            0.,
            "never before the start"
        );
    }

    #[test]
    fn zoom_holds_the_playhead_still_when_it_is_in_view() {
        assert_eq!(zoom_anchor(0., 0., 800.), 0., "at the song start");
        assert_eq!(zoom_anchor(500., 200., 800.), 300.);
        assert_eq!(
            zoom_anchor(5000., 200., 800.),
            400.,
            "off screen: the middle"
        );
        // With the playhead at the start, zooming in keeps the start put.
        assert_eq!(keep_anchor(0., zoom_anchor(0., 0., 800.), 24., 96.), 0.);
    }

    #[test]
    fn beats_and_pixels_convert_at_the_zoom() {
        let zoom = Zoom::new(10., 40.);
        assert_eq!(zoom.beat_to_x(2.5), 25.);
        assert_eq!(zoom.x_to_beats(25.), 2.5);
    }
}
