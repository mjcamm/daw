//! Dense content painted in one pass (`docs/design/ui-speed.md`, step 4).
//!
//! GPUI lays out and paints every element at a cost, and files every shape
//! it paints into an index each frame (about 2 µs a shape, even when a
//! cached view is reused). So many small shapes (notes in a clip, grid
//! lines, velocity bars) are painted by one canvas instead of one element
//! each, bars of one colour go into a single path (one shape however many
//! bars), and bars outside the visible area are skipped.

use gpui::{Bounds, Canvas, Hsla, Path, Pixels, Window, canvas, fill, point, px, size};

/// A rounded bar, placed from the canvas's top-left corner.
#[derive(Debug, Clone, Copy)]
pub struct Bar {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: Hsla,
    pub radius: f32,
}

/// Bars on screen with their corner radii.
type Rects = Vec<(Bounds<Pixels>, f32)>;

/// Up to this many bars of a colour are painted as rounded quads; more go
/// into one path (square corners, which nobody sees at that density).
const QUADS_UP_TO: usize = 6;

/// One canvas painting `bars`. Give it a size, usually `absolute` and
/// `size_full` over the area the bars are placed in.
pub fn bars(bars: Vec<Bar>) -> Canvas<()> {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| paint_bars(&bars, bounds, window),
    )
}

/// Paints `bars` placed from `bounds`' top-left, clipped to `bounds`,
/// skipping any outside the visible area.
pub fn paint_bars(bars: &[Bar], bounds: Bounds<Pixels>, window: &mut Window) {
    let visible = window.content_mask().bounds;
    // Visible bars, grouped by colour in first-seen order.
    let mut groups: Vec<(Hsla, Rects)> = Vec::new();
    for bar in bars {
        let rect = Bounds::new(
            point(bounds.left() + px(bar.x), bounds.top() + px(bar.y)),
            size(px(bar.w), px(bar.h)),
        )
        .intersect(&bounds);
        if rect.size.width <= px(0.) || rect.size.height <= px(0.) || !rect.intersects(&visible) {
            continue;
        }
        match groups.iter_mut().find(|(c, _)| *c == bar.color) {
            Some((_, rects)) => rects.push((rect, bar.radius)),
            None => groups.push((bar.color, vec![(rect, bar.radius)])),
        }
    }
    for (color, rects) in groups {
        if rects.len() <= QUADS_UP_TO {
            for (rect, radius) in rects {
                window.paint_quad(fill(rect, color).corner_radii(px(radius)));
            }
            continue;
        }
        let mut path = Path::new(rects[0].0.origin);
        let inside = (point(0., 1.), point(0., 1.), point(0., 1.));
        for (rect, _) in rects {
            let (a, b) = (rect.origin, rect.top_right());
            let (c, d) = (rect.bottom_right(), rect.bottom_left());
            path.push_triangle((a, b, c), inside);
            path.push_triangle((a, c, d), inside);
        }
        window.paint_path(path, color);
    }
}

/// The range of the timeline on screen, in pixels from beat 0, from a
/// horizontal scroll area, with a margin of a quarter screen either side
/// (scrolling redraws the pane in the same frame, so it never shows a gap).
/// `None` before the area has been laid out (then draw everything).
pub fn visible_x(handle: &gpui::ScrollHandle) -> Option<(f32, f32)> {
    let bounds = handle.bounds();
    let width = bounds.size.width / px(1.);
    if width <= 0. {
        return None;
    }
    let left = -handle.offset().x / px(1.);
    Some((left - width / 4., left + width * 1.25))
}

/// The same for a vertical scroll area: the range of rows on screen.
pub fn visible_y(handle: &gpui::ScrollHandle) -> Option<(f32, f32)> {
    let bounds = handle.bounds();
    let height = bounds.size.height / px(1.);
    if height <= 0. {
        return None;
    }
    let top = -handle.offset().y / px(1.);
    Some((top - height / 4., top + height * 1.25))
}
