//! The audio side of the engine.
//!
//! [`AudioProcessor`] runs on the audio thread. It never allocates, frees,
//! locks or blocks: it only reads from and writes to lock-free queues. State
//! arrives as whole replacement [`RenderState`]s, and each replaced state is
//! sent back to the control side to be freed there.
//!
//! Every node has a player (its instrument voices, effect memory and gain
//! smoothing) from a pool made up front, matched to its node by id across
//! state swaps so editing never resets sound. Nodes render in the state's
//! order, children before parents: each adds its output into its parent's
//! bus, and the Song's output is the processor's.

use crate::devices::{
    CHANNEL_PARAMS, INSTRUMENT_PARAMS, MAX_DEVICE_PARAMS, ParamInfo, channel_settings,
    effect_params, filter_params, synth_params,
};
use crate::project::{MAX_EFFECTS, MAX_NODES};
use crate::schedule::{EffectState, NodeRender, RenderState};
use crate::transport::Transport;
use daw_automation::Automation;
use daw_dsp::filter_effect::FilterEffect;
use daw_dsp::pan::{balance, equal_power};
use daw_dsp::smoothing::Smoothed;
use daw_dsp::synth::{MAX_VOICES, Synth};
use daw_protocol::{DeviceId, EffectKind, NodeId};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
/// Marks a slot with no player yet while players are being assigned.
const NO_PLAYER: usize = usize::MAX;

/// Control-to-audio messages.
#[derive(Debug)]
pub(crate) enum AudioMsg {
    SetPlaying(bool),
    Locate(f64),
    SetLoop(Option<(f64, f64)>),
    SwapState(Box<RenderState>),
}

/// Peaks since last read, as `f32` bits. Non-negative floats order the same
/// as their bits, so `fetch_max` on the bits keeps the louder.
#[derive(Debug, Default)]
struct Peaks {
    left: AtomicU32,
    right: AtomicU32,
}

impl Peaks {
    fn record(&self, left: f32, right: f32) {
        self.left.fetch_max(left.to_bits(), Ordering::Relaxed);
        self.right.fetch_max(right.to_bits(), Ordering::Relaxed);
    }

    fn take(&self) -> (f32, f32) {
        (
            f32::from_bits(self.left.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.right.swap(0, Ordering::Relaxed)),
        )
    }
}

/// One player's meter: the node it plays (id + 1; 0 while unused) and its
/// output peaks.
#[derive(Debug, Default)]
struct PlayerMeter {
    node: AtomicU64,
    peaks: Peaks,
}

/// Values the audio side reports back without locking.
#[derive(Debug)]
pub(crate) struct SharedStatus {
    /// Transport position in beats, as `f64` bits.
    position_beats: AtomicU64,
    /// The processor's output.
    output: Peaks,
    /// One per player.
    meters: Box<[PlayerMeter]>,
}

impl Default for SharedStatus {
    fn default() -> Self {
        Self {
            position_beats: AtomicU64::default(),
            output: Peaks::default(),
            meters: (0..MAX_NODES).map(|_| PlayerMeter::default()).collect(),
        }
    }
}

impl SharedStatus {
    pub fn position_beats(&self) -> f64 {
        f64::from_bits(self.position_beats.load(Ordering::Relaxed))
    }

    fn set_position_beats(&self, beats: f64) {
        self.position_beats
            .store(beats.to_bits(), Ordering::Relaxed);
    }

    /// Output peaks since the last call, resetting them.
    pub fn take_peaks(&self) -> (f32, f32) {
        self.output.take()
    }

    /// Each node's output peaks since the last call, resetting them.
    pub fn take_node_peaks(&self) -> Vec<(NodeId, f32, f32)> {
        self.meters
            .iter()
            .filter_map(|meter| {
                let node = meter.node.load(Ordering::Relaxed).checked_sub(1)?;
                let (left, right) = meter.peaks.take();
                Some((NodeId(node), left, right))
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct AudioProcessor {
    channels: usize,
    sample_rate: f32,
    transport: Transport,
    state: Box<RenderState>,
    /// The pool: one player per possible node.
    players: Box<[NodePlayer]>,
    /// For each node in `state.nodes`, the player playing it.
    slots: Vec<usize>,
    /// Where an instrument renders before joining its node's bus.
    scratch: Box<[f32; CHUNK_FRAMES]>,
    left: Box<[f32; CHUNK_FRAMES]>,
    right: Box<[f32; CHUNK_FRAMES]>,
    /// Frames rendered since the processor started; sets the control grid.
    frames_processed: u64,
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
        let mut processor = Self {
            channels,
            sample_rate,
            transport: Transport::new(f64::from(sample_rate), state.tempo_bpm),
            state,
            players: (0..MAX_NODES)
                .map(|_| NodePlayer::new(sample_rate))
                .collect(),
            slots: Vec::with_capacity(MAX_NODES),
            scratch: Box::new([0.0; CHUNK_FRAMES]),
            left: Box::new([0.0; CHUNK_FRAMES]),
            right: Box::new([0.0; CHUNK_FRAMES]),
            frames_processed: 0,
            from_control,
            garbage,
            status,
        };
        processor.assign_players();
        processor
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
                self.render(frames);
                let (left, right) = (&self.left[..frames], &self.right[..frames]);
                self.status.output.record(peak(left), peak(right));
                write_output(left, right, chunk, self.channels);
            }
            self.status.set_position_beats(self.transport.position());
        });
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &RenderState {
        &self.state
    }

    fn receive(&mut self) {
        // Only take a message while the garbage queue has room, so a replaced
        // state can always be handed back instead of freed here.
        while self.garbage.slots() > 0 {
            let Ok(msg) = self.from_control.pop() else {
                break;
            };
            match msg {
                AudioMsg::SetPlaying(playing) => {
                    if !playing {
                        self.release_all();
                    }
                    self.transport.set_playing(playing);
                }
                AudioMsg::Locate(beats) => {
                    self.release_all();
                    self.transport.locate(beats);
                }
                AudioMsg::SetLoop(range) => self.transport.set_loop(range),
                AudioMsg::SwapState(state) => {
                    self.transport.set_tempo(state.tempo_bpm);
                    let old = std::mem::replace(&mut self.state, state);
                    self.assign_players();
                    if self.garbage.push(old).is_err() {
                        unreachable!("garbage queue had a free slot");
                    }
                }
            }
        }
        self.seek();
    }

    /// Gives every node in the state a player: its own if it had one, else
    /// a free one, started fresh. Players whose nodes are gone are freed.
    /// Allocation-free.
    fn assign_players(&mut self) {
        debug_assert!(self.state.nodes.len() <= MAX_NODES);
        let mut taken = [false; MAX_NODES];
        self.slots.clear();
        for node in &self.state.nodes {
            let own = self.players.iter().position(|p| p.node == Some(node.id));
            if let Some(index) = own {
                taken[index] = true;
            }
            self.slots.push(own.unwrap_or(NO_PLAYER));
        }
        for (index, player) in self.players.iter_mut().enumerate() {
            if !taken[index] && player.node.is_some() {
                player.free(&self.status.meters[index]);
            }
        }
        for slot in 0..self.slots.len() {
            if self.slots[slot] == NO_PLAYER {
                let Some(free) = taken.iter().position(|t| !t) else {
                    unreachable!("the control side keeps nodes within MAX_NODES");
                };
                taken[free] = true;
                self.slots[slot] = free;
                let id = self.state.nodes[slot].id;
                self.players[free].start(id, &self.status.meters[free]);
            }
        }
        let beat = self.transport.position();
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            let player = &mut self.players[index];
            player.sync_effects(&node.effects, self.sample_rate);
            if player.fresh {
                // Start at the node's gains rather than ramping up from
                // silence.
                player.update_params(node, beat);
                player.settle_gains();
                player.fresh = false;
            }
        }
    }

    /// Renders `frames` of the whole tree into `left` and `right`. Notes
    /// start and end on their exact frames; parameters update on the
    /// control grid.
    fn render(&mut self, frames: usize) {
        for &index in &self.slots {
            self.players[index].clear_bus(frames);
        }
        let frames = frames as u64;
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
                self.render_span(done as usize..(done + next) as usize);
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
        for (slot, &index) in self.slots.iter().enumerate() {
            let player = &self.players[index];
            let frames = frames as usize;
            self.status.meters[index].peaks.record(
                peak(&player.bus_left[..frames]),
                peak(&player.bus_right[..frames]),
            );
            if self.state.nodes[slot].parent.is_none() {
                self.left[..frames].copy_from_slice(&player.bus_left[..frames]);
                self.right[..frames].copy_from_slice(&player.bus_right[..frames]);
            }
        }
    }

    /// Renders a span with steady parameters, node by node in render order:
    /// instrument into the node's bus (which already holds its children),
    /// effects, volume and pan, then into the parent's bus.
    fn render_span(&mut self, span: std::ops::Range<usize>) {
        for (slot, node) in self.state.nodes.iter().enumerate() {
            let index = self.slots[slot];
            let player = &mut self.players[index];
            if node.instrument.is_some() {
                let scratch = &mut self.scratch[span.clone()];
                player.synth.render(scratch);
                if node.instrument_on {
                    let bus = player.bus_left[span.clone()]
                        .iter_mut()
                        .zip(&mut player.bus_right[span.clone()]);
                    for ((l, r), s) in bus.zip(scratch.iter()) {
                        *l += *s;
                        *r += *s;
                    }
                }
            }
            player.process_span(node, span.clone());
            if let Some(parent) = node.parent {
                let (child, parent) = two_mut(&mut self.players, index, self.slots[parent]);
                for (to, from) in parent.bus_left[span.clone()]
                    .iter_mut()
                    .zip(&child.bus_left[span.clone()])
                {
                    *to += *from;
                }
                for (to, from) in parent.bus_right[span.clone()]
                    .iter_mut()
                    .zip(&child.bus_right[span.clone()])
                {
                    *to += *from;
                }
            }
        }
    }

    fn update_params(&mut self) {
        let beat = self.transport.position();
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            self.players[index].update_params(node, beat);
        }
    }

    /// Ends and starts every note due on the current frame, on every node.
    fn fire_due_events(&mut self) {
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            self.players[index].fire_due_events(node, &self.transport);
        }
    }

    /// Frames until the next note starts or ends on any node; at least 1.
    fn frames_to_next_event(&self) -> u64 {
        self.state
            .nodes
            .iter()
            .zip(&self.slots)
            .map(|(node, &index)| self.players[index].frames_to_next_event(node, &self.transport))
            .min()
            .unwrap_or(i64::MAX)
            .max(1) as u64
    }

    /// Points every node's cursor at its first note at or after now.
    fn seek(&mut self) {
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            self.players[index].seek(node, &self.transport);
        }
    }

    fn release_all(&mut self) {
        for &index in &self.slots {
            self.players[index].release_all();
        }
    }
}

/// Two different elements of a slice, mutably.
fn two_mut<T>(items: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    debug_assert_ne!(a, b);
    if a < b {
        let (low, high) = items.split_at_mut(b);
        (&mut low[a], &mut high[0])
    } else {
        let (low, high) = items.split_at_mut(a);
        (&mut high[0], &mut low[b])
    }
}

/// Writes stereo output into interleaved `out`. Mono output sums both sides
/// at equal power; channels beyond two are silent.
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

/// Plays one node: its notes into its instrument, then its bus through its
/// effect chain, volume and pan, with every parameter following its layers.
#[derive(Debug)]
struct NodePlayer {
    /// The node this player plays; `None` while it waits in the pool.
    node: Option<NodeId>,
    /// Just given a node: its gains start where they should be.
    fresh: bool,
    synth: Synth,
    /// The running chain, in the order of the node's effects. Capacity is
    /// reserved up front so rebuilding it never allocates.
    effects: Vec<(DeviceId, EffectInstance)>,
    spare_effects: Vec<(DeviceId, EffectInstance)>,
    gain_left: Smoothed,
    gain_right: Smoothed,
    /// Index of the next note in the node's notes that has not started.
    cursor: usize,
    pending_ends: [Option<PendingEnd>; MAX_PENDING_ENDS],
    next_note_id: u32,
    /// The node's sound for the current chunk: its children's, its
    /// instrument's, then processed in place.
    bus_left: Box<[f32; CHUNK_FRAMES]>,
    bus_right: Box<[f32; CHUNK_FRAMES]>,
}

impl NodePlayer {
    fn new(sample_rate: f32) -> Self {
        Self {
            node: None,
            fresh: false,
            synth: Synth::new(sample_rate),
            effects: Vec::with_capacity(MAX_EFFECTS),
            spare_effects: Vec::with_capacity(MAX_EFFECTS),
            gain_left: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            gain_right: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            cursor: 0,
            pending_ends: [None; MAX_PENDING_ENDS],
            next_note_id: 0,
            bus_left: Box::new([0.0; CHUNK_FRAMES]),
            bus_right: Box::new([0.0; CHUNK_FRAMES]),
        }
    }

    /// Takes on `node`, starting from silence. Allocation-free.
    fn start(&mut self, node: NodeId, meter: &PlayerMeter) {
        self.node = Some(node);
        self.fresh = true;
        self.synth.release_all();
        self.effects.clear();
        self.cursor = 0;
        self.pending_ends = [None; MAX_PENDING_ENDS];
        meter.node.store(node.0 + 1, Ordering::Relaxed);
        meter.peaks.take();
    }

    /// Goes back to the pool.
    fn free(&mut self, meter: &PlayerMeter) {
        self.node = None;
        self.release_all();
        self.effects.clear();
        meter.node.store(0, Ordering::Relaxed);
        meter.peaks.take();
    }

    fn clear_bus(&mut self, frames: usize) {
        self.bus_left[..frames].fill(0.0);
        self.bus_right[..frames].fill(0.0);
    }

    fn settle_gains(&mut self) {
        self.gain_left = Smoothed::new(self.gain_left.target(), CONTROL_FRAMES as u32);
        self.gain_right = Smoothed::new(self.gain_right.target(), CONTROL_FRAMES as u32);
    }

    /// Rebuilds the running chain to match `wanted`, keeping existing effects
    /// (and their filter memory) by id. Allocation-free.
    fn sync_effects(&mut self, wanted: &[EffectState], sample_rate: f32) {
        debug_assert!(wanted.len() <= MAX_EFFECTS);
        self.spare_effects.clear();
        for effect in wanted {
            let existing = self
                .effects
                .iter()
                .position(|(id, e)| *id == effect.id && e.kind() == effect.kind);
            let instance = match existing {
                Some(index) => self.effects.swap_remove(index).1,
                None => EffectInstance::new(effect.kind, sample_rate),
            };
            self.spare_effects.push((effect.id, instance));
        }
        // Whatever is left was removed from the chain.
        self.effects.clear();
        std::mem::swap(&mut self.effects, &mut self.spare_effects);
    }

    /// Evaluates every parameter's layers at `beat` and applies the results.
    fn update_params(&mut self, node: &NodeRender, beat: f64) {
        let mut plain = [0.0; MAX_DEVICE_PARAMS];

        if let Some(instrument) = &node.instrument {
            evaluate(instrument, &INSTRUMENT_PARAMS, beat, &mut plain);
            self.synth.set_params(synth_params(&plain));
        }

        for ((_, instance), effect) in self.effects.iter_mut().zip(&node.effects) {
            evaluate(&effect.params, effect_params(effect.kind), beat, &mut plain);
            instance.set_params(&plain);
        }

        evaluate(&node.channel, &CHANNEL_PARAMS, beat, &mut plain);
        let (gain, pan) = channel_settings(&plain);
        let gain = if node.output_on { gain } else { 0.0 };
        // An instrument is a mono source: pan it with equal power. Anything
        // else is a stereo bus: balance it, so a centred folder passes its
        // sound unchanged.
        let (left, right) = if node.instrument.is_some() {
            equal_power(pan)
        } else {
            balance(pan)
        };
        self.gain_left.set_target(gain * left);
        self.gain_right.set_target(gain * right);
    }

    /// Effects (those switched on), then volume and pan, over a span of
    /// the bus.
    fn process_span(&mut self, node: &NodeRender, span: std::ops::Range<usize>) {
        let (left, right) = (&mut self.bus_left[span.clone()], &mut self.bus_right[span]);
        for ((_, effect), state) in self.effects.iter_mut().zip(&node.effects) {
            if state.enabled {
                effect.process(left, right);
            }
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l *= self.gain_left.next_value();
            *r *= self.gain_right.next_value();
        }
    }

    /// Ends and starts every note due on the current frame. Afterwards every
    /// remaining event is in the future.
    fn fire_due_events(&mut self, node: &NodeRender, transport: &Transport) {
        self.end_due_notes(transport);
        while let Some(&note) = node.notes.get(self.cursor) {
            if transport.frames_until(note.start_beats) > 0 {
                break;
            }
            self.cursor += 1;
            self.start_note(note.end_beats, note.pitch, note.velocity);
        }
        // Notes shorter than a frame end on the frame they start.
        self.end_due_notes(transport);
    }

    fn end_due_notes(&mut self, transport: &Transport) {
        for slot in &mut self.pending_ends {
            if let Some(pending) = *slot
                && transport.frames_until(pending.end_beats) <= 0
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

    /// Frames until this node's next note starts or ends.
    fn frames_to_next_event(&self, node: &NodeRender, transport: &Transport) -> i64 {
        let next_start = node
            .notes
            .get(self.cursor)
            .map(|n| transport.frames_until(n.start_beats));
        let next_end = self
            .pending_ends
            .iter()
            .flatten()
            .map(|p| transport.frames_until(p.end_beats))
            .min();
        match (next_start, next_end) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => i64::MAX,
        }
    }

    /// Points the cursor at the first note that starts at or after now.
    fn seek(&mut self, node: &NodeRender, transport: &Transport) {
        self.cursor = node
            .notes
            .partition_point(|n| transport.frames_until(n.start_beats) < 0);
    }

    fn release_all(&mut self) {
        self.synth.release_all();
        self.pending_ends = [None; MAX_PENDING_ENDS];
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |p, s| p.max(s.abs()))
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

    #[test]
    fn players_follow_their_nodes_across_swaps() {
        let (mut to_audio, from_control) = rtrb::RingBuffer::new(8);
        let (garbage_tx, _garbage_rx) = rtrb::RingBuffer::new(8);
        let mut processor = AudioProcessor::new(
            48_000,
            2,
            state(120.0),
            from_control,
            garbage_tx,
            Arc::default(),
        );
        let player_of = |p: &AudioProcessor, id: NodeId| {
            p.slots[p.state.nodes.iter().position(|n| n.id == id).unwrap()]
        };
        let first = player_of(&processor, crate::project::FIRST_NODE);

        let mut project = Project::default();
        project
            .apply(daw_protocol::Edit::AddNode {
                id: NodeId(7),
                parent: NodeId::ROOT,
                index: 0,
                node: daw_protocol::Node::new("Folder", 0),
            })
            .unwrap();
        to_audio
            .push(AudioMsg::SwapState(Box::new(RenderState::compile(
                &project,
            ))))
            .unwrap();
        processor.process(&mut [0.0; 4]);
        assert_eq!(
            player_of(&processor, crate::project::FIRST_NODE),
            first,
            "kept its player"
        );
        let folder = player_of(&processor, NodeId(7));
        assert_ne!(folder, first);
        assert_eq!(processor.players[folder].node, Some(NodeId(7)));
    }
}
