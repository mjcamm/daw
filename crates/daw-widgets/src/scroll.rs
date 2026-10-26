//! Scrollbars for timeline pages: a slim, always-visible track with a thumb
//! sized to how much of the content is in view. GPUI scroll areas draw no
//! scrollbars of their own, so the page draws these beside its scroll areas
//! and moves the areas' `ScrollHandle`s when a thumb is dragged.
//!
//! Wheel scrolling stays with GPUI, but a sideways scroll area inside a
//! vertical one must be [`axis_locked`]: otherwise GPUI turns a plain wheel
//! into a sideways scroll there while the page also scrolls down, and the
//! view moves diagonally. Locked, the wheel scrolls up and down and
//! Shift+wheel (which Windows sends as a sideways delta) scrolls sideways.

use crate::slider::Axis;
use crate::theme::Theme;
use gpui::{
    Div, ElementId, Pixels, Point, ScrollHandle, Stateful, Styled, div, point, prelude::*, px,
    relative,
};

/// Within this many pixels of a scroll area's edge, a drag scrolls it.
pub const EDGE_ZONE: f32 = 32.;
/// Fastest auto-scroll, in pixels a frame, at the very edge.
pub const EDGE_SPEED: f32 = 18.;

/// How far to auto-scroll this frame with the pointer at `at` in a view
/// running `low..high`: faster the deeper into an edge zone, negative
/// toward `low`, zero away from the edges.
pub fn edge_step(at: f32, low: f32, high: f32) -> f32 {
    let speed = |depth: f32| (depth / EDGE_ZONE).clamp(0., 1.) * EDGE_SPEED;
    if at > high - EDGE_ZONE {
        speed(at - (high - EDGE_ZONE))
    } else if at < low + EDGE_ZONE {
        -speed(low + EDGE_ZONE - at)
    } else {
        0.
    }
}

/// Cross size of a scrollbar.
pub const THICKNESS: f32 = crate::slider::THICKNESS;
/// Thinnest a thumb gets, as a share of the track.
const MIN_THUMB: f32 = 0.06;

/// Stops a scroll area from turning a wheel delta on the other axis into
/// its own (see the module notes).
pub fn axis_locked<E: Styled>(mut element: E) -> E {
    element.style().restrict_scroll_to_axis = Some(true);
    element
}

/// How far a scroll area can show along one axis: its visible length, its
/// content length and how far it is scrolled, all in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Extent {
    pub view: f32,
    pub content: f32,
    pub scrolled: f32,
}

impl Extent {
    /// Reads one axis of a scroll area.
    pub fn of(handle: &ScrollHandle, axis: Axis) -> Self {
        let bounds = handle.bounds();
        let max = handle.max_offset();
        let offset = handle.offset();
        let (view, max, offset) = match axis {
            Axis::Horizontal => (bounds.size.width, max.width, offset.x),
            Axis::Vertical => (bounds.size.height, max.height, offset.y),
        };
        let view = view / px(1.);
        Self {
            view,
            content: view + max / px(1.),
            scrolled: -offset / px(1.),
        }
    }

    fn max_scroll(self) -> f32 {
        (self.content - self.view).max(0.)
    }

    /// The thumb's start and length as shares of the track (`0..=1`), or
    /// `None` when everything fits and there is nothing to scroll.
    pub fn thumb(self) -> Option<(f32, f32)> {
        if self.max_scroll() <= 0.5 || self.content <= 0. {
            return None;
        }
        let length = (self.view / self.content).clamp(MIN_THUMB, 1.);
        let start = self.scrolled.clamp(0., self.max_scroll()) / self.max_scroll() * (1. - length);
        Some((start, length))
    }

    /// The scroll distance after dragging the thumb `delta` pixels along a
    /// track `track` pixels long, from where it was.
    pub fn dragged(self, delta: f32, track: f32) -> f32 {
        let Some((_, length)) = self.thumb() else {
            return self.scrolled;
        };
        let travel = track * (1. - length);
        if travel <= 0. {
            return self.scrolled;
        }
        (self.scrolled + delta / travel * self.max_scroll()).clamp(0., self.max_scroll())
    }
}

/// Scrolls `handle` to `scrolled` pixels along `axis`, keeping the other
/// axis where it is.
pub fn scroll_to(handle: &ScrollHandle, axis: Axis, scrolled: f32) {
    let offset = handle.offset();
    handle.set_offset(match axis {
        Axis::Horizontal => point(px(-scrolled), offset.y),
        Axis::Vertical => point(offset.x, px(-scrolled)),
    });
}

/// A scrollbar filling its parent along `axis`, showing `extent`. Attach
/// `on_mouse_down` to start a thumb drag. `active` highlights the thumb.
pub fn scrollbar(
    id: impl Into<ElementId>,
    axis: Axis,
    extent: Extent,
    active: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let thumb = extent.thumb().map(|(start, length)| {
        let thumb = div().absolute().rounded(px(THICKNESS / 2.)).bg(if active {
            theme.accent
        } else {
            theme.text_tertiary
        });
        match axis {
            Axis::Horizontal => thumb
                .top(px(2.))
                .bottom(px(2.))
                .left(relative(start))
                .w(relative(length)),
            Axis::Vertical => thumb
                .left(px(2.))
                .right(px(2.))
                .top(relative(start))
                .h(relative(length)),
        }
    });
    let bar = div().id(id).relative().bg(theme.sidebar);
    match axis {
        Axis::Horizontal => bar.h(px(THICKNESS)).flex_1().min_w_0(),
        Axis::Vertical => bar.w(px(THICKNESS)).flex_1().min_h_0(),
    }
    .children(thumb)
}

/// A scrollbar thumb being dragged.
#[derive(Debug, Clone, Copy)]
pub struct ScrollDrag {
    pub axis: Axis,
    pub origin: Point<Pixels>,
    /// The scroll area as it was when the drag began.
    pub extent: Extent,
    /// The track's length in pixels.
    pub track: f32,
}

impl ScrollDrag {
    /// The scroll distance with the pointer at `position`.
    pub fn scrolled_at(&self, position: Point<Pixels>) -> f32 {
        let delta = match self.axis {
            Axis::Horizontal => position.x - self.origin.x,
            Axis::Vertical => position.y - self.origin.y,
        } / px(1.);
        self.extent.dragged(delta, self.track)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(view: f32, content: f32, scrolled: f32) -> Extent {
        Extent {
            view,
            content,
            scrolled,
        }
    }

    #[test]
    fn the_thumb_shows_how_much_is_in_view() {
        assert_eq!(extent(200., 800., 0.).thumb(), Some((0., 0.25)));
        assert_eq!(
            extent(200., 800., 600.).thumb(),
            Some((0.75, 0.25)),
            "at the end"
        );
        assert_eq!(extent(800., 800., 0.).thumb(), None, "everything fits");
        let (_, tiny) = extent(10., 100_000., 0.).thumb().unwrap();
        assert_eq!(tiny, MIN_THUMB, "never too thin to grab");
    }

    #[test]
    fn dragging_the_thumb_scrolls_in_proportion() {
        // A 400 px track with a quarter-length thumb travels 300 px for the
        // full 600 px of scrolling.
        let e = extent(200., 800., 0.);
        assert_eq!(e.dragged(150., 400.), 300.);
        assert_eq!(e.dragged(1000., 400.), 600., "stops at the end");
        assert_eq!(e.dragged(-50., 400.), 0., "stops at the start");
        assert_eq!(
            extent(800., 800., 0.).dragged(50., 400.),
            0.,
            "nothing to scroll"
        );
    }

    #[test]
    fn dragging_near_an_edge_scrolls_faster_the_deeper() {
        assert_eq!(edge_step(500., 0., 1000.), 0.);
        assert_eq!(edge_step(1000., 0., 1000.), EDGE_SPEED);
        assert_eq!(
            edge_step(1000. - EDGE_ZONE / 2., 0., 1000.),
            EDGE_SPEED / 2.
        );
        assert_eq!(
            edge_step(-50., 0., 1000.),
            -EDGE_SPEED,
            "beyond the edge: full speed"
        );
    }
}
