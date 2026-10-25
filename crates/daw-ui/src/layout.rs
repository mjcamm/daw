//! Pure geometry and formatting for the views, kept free of GPUI so it can be
//! unit tested.

use daw_engine::{LoopRange, MidiClip, Ticks};

/// Starting horizontal zoom of a timeline page (each page zooms on its own).
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

/// The song position under a horizontal offset at `pixels_per_beat`,
/// snapped to sixteenths and never before the start.
pub fn x_to_snapped_position(x: f32, pixels_per_beat: f32) -> Ticks {
    let beats = f64::from(x / pixels_per_beat).max(0.0);
    Ticks::from_beats_f64((beats * 4.0).round() / 4.0)
}

/// Closest spacing of grid lines, in pixels.
pub const MIN_GRID_SPACING: f32 = 6.;

/// Show a bar line every this many bars, so bar lines stay at least
/// `MIN_GRID_SPACING` apart when zoomed out.
pub fn grid_bar_step(pixels_per_beat: f32) -> u32 {
    let bar = pixels_per_beat * BEATS_PER_BAR as f32;
    let mut step = 1;
    while bar * (step as f32) < MIN_GRID_SPACING {
        step *= 2;
    }
    step
}

/// Label every this many bars on a ruler, so labels stay readable when
/// zoomed out.
pub fn ruler_label_step(pixels_per_beat: f32) -> u32 {
    const MIN_LABEL_SPACING: f32 = 28.;
    let bar = pixels_per_beat * BEATS_PER_BAR as f32;
    let mut step = 1;
    while bar * (step as f32) < MIN_LABEL_SPACING {
        step *= 2;
    }
    step
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

/// Where a level of `db` sits on a strip, `0..=1` from the bottom: the
/// volume's own scale (-60 to +12 dB). The fader, its meters and the dB
/// marks beside them all use it, so one number means one height (as in
/// Studio One, Live and Logic).
pub fn level_position(db: f32) -> f32 {
    let scale =
        daw_engine::devices::CHANNEL_PARAMS[daw_engine::devices::channel::VOLUME as usize].scale;
    if db < -60. {
        return 0.;
    }
    (scale.to_normalized(f64::from(db)) as f32).clamp(0., 1.)
}

/// Display height (0..=1) of a meter for a linear peak amplitude, on the
/// fader's scale ([`level_position`]); nothing below -60 dB.
pub fn meter_height(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    level_position(20.0 * peak.log10())
}

/// Whether a peak is at or over 0 dB (full scale): the meter shows it red.
pub fn is_over(peak: f32) -> bool {
    peak >= 1.0
}

/// The dB marks beside a fader, in order of how much they matter.
const MARK_PRIORITY: [i32; 9] = [0, -12, -24, -48, 6, -6, -36, 12, -60];
/// Closest two marks may sit, in pixels, before one drops out.
pub const MIN_MARK_GAP: f32 = 11.;

/// The marks for a fader with `travel` pixels of travel, top first: each
/// level and its height above the travel's bottom. 0 dB always shows;
/// others drop out where they would crowd it.
pub fn db_marks(travel: f32) -> Vec<(i32, f32)> {
    let mut marks: Vec<(i32, f32)> = Vec::new();
    for db in MARK_PRIORITY {
        let y = level_position(db as f32) * travel;
        if marks
            .iter()
            .all(|(_, other)| (other - y).abs() >= MIN_MARK_GAP)
        {
            marks.push((db, y));
        }
    }
    marks.sort_by(|a, b| b.1.total_cmp(&a.1));
    marks
}

/// How quickly a meter falls when the signal drops, per 60 Hz frame:
/// roughly 20 dB per second.
pub const METER_FALL_PER_FRAME: f32 = 0.9624;

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
            looping: None,
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
        assert_eq!(x_to_snapped_position(0.0, 24.), Ticks::ZERO);
        assert_eq!(
            x_to_snapped_position(24. * 1.3, 24.),
            Ticks::from_beats_f64(1.25)
        );
        assert_eq!(x_to_snapped_position(-50.0, 24.), Ticks::ZERO);
        assert_eq!(x_to_snapped_position(26., 10.), Ticks::from_beats_f64(2.5));
    }

    #[test]
    fn grid_lines_thin_out_when_zoomed_out() {
        assert_eq!(grid_bar_step(24.), 1);
        assert_eq!(grid_bar_step(1.), 2);
        assert_eq!(grid_bar_step(0.25), 8);
    }

    #[test]
    fn ruler_labels_thin_out_when_zoomed_out() {
        assert_eq!(ruler_label_step(24.), 1);
        assert_eq!(ruler_label_step(4.), 2);
        assert_eq!(ruler_label_step(2.), 4);
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
    fn meters_share_the_faders_scale() {
        // The volume's range, -60 to +12 dB: 0 dB five sixths of the way up.
        assert_eq!(level_position(-60.), 0.);
        assert_eq!(level_position(12.), 1.);
        assert!((level_position(0.) - 60. / 72.).abs() < 1e-6);
        assert!((level_position(-24.) - 0.5).abs() < 1e-6);
        assert_eq!(meter_height(0.0), 0.0, "silence");
        assert_eq!(meter_height(0.0005), 0.0, "below -60 dB");
        assert!(
            (meter_height(1.0) - level_position(0.)).abs() < 1e-6,
            "0 dB"
        );
        assert_eq!(meter_height(8.0), 1.0, "above +12 dB: full");
        assert!(!is_over(0.99) && is_over(1.0), "red from 0 dB");
    }

    #[test]
    fn db_marks_fit_the_fader_and_always_show_0_db() {
        let all: Vec<i32> = db_marks(400.).iter().map(|(db, _)| *db).collect();
        assert_eq!(
            all,
            vec![12, 6, 0, -6, -12, -24, -36, -48, -60],
            "a tall fader"
        );
        for travel in [60., 120., 200., 400.] {
            let marks = db_marks(travel);
            assert!(marks.iter().any(|(db, _)| *db == 0), "{travel}: 0 dB");
            for pair in marks.windows(2) {
                assert!(
                    pair[0].1 - pair[1].1 >= MIN_MARK_GAP - 1e-3,
                    "{travel}: {pair:?} overlap"
                );
            }
            // Each sits where the fader puts its level.
            for (db, y) in &marks {
                assert!((y - level_position(*db as f32) * travel).abs() < 1e-3);
            }
        }
        assert!(db_marks(60.).len() < 9, "a short fader thins them out");
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
