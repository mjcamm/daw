//! The audio side of the engine.
//!
//! [`AudioProcessor`] runs on the audio thread. It never allocates, frees,
//! locks or blocks: it only reads from and writes to lock-free queues. State
//! arrives as whole replacement [`RenderState`]s, and each replaced state is
//! sent back to the control side to be freed there.

use crate::schedule::RenderState;
use crate::transport::Transport;
use daw_dsp::gain::db_to_gain;
use daw_dsp::pan::equal_power;
use daw_dsp::smoothing::Smoothed;
use daw_dsp::synth::{MAX_VOICES, Synth};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Frames rendered per internal chunk; host blocks of any size are split
/// into chunks of at most this many frames.
const CHUNK_FRAMES: usize = 1024;
/// Volume and pan changes ramp over this long to avoid clicks.
const GAIN_RAMP_SECONDS: f32 = 0.01;
/// Notes awaiting their scheduled end. More than the synth's voices, since
/// stolen voices may still have an end pending.
const MAX_PENDING_ENDS: usize = MAX_VOICES * 2;

/// Control-to-audio messages.
#[derive(Debug)]
pub(crate) enum AudioMsg {
    SetPlaying(bool),
    Locate(f64),
    SetLoop(Option<(f64, f64)>),
    SwapState(Box<RenderState>),
}

/// Values the audio side reports back without locking.
#[derive(Debug, Default)]
pub(crate) struct SharedStatus {
    /// Transport position in beats, as `f64` bits.
    position_beats: AtomicU64,
}

impl SharedStatus {
    pub fn position_beats(&self) -> f64 {
        f64::from_bits(self.position_beats.load(Ordering::Relaxed))
    }

    fn set_position_beats(&self, beats: f64) {
        self.position_beats
            .store(beats.to_bits(), Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub struct AudioProcessor {
    channels: usize,
    sample_rate: f32,
    node: NodePlayer,
    gain_left: Smoothed,
    gain_right: Smoothed,
    mono: Box<[f32; CHUNK_FRAMES]>,
    from_control: rtrb::Consumer<AudioMsg>,
    garbage: rtrb::Producer<Box<RenderState>>,
    status: Arc<SharedStatus>,
}

impl AudioProcessor {
    pub(crate) fn new(
        sample_rate: u32,
        channels: usize,
        state: Box<RenderState>,
        from_control: rtrb::Consumer<AudioMsg>,
        garbage: rtrb::Producer<Box<RenderState>>,
        status: Arc<SharedStatus>,
    ) -> Self {
        let sample_rate = sample_rate as f32;
        let ramp = Smoothed::ramp_for(GAIN_RAMP_SECONDS, sample_rate);
        let (left, right) = channel_gains(&state, channels);
        Self {
            channels,
            sample_rate,
            node: NodePlayer::new(sample_rate, state),
            gain_left: Smoothed::new(left, ramp),
            gain_right: Smoothed::new(right, ramp),
            mono: Box::new([0.0; CHUNK_FRAMES]),
            from_control,
            garbage,
            status,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Renders one block of interleaved audio into `out`.
    ///
    /// `out.len()` must be a multiple of the channel count. Real-time safe.
    pub fn process(&mut self, out: &mut [f32]) {
        daw_rt::realtime(|| {
            debug_assert_eq!(out.len() % self.channels, 0, "partial frame in block");
            self.receive();
            for chunk in out.chunks_mut(CHUNK_FRAMES * self.channels) {
                let mono = &mut self.mono[..chunk.len() / self.channels];
                self.node.render(mono);
                self.mix(chunk);
            }
            self.status
                .set_position_beats(self.node.transport.position());
        });
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &RenderState {
        &self.node.state
    }

    fn receive(&mut self) {
        // Only take a message while the garbage queue has room, so a replaced
        // state can always be handed back instead of freed here.
        while self.garbage.slots() > 0 {
            let Ok(msg) = self.from_control.pop() else {
                break;
            };
            match msg {
                AudioMsg::SetPlaying(playing) => self.node.set_playing(playing),
                AudioMsg::Locate(beats) => self.node.locate(beats),
                AudioMsg::SetLoop(range) => self.node.transport.set_loop(range),
                AudioMsg::SwapState(state) => {
                    let (left, right) = channel_gains(&state, self.channels);
                    self.gain_left.set_target(left);
                    self.gain_right.set_target(right);
                    let old = self.node.swap_state(state);
                    if self.garbage.push(old).is_err() {
                        unreachable!("garbage queue had a free slot");
                    }
                }
            }
        }
        self.node.seek();
    }

    /// Writes the first `out.len() / channels` mono samples to `out` with
    /// volume and pan applied.
    fn mix(&mut self, out: &mut [f32]) {
        for (frame, &sample) in out.chunks_exact_mut(self.channels).zip(self.mono.iter()) {
            let left = self.gain_left.next_value();
            let right = self.gain_right.next_value();
            match frame {
                [only] => *only = sample * left,
                [l, r, rest @ ..] => {
                    *l = sample * left;
                    *r = sample * right;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
    }
}

/// Target gains for the first two output channels. Mono output ignores pan.
fn channel_gains(state: &RenderState, channels: usize) -> (f32, f32) {
    let gain = db_to_gain(state.volume_db);
    if channels == 1 {
        return (gain, gain);
    }
    let (left, right) = equal_power(state.pan);
    (gain * left, gain * right)
}

/// A sounding note waiting for its scheduled end.
#[derive(Debug, Clone, Copy)]
struct PendingEnd {
    note_id: u32,
    end_beats: f64,
}

/// Plays a node's notes through its instrument in song time.
#[derive(Debug)]
struct NodePlayer {
    state: Box<RenderState>,
    transport: Transport,
    synth: Synth,
    /// Index of the next note in `state.notes` that has not started.
    cursor: usize,
    pending_ends: [Option<PendingEnd>; MAX_PENDING_ENDS],
    next_note_id: u32,
}

impl NodePlayer {
    fn new(sample_rate: f32, state: Box<RenderState>) -> Self {
        let mut synth = Synth::new(sample_rate);
        synth.set_params(state.instrument);
        Self {
            transport: Transport::new(f64::from(sample_rate), state.tempo_bpm),
            state,
            synth,
            cursor: 0,
            pending_ends: [None; MAX_PENDING_ENDS],
            next_note_id: 0,
        }
    }

    fn set_playing(&mut self, playing: bool) {
        if !playing {
            self.release_all();
        }
        self.transport.set_playing(playing);
    }

    fn locate(&mut self, beats: f64) {
        self.release_all();
        self.transport.locate(beats);
    }

    /// Installs a new state and returns the old one, which the caller must
    /// not free on the audio thread.
    fn swap_state(&mut self, state: Box<RenderState>) -> Box<RenderState> {
        self.transport.set_tempo(state.tempo_bpm);
        self.synth.set_params(state.instrument);
        std::mem::replace(&mut self.state, state)
    }

    /// Renders mono audio, starting and ending notes on their exact frames.
    fn render(&mut self, mono: &mut [f32]) {
        if !self.transport.is_playing() {
            self.synth.render(mono);
            return;
        }
        let frames = mono.len() as u64;
        let mut done = 0;
        while done < frames {
            let (segment, jumps) = self.transport.next_segment(frames - done);
            let segment_end = done + segment;
            while done < segment_end {
                self.fire_due_events();
                let next = self.frames_to_next_event().min(segment_end - done);
                self.synth
                    .render(&mut mono[done as usize..(done + next) as usize]);
                self.transport.advance(next);
                done += next;
            }
            if jumps {
                self.transport.jump_to_loop_start();
                self.release_all();
                self.seek();
            }
        }
    }

    /// Ends and starts every note due on the current frame. Afterwards every
    /// remaining event is in the future.
    fn fire_due_events(&mut self) {
        self.end_due_notes();
        while let Some(&note) = self.state.notes.get(self.cursor) {
            if self.transport.frames_until(note.start_beats) > 0 {
                break;
            }
            self.cursor += 1;
            self.start_note(note.end_beats, note.pitch, note.velocity);
        }
        // Notes shorter than a frame end on the frame they start.
        self.end_due_notes();
    }

    fn end_due_notes(&mut self) {
        for slot in &mut self.pending_ends {
            if let Some(pending) = *slot
                && self.transport.frames_until(pending.end_beats) <= 0
            {
                self.synth.note_off(pending.note_id);
                *slot = None;
            }
        }
    }

    fn start_note(&mut self, end_beats: f64, pitch: u8, velocity: u8) {
        let note_id = self.next_note_id;
        self.next_note_id = self.next_note_id.wrapping_add(1);
        let slot = match self.pending_ends.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                // Too many overlapping notes: end the one due soonest now.
                let (slot, soonest) = self
                    .pending_ends
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.map(|p| (i, p)))
                    .min_by(|(_, a), (_, b)| a.end_beats.total_cmp(&b.end_beats))
                    .unwrap_or((0, PendingEnd { note_id, end_beats }));
                self.synth.note_off(soonest.note_id);
                slot
            }
        };
        self.pending_ends[slot] = Some(PendingEnd { note_id, end_beats });
        self.synth.note_on(note_id, pitch, velocity);
    }

    /// Frames until the next note starts or ends; at least 1.
    fn frames_to_next_event(&self) -> u64 {
        let next_start = self
            .state
            .notes
            .get(self.cursor)
            .map(|n| self.transport.frames_until(n.start_beats));
        let next_end = self
            .pending_ends
            .iter()
            .flatten()
            .map(|p| self.transport.frames_until(p.end_beats))
            .min();
        let next = match (next_start, next_end) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => i64::MAX,
        };
        next.max(1) as u64
    }

    /// Points the cursor at the first note that starts at or after now.
    fn seek(&mut self) {
        let transport = &self.transport;
        self.cursor = self
            .state
            .notes
            .partition_point(|n| transport.frames_until(n.start_beats) < 0);
    }

    fn release_all(&mut self) {
        self.synth.release_all();
        self.pending_ends = [None; MAX_PENDING_ENDS];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    fn state(tempo_bpm: f64) -> Box<RenderState> {
        let project = Project {
            tempo_bpm,
            ..Project::default()
        };
        Box::new(RenderState::compile(&project))
    }

    #[test]
    fn full_garbage_queue_defers_state_swaps_instead_of_freeing() {
        let (mut to_audio, from_control) = rtrb::RingBuffer::new(8);
        let (garbage_tx, mut garbage_rx) = rtrb::RingBuffer::new(1);
        let mut processor = AudioProcessor::new(
            48_000,
            2,
            state(100.0),
            from_control,
            garbage_tx,
            Arc::default(),
        );
        for bpm in [110.0, 120.0, 130.0] {
            to_audio.push(AudioMsg::SwapState(state(bpm))).unwrap();
        }
        let mut block = [0.0; 4];

        // Only one swap fits in the garbage queue; the rest wait. The realtime
        // guard aborts the test if the processor frees anything.
        processor.process(&mut block);
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 110.0);

        drop(garbage_rx.pop().unwrap());
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 120.0);

        drop(garbage_rx.pop().unwrap());
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 130.0);
    }
}
