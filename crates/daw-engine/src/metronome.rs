//! The metronome on the audio side: finds the beats a chunk crosses from
//! song time (so a click's frame never depends on block size), and plays a
//! click on each, after the Song's channel. Clicks wait out the song's
//! latency, so they line up with what is heard.

use crate::transport::Transport;
use daw_dsp::click::{ClickTone, ClickVoice};
use daw_protocol::{ClickSound, Metronome};

/// Beats in a bar (the app is in 4/4).
const BEATS_PER_BAR: i64 = 4;
/// Level at full volume.
const CLICK_LEVEL: f32 = 0.7;
/// Clicks waiting for their frame. A song's latency is far under a beat,
/// so a few are plenty; more are dropped rather than allocated.
const MAX_PENDING: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct MetronomePlayer {
    settings: Metronome,
    voice: ClickVoice,
    /// (frame, accented) of clicks still to start, in frame order.
    pending: [(u64, bool); MAX_PENDING],
    pending_len: usize,
    /// One accented click at the next chunk's start ([`Self::preview`]).
    preview: bool,
}

impl MetronomePlayer {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            settings: Metronome::default(),
            voice: ClickVoice::new(sample_rate),
            pending: [(0, false); MAX_PENDING],
            pending_len: 0,
            preview: false,
        }
    }

    pub fn set(&mut self, settings: Metronome) {
        self.settings = settings;
        if !settings.on {
            self.pending_len = 0;
        }
    }

    pub fn preview(&mut self) {
        self.preview = true;
    }

    /// Forgets clicks not yet heard (the transport stopped or moved).
    pub fn clear(&mut self) {
        self.pending_len = 0;
    }

    /// Queues a click for every beat the transport crosses in the next
    /// `frames` (starting at processor frame `chunk_start`), heard
    /// `latency` frames later.
    pub fn schedule(&mut self, start: &Transport, chunk_start: u64, frames: u64, latency: u32) {
        if !self.settings.on || !start.is_playing() {
            return;
        }
        let mut t = start.clone();
        let mut done = 0;
        while done < frames {
            let (segment, jumps) = t.next_segment(frames - done);
            let mut beat = t.position().floor() as i64;
            loop {
                let until = t.frames_until(beat as f64);
                if until < 0 {
                    beat += 1;
                    continue;
                }
                if until as u64 >= segment {
                    break;
                }
                let frame = chunk_start + done + until as u64 + u64::from(latency);
                self.push(frame, beat.rem_euclid(BEATS_PER_BAR) == 0);
                beat += 1;
            }
            t.advance(segment);
            done += segment;
            if jumps {
                t.jump_to_loop_start();
            }
        }
    }

    fn push(&mut self, frame: u64, accent: bool) {
        if self.pending_len < MAX_PENDING {
            self.pending[self.pending_len] = (frame, accent);
            self.pending_len += 1;
        }
    }

    /// Adds the clicks due in this chunk (processor frames from
    /// `chunk_start`) to both sides of the output.
    pub fn render(&mut self, chunk_start: u64, left: &mut [f32], right: &mut [f32]) {
        let settings = self.settings;
        let gain = {
            let v = f32::from(settings.volume) / f32::from(Metronome::MAX_VOLUME);
            v * v * CLICK_LEVEL
        };
        let tone = tone_of(settings.sound);
        let pitch = 2f32.powf(f32::from(settings.pitch) / 12.);
        if std::mem::take(&mut self.preview) {
            self.voice.trigger(tone, true, pitch, gain);
        }
        if self.pending_len == 0 && !self.voice.is_sounding() {
            return;
        }
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            let frame = chunk_start + i as u64;
            while self.pending_len > 0 && self.pending[0].0 <= frame {
                let (_, accent) = self.pending[0];
                self.pending.copy_within(1..self.pending_len, 0);
                self.pending_len -= 1;
                self.voice
                    .trigger(tone, accent && settings.accent, pitch, gain);
            }
            let s = self.voice.next_sample();
            *l += s;
            *r += s;
        }
    }
}

fn tone_of(sound: ClickSound) -> ClickTone {
    match sound {
        ClickSound::Beep => ClickTone::Beep,
        ClickSound::Wood => ClickTone::Wood,
        ClickSound::Tick => ClickTone::Tick,
        ClickSound::Cowbell => ClickTone::Cowbell,
    }
}
