//! The app's one slider: a hairline track filled up to the value, with a
//! round thumb. Used wherever a single value is dragged (a parameter's
//! resting value, a layer's mix, zoom), so they all look and feel the same.
//!
//! Sliders change by relative drags: the owner records where the press
//! began and calls [`dragged`] as the pointer moves.

use crate::theme::Theme;
use gpui::{Div, ElementId, Stateful, div, prelude::*, px};

/// Diameter of the thumb.
const THUMB: f32 = 9.;
/// Cross size of a slider.
pub const THICKNESS: f32 = 12.;
/// Pixels of drag across the full range, and with Shift held.
const DRAG_RANGE: f32 = 200.;
const DRAG_RANGE_FINE: f32 = 1000.;

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

/// A value after a drag of `travel` pixels from `original` (positive raises
/// it: right or up). Shift-style `fine` drags move five times slower.
pub fn dragged(original: f64, travel: f32, fine: bool) -> f64 {
    let range = if fine { DRAG_RANGE_FINE } else { DRAG_RANGE };
    (original + f64::from(travel / range)).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drags_move_the_value_and_stay_in_range() {
        assert!((dragged(0.5, 100., false) - 1.0).abs() < 1e-6);
        assert!((dragged(0.5, -20., false) - 0.4).abs() < 1e-6);
        assert!((dragged(0.5, 100., true) - 0.6).abs() < 1e-6);
        assert_eq!(dragged(0.9, 500., false), 1.);
        assert_eq!(dragged(0.1, -500., false), 0.);
    }
}
