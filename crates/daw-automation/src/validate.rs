//! Checking blocks and signals before they enter the project.

use daw_protocol::{
    AudioSource, Block, Curve, InvalidBlock, InvalidSignal, NotesSource, Segment, Shape, Ticks,
};

pub fn validate_block(block: &Block) -> Result<(), InvalidBlock> {
    if block.start < Ticks::ZERO {
        return Err(InvalidBlock::NegativeStart);
    }
    if block.length <= Ticks::ZERO {
        return Err(InvalidBlock::EmptyLength);
    }
    if block.cycle.is_some_and(|c| c <= Ticks::ZERO) {
        return Err(InvalidBlock::EmptyCycle);
    }
    if block.fade_in < Ticks::ZERO || block.fade_out < Ticks::ZERO {
        return Err(InvalidBlock::NegativeFade);
    }
    if !(0.0..=1.0).contains(&block.depth) {
        return Err(InvalidBlock::Depth);
    }
    validate_shape(&block.shape)
}

/// Checks a notes signal's filter, window and shape.
pub fn validate_notes_source(source: &NotesSource) -> Result<(), InvalidSignal> {
    if source.window <= Ticks::ZERO {
        return Err(InvalidSignal::Window);
    }
    validate_curve(&source.shape)?;
    let (min, max) = (source.velocity_min, source.velocity_max);
    if !(0.0..=1.0).contains(&min) || !(min..=1.0).contains(&max) {
        return Err(InvalidSignal::VelocityRange);
    }
    if source.lowest > source.highest || source.highest > 127 {
        return Err(InvalidSignal::PitchRange);
    }
    Ok(())
}

/// Checks an audio signal's follower: attack and release within
/// `0..=MAX_TIME_MS`, and `MIN_DB <= floor < ceiling <= MAX_DB`.
pub fn validate_audio_source(source: &AudioSource) -> Result<(), InvalidSignal> {
    let time = 0.0..=AudioSource::MAX_TIME_MS;
    if !time.contains(&source.attack_ms) || !time.contains(&source.release_ms) {
        return Err(InvalidSignal::FollowTime);
    }
    let (floor, ceiling) = (source.floor_db, source.ceiling_db);
    if !(AudioSource::MIN_DB..AudioSource::MAX_DB).contains(&floor)
        || !(floor..=AudioSource::MAX_DB).contains(&ceiling)
        || floor == ceiling
    {
        return Err(InvalidSignal::FollowRange);
    }
    Ok(())
}

/// Checks a curve: at least two points in time order within `0..=1`, the
/// first at 0 and the last at 1, values within `0..=1`, and finite handles
/// pointing their own way in time.
pub fn validate_curve(curve: &Curve) -> Result<(), InvalidSignal> {
    let points = &curve.points;
    let last = points.len().saturating_sub(1);
    if points.len() < 2 {
        return Err(InvalidSignal::Curve { index: last });
    }
    let mut previous = 0.0;
    for (index, point) in points.iter().enumerate() {
        let pinned = match index {
            0 => point.at == 0.0,
            i if i == last => point.at == 1.0,
            _ => true,
        };
        if !pinned || !(previous..=1.0).contains(&point.at) || !(0.0..=1.0).contains(&point.value) {
            return Err(InvalidSignal::Curve { index });
        }
        previous = point.at;
        let (hin, hout) = (point.handle_in, point.handle_out);
        let finite = [hin.at, hin.value, hout.at, hout.value]
            .iter()
            .all(|v| v.is_finite());
        if !finite || hin.at > 0.0 || hout.at < 0.0 {
            return Err(InvalidSignal::Handle { index });
        }
    }
    Ok(())
}

fn validate_shape(shape: &Shape) -> Result<(), InvalidBlock> {
    let points = &shape.points;
    if points.is_empty() {
        return Err(InvalidBlock::NoPoints);
    }
    let mut previous = 0.0;
    for (index, point) in points.iter().enumerate() {
        if !(previous..=1.0).contains(&point.at) {
            return Err(InvalidBlock::PointOrder { index });
        }
        previous = point.at;
        if !(0.0..=1.0).contains(&point.value) {
            return Err(InvalidBlock::PointValue { index });
        }
        if let Segment::Curve(t) = point.segment
            && !(-1.0..=1.0).contains(&t)
        {
            return Err(InvalidBlock::CurveTension { index });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_protocol::{CurvePoint, Handle, PointKind, Shape, ShapePoint};

    fn valid() -> Block {
        Block::new(Ticks::ZERO, Ticks::from_beats(1), Shape::ramp(0.0, 1.0))
    }

    fn check(change: impl FnOnce(&mut Block)) -> Result<(), InvalidBlock> {
        let mut block = valid();
        change(&mut block);
        validate_block(&block)
    }

    #[test]
    fn accepts_a_valid_block() {
        assert_eq!(validate_block(&valid()), Ok(()));
    }

    #[test]
    fn rejects_bad_timing() {
        assert_eq!(
            check(|b| b.start = Ticks(-1)),
            Err(InvalidBlock::NegativeStart)
        );
        assert_eq!(
            check(|b| b.length = Ticks::ZERO),
            Err(InvalidBlock::EmptyLength)
        );
        assert_eq!(
            check(|b| b.cycle = Some(Ticks::ZERO)),
            Err(InvalidBlock::EmptyCycle)
        );
        assert_eq!(
            check(|b| b.fade_out = Ticks(-1)),
            Err(InvalidBlock::NegativeFade)
        );
    }

    #[test]
    fn rejects_bad_depth() {
        for depth in [-0.1, 1.1, f64::NAN] {
            assert_eq!(check(|b| b.depth = depth), Err(InvalidBlock::Depth));
        }
    }

    #[test]
    fn rejects_bad_shapes() {
        assert_eq!(
            check(|b| b.shape.points.clear()),
            Err(InvalidBlock::NoPoints)
        );
        assert_eq!(
            check(|b| b.shape.points.swap(0, 1)),
            Err(InvalidBlock::PointOrder { index: 1 })
        );
        assert_eq!(
            check(|b| b.shape.points[0].at = -0.1),
            Err(InvalidBlock::PointOrder { index: 0 })
        );
        assert_eq!(
            check(|b| b.shape.points[1].value = f64::NAN),
            Err(InvalidBlock::PointValue { index: 1 })
        );
        assert_eq!(
            check(|b| b.shape.points[0].segment = Segment::Curve(1.5)),
            Err(InvalidBlock::CurveTension { index: 0 })
        );
    }

    #[test]
    fn checks_notes_sources() {
        let valid = NotesSource {
            lowest: 36,
            highest: 36,
            ..NotesSource::new(daw_protocol::NodeId(1))
        };
        assert_eq!(validate_notes_source(&valid), Ok(()));
        let with = |change: fn(&mut NotesSource)| {
            let mut source = valid.clone();
            change(&mut source);
            validate_notes_source(&source)
        };
        assert_eq!(with(|s| s.window = Ticks::ZERO), Err(InvalidSignal::Window));
        assert_eq!(
            with(|s| s.shape.points.truncate(1)),
            Err(InvalidSignal::Curve { index: 0 })
        );
        for change in [
            (|s: &mut NotesSource| s.velocity_min = -0.1) as fn(&mut NotesSource),
            |s| s.velocity_max = 1.1,
            |s| s.velocity_max = f64::NAN,
            |s| {
                s.velocity_min = 0.8;
                s.velocity_max = 0.2;
            },
        ] {
            assert_eq!(with(change), Err(InvalidSignal::VelocityRange));
        }
        assert_eq!(
            with(|s| {
                s.velocity_min = 0.5;
                s.velocity_max = 0.5;
            }),
            Ok(()),
            "a fixed height"
        );
        assert_eq!(with(|s| s.lowest = 40), Err(InvalidSignal::PitchRange));
        assert_eq!(with(|s| s.highest = 128), Err(InvalidSignal::PitchRange));
    }

    fn curve_check(change: impl FnOnce(&mut Curve)) -> Result<(), InvalidSignal> {
        let mut curve = Curve {
            points: vec![
                CurvePoint::corner(0.0, 0.5),
                CurvePoint::corner(0.5, 1.0),
                CurvePoint::corner(1.0, 0.5),
            ],
        };
        change(&mut curve);
        validate_curve(&curve)
    }

    #[test]
    fn accepts_curves_with_jumps_and_handles() {
        assert_eq!(curve_check(|_| {}), Ok(()));
        assert_eq!(
            curve_check(|c| c.points.insert(2, CurvePoint::corner(0.5, 0.0))),
            Ok(()),
            "a jump"
        );
        assert_eq!(
            curve_check(|c| {
                c.points[1] = c.points[1].with_handles(
                    PointKind::Smooth,
                    Handle::new(-0.2, 0.0),
                    Handle::new(0.2, 3.0),
                );
            }),
            Ok(()),
            "handles may reach past the edge"
        );
    }

    #[test]
    fn rejects_curves_out_of_place() {
        let bad = |index| Err(InvalidSignal::Curve { index });
        assert_eq!(
            curve_check(|c| c.points[0].at = 0.1),
            bad(0),
            "unpinned start"
        );
        assert_eq!(
            curve_check(|c| c.points[2].at = 0.9),
            bad(2),
            "unpinned end"
        );
        assert_eq!(curve_check(|c| c.points[1].at = -0.5), bad(1));
        assert_eq!(curve_check(|c| c.points.swap(0, 1)), bad(0));
        assert_eq!(curve_check(|c| c.points[1].value = 1.5), bad(1));
        assert_eq!(curve_check(|c| c.points[1].value = f64::NAN), bad(1));
    }

    #[test]
    fn rejects_handles_pointing_the_wrong_way() {
        let bad = Err(InvalidSignal::Handle { index: 1 });
        assert_eq!(curve_check(|c| c.points[1].handle_in.at = 0.1), bad);
        assert_eq!(curve_check(|c| c.points[1].handle_out.at = -0.1), bad);
        assert_eq!(
            curve_check(|c| c.points[1].handle_out.value = f64::INFINITY),
            bad
        );
    }

    #[test]
    fn allows_coincident_points_for_jumps() {
        assert_eq!(
            check(|b| b.shape.points.insert(1, ShapePoint::new(1.0, 0.5))),
            Ok(())
        );
    }

    #[test]
    fn audio_followers_need_sane_times_and_an_ordered_range() {
        use daw_protocol::{AudioSource, NodeId};
        let valid = AudioSource::new(NodeId(1));
        assert_eq!(validate_audio_source(&valid), Ok(()));
        let check = |change: fn(&mut AudioSource)| {
            let mut source = valid.clone();
            change(&mut source);
            validate_audio_source(&source)
        };
        let time = Err(InvalidSignal::FollowTime);
        let range = Err(InvalidSignal::FollowRange);
        assert_eq!(check(|s| s.attack_ms = -1.0), time);
        assert_eq!(check(|s| s.release_ms = 5_000.0), time);
        assert_eq!(check(|s| s.release_ms = f64::NAN), time);
        assert_eq!(check(|s| s.attack_ms = 0.0), Ok(()));
        assert_eq!(check(|s| s.floor_db = 0.0), range, "floor at the ceiling");
        assert_eq!(
            check(|s| s.floor_db = 6.0),
            range,
            "floor above the ceiling"
        );
        assert_eq!(check(|s| s.floor_db = -120.0), range);
        assert_eq!(check(|s| s.ceiling_db = 20.0), range);
        assert_eq!(check(|s| s.ceiling_db = f64::NAN), range);
    }
}
