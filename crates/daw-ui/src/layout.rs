//! Pure geometry and formatting for the views, kept free of GPUI so it can be
//! unit tested.

use daw_engine::{LoopRange, MidiClip, Ticks};

/// Horizontal zoom of the timeline.
pub const PIXELS_PER_BEAT: f32 = 24.0;
pub const BEATS_PER_BAR: f64 = 4.0;
/// Empty bars shown after the last content.
const TRAILING_BARS: f64 = 8.0;
/// Shortest timeline shown, in bars.
const MIN_BARS: f64 = 16.0;

/// Bar · beat · sixteenth, counted from 1, for a position in beats (4/4).
pub fn format_position(beats: f64) -> String {
    let beats = beats.max(0.0);
    let bar = (beats / BEATS_PER_BAR).floor() as i64 + 1;
    let beat = (beats % BEATS_PER_BAR).floor() as i64 + 1;
    let sixteenth = (beats.fract() * 4.0).floor() as i64 + 1;
    format!("{bar} · {beat} · {sixteenth}")
}

pub fn beat_to_x(beats: f64) -> f32 {
    beats as f32 * PIXELS_PER_BEAT
}

/// The song position under a horizontal offset, snapped to sixteenths and
/// never before the start.
pub fn x_to_snapped_position(x: f32) -> Ticks {
    let beats = f64::from(x / PIXELS_PER_BEAT).max(0.0);
    Ticks::from_beats_f64((beats * 4.0).round() / 4.0)
}

/// Whole bars to show: everything that exists, plus room after it.
pub fn timeline_bars<'a>(
    clips: impl IntoIterator<Item = &'a MidiClip>,
    loop_range: Option<LoopRange>,
) -> u32 {
    let content_end = clips
        .into_iter()
        .map(|c| (c.start + c.length).as_beats())
        .chain(loop_range.map(|r| r.end.as_beats()))
        .fold(0.0, f64::max);
    let bars = (content_end / BEATS_PER_BAR).ceil() + TRAILING_BARS;
    bars.max(MIN_BARS) as u32
}

/// Display height (0..=1) of a meter for a linear peak amplitude: -60 dB to
/// 0 dB across the full height.
pub fn meter_height(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// How quickly a meter falls when the signal drops, per 60 Hz frame:
/// roughly 20 dB per second.
pub const METER_FALL_PER_FRAME: f32 = 0.9624;

/// Vertical position (0 at the top, 1 at the bottom) of each note in a clip,
/// spreading the clip's pitch range over the height.
pub fn note_rows(clip: &MidiClip) -> impl Iterator<Item = f32> + '_ {
    let lowest = clip.notes.iter().map(|n| n.pitch).min().unwrap_or(60);
    let highest = clip.notes.iter().map(|n| n.pitch).max().unwrap_or(60);
    let span = f32::from(highest - lowest).max(1.0);
    clip.notes
        .iter()
        .map(move |n| 1.0 - f32::from(n.pitch - lowest) / span)
}

/// How long ago a time was, in words, for lists like recent projects. Times
/// are seconds since 1970.
pub fn format_age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => plural(seconds / 60, "minute"),
        3_600..86_400 => plural(seconds / 3_600, "hour"),
        86_400..604_800 => plural(seconds / 86_400, "day"),
        _ => plural(seconds / 604_800, "week"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::MidiNote;

    fn clip(start: i64, length: i64, pitches: &[u8]) -> MidiClip {
        MidiClip {
            start: Ticks::from_beats(start),
            length: Ticks::from_beats(length),
            notes: pitches
                .iter()
                .map(|&pitch| MidiNote {
                    start: Ticks::ZERO,
                    length: Ticks::from_beats(1),
                    pitch,
                    velocity: 100,
                })
                .collect(),
        }
    }

    #[test]
    fn positions_count_from_one() {
        assert_eq!(format_position(0.0), "1 · 1 · 1");
        assert_eq!(format_position(4.0), "2 · 1 · 1");
        assert_eq!(format_position(5.75), "2 · 2 · 4");
        assert_eq!(format_position(-1.0), "1 · 1 · 1");
    }

    #[test]
    fn clicks_snap_to_sixteenths() {
        assert_eq!(x_to_snapped_position(0.0), Ticks::ZERO);
        assert_eq!(
            x_to_snapped_position(PIXELS_PER_BEAT * 1.3),
            Ticks::from_beats_f64(1.25)
        );
        assert_eq!(x_to_snapped_position(-50.0), Ticks::ZERO);
    }

    #[test]
    fn timeline_covers_content_plus_room() {
        assert_eq!(timeline_bars([], None), 16);
        let clips = [clip(0, 4 * 20, &[])];
        assert_eq!(timeline_bars(&clips, None), 28);
        let looped = LoopRange {
            start: Ticks::ZERO,
            end: Ticks::from_bars(30),
        };
        assert_eq!(timeline_bars([], Some(looped)), 38);
    }

    #[test]
    fn meters_span_sixty_decibels() {
        assert_eq!(meter_height(0.0), 0.0);
        assert_eq!(meter_height(1.0), 1.0);
        assert_eq!(meter_height(2.0), 1.0);
        assert!((meter_height(0.001) - 0.0).abs() < 1e-6);
        assert!((meter_height(0.031_622_8) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn notes_spread_over_the_pitch_range() {
        let c = clip(0, 4, &[60, 72, 66]);
        let rows: Vec<f32> = note_rows(&c).collect();
        assert_eq!(rows, [1.0, 0.0, 0.5]);
    }

    #[test]
    fn a_single_pitch_does_not_divide_by_zero() {
        let c = clip(0, 4, &[60, 60]);
        assert!(note_rows(&c).all(|y| y == 1.0));
    }
}

#[cfg(test)]
mod age_tests {
    use super::format_age;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(format_age(1_000, 1_000), "just now");
        assert_eq!(format_age(1_000, 2_000), "just now", "clock skew");
        assert_eq!(format_age(120, 0), "2 minutes ago");
        assert_eq!(format_age(3_600, 0), "1 hour ago");
        assert_eq!(format_age(3 * 86_400, 0), "3 days ago");
        assert_eq!(format_age(15 * 86_400, 0), "2 weeks ago");
    }
}
