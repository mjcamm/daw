//! The audio side of the engine.
//!
//! [`AudioProcessor`] runs on the audio thread. It never allocates, frees,
//! locks or blocks: it only reads from and writes to lock-free queues. State
//! arrives as whole replacement [`RenderState`]s, and each replaced state is
//! sent back to the control side to be freed there.

use crate::devices::{
    CHANNEL_PARAMS, INSTRUMENT_PARAMS, MAX_DEVICE_PARAMS, ParamInfo, channel_settings,
    effect_params, filter_params, synth_params,
};
use crate::project::MAX_EFFECTS;
use crate::schedule::RenderState;
use crate::transport::Transport;
use daw_automation::Automation;
use daw_dsp::filter_effect::FilterEffect;
use daw_dsp::pan::equal_power;
use daw_dsp::smoothing::Smoothed;
use daw_dsp::synth::{MAX_VOICES, Synth};
use daw_protocol::{DeviceId, EffectKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Frames rendered per internal chunk; host blocks of any size are split
/// into chunks of at most this many frames.
const CHUNK_FRAMES: usize = 1024;
/// Parameters are evaluated every this many frames, counted from the
/// processor's start so results never depend on host block sizes. Gains ramp
/// linearly between evaluations; other parameters step.
pub const CONTROL_FRAMES: u64 = 32;
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
    left: Box<[f32; CHUNK_FRAMES]>,
    right: Box<[f32; CHUNK_FRAMES]>,
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
        Self {
            channels,
            sample_rate,
            node: NodePlayer::new(sample_rate, state),
            left: Box::new([0.0; CHUNK_FRAMES]),
            right: Box::new([0.0; CHUNK_FRAMES]),
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
                let frames = chunk.len() / self.channels;
                let (left, right) = (&mut self.left[..frames], &mut self.right[..frames]);
                self.node.render(left, right);
                write_output(left, right, chunk, self.channels);
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
                    let old = self.node.swap_state(state);
                    if self.garbage.push(old).is_err() {
                        unreachable!("garbage queue had a free slot");
                    }
                }
            }
        }
        self.node.seek();
    }
}

/// Writes the node's stereo output into interleaved `out`. Mono output sums
/// both sides at equal power; channels beyond two are silent.
fn write_output(left: &[f32], right: &[f32], out: &mut [f32], channels: usize) {
    for ((frame, &l), &r) in out.chunks_exact_mut(channels).zip(left).zip(right) {
        match frame {
            [only] => *only = (l + r) * std::f32::consts::FRAC_1_SQRT_2,
            [a, b, rest @ ..] => {
                *a = l;
                *b = r;
                rest.fill(0.0);
            }
            [] => {}
        }
    }
}

/// A running effect. Holds no heap memory, so creating and dropping one on
/// the audio thread is allocation-free.
#[derive(Debug)]
enum EffectInstance {
    Filter(FilterEffect),
}

impl EffectInstance {
    fn new(kind: EffectKind, sample_rate: f32) -> Self {
        match kind {
            EffectKind::Filter => Self::Filter(FilterEffect::new(sample_rate)),
        }
    }

    fn kind(&self) -> EffectKind {
        match self {
            Self::Filter(_) => EffectKind::Filter,
        }
    }

    /// Applies real-unit parameter values in the kind's parameter order.
    fn set_params(&mut self, plain: &[f64]) {
        match self {
            Self::Filter(filter) => filter.set_params(filter_params(plain)),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            Self::Filter(filter) => filter.process(left, right),
        }
    }
}

/// A sounding note waiting for its scheduled end.
#[derive(Debug, Clone, Copy)]
struct PendingEnd {
    note_id: u32,
    end_beats: f64,
}

/// Plays a node in song time: notes into the instrument, then the effect
/// chain, then volume and pan, with every parameter following its layers.
#[derive(Debug)]
struct NodePlayer {
    sample_rate: f32,
    state: Box<RenderState>,
    transport: Transport,
    synth: Synth,
    /// The running chain, in the order of `state.effects`. Capacity is
    /// reserved up front so rebuilding it never allocates.
    effects: Vec<(DeviceId, EffectInstance)>,
    spare_effects: Vec<(DeviceId, EffectInstance)>,
    gain_left: Smoothed,
    gain_right: Smoothed,
    /// Frames rendered since the processor started; sets the control grid.
    frames_processed: u64,
    /// Index of the next note in `state.notes` that has not started.
    cursor: usize,
    pending_ends: [Option<PendingEnd>; MAX_PENDING_ENDS],
    next_note_id: u32,
}

impl NodePlayer {
    fn new(sample_rate: f32, state: Box<RenderState>) -> Self {
        let mut player = Self {
            sample_rate,
            transport: Transport::new(f64::from(sample_rate), state.tempo_bpm),
            state,
            synth: Synth::new(sample_rate),
            effects: Vec::with_capacity(MAX_EFFECTS),
            spare_effects: Vec::with_capacity(MAX_EFFECTS),
            gain_left: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            gain_right: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            frames_processed: 0,
            cursor: 0,
            pending_ends: [None; MAX_PENDING_ENDS],
            next_note_id: 0,
        };
        player.sync_effects();
        player.update_params();
        // Start at the initial gains rather than ramping up from silence.
        player.gain_left = Smoothed::new(player.gain_left.target(), CONTROL_FRAMES as u32);
        player.gain_right = Smoothed::new(player.gain_right.target(), CONTROL_FRAMES as u32);
        player
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
        let old = std::mem::replace(&mut self.state, state);
        self.sync_effects();
        old
    }

    /// Rebuilds the running chain to match `state.effects`, keeping existing
    /// effects (and their filter memory) by id. Allocation-free.
    fn sync_effects(&mut self) {
        debug_assert!(self.state.effects.len() <= MAX_EFFECTS);
        self.spare_effects.clear();
        for wanted in &self.state.effects {
            let existing = self
                .effects
                .iter()
                .position(|(id, e)| *id == wanted.id && e.kind() == wanted.kind);
            let instance = match existing {
                Some(index) => self.effects.swap_remove(index).1,
                None => EffectInstance::new(wanted.kind, self.sample_rate),
            };
            self.spare_effects.push((wanted.id, instance));
        }
        // Whatever is left was removed from the chain.
        self.effects.clear();
        std::mem::swap(&mut self.effects, &mut self.spare_effects);
    }

    /// Evaluates every parameter's layers at the current position and applies
    /// the results.
    fn update_params(&mut self) {
        let beat = self.transport.position();
        let mut plain = [0.0; MAX_DEVICE_PARAMS];

        evaluate(&self.state.instrument, &INSTRUMENT_PARAMS, beat, &mut plain);
        self.synth.set_params(synth_params(&plain));

        for ((_, instance), effect) in self.effects.iter_mut().zip(&self.state.effects) {
            evaluate(&effect.params, effect_params(effect.kind), beat, &mut plain);
            instance.set_params(&plain);
        }

        evaluate(&self.state.channel, &CHANNEL_PARAMS, beat, &mut plain);
        let (gain, pan) = channel_settings(&plain);
        let (left, right) = equal_power(pan);
        self.gain_left.set_target(gain * left);
        self.gain_right.set_target(gain * right);
    }

    /// Renders stereo audio. Notes start and end on their exact frames;
    /// parameters update on the control grid.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len() as u64;
        let mut done = 0;
        while done < frames {
            let playing = self.transport.is_playing();
            let (segment, jumps) = if playing {
                self.transport.next_segment(frames - done)
            } else {
                (frames - done, false)
            };
            let segment_end = done + segment;
            while done < segment_end {
                let into_control = self.frames_processed % CONTROL_FRAMES;
                if into_control == 0 {
                    self.update_params();
                }
                let mut next = (CONTROL_FRAMES - into_control).min(segment_end - done);
                if playing {
                    self.fire_due_events();
                    next = next.min(self.frames_to_next_event());
                }
                let span = done as usize..(done + next) as usize;
                self.render_span(&mut left[span.clone()], &mut right[span]);
                if playing {
                    self.transport.advance(next);
                }
                self.frames_processed += next;
                done += next;
            }
            if jumps {
                self.transport.jump_to_loop_start();
                self.release_all();
                self.seek();
            }
        }
    }

    /// Renders a span with steady parameters: instrument, effects, channel.
    fn render_span(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.synth.render(left);
        right.copy_from_slice(left);
        for (_, effect) in &mut self.effects {
            effect.process(left, right);
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l *= self.gain_left.next_value();
            *r *= self.gain_right.next_value();
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

/// Evaluates each parameter's automation at `beat` into real units.
fn evaluate(automations: &[Automation], infos: &[ParamInfo], beat: f64, plain: &mut [f64]) {
    for ((out, automation), info) in plain.iter_mut().zip(automations).zip(infos) {
        *out = info.scale.to_plain(automation.value_at(beat));
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
