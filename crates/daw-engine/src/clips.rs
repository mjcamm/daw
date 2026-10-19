//! Playing audio clips: one stretcher per audio node, fed from song time.
//!
//! A node's audio clips never overlap, so one [`StemStream`] plays them all,
//! seeking at each clip's start. Where in its file a clip plays is a pure
//! function of song position ([`ClipRender::source_frame`]); the stream seeks
//! there whenever what plays changes (a clip starts, the song jumps, an edit
//! moves the clip). A tempo change alone only changes the ratio, which the
//! stretcher glides to (`docs/design/audio-clips.md`).
//!
//! Real-time safe: the stream and its buffers are made up front, and the
//! player never holds the clips' audio itself (it could end up the last
//! owner and free it here); it reads it from the current render state.

use crate::samples::SampleData;
use daw_protocol::{ClipId, NodeId};
use daw_stretch::StemStream;
use std::sync::Arc;

/// Frames over which a sounding clip fades out when the transport stops.
const STOP_FADE_FRAMES: u32 = 256;
/// Frame grids the streams spread across, so nodes compute their frames in
/// different blocks. Chosen by node id, never by creation order, so the
/// output is the same in every render.
const STAGGER_SLOTS: usize = 16;

/// An audio clip compiled for playing, its times in song beats and its
/// audio at the engine's rate.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRender {
    pub id: ClipId,
    pub start_beats: f64,
    pub end_beats: f64,
    pub audio: Arc<SampleData>,
    /// The file frame (at the engine's rate) that plays at the clip's start.
    pub offset_frames: f64,
    /// File frames per song beat: the file's own tempo when the clip
    /// follows the song's, else the song's.
    pub frames_per_beat: f64,
    /// Output length / input length: clip BPM / song BPM, or 1.
    pub ratio: f64,
    /// Linear clip gain.
    pub gain: f32,
    pub fade_in_beats: f64,
    pub fade_out_beats: f64,
}

impl ClipRender {
    /// The file frame that plays at song position `beat`.
    pub fn source_frame(&self, beat: f64) -> f64 {
        self.offset_frames + (beat - self.start_beats) * self.frames_per_beat
    }

    /// Clip gain with the fades, at `beat`.
    pub fn level(&self, beat: f64) -> f32 {
        let mut level = 1.0f64;
        if self.fade_in_beats > 0.0 {
            level = level.min((beat - self.start_beats) / self.fade_in_beats);
        }
        if self.fade_out_beats > 0.0 {
            level = level.min((self.end_beats - beat) / self.fade_out_beats);
        }
        self.gain * level.clamp(0.0, 1.0) as f32
    }

    fn placement(&self) -> Placement {
        Placement {
            id: self.id,
            start_beats: self.start_beats,
            offset_frames: self.offset_frames,
            frames_per_beat: self.frames_per_beat,
            audio: Arc::as_ptr(&self.audio) as usize,
        }
    }
}

/// The clip at song position `beat` among clips sorted by start.
pub fn clip_at(clips: &[ClipRender], beat: f64) -> Option<&ClipRender> {
    let after = clips.partition_point(|c| c.start_beats <= beat);
    after
        .checked_sub(1)
        .map(|i| &clips[i])
        .filter(|c| beat < c.end_beats)
}

/// Where a clip plays from: if this is unchanged, a running stream carries
/// on into the next span.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    id: ClipId,
    start_beats: f64,
    offset_frames: f64,
    frames_per_beat: f64,
    /// The audio's address, compared only (never read).
    audio: usize,
}

#[derive(Debug, Clone, Copy)]
struct Playing {
    placement: Placement,
    ratio: f64,
    /// The song position the next frame should be at; anything else means
    /// the song jumped.
    next_beat: f64,
    /// Frames of the stop fade left, once the transport has stopped.
    stopping: Option<u32>,
}

/// Plays one audio node's clips.
#[derive(Debug)]
pub struct ClipPlayer {
    stream: StemStream,
    playing: Option<Playing>,
    left: Box<[f32]>,
    right: Box<[f32]>,
}

impl ClipPlayer {
    /// A player for spans of up to `max_frames`. Allocates: make it on the
    /// control side.
    pub fn new(sample_rate: f32, max_frames: usize) -> Self {
        let stream = StemStream::for_sample_rate(2, max_frames, f64::from(sample_rate))
            .or_else(|_| StemStream::for_sample_rate(2, max_frames, 48_000.0))
            .unwrap_or_else(|_| unreachable!("48 kHz stereo is a valid stream setup"));
        Self {
            stream,
            playing: None,
            left: vec![0.0; max_frames].into_boxed_slice(),
            right: vec![0.0; max_frames].into_boxed_slice(),
        }
    }

    /// Stops at once.
    pub fn reset(&mut self) {
        self.playing = None;
    }

    /// Adds a span of the node's clips into `left` and `right` (equal
    /// lengths, at most the player's `max_frames`), from song position
    /// `beat`, `beats_per_frame` apart. `clips` are the node's clips sorted
    /// by start; the caller splits spans at their edges. While the
    /// transport is stopped a sounding clip fades out.
    #[allow(
        clippy::too_many_arguments,
        reason = "the span's place in song time, spelled out"
    )]
    pub fn render(
        &mut self,
        node: NodeId,
        clips: &[ClipRender],
        transport_playing: bool,
        beat: f64,
        beats_per_frame: f64,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let frames = left.len().min(right.len()).min(self.left.len());
        if !transport_playing {
            self.fade_out(clips, frames, left, right);
            return;
        }
        let Some(clip) = clip_at(clips, beat) else {
            self.playing = None;
            return;
        };
        let placement = clip.placement();
        let carries_on = self.playing.is_some_and(|p| {
            p.stopping.is_none()
                && p.placement == placement
                && (p.next_beat - beat).abs() <= beats_per_frame * 0.5
        });
        if !carries_on {
            self.stream
                .set_stagger(node.0 as usize % STAGGER_SLOTS, STAGGER_SLOTS);
            self.stream.set_ratio(clip.ratio);
            self.stream.seek(clip.source_frame(beat));
        } else if self.playing.is_some_and(|p| p.ratio != clip.ratio) {
            self.stream.set_ratio(clip.ratio);
        }
        self.stream_into(&clip.audio, frames);
        for i in 0..frames {
            let level = clip.level(beat + i as f64 * beats_per_frame);
            left[i] += self.left[i] * level;
            right[i] += self.right[i] * level;
        }
        self.playing = Some(Playing {
            placement,
            ratio: clip.ratio,
            next_beat: beat + frames as f64 * beats_per_frame,
            stopping: None,
        });
    }

    /// The stream's next `frames` into the player's own buffers.
    fn stream_into(&mut self, audio: &SampleData, frames: usize) {
        let right = audio.right.as_deref().unwrap_or(&audio.left);
        let source: [&[f32]; 2] = [&audio.left, right];
        let mut out: [&mut [f32]; 2] = [&mut self.left[..frames], &mut self.right[..frames]];
        self.stream.process(&source, &mut out);
    }

    /// While stopped: a sounding clip carries on for a moment, fading out,
    /// so stopping never clicks.
    fn fade_out(
        &mut self,
        clips: &[ClipRender],
        frames: usize,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let Some(mut playing) = self.playing.take() else {
            return;
        };
        // The clip may have been edited away meanwhile: then it just stops.
        let Some(clip) = clips.iter().find(|c| c.placement() == playing.placement) else {
            return;
        };
        let remaining = playing.stopping.unwrap_or(STOP_FADE_FRAMES);
        let count = frames.min(remaining as usize);
        self.stream_into(&clip.audio, count);
        let level = clip.level(playing.next_beat);
        for i in 0..count {
            let fade = (remaining as usize - i) as f32 / STOP_FADE_FRAMES as f32;
            left[i] += self.left[i] * level * fade;
            right[i] += self.right[i] * level * fade;
        }
        let rest = remaining - count as u32;
        if rest > 0 {
            playing.stopping = Some(rest);
            self.playing = Some(playing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;
    /// 120 BPM at 48 kHz.
    const BEATS_PER_FRAME: f64 = 2.0 / 48_000.0;

    /// A clip of `audio` from beat 1 to 3, playing its file from frame
    /// 1000, unstretched.
    fn clip(audio: &Arc<SampleData>) -> ClipRender {
        ClipRender {
            id: ClipId(1),
            start_beats: 1.0,
            end_beats: 3.0,
            audio: Arc::clone(audio),
            offset_frames: 1000.0,
            frames_per_beat: 24_000.0,
            ratio: 1.0,
            gain: 1.0,
            fade_in_beats: 0.0,
            fade_out_beats: 0.0,
        }
    }

    /// Distinct, recognisable samples: frame `i` holds `i / 1e6`.
    fn counting(frames: usize) -> Arc<SampleData> {
        Arc::new(SampleData::mono(
            48_000,
            (0..frames).map(|i| i as f32 / 1e6).collect(),
        ))
    }

    /// Renders `frames` from `beat` in spans of `span`, as the transport plays.
    fn render(
        player: &mut ClipPlayer,
        clips: &[ClipRender],
        beat: f64,
        frames: usize,
        span: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        let mut right = vec![0.0; frames];
        let mut done = 0;
        while done < frames {
            let n = span.min(frames - done);
            player.render(
                NodeId(3),
                clips,
                true,
                beat + done as f64 * BEATS_PER_FRAME,
                BEATS_PER_FRAME,
                &mut out[done..done + n],
                &mut right[done..done + n],
            );
            done += n;
        }
        out
    }

    #[test]
    fn a_clip_plays_its_file_from_where_song_time_says() {
        let audio = counting(200_000);
        let clips = [clip(&audio)];
        let mut player = ClipPlayer::new(RATE, 1024);
        // Half a beat into the clip: 12000 frames past its offset.
        let out = render(&mut player, &clips, 1.5, 100, 100);
        assert_eq!(out[0], audio.left[13_000]);
        assert_eq!(out[99], audio.left[13_099]);
    }

    #[test]
    fn nothing_plays_outside_clips() {
        let clips = [clip(&counting(200_000))];
        let mut player = ClipPlayer::new(RATE, 1024);
        assert!(
            render(&mut player, &clips, 0.0, 512, 512)
                .iter()
                .all(|&s| s == 0.0)
        );
        assert!(
            render(&mut player, &clips, 3.0, 512, 512)
                .iter()
                .all(|&s| s == 0.0)
        );
    }

    #[test]
    fn a_jump_in_song_time_seeks_the_stream() {
        let audio = counting(200_000);
        let clips = [clip(&audio)];
        let mut player = ClipPlayer::new(RATE, 1024);
        render(&mut player, &clips, 1.0, 512, 512);
        let out = render(&mut player, &clips, 2.0, 16, 16);
        assert_eq!(
            out[0], audio.left[25_000],
            "a beat in, not where it left off"
        );
    }

    #[test]
    fn stretched_output_does_not_depend_on_span_sizes() {
        let audio = Arc::new(SampleData::mono(
            48_000,
            (0..200_000)
                .map(|i| ((i as f32) * 0.05).sin() * if i % 9_000 < 200 { 1.0 } else { 0.3 })
                .collect(),
        ));
        let mut stretched = clip(&audio);
        stretched.ratio = 1.25;
        stretched.frames_per_beat = 24_000.0 / 1.25;
        let clips = [stretched];
        let whole = render(&mut ClipPlayer::new(RATE, 1024), &clips, 1.0, 20_000, 1024);
        let sliced = render(&mut ClipPlayer::new(RATE, 1024), &clips, 1.0, 20_000, 32);
        assert_eq!(whole, sliced);
        assert!(whole.iter().any(|&s| s.abs() > 0.1), "it plays");
    }

    #[test]
    fn fades_and_gain_shape_the_level() {
        let mut faded = clip(&counting(10));
        faded.gain = 0.5;
        faded.fade_in_beats = 0.5;
        faded.fade_out_beats = 1.0;
        assert_eq!(faded.level(1.0), 0.0);
        assert_eq!(faded.level(1.25), 0.25);
        assert_eq!(faded.level(2.0), 0.5);
        assert_eq!(faded.level(2.5), 0.25);
    }

    #[test]
    fn clips_are_found_by_song_position() {
        let audio = counting(10);
        let mut second = clip(&audio);
        second.id = ClipId(2);
        second.start_beats = 3.0;
        second.end_beats = 4.0;
        let clips = [clip(&audio), second];
        assert_eq!(clip_at(&clips, 0.5), None);
        assert_eq!(clip_at(&clips, 1.0).map(|c| c.id), Some(ClipId(1)));
        assert_eq!(clip_at(&clips, 3.0).map(|c| c.id), Some(ClipId(2)));
        assert_eq!(clip_at(&clips, 4.0), None);
    }

    #[test]
    fn stopping_fades_out_instead_of_cutting() {
        let audio = Arc::new(SampleData::mono(48_000, vec![0.5; 200_000]));
        let clips = [clip(&audio)];
        let mut player = ClipPlayer::new(RATE, 1024);
        render(&mut player, &clips, 1.0, 512, 512);
        let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
        player.render(
            NodeId(3),
            &clips,
            false,
            1.1,
            BEATS_PER_FRAME,
            &mut left,
            &mut right,
        );
        assert!((left[0] - 0.5).abs() < 1e-6, "starts at full level");
        assert!(left[128] > 0.0 && left[128] < 0.5, "fading");
        assert!(left[256..].iter().all(|&s| s == 0.0), "then silent");
    }

    #[test]
    fn playing_allocates_nothing() {
        let clips = [clip(&counting(200_000))];
        let mut player = ClipPlayer::new(RATE, 1024);
        let (mut left, mut right) = (vec![0.0; 1024], vec![0.0; 1024]);
        let ((), violations) = daw_rt::count_violations(|| {
            daw_rt::realtime(|| {
                for k in 0..20 {
                    let beat = 1.0 + k as f64 * 1024.0 * BEATS_PER_FRAME;
                    player.render(
                        NodeId(3),
                        &clips,
                        true,
                        beat,
                        BEATS_PER_FRAME,
                        &mut left,
                        &mut right,
                    );
                }
                player.render(
                    NodeId(3),
                    &clips,
                    false,
                    2.0,
                    BEATS_PER_FRAME,
                    &mut left,
                    &mut right,
                );
            });
        });
        assert_eq!(violations, 0);
    }
}
