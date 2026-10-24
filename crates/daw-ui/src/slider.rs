//! The app's one slider: a hairline track filled up to the value, with a
//! round thumb. Used wherever a single value is dragged (a parameter's
//! resting value, a layer's mix, zoom), so they all look and feel the same.
//!
//! Sliders change by relative drags: the owner records where the press
//! began and calls [`dragged`] as the pointer moves. The thumb stays under
//! the pointer (a drag the slider's length covers its range).

use crate::theme::Theme;
use gpui::{Div, ElementId, Stateful, div, prelude::*, px};

/// Diameter of the thumb.
const THUMB: f32 = 9.;
/// Cross size of a slider.
pub const THICKNESS: f32 = 12.;
/// How much slower a fine (Shift) drag moves.
const FINE: f32 = 5.;
/// The length to pass [`dragged`] for a value scrubbed without a slider:
/// 200 pixels across the full range.
pub const SCRUB_LENGTH: f32 = 200. + THUMB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Left is 0, right is 1.
    Horizontal,
    /// Bottom is 0, top is 1.
    Vertical,
}

/// A slider `length` pixels long showing `value` (`0..=1`). Attach
/// `on_mouse_down` to start a drag. `active` highlights it while dragged.
pub fn slider(
    id: impl Into<ElementId>,
    axis: Axis,
    value: f32,
    length: f32,
    active: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let travel = (length - THUMB).max(0.);
    let along = value.clamp(0., 1.) * travel;
    let cross = (THICKNESS - THUMB) / 2.;
    let color = if active {
        theme.accent
    } else {
        theme.text_secondary
    };
    let track = div().absolute().rounded(px(1.)).bg(theme.control);
    let fill = div().absolute().rounded(px(1.)).bg(color);
    let thumb = div()
        .absolute()
        .size(px(THUMB))
        .rounded(px(THUMB / 2.))
        .bg(color);
    let line = THICKNESS / 2. - 1.;
    let (base, track, fill, thumb) = match axis {
        Axis::Horizontal => (
            div().w(px(length)).h(px(THICKNESS)).cursor_ew_resize(),
            track
                .left(px(THUMB / 2.))
                .right(px(THUMB / 2.))
                .top(px(line))
                .h(px(2.)),
            fill.left(px(THUMB / 2.))
                .w(px(along))
                .top(px(line))
                .h(px(2.)),
            thumb.left(px(along)).top(px(cross)),
        ),
        Axis::Vertical => (
            div().w(px(THICKNESS)).h(px(length)).cursor_ns_resize(),
            track
                .top(px(THUMB / 2.))
                .bottom(px(THUMB / 2.))
                .left(px(line))
                .w(px(2.)),
            fill.bottom(px(THUMB / 2.))
                .h(px(along))
                .left(px(line))
                .w(px(2.)),
            thumb.bottom(px(along)).left(px(cross)),
        ),
    };
    base.id(id)
        .flex_none()
        .relative()
        .child(track)
        .child(fill)
        .child(thumb)
}

/// A value after a drag of `moved` pixels from `original` (positive raises
/// it: right or up) on a slider `length` pixels long, so the thumb follows
/// the pointer. Shift-style `fine` drags move five times slower.
pub fn dragged(original: f64, moved: f32, length: f32, fine: bool) -> f64 {
    let travel = (length - THUMB).max(1.);
    let range = if fine { travel * FINE } else { travel };
    (original + f64::from(moved / range)).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_move_the_value_and_stay_in_range() {
        let length = SCRUB_LENGTH;
        assert!((dragged(0.5, 100., length, false) - 1.0).abs() < 1e-6);
        assert!((dragged(0.5, -20., length, false) - 0.4).abs() < 1e-6);
        assert!((dragged(0.5, 100., length, true) - 0.6).abs() < 1e-6);
        assert_eq!(dragged(0.9, 500., length, false), 1.);
        assert_eq!(dragged(0.1, -500., length, false), 0.);
    }

    #[test]
    fn the_thumb_follows_the_pointer() {
        // The thumb travels the slider's length less its own size: a drag
        // that far covers the whole range, on any length.
        for length in [40., 109., 300.] {
            let travel = length - THUMB;
            assert!((dragged(0., travel, length, false) - 1.).abs() < 1e-6);
            assert!((dragged(0.5, travel / 4., length, false) - 0.75).abs() < 1e-6);
        }
    }
}
