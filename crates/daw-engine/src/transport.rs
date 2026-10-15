//! Song-time arithmetic for the audio side.
//!
//! Position is stored as an anchor beat plus a whole number of frames played
//! since it. The frame on which any beat is reached is therefore a pure
//! function of the anchor and the tempo, independent of how audio is split
//! into blocks. That is what makes live and offline renders identical.
//! Anchors move only on tempo changes, locates and loop jumps.

#[derive(Debug, Clone)]
pub(crate) struct Transport {
    playing: bool,
    anchor_beats: f64,
    frames_since_anchor: u64,
    samples_per_beat: f64,
    sample_rate: f64,
    loop_range: Option<(f64, f64)>,
}

impl Transport {
    pub fn new(sample_rate: f64, tempo_bpm: f64) -> Self {
        Self {
            playing: false,
            anchor_beats: 0.0,
            frames_since_anchor: 0,
            samples_per_beat: samples_per_beat(sample_rate, tempo_bpm),
            sample_rate,
            loop_range: None,
        }
    }

    pub fn position(&self) -> f64 {
        self.anchor_beats + self.frames_since_anchor as f64 / self.samples_per_beat
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// Changes tempo, keeping the current musical position.
    pub fn set_tempo(&mut self, tempo_bpm: f64) {
        let samples_per_beat = samples_per_beat(self.sample_rate, tempo_bpm);
        if samples_per_beat != self.samples_per_beat {
            self.anchor_beats = self.position();
            self.frames_since_anchor = 0;
            self.samples_per_beat = samples_per_beat;
        }
    }

    pub fn locate(&mut self, beats: f64) {
        self.anchor_beats = beats;
        self.frames_since_anchor = 0;
    }

    pub fn set_loop(&mut self, range: Option<(f64, f64)>) {
        self.loop_range = range;
    }

    /// Signed number of frames from now until `beat` is reached: zero if it
    /// falls on the current frame, negative if it has passed.
    pub fn frames_until(&self, beat: f64) -> i64 {
        let frame = ((beat - self.anchor_beats) * self.samples_per_beat).ceil() as i64;
        frame - self.frames_since_anchor as i64
    }

    pub fn advance(&mut self, frames: u64) {
        self.frames_since_anchor += frames;
    }

    /// How many of the next `max_frames` frames play before the loop jumps
    /// back, and whether it jumps after them. The loop only applies while the
    /// playhead is before its end.
    pub fn next_segment(&self, max_frames: u64) -> (u64, bool) {
        if let Some((_, end)) = self.loop_range.filter(|_| self.playing) {
            let until_end = self.frames_until(end);
            if until_end > 0 && until_end as u64 <= max_frames {
                return (until_end as u64, true);
            }
        }
        (max_frames, false)
    }

    /// Jumps to the loop start. Call when [`Self::next_segment`] says so.
    pub fn jump_to_loop_start(&mut self) {
        if let Some((start, _)) = self.loop_range {
            self.locate(start);
        }
    }
}

fn samples_per_beat(sample_rate: f64, tempo_bpm: f64) -> f64 {
    sample_rate * 60.0 / tempo_bpm
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM at 48 kHz: exactly 24,000 frames per beat.
    fn transport() -> Transport {
        let mut t = Transport::new(48_000.0, 120.0);
        t.set_playing(true);
        t
    }

    #[test]
    fn beats_land_on_exact_frames() {
        let t = transport();
        assert_eq!(t.frames_until(0.0), 0);
        assert_eq!(t.frames_until(1.0), 24_000);
        assert_eq!(t.frames_until(0.5), 12_000);
    }

    #[test]
    fn fractional_frames_round_up_to_the_first_frame_at_or_after_the_beat() {
        let t = transport();
        // 1/24000 beat is one frame; a third of that falls inside frame 1.
        assert_eq!(t.frames_until(1.0 / 72_000.0), 1);
    }

    #[test]
    fn advancing_moves_position_and_brings_beats_closer() {
        let mut t = transport();
        t.advance(6_000);
        assert_eq!(t.position(), 0.25);
        assert_eq!(t.frames_until(1.0), 18_000);
        assert_eq!(t.frames_until(0.0), -6_000);
    }

    #[test]
    fn block_splitting_does_not_change_where_beats_land() {
        let beat = 1.0 / 3.0;
        let mut whole = transport();
        let mut pieces = transport();
        whole.advance(1_000);
        for _ in 0..1_000 {
            pieces.advance(1);
        }
        assert_eq!(whole.frames_until(beat), pieces.frames_until(beat));
    }

    #[test]
    fn tempo_change_keeps_musical_position() {
        let mut t = transport();
        t.advance(24_000);
        t.set_tempo(60.0);
        assert_eq!(t.position(), 1.0);
        assert_eq!(t.frames_until(2.0), 48_000);
    }

    #[test]
    fn locate_moves_the_playhead() {
        let mut t = transport();
        t.advance(100);
        t.locate(4.0);
        assert_eq!(t.position(), 4.0);
        assert_eq!(t.frames_until(5.0), 24_000);
    }

    #[test]
    fn loop_end_splits_the_segment() {
        let mut t = transport();
        t.set_loop(Some((0.0, 1.0)));
        assert_eq!(t.next_segment(30_000), (24_000, true));
        assert_eq!(t.next_segment(1_000), (1_000, false));
    }

    #[test]
    fn jumping_returns_to_loop_start() {
        let mut t = transport();
        t.set_loop(Some((1.0, 2.0)));
        t.locate(1.5);
        let (frames, jumps) = t.next_segment(100_000);
        assert_eq!((frames, jumps), (12_000, true));
        t.advance(frames);
        t.jump_to_loop_start();
        assert_eq!(t.position(), 1.0);
    }

    #[test]
    fn loop_is_ignored_when_stopped_or_past_its_end() {
        let mut t = transport();
        t.set_loop(Some((0.0, 1.0)));
        t.locate(2.0);
        assert_eq!(t.next_segment(100_000), (100_000, false));
        t.locate(0.0);
        t.set_playing(false);
        assert_eq!(t.next_segment(100_000), (100_000, false));
    }
}
