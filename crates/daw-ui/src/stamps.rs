//! The built-in stamps (`docs/design/curve-editor.md`): shapes a stamp card
//! writes across a curve, once per grid cell. Saved stamps live in
//! `daw_host::stamps`.

use daw_engine::{Curve, CurvePoint, Handle, PointKind};

/// A stamp as the strip shows it: built in, or saved by the user (by its
/// place in the saved list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampRef {
    BuiltIn(usize),
    Saved(usize),
}

/// The built-in stamps, in the strip's order.
pub fn built_in() -> [(&'static str, Curve); 6] {
    let corner = CurvePoint::corner;
    let smooth = |at, value, handle_in, handle_out| {
        corner(at, value).with_handles(PointKind::Smooth, handle_in, handle_out)
    };
    let flat = |reach: f64| Handle::new(reach, 0.);
    // A sine: through the centre, up to the top, down to the bottom and
    // back, with handles a third of the way to each neighbour (the centre
    // crossings as steep as a sine's).
    let (quarter, half) = (0.25 / 3., 0.5 / 3.);
    let rise = Handle::new(quarter, std::f64::consts::PI * quarter);
    let sine = Curve {
        points: vec![
            smooth(0., 0.5, Handle::NONE, rise),
            smooth(0.25, 1., flat(-quarter), flat(half)),
            smooth(0.75, 0., flat(-half), flat(quarter)),
            smooth(1., 0.5, Handle::new(-rise.at, -rise.value), Handle::NONE),
        ],
    };
    // A duck: straight to the bottom, then curving back up to the centre,
    // quick at first and settling.
    let duck = Curve {
        points: vec![
            corner(0., 0.).with_handles(PointKind::Broken, Handle::NONE, Handle::new(0.1, 0.3)),
            smooth(1., 0.5, Handle::new(-0.5, 0.), Handle::NONE),
        ],
    };
    [
        ("Ramp up", Curve::line(0., 1.)),
        ("Ramp down", Curve::line(1., 0.)),
        (
            "Triangle",
            Curve {
                points: vec![corner(0., 0.), corner(0.5, 1.), corner(1., 0.)],
            },
        ),
        (
            "Square",
            Curve {
                points: vec![
                    corner(0., 1.),
                    corner(0.5, 1.),
                    corner(0.5, 0.),
                    corner(1., 0.),
                ],
            },
        ),
        ("Sine", sine),
        ("Duck", duck),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_automation::curve_value;

    #[test]
    fn built_in_stamps_are_valid_curves() {
        for (name, curve) in built_in() {
            assert_eq!(daw_engine::validate_curve(&curve), Ok(()), "{name}");
        }
    }

    #[test]
    fn the_sine_rises_falls_and_comes_back() {
        let sine = &built_in()[4].1.points;
        let at = |phase| curve_value(sine, phase);
        assert!((at(0.25) - 1.).abs() < 1e-9 && (at(0.75)).abs() < 1e-9);
        assert!(at(0.125) > 0.75 && at(0.125) < 0.95, "{}", at(0.125));
        assert!((at(0.125) + at(0.625) - 1.).abs() < 0.02, "symmetric");
    }

    #[test]
    fn the_duck_drops_to_the_bottom_and_settles_at_the_centre() {
        let duck = &built_in()[5].1.points;
        assert_eq!(curve_value(duck, 0.), 0.);
        assert!(curve_value(duck, 0.25) > 0.25, "quick at first");
        assert!(curve_value(duck, 0.9) > 0.49);
        assert_eq!(curve_value(duck, 1.), 0.5);
    }
}
