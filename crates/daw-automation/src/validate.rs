//! Checking blocks and signals before they enter the project.

use daw_protocol::{Block, InvalidBlock, InvalidSignal, NotesSource, Segment, Shape, Ticks};

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

/// Checks a notes signal's filter and envelope.
pub fn validate_notes_source(source: &NotesSource) -> Result<(), InvalidSignal> {
    if [source.attack, source.decay, source.release]
        .iter()
        .any(|t| *t < Ticks::ZERO)
    {
        return Err(InvalidSignal::EnvelopeTime);
    }
    if !(0.0..=1.0).contains(&source.sustain) {
        return Err(InvalidSignal::EnvelopeLevel);
    }
    let (min, max) = (source.velocity_min, source.velocity_max);
    if !(0.0..=1.0).contains(&min) || !(min..=1.0).contains(&max) {
        return Err(InvalidSignal::VelocityRange);
    }
    if source.lowest > source.highest || source.highest > 127 {
        return Err(InvalidSignal::PitchRange);
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
    use daw_protocol::{Shape, ShapePoint};

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
            ..NotesSource::new()
        };
        assert_eq!(validate_notes_source(&valid), Ok(()));
        let with = |change: fn(&mut NotesSource)| {
            let mut source = valid.clone();
            change(&mut source);
            validate_notes_source(&source)
        };
        assert_eq!(with(|s| s.attack = Ticks::ZERO), Ok(()), "instant attack");
        for change in [
            (|s: &mut NotesSource| s.attack = Ticks(-1)) as fn(&mut NotesSource),
            |s| s.decay = Ticks(-1),
            |s| s.release = Ticks(-1),
        ] {
            assert_eq!(with(change), Err(InvalidSignal::EnvelopeTime));
        }
        for change in [
            (|s: &mut NotesSource| s.sustain = 1.5) as fn(&mut NotesSource),
            |s| s.sustain = f64::NAN,
        ] {
            assert_eq!(with(change), Err(InvalidSignal::EnvelopeLevel));
        }
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

    #[test]
    fn allows_coincident_points_for_jumps() {
        assert_eq!(
            check(|b| b.shape.points.insert(1, ShapePoint::new(1.0, 0.5))),
            Ok(())
        );
    }
}
