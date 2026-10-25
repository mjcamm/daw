//! Curves: one cycle of a shape as points joined by straight lines or Bézier
//! curves, like a pen path (`docs/design/curve-editor.md`).

/// One cycle of a shape: time `0..=1` across, value `0..=1` up (`0.5` is the
/// centre line). Points are sorted by time; the first sits at 0 and the last
/// at 1. Two points at the same time make a jump.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Curve {
    pub points: Vec<CurvePoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CurvePoint {
    /// Time within the cycle, `0..=1`.
    pub at: f64,
    /// `0..=1`.
    pub value: f64,
    pub kind: PointKind,
    /// Offset of the handle shaping the curve in from the previous point.
    /// Its time is never after the point (`at <= 0`).
    pub handle_in: Handle,
    /// Offset of the handle shaping the curve out to the next point. Its
    /// time is never before the point (`at >= 0`).
    pub handle_out: Handle,
}

/// How a point joins its two segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PointKind {
    /// Sharp, straight lines both sides. The handles are kept but not used,
    /// so switching back restores them.
    Corner,
    /// Curves through without a kink: the editor keeps the handles in a
    /// straight line through the point.
    Smooth,
    /// A sharp tip with curves both sides: each handle moves on its own.
    Broken,
}

/// A handle as an offset from its point, in the curve's time and value.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Handle {
    pub at: f64,
    pub value: f64,
}

impl Handle {
    pub const NONE: Self = Self {
        at: 0.0,
        value: 0.0,
    };

    pub fn new(at: f64, value: f64) -> Self {
        Self { at, value }
    }
}

impl CurvePoint {
    /// A corner point with no handles.
    pub fn corner(at: f64, value: f64) -> Self {
        Self {
            at,
            value,
            kind: PointKind::Corner,
            handle_in: Handle::NONE,
            handle_out: Handle::NONE,
        }
    }

    /// A point of `kind` with the given handles.
    pub fn with_handles(self, kind: PointKind, handle_in: Handle, handle_out: Handle) -> Self {
        Self {
            kind,
            handle_in,
            handle_out,
            ..self
        }
    }

    /// Whether the curve uses this point's handles.
    pub fn has_handles(&self) -> bool {
        self.kind != PointKind::Corner
    }
}

impl Curve {
    /// Straight from `from` at the start to `to` at the end.
    pub fn line(from: f64, to: f64) -> Self {
        Self {
            points: vec![CurvePoint::corner(0.0, from), CurvePoint::corner(1.0, to)],
        }
    }
}
