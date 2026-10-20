//! Audio clips: windows onto a sample (`docs/design/audio-clips.md`).

use crate::{SampleId, Ticks};

/// A window onto an audio file on the timeline. Trimming moves the window;
/// the file is untouched, so a trimmed edge can be dragged back out.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioClip {
    /// Song position of the clip start.
    pub start: Ticks,
    pub length: Ticks,
    pub sample: SampleId,
    /// Seconds into the file where the clip's start plays.
    pub offset: f64,
    /// The file's own tempo. While the clip follows the song's tempo it is
    /// stretched by song tempo / this.
    pub bpm: f64,
    /// Stretch to the song's tempo. Off, the file plays at its own speed.
    pub follow_tempo: bool,
    pub gain_db: f64,
    pub fade_in: Ticks,
    pub fade_out: Ticks,
    /// Repeats the window across the clip (`clip_loop.rs`); then `offset` is
    /// where the cycle's start plays. `None` plays the window once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub looping: Option<crate::ClipLoop>,
}

/// Clip gain limits, in dB.
pub const MIN_CLIP_GAIN_DB: f64 = -60.0;
pub const MAX_CLIP_GAIN_DB: f64 = 24.0;
/// A clip's own tempo, in BPM.
pub const MIN_CLIP_BPM: f64 = 10.0;
pub const MAX_CLIP_BPM: f64 = 999.0;

impl AudioClip {
    /// A clip playing the whole of a file `seconds` long from `start`,
    /// at the song's tempo (so unstretched until its own tempo is set).
    pub fn whole_file(sample: SampleId, start: Ticks, seconds: f64, song_bpm: f64) -> Self {
        Self {
            start,
            length: Ticks::from_beats_f64(seconds * song_bpm / 60.0).max(Ticks(1)),
            sample,
            offset: 0.0,
            bpm: song_bpm,
            follow_tempo: true,
            gain_db: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            looping: None,
        }
    }

    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// File seconds that play per song beat.
    pub fn seconds_per_beat(&self, song_bpm: f64) -> f64 {
        60.0 / if self.follow_tempo {
            self.bpm
        } else {
            song_bpm
        }
    }

    /// Output length / input length for the stretcher: 1 means unstretched.
    pub fn ratio(&self, song_bpm: f64) -> f64 {
        if self.follow_tempo {
            self.bpm / song_bpm
        } else {
            1.0
        }
    }

    /// The file position, in seconds, that plays at song position `at`.
    pub fn file_seconds(&self, at: Ticks, song_bpm: f64) -> f64 {
        self.offset + (at - self.start).as_beats() * self.seconds_per_beat(song_bpm)
    }

    /// The song positions where the file's start and end would play: how
    /// far the edges can be dragged out.
    pub fn file_span(&self, file_seconds: f64, song_bpm: f64) -> (f64, f64) {
        let per_beat = self.seconds_per_beat(song_bpm);
        let start = self.start.as_beats() - self.offset / per_beat;
        (start, start + file_seconds / per_beat)
    }

    /// The same audio in place, seen through the window `start..end`:
    /// moving the start moves the offset with it. Fades are kept, shortened
    /// to fit. The caller keeps the window inside the file and the song.
    /// A looped clip keeps its cycle in place instead: its phase moves.
    pub fn windowed(&self, start: Ticks, end: Ticks, song_bpm: f64) -> Self {
        let length = end - start;
        let fade_in = self.fade_in.min(length);
        if let Some(looping) = self.looping {
            return Self {
                start,
                length,
                looping: Some(looping.shifted(start - self.start)),
                fade_in,
                fade_out: self.fade_out.min(length - fade_in),
                ..self.clone()
            };
        }
        Self {
            start,
            length,
            offset: self.file_seconds(start, song_bpm),
            fade_in,
            fade_out: self.fade_out.min(length - fade_in),
            ..self.clone()
        }
    }

    /// Cut in two at song position `at` (inside the clip): each half shows
    /// its own part, and either can be dragged back out to the whole file.
    /// The left keeps the fade in, the right the fade out.
    pub fn split(&self, at: Ticks, song_bpm: f64) -> (Self, Self) {
        let left = Self {
            fade_out: Ticks::ZERO,
            ..self.windowed(self.start, at, song_bpm)
        };
        let right = Self {
            fade_in: Ticks::ZERO,
            ..self.windowed(at, self.end(), song_bpm)
        };
        (left, right)
    }

    /// A new own tempo, keeping the stretch of audio the clip shows: its
    /// length (and fades, and a loop's cycle and phase) scale by the tempo
    /// change.
    pub fn with_bpm(&self, bpm: f64) -> Self {
        let scale = |t: Ticks| Ticks((t.0 as f64 * bpm / self.bpm).round() as i64);
        let looping = self.looping.map(|l| {
            let cycle = scale(l.cycle).max(Ticks(1));
            crate::ClipLoop {
                cycle,
                phase: scale(l.phase).min(cycle - Ticks(1)),
            }
        });
        Self {
            bpm,
            length: scale(self.length).max(Ticks(1)),
            fade_in: scale(self.fade_in),
            fade_out: scale(self.fade_out),
            looping,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four beats from bar 2, a 120 BPM file from 1 s in, in a 120 BPM song.
    fn clip() -> AudioClip {
        AudioClip {
            start: Ticks::from_beats(4),
            length: Ticks::from_beats(4),
            sample: SampleId(1),
            offset: 1.0,
            bpm: 120.0,
            follow_tempo: true,
            gain_db: 0.0,
            fade_in: Ticks::from_beats(1),
            fade_out: Ticks::from_beats(1),
            looping: None,
        }
    }

    #[test]
    fn a_whole_file_clip_lasts_as_long_as_the_file_at_song_tempo() {
        let c = AudioClip::whole_file(SampleId(2), Ticks::ZERO, 3.0, 120.0);
        assert_eq!(c.length, Ticks::from_beats(6));
        assert_eq!((c.bpm, c.offset, c.follow_tempo), (120.0, 0.0, true));
        assert_eq!(c.ratio(120.0), 1.0, "unstretched");
    }

    #[test]
    fn following_tempo_stretches_and_reads_the_file_at_its_own_tempo() {
        let c = clip();
        assert_eq!(
            c.ratio(128.0),
            120.0 / 128.0,
            "a faster song plays it faster"
        );
        assert_eq!(
            c.seconds_per_beat(128.0),
            0.5,
            "a beat of the file per song beat"
        );
        let free = AudioClip {
            follow_tempo: false,
            ..clip()
        };
        assert_eq!(free.ratio(128.0), 1.0);
        assert_eq!(free.seconds_per_beat(128.0), 60.0 / 128.0);
    }

    #[test]
    fn trimming_the_start_moves_the_offset_so_the_audio_stays_put() {
        let c = clip();
        let t = c.windowed(Ticks::from_beats(5), c.end(), 120.0);
        assert_eq!(t.offset, 1.5);
        assert_eq!(t.length, Ticks::from_beats(3));
        assert_eq!(
            t.file_seconds(Ticks::from_beats(6), 120.0),
            c.file_seconds(Ticks::from_beats(6), 120.0)
        );
        // Dragged back out again: as it was.
        assert_eq!(t.windowed(c.start, c.end(), 120.0), c);
    }

    #[test]
    fn the_file_bounds_how_far_edges_reveal() {
        // The file is 10 s: its start plays 2 beats before the clip, its
        // end 18 beats after that.
        assert_eq!(clip().file_span(10.0, 120.0), (2.0, 22.0));
    }

    #[test]
    fn a_split_gives_two_windows_onto_the_same_file() {
        let (left, right) = clip().split(Ticks::from_beats(6), 120.0);
        assert_eq!(
            (left.start, left.length),
            (Ticks::from_beats(4), Ticks::from_beats(2))
        );
        assert_eq!(
            (right.start, right.length),
            (Ticks::from_beats(6), Ticks::from_beats(2))
        );
        assert_eq!((left.offset, right.offset), (1.0, 2.0));
        assert_eq!(
            (left.fade_in, left.fade_out),
            (Ticks::from_beats(1), Ticks::ZERO)
        );
        assert_eq!(
            (right.fade_in, right.fade_out),
            (Ticks::ZERO, Ticks::from_beats(1))
        );
        assert_eq!(right.file_span(10.0, 120.0), clip().file_span(10.0, 120.0));
    }

    #[test]
    fn fades_shrink_to_fit_a_shorter_window() {
        let c = AudioClip {
            fade_in: Ticks::from_beats(3),
            ..clip()
        };
        let t = c.windowed(c.start, Ticks::from_beats(6), 120.0);
        assert_eq!((t.fade_in, t.fade_out), (Ticks::from_beats(2), Ticks::ZERO));
    }

    #[test]
    fn a_new_tempo_keeps_the_audio_the_clip_shows() {
        // The file is really 60 BPM: its two seconds shown are two of its
        // beats, so two song beats.
        let c = clip().with_bpm(60.0);
        assert_eq!(c.length, Ticks::from_beats(2));
        assert_eq!(c.fade_in, Ticks(480));
        assert_eq!(
            c.file_seconds(c.end(), 120.0),
            clip().file_seconds(clip().end(), 120.0)
        );
    }
}
