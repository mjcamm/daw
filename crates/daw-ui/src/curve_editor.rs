//! The curve editor (`docs/design/curve-editor.md`): one curve across a box
//! with its grid, points and the selected point's handles, painted in one
//! canvas, and the geometry that turns the pointer into curve positions.
//! It knows curves, not what they are for: the page around it owns the
//! curve, the selection and the drags.

use crate::curve_edit::Side;
use daw_automation::curve_value;
use daw_engine::{Curve, CurvePoint, Handle, PointKind};
use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, fill, point, px, size};

/// Space around the curve inside its box, so the end points can be grabbed.
pub const PADDING: f32 = 12.;
/// How close the pointer must be to grab a point or handle.
const GRAB: f32 = 7.;
/// Half the size of a drawn point.
const POINT: f32 = 4.;
/// Radius of a drawn handle's end.
const HANDLE: f32 = 3.;
/// Grid lines closer than this are not drawn.
const MIN_GRID_GAP: f32 = 6.;

/// What a press lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveHit {
    Point(usize),
    Handle(usize, Side),
}

/// Where the curve is drawn, in window pixels: time across, value up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveArea {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl CurveArea {
    /// The curve's area inside a box laid out at `bounds`.
    pub fn new(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: bounds.left() / px(1.) + PADDING,
            top: bounds.top() / px(1.) + PADDING,
            width: (bounds.size.width / px(1.) - 2. * PADDING).max(1.),
            height: (bounds.size.height / px(1.) - 2. * PADDING).max(1.),
        }
    }

    /// The editor's width over its height: how much a unit of time weighs
    /// against a unit of value on screen.
    pub fn aspect(&self) -> f64 {
        f64::from(self.width / self.height)
    }

    pub fn screen_at(&self, at: f64, value: f64) -> (f32, f32) {
        (
            self.left + at as f32 * self.width,
            self.top + (1. - value as f32) * self.height,
        )
    }

    /// The curve position under `x`, `y`, held inside the curve.
    pub fn curve_at(&self, x: f32, y: f32) -> (f64, f64) {
        let at = f64::from((x - self.left) / self.width).clamp(0., 1.);
        let value = f64::from(1. - (y - self.top) / self.height).clamp(0., 1.);
        (at, value)
    }

    /// A handle reaching from `point` to the pointer at `x`, `y`.
    pub fn handle_to(&self, point: &CurvePoint, x: f32, y: f32) -> Handle {
        let (px_, py_) = self.screen_at(point.at, point.value);
        Handle::new(
            f64::from((x - px_) / self.width),
            f64::from(-(y - py_) / self.height),
        )
    }

    fn handle_end(&self, point: &CurvePoint, handle: Handle) -> (f32, f32) {
        self.screen_at(point.at + handle.at, point.value + handle.value)
    }
}

/// What a press at `x`, `y` lands on: the selected point's handles first,
/// then the nearest point in reach.
pub fn hit(
    curve: &Curve,
    selected: Option<usize>,
    area: CurveArea,
    x: f32,
    y: f32,
) -> Option<CurveHit> {
    let near = |(hx, hy): (f32, f32)| {
        let d = (hx - x).hypot(hy - y);
        (d <= GRAB).then_some(d)
    };
    if let Some(index) = selected
        && let Some(p) = curve.points.get(index)
        && p.has_handles()
    {
        for (side, handle) in [(Side::In, p.handle_in), (Side::Out, p.handle_out)] {
            if handle != Handle::NONE && near(area.handle_end(p, handle)).is_some() {
                return Some(CurveHit::Handle(index, side));
            }
        }
    }
    curve
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| near(area.screen_at(p.at, p.value)).map(|d| (i, d)))
        // The later of two points on one spot, so a jump's second point can
        // be grabbed off the first.
        .min_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(i, _)| CurveHit::Point(i))
}

/// The colours the editor paints with.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub line: Hsla,
    pub fill: Hsla,
    pub grid: Hsla,
    pub centre: Hsla,
    pub point: Hsla,
    /// Inside unselected points.
    pub hollow: Hsla,
    pub selected: Hsla,
}

/// Paints `curve` over a box laid out at `bounds`: the grid of `cells`
/// across, the centre and quarter lines, the curve with its area to the
/// centre line, the points, and the selected point's handles.
pub fn paint(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    curve: &Curve,
    selected: Option<usize>,
    cells: usize,
    look: Look,
) {
    let area = CurveArea::new(bounds);
    let at = |x: f32, y: f32| point(px(x), px(y));
    let rect = |x: f32, y: f32, w: f32, h: f32| Bounds::new(at(x, y), size(px(w), px(h)));

    // The grid: one path for the cell lines, the quarter lines, then the
    // centre line on top.
    let mut grid = gpui::Path::new(at(area.left, area.top));
    let mut push_rect = |r: Bounds<Pixels>| {
        let inside = (point(0., 1.), point(0., 1.), point(0., 1.));
        let (a, b, c, d) = (r.origin, r.top_right(), r.bottom_right(), r.bottom_left());
        grid.push_triangle((a, b, c), inside);
        grid.push_triangle((a, c, d), inside);
    };
    let cells = cells.max(1);
    if area.width / cells as f32 >= MIN_GRID_GAP {
        for cell in 0..=cells {
            let (x, _) = area.screen_at(cell as f64 / cells as f64, 0.);
            push_rect(rect(x.round() - 0.5, area.top, 1., area.height));
        }
    }
    for value in [0., 0.25, 0.75, 1.] {
        let (_, y) = area.screen_at(0., value);
        push_rect(rect(area.left, y.round() - 0.5, area.width, 1.));
    }
    window.paint_path(grid, look.grid);
    let (_, centre) = area.screen_at(0., 0.5);
    window.paint_quad(fill(
        rect(area.left, centre.round() - 0.5, area.width, 1.),
        look.centre,
    ));

    // The curve: every segment sampled every two pixels, jumps straight up
    // or down where two points share a time.
    let points = &curve.points;
    let mut line: Vec<(f32, f32)> = Vec::new();
    for (i, a) in points.iter().enumerate() {
        line.push(area.screen_at(a.at, a.value));
        let Some(b) = points.get(i + 1) else { break };
        let span = (b.at - a.at) as f32 * area.width;
        let steps = (span / 2.).ceil() as usize;
        for step in 1..steps {
            let phase = a.at + (b.at - a.at) * step as f64 / steps as f64;
            line.push(area.screen_at(phase, curve_value(points, phase)));
        }
    }
    if let (Some(first), Some(last)) = (line.first().copied(), line.last().copied()) {
        let mut shade = PathBuilder::fill();
        shade.move_to(at(first.0, centre));
        line.iter().for_each(|&(x, y)| shade.line_to(at(x, y)));
        shade.line_to(at(last.0, centre));
        shade.close();
        if let Ok(path) = shade.build() {
            window.paint_path(path, look.fill);
        }
        let mut stroke = PathBuilder::stroke(px(1.5));
        stroke.move_to(at(first.0, first.1));
        line.iter()
            .skip(1)
            .for_each(|&(x, y)| stroke.line_to(at(x, y)));
        if let Ok(path) = stroke.build() {
            window.paint_path(path, look.line);
        }
    }

    // The selected point's handles: a line out to each end, and the end.
    if let Some(p) = selected.and_then(|i| points.get(i))
        && p.has_handles()
    {
        let (px_, py_) = area.screen_at(p.at, p.value);
        for handle in [p.handle_in, p.handle_out] {
            if handle == Handle::NONE {
                continue;
            }
            let (hx, hy) = area.handle_end(p, handle);
            let mut arm = PathBuilder::stroke(px(1.));
            arm.move_to(at(px_, py_));
            arm.line_to(at(hx, hy));
            if let Ok(path) = arm.build() {
                window.paint_path(path, look.selected);
            }
            window.paint_quad(
                fill(
                    rect(hx - HANDLE, hy - HANDLE, 2. * HANDLE, 2. * HANDLE),
                    look.selected,
                )
                .corner_radii(px(HANDLE)),
            );
        }
    }

    // The points: square corners, round smooth points, diamond broken ones.
    // Every point's outline goes in one path and every inside in another
    // (one shape each however many points); the selected point on top.
    let mut outlines = Shapes::default();
    let mut insides = Shapes::default();
    let mut chosen = Shapes::default();
    for (i, p) in points.iter().enumerate() {
        let (x, y) = area.screen_at(p.at, p.value);
        let (sides, turn, scale) = match p.kind {
            // A square standing on its side: corners on the diagonals.
            PointKind::Corner => (4, std::f32::consts::FRAC_PI_4, std::f32::consts::SQRT_2),
            PointKind::Smooth => (12, 0., 1.),
            PointKind::Broken => (4, 0., 1.3),
        };
        let outer = POINT * scale;
        let inner = (POINT - 1.5) * scale;
        if selected == Some(i) {
            chosen.polygon(x, y, outer, sides, turn);
        } else {
            outlines.polygon(x, y, outer, sides, turn);
            insides.polygon(x, y, inner, sides, turn);
        }
    }
    outlines.paint(window, look.point);
    insides.paint(window, look.hollow);
    chosen.paint(window, look.selected);
}

/// Filled shapes gathered into one path.
#[derive(Default)]
struct Shapes(Option<gpui::Path<Pixels>>);

impl Shapes {
    /// A regular polygon of `sides` around `x`, `y`, its corners `radius`
    /// out, the first `turn` radians round from straight up.
    fn polygon(&mut self, x: f32, y: f32, radius: f32, sides: usize, turn: f32) {
        let corner = |k: usize| {
            let angle = turn + k as f32 * std::f32::consts::TAU / sides as f32;
            point(px(x + radius * angle.sin()), px(y - radius * angle.cos()))
        };
        let centre = point(px(x), px(y));
        let path = self.0.get_or_insert_with(|| gpui::Path::new(centre));
        // Filled: every vertex at (0, 1) in the curve space.
        let inside = (
            gpui::point(0., 1.),
            gpui::point(0., 1.),
            gpui::point(0., 1.),
        );
        for k in 0..sides {
            path.push_triangle((centre, corner(k), corner(k + 1)), inside);
        }
    }

    fn paint(self, window: &mut Window, color: Hsla) {
        if let Some(path) = self.0 {
            window.paint_path(path, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    /// A 200 × 100 curve area at the window's top-left.
    fn area() -> CurveArea {
        CurveArea::new(Bounds::new(
            point(px(0.), px(0.)),
            size(px(200. + 2. * PADDING), px(100. + 2. * PADDING)),
        ))
    }

    fn triangle() -> Curve {
        Curve {
            points: vec![
                CurvePoint::corner(0., 0.),
                CurvePoint::corner(0.5, 1.),
                CurvePoint::corner(1., 0.),
            ],
        }
    }

    #[test]
    fn positions_round_trip_through_the_screen() {
        let area = area();
        assert_eq!(area.screen_at(0., 1.), (PADDING, PADDING));
        assert_eq!(area.screen_at(1., 0.), (PADDING + 200., PADDING + 100.));
        let (x, y) = area.screen_at(0.25, 0.75);
        assert_eq!(area.curve_at(x, y), (0.25, 0.75));
        assert_eq!(area.curve_at(-50., 500.), (0., 0.), "held inside");
        assert_eq!(area.aspect(), 2.);
    }

    #[test]
    fn presses_find_the_nearest_point_in_reach() {
        let (x, y) = area().screen_at(0.5, 1.);
        assert_eq!(
            hit(&triangle(), None, area(), x + 3., y + 3.),
            Some(CurveHit::Point(1))
        );
        assert_eq!(hit(&triangle(), None, area(), x + 20., y), None);
    }

    #[test]
    fn the_second_point_of_a_jump_is_the_one_grabbed() {
        let jump = Curve {
            points: vec![
                CurvePoint::corner(0., 0.),
                CurvePoint::corner(0.5, 0.5),
                CurvePoint::corner(0.5, 0.5),
                CurvePoint::corner(1., 1.),
            ],
        };
        let (x, y) = area().screen_at(0.5, 0.5);
        assert_eq!(hit(&jump, None, area(), x, y), Some(CurveHit::Point(2)));
    }

    #[test]
    fn only_the_selected_points_handles_can_be_grabbed_before_points() {
        let mut curve = triangle();
        curve.points[1] = curve.points[1].with_handles(
            PointKind::Smooth,
            Handle::new(-0.1, 0.),
            Handle::new(0.1, 0.),
        );
        let (x, y) = area().screen_at(0.6, 1.);
        assert_eq!(hit(&curve, None, area(), x, y), None, "not selected");
        assert_eq!(
            hit(&curve, Some(1), area(), x, y),
            Some(CurveHit::Handle(1, Side::Out))
        );
    }

    #[test]
    fn a_handle_reaches_from_its_point_to_the_pointer() {
        let area = area();
        let p = CurvePoint::corner(0.5, 0.5);
        let (x, y) = area.screen_at(0.75, 0.25);
        let handle = area.handle_to(&p, x, y);
        assert!((handle.at - 0.25).abs() < 1e-6 && (handle.value + 0.25).abs() < 1e-6);
    }
}
