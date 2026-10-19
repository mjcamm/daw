//! Drawing an audio clip's waveform from its sample's overview.
//!
//! Each pixel column shows the lowest and highest sample of the file that
//! plays under it, both sides together. Only columns on screen are drawn,
//! so a long clip zoomed in costs no more than a short one.

use daw_engine::{AudioClip, Peaks};
use gpui::{Bounds, Hsla, IntoElement, Pixels, Styled, canvas, fill, point, px, size};
use std::sync::Arc;

/// Where a clip's columns read the file: seconds at the clip's left edge,
/// and file seconds per pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    pub first_second: f64,
    pub seconds_per_pixel: f64,
}

impl Reading {
    pub fn of(clip: &AudioClip, song_bpm: f64, pixels_per_beat: f32) -> Self {
        Self {
            first_second: clip.offset,
            seconds_per_pixel: clip.seconds_per_beat(song_bpm) / f64::from(pixels_per_beat),
        }
    }

    /// The file frames under column `x` (pixels from the clip's left).
    pub fn frames(&self, x: f32, sample_rate: u32) -> (f64, f64) {
        let rate = f64::from(sample_rate);
        let from = self.first_second + f64::from(x) * self.seconds_per_pixel;
        (from * rate, (from + self.seconds_per_pixel) * rate)
    }
}

/// The waveform filling its parent, centred, in `color`: both sides of the
/// audio together.
pub fn waveform(peaks: Arc<Peaks>, reading: Reading, color: Hsla) -> impl IntoElement {
    channel(peaks, reading, None, color)
}

/// One side of the audio (`Some(channel)`), or all of them (`None`).
pub fn channel(
    peaks: Arc<Peaks>,
    reading: Reading,
    only: Option<usize>,
    color: Hsla,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, (), window, _| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            if visible.size.width <= px(0.) {
                return;
            }
            let height = bounds.size.height / px(1.);
            let middle = height / 2.;
            // Leave a hairline of room above and below.
            let scale = (middle - 1.).max(0.);
            let first = ((visible.left() - bounds.left()) / px(1.)).floor().max(0.) as i32;
            let last = ((visible.right() - bounds.left()) / px(1.)).ceil() as i32;
            for x in first..last {
                let (from, to) = reading.frames(x as f32, peaks.sample_rate);
                let range = (0..peaks.channels())
                    .filter(|c| only.is_none_or(|only| *c == only))
                    .map(|c| peaks.range(c, from, to))
                    .fold(daw_engine::MinMax::EMPTY, |m, r| m.union(r));
                if range.is_empty() {
                    continue;
                }
                let top = middle - range.max.clamp(-1., 1.) * scale;
                let bottom = (middle - range.min.clamp(-1., 1.) * scale).max(top + 1.);
                window.paint_quad(fill(
                    Bounds::new(
                        point(bounds.left() + px(x as f32), bounds.top() + px(top)),
                        size(px(1.), px(bottom - top)),
                    ),
                    color,
                ));
            }
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{SampleId, Ticks};

    #[test]
    fn columns_read_the_file_at_the_clips_tempo() {
        // A 120 BPM file from 1 s in, at 100 px per beat: 5 ms per pixel.
        let clip = AudioClip {
            offset: 1.0,
            ..AudioClip::whole_file(SampleId(1), Ticks::ZERO, 10.0, 120.0)
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let reading = Reading::of(&clip, 140.0, 100.0);
        let (from, to) = reading.frames(0., 48_000);
        assert!(close(from, 48_000.0) && close(to, 48_240.0));
        assert!(close(reading.frames(10., 48_000).0, 48_000.0 + 2_400.0));
        // Not following the tempo: the song's beats.
        let free = AudioClip {
            follow_tempo: false,
            ..clip
        };
        let reading = Reading::of(&free, 60.0, 100.0);
        assert!(close(reading.frames(1., 1_000).0, 1_000.0 + 10.0));
    }
}
