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

use crate::clips::{ClipPlayer, ClipRender};
use crate::compensation::Compensation;
use crate::devices::{
    CHANNEL_PARAMS, MAX_DEVICE_PARAMS, ParamInfo, channel_settings, effect_params, filter_params,
    instrument_params, sampler_level, synth_params,
};
use crate::parallel::{Pool, Shards};
use crate::project::{MAX_EFFECTS, MAX_INSTRUMENTS, MAX_NODES};
use crate::samples::SampleData;
use crate::schedule::{EffectState, InstrumentState, NodeRender, RenderState};
use crate::transport::Transport;
use daw_automation::Automation;
use daw_dsp::filter_effect::FilterEffect;
use daw_dsp::pan::{balance, equal_power};
use daw_dsp::sampler::Sampler;
use daw_dsp::smoothing::Smoothed;
use daw_dsp::synth::{MAX_VOICES, Synth};
use daw_plugins::block::{
    Context as PluginContext, DawBlock, MAX_PARAM_CHANGES, NoteEvent, Outcome, ParamChange,
};
use daw_protocol::{DeviceId, EffectKind, InstrumentKind, NodeId};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Frames rendered per internal chunk; host blocks of any size are split
/// into chunks of at most this many frames.
pub(crate) const CHUNK_FRAMES: usize = 1024;
/// Parameters are evaluated every this many frames, counted from the
/// processor's start so results never depend on host block sizes. Gains ramp
/// linearly between evaluations; other parameters step.
pub const CONTROL_FRAMES: u64 = 32;
/// Notes awaiting their scheduled end. More than the synth's voices, since
/// stolen voices may still have an end pending.
const MAX_PENDING_ENDS: usize = MAX_VOICES * 2;
/// Marks a slot with no player yet while players are being assigned.
const NO_PLAYER: usize = usize::MAX;
/// Clip players made up front: at most this many audio nodes play clips.
pub const MAX_CLIP_PLAYERS: usize = 64;
/// Share of each block's time a plugin may take, live, before its block is
/// dropped (the rest of the tree and the device need the remainder).
const PLUGIN_DEADLINE_SHARE: f64 = 0.7;

/// What the audio side lets go of, freed on the control side: plugin
/// channels and nodes' delay lines.
#[derive(Debug)]
pub(crate) enum Leftover {
    Block(Box<DawBlock>),
    Delays(Box<Delays>),
}

pub(crate) type Retired = rtrb::Producer<Leftover>;

/// Hands a channel back to be freed on the control side. The queue has
/// room for every plugin there can be; were it ever full, leaking the
/// channel is better than freeing it here.
fn retire(retired: &mut Retired, block: Box<DawBlock>) {
    if let Err(rtrb::PushError::Full(left)) = retired.push(Leftover::Block(block)) {
        std::mem::forget(left);
    }
}

fn retire_delays(retired: &mut Retired, delays: Box<Delays>) {
    if let Err(rtrb::PushError::Full(left)) = retired.push(Leftover::Delays(delays)) {
        std::mem::forget(left);
    }
}

/// A stereo delay line for lining sound up (`compensation.rs`). Its
/// room is made on the control side; delaying never allocates.
#[derive(Debug)]
pub(crate) struct DelayLine {
    left: Box<[f32]>,
    right: Box<[f32]>,
    write: usize,
}

impl DelayLine {
    fn new(capacity: usize) -> Self {
        Self {
            left: vec![0.0; capacity].into_boxed_slice(),
            right: vec![0.0; capacity].into_boxed_slice(),
            write: 0,
        }
    }

    /// Delays the sound in place by `delay` frames (at most the room
    /// less one).
    fn process(&mut self, left: &mut [f32], right: &mut [f32], delay: usize) {
        let room = self.left.len();
        if delay == 0 || room < 2 {
            return;
        }
        let delay = delay.min(room - 1);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let w = self.write;
            self.left[w] = *l;
            self.right[w] = *r;
            let read = (w + room - delay) % room;
            *l = self.left[read];
            *r = self.right[read];
            self.write = (w + 1) % room;
        }
    }

    /// Takes `old`'s most recent sound, so a bigger line carries on where
    /// the old one was.
    fn take_history(&mut self, old: &Self) {
        let (room, old_room) = (self.left.len(), old.left.len());
        let n = room.min(old_room);
        for k in 0..n {
            let from = (old.write + old_room - n + k) % old_room;
            self.left[k] = old.left[from];
            self.right[k] = old.right[from];
        }
        self.write = if room == 0 { 0 } else { n % room };
    }
}

/// A node's delay lines: for its built-in sources, each instrument (by its
/// place) and its output, all with the same room.
#[derive(Debug)]
pub(crate) struct Delays {
    sources: DelayLine,
    instruments: [DelayLine; MAX_INSTRUMENTS],
    /// Standing in for switched-off plugins without their own bypass.
    effects: [DelayLine; MAX_EFFECTS],
    output: DelayLine,
}

/// A node's delay lines: sources, each instrument, each effect, output.
pub(crate) const DELAY_LINES: usize = 2 + MAX_INSTRUMENTS + MAX_EFFECTS;

/// The room each of a node's delay lines needs.
pub(crate) type Rooms = [usize; DELAY_LINES];

impl Delays {
    /// Lines with the room `rooms` says (0: none).
    pub(crate) fn new(rooms: &Rooms) -> Self {
        let line = |i: usize| DelayLine::new(rooms[i]);
        Self {
            sources: line(0),
            instruments: std::array::from_fn(|i| line(1 + i)),
            effects: std::array::from_fn(|i| line(1 + MAX_INSTRUMENTS + i)),
            output: line(DELAY_LINES - 1),
        }
    }

    /// The room each line needs for `comp`: a power of two above its
    /// delay, none for no delay.
    pub(crate) fn rooms(comp: &Compensation) -> Rooms {
        let room = |delay: u32| {
            if delay == 0 {
                0
            } else {
                (delay as usize + 1).next_power_of_two().max(64)
            }
        };
        let mut rooms = [0; DELAY_LINES];
        rooms[0] = room(comp.sources);
        for (i, &d) in comp.instruments.iter().take(MAX_INSTRUMENTS).enumerate() {
            rooms[1 + i] = room(d);
        }
        for (i, &d) in comp.bypass.iter().take(MAX_EFFECTS).enumerate() {
            rooms[1 + MAX_INSTRUMENTS + i] = room(d);
        }
        rooms[DELAY_LINES - 1] = room(comp.output);
        rooms
    }

    fn take_history(&mut self, old: &Self) {
        self.sources.take_history(&old.sources);
        for (new, old) in self.instruments.iter_mut().zip(&old.instruments) {
            new.take_history(old);
        }
        for (new, old) in self.effects.iter_mut().zip(&old.effects) {
            new.take_history(old);
        }
        self.output.take_history(&old.output);
    }
}

/// How plugin effects run this block.
#[derive(Debug, Clone, Copy)]
struct PluginRun {
    /// `None` offline: plugins take as long as they need.
    deadline: Option<Instant>,
    offline: bool,
    tempo_bpm: f64,
}

/// Control-to-audio messages.
#[derive(Debug)]
pub(crate) enum AudioMsg {
    SetPlaying(bool),
    Locate(f64),
    SetLoop(Option<(f64, f64)>),
    SwapState(Box<RenderState>),
    /// A plugin effect's audio channel to its host process.
    AttachPlugin {
        effect: DeviceId,
        block: Box<DawBlock>,
    },
    /// A node's delay lines, bigger than those it had (if any).
    SetDelays {
        node: NodeId,
        delays: Box<Delays>,
    },
    /// Takes a plugin effect's channel away (its instance is ending), so it
    /// is silent until a new one attaches.
    DetachPlugin(DeviceId),
    /// A short tone over the output, to check the device.
    TestTone,
    /// A note heard straight away while editing.
    Preview {
        node: NodeId,
        pitch: u8,
        velocity: u8,
        on: bool,
    },
}

/// The test tone: one second of A4 at -12 dB, faded in and out.
const TEST_TONE_SECONDS: f32 = 1.0;
const TEST_TONE_HZ: f32 = 440.0;
const TEST_TONE_GAIN: f32 = 0.251;

/// Preview notes use ids from here up, one per pitch, so they never meet
/// the song's own notes.
const PREVIEW_NOTE_IDS: u32 = u32::MAX - 128;

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

    pub(crate) fn set_position_beats(&self, beats: f64) {
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
    /// The pool: one player per possible node. Players of different nodes
    /// render in parallel (see [`Self::render`]).
    players: Shards<NodePlayer>,
    /// For each node in `state.nodes`, the player playing it.
    slots: Vec<usize>,
    /// Node positions grouped into levels that can render at once: leaves
    /// first, each folder after everything under it. `level_ends[n]` is
    /// where level `n` ends in `order`.
    order: Vec<usize>,
    level_ends: Vec<usize>,
    /// Each node's children, in render order: `child_ranges[node]` is a
    /// range of `child_list`.
    child_list: Vec<usize>,
    child_ranges: Vec<(usize, usize)>,
    heights: Vec<usize>,
    /// Worker threads that render nodes alongside this one.
    pool: Pool,
    /// Clip players not lent to a node. Room for all of them, so returning
    /// one never allocates.
    #[allow(
        clippy::vec_box,
        reason = "players move between this list and nodes; a box moves as one pointer"
    )]
    spare_clips: Vec<Box<ClipPlayer>>,
    left: Box<[f32; CHUNK_FRAMES]>,
    right: Box<[f32; CHUNK_FRAMES]>,
    /// Frames rendered since the processor started; sets the control grid.
    frames_processed: u64,
    /// Frames of the test tone still to play, and how far it has played.
    tone_left: u32,
    tone_played: u32,
    from_control: rtrb::Consumer<AudioMsg>,
    garbage: rtrb::Producer<Box<RenderState>>,
    retired: Retired,
    /// Offline (rendering to a file), plugins have no deadline.
    offline: bool,
    status: Arc<SharedStatus>,
}

/// The audio side's ends of its queues to the control side.
#[derive(Debug)]
pub(crate) struct AudioQueues {
    pub from_control: rtrb::Consumer<AudioMsg>,
    /// Replaced render states, freed on the control side.
    pub garbage: rtrb::Producer<Box<RenderState>>,
    pub retired: Retired,
}

impl AudioProcessor {
    /// `threads` is how many threads render, this one included (1: no
    /// workers). Workers run at audio priority when `realtime_workers`;
    /// renders to a file beside live play leave them at normal priority, so
    /// they never compete with the live song.
    pub(crate) fn new(
        sample_rate: u32,
        channels: usize,
        threads: usize,
        realtime_workers: bool,
        state: Box<RenderState>,
        queues: AudioQueues,
        status: Arc<SharedStatus>,
    ) -> Self {
        let AudioQueues {
            from_control,
            garbage,
            retired,
        } = queues;
        let rate = sample_rate;
        let sample_rate = sample_rate as f32;
        let mut processor = Self {
            channels,
            sample_rate,
            transport: Transport::new(f64::from(sample_rate), state.tempo_bpm),
            state,
            players: Shards::new((0..MAX_NODES).map(|_| NodePlayer::new(sample_rate))),
            slots: Vec::with_capacity(MAX_NODES),
            order: Vec::with_capacity(MAX_NODES),
            level_ends: Vec::with_capacity(MAX_NODES),
            child_list: Vec::with_capacity(MAX_NODES),
            child_ranges: Vec::with_capacity(MAX_NODES),
            heights: Vec::with_capacity(MAX_NODES),
            pool: Pool::new(threads.saturating_sub(1), move || {
                // Workers run at audio priority, like the device's thread.
                if !realtime_workers {
                    return;
                }
                if let Ok(handle) =
                    audio_thread_priority::promote_current_thread_to_real_time(512, rate)
                {
                    #[allow(
                        clippy::forget_non_drop,
                        reason = "elsewhere the handle's drop may lower the thread's priority again"
                    )]
                    std::mem::forget(handle);
                }
            }),
            spare_clips: (0..MAX_CLIP_PLAYERS)
                .map(|_| Box::new(ClipPlayer::new(sample_rate, CHUNK_FRAMES)))
                .collect(),
            left: Box::new([0.0; CHUNK_FRAMES]),
            right: Box::new([0.0; CHUNK_FRAMES]),
            frames_processed: 0,
            tone_left: 0,
            tone_played: 0,
            from_control,
            garbage,
            retired,
            offline: false,
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

    /// Threads that render, this one included.
    pub fn threads(&self) -> usize {
        self.pool.threads()
    }

    /// Offline (rendering to a file) plugins take as long as they need;
    /// live (the default), a plugin that misses its share of a block's
    /// time is silent for that block.
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// Renders one block of interleaved audio into `out`.
    ///
    /// `out.len()` must be a multiple of the channel count. Real-time safe.
    pub fn process(&mut self, out: &mut [f32]) {
        // Denormals (numbers near zero as sounds fade out) are flushed to
        // zero: the CPU's slow path for them could cause dropouts.
        daw_rt::realtime(|| daw_rt::without_denormals(|| self.process_block(out)));
    }

    fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len() % self.channels, 0, "partial frame in block");
        self.receive();
        let block_frames = out.len() / self.channels;
        let deadline = (!self.offline).then(|| {
            let seconds = block_frames as f64 / f64::from(self.sample_rate);
            Instant::now() + Duration::from_secs_f64(seconds * PLUGIN_DEADLINE_SHARE)
        });
        for chunk in out.chunks_mut(CHUNK_FRAMES * self.channels) {
            let frames = chunk.len() / self.channels;
            self.render(frames, deadline);
            self.add_test_tone(frames);
            let (left, right) = (&self.left[..frames], &self.right[..frames]);
            self.status.output.record(peak(left), peak(right));
            write_output(left, right, chunk, self.channels);
        }
        // While playing, what is heard is the song's latency behind.
        let behind = if self.transport.is_playing() {
            f64::from(self.state.latency) * self.transport.beats_per_frame()
        } else {
            0.0
        };
        self.status
            .set_position_beats(self.transport.position() - behind);
    }

    /// How late the song comes out (plugins' latency), in frames: a render
    /// to a file skips this many at its start.
    pub fn latency(&self) -> u32 {
        self.state.latency
    }

    /// Adds the test tone, while it plays, over the output.
    fn add_test_tone(&mut self, frames: usize) {
        if self.tone_left == 0 {
            return;
        }
        let total = (TEST_TONE_SECONDS * self.sample_rate) as u32;
        let fade = (0.01 * self.sample_rate) as u32;
        for (l, r) in self.left[..frames]
            .iter_mut()
            .zip(&mut self.right[..frames])
        {
            if self.tone_left == 0 {
                break;
            }
            let t = self.tone_played as f32 / self.sample_rate;
            let edge = self.tone_played.min(total - self.tone_played).min(fade) as f32;
            let envelope = edge / fade.max(1) as f32;
            let s = (std::f32::consts::TAU * TEST_TONE_HZ * t).sin() * TEST_TONE_GAIN * envelope;
            *l += s;
            *r += s;
            self.tone_played += 1;
            self.tone_left -= 1;
        }
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
                AudioMsg::TestTone => {
                    self.tone_left = (TEST_TONE_SECONDS * self.sample_rate) as u32;
                    self.tone_played = 0;
                }
                AudioMsg::Preview {
                    node,
                    pitch,
                    velocity,
                    on,
                } => {
                    let player = self.players.iter_mut().find(|p| p.node == Some(node));
                    if let Some(player) = player {
                        let id = PREVIEW_NOTE_IDS + u32::from(pitch);
                        let states = self
                            .state
                            .nodes
                            .iter()
                            .find(|n| n.id == node)
                            .map_or(&[][..], |n| &n.instruments[..]);
                        for (((_, instrument), state), notes) in player
                            .instruments
                            .iter_mut()
                            .zip(states)
                            .zip(player.plugin_notes.iter_mut())
                        {
                            if on {
                                if state.plays.contains(pitch) {
                                    instrument.note_on(id, pitch, velocity, 0, notes);
                                }
                            } else {
                                instrument.note_off(id, 0, notes);
                            }
                        }
                    }
                }
                AudioMsg::AttachPlugin { effect, block } => self.attach_plugin(effect, block),
                AudioMsg::DetachPlugin(effect) => self.detach_plugin(effect),
                AudioMsg::SetDelays { node, mut delays } => {
                    match self.players.iter_mut().find(|p| p.node == Some(node)) {
                        Some(player) => {
                            if let Some(old) = player.delays.take() {
                                delays.take_history(&old);
                                retire_delays(&mut self.retired, old);
                            }
                            player.delays = Some(delays);
                        }
                        None => retire_delays(&mut self.retired, delays),
                    }
                }
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

    /// Puts a plugin effect's channel into its running instance, handing
    /// back any it replaces (or this one, if the effect is gone).
    fn attach_plugin(&mut self, effect: DeviceId, block: Box<DawBlock>) {
        for player in self.players.iter_mut() {
            let slot = match player.effects.iter_mut().find(|(id, _)| *id == effect) {
                Some((_, EffectInstance::Plugin(slot))) => Some(slot),
                _ => match player.instruments.iter_mut().find(|(id, _)| *id == effect) {
                    Some((_, InstrumentInstance::Plugin(plugin))) => Some(&mut plugin.block),
                    _ => None,
                },
            };
            if let Some(slot) = slot {
                if let Some(old) = slot.replace(block) {
                    retire(&mut self.retired, old);
                }
                return;
            }
        }
        retire(&mut self.retired, block);
    }

    fn detach_plugin(&mut self, effect: DeviceId) {
        for player in self.players.iter_mut() {
            if let Some((_, EffectInstance::Plugin(slot))) =
                player.effects.iter_mut().find(|(id, _)| *id == effect)
                && let Some(old) = slot.take()
            {
                retire(&mut self.retired, old);
            }
            if let Some((_, InstrumentInstance::Plugin(plugin))) =
                player.instruments.iter_mut().find(|(id, _)| *id == effect)
                && let Some(old) = plugin.block.take()
            {
                retire(&mut self.retired, old);
            }
        }
    }

    /// Gives every node in the state a player: its own if it had one, else
    /// a free one, started fresh. Players whose nodes are gone are freed.
    /// Allocation-free.
    fn assign_players(&mut self) {
        debug_assert!(self.state.nodes.len() <= MAX_NODES);
        let mut taken = [false; MAX_NODES];
        self.slots.clear();
        for node in &self.state.nodes {
            let own = self
                .players
                .iter_mut()
                .position(|p| p.node == Some(node.id));
            if let Some(index) = own {
                taken[index] = true;
            }
            self.slots.push(own.unwrap_or(NO_PLAYER));
        }
        for (index, player) in self.players.iter_mut().enumerate() {
            if !taken[index] && player.node.is_some() {
                player.free(&self.status.meters[index], &mut self.retired);
                if let Some(clips) = player.clips.take() {
                    self.spare_clips.push(clips);
                }
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
                self.players
                    .get_mut(free)
                    .start(id, &self.status.meters[free], &mut self.retired);
            }
        }
        let beat = self.transport.position();
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            let player = self.players.get_mut(index);
            player.sync_effects(&node.effects, self.sample_rate, &mut self.retired);
            player.sync_instruments(&node.instruments, &mut self.retired);
            // Audio nodes with clips borrow a clip player; without clips
            // they give it back.
            if node.clips.is_empty() {
                if let Some(mut clips) = player.clips.take() {
                    clips.reset();
                    self.spare_clips.push(clips);
                }
            } else if player.clips.is_none() {
                player.clips = self.spare_clips.pop();
            }
            if player.fresh {
                // Start at the node's gains rather than ramping up from
                // silence.
                player.update_params(node, beat);
                player.settle_gains();
                player.fresh = false;
            }
        }
        self.plan_levels();
    }

    /// Groups the nodes into levels that can render at the same time (a
    /// node's height: 0 for a leaf, one more than its tallest child), and
    /// lists each node's children in render order. Allocation-free: every
    /// list has room for [`MAX_NODES`].
    fn plan_levels(&mut self) {
        let nodes = &self.state.nodes;
        let count = nodes.len();
        self.heights.clear();
        self.heights.resize(count, 0);
        // Children come before their parent, so a parent's height is final
        // by the time it is reached.
        for (index, node) in nodes.iter().enumerate() {
            if let Some(parent) = node.parent {
                self.heights[parent] = self.heights[parent].max(self.heights[index] + 1);
            }
        }
        self.child_list.clear();
        self.child_ranges.clear();
        for parent in 0..count {
            let from = self.child_list.len();
            for (child, node) in nodes[..parent].iter().enumerate() {
                if node.parent == Some(parent) {
                    self.child_list.push(child);
                }
            }
            self.child_ranges.push((from, self.child_list.len() - from));
        }
        self.order.clear();
        self.level_ends.clear();
        let tallest = self.heights.iter().copied().max().unwrap_or(0);
        for height in 0..=tallest {
            for (index, &h) in self.heights.iter().enumerate() {
                if h == height {
                    self.order.push(index);
                }
            }
            self.level_ends.push(self.order.len());
        }
    }

    /// Renders `frames` of the whole tree into `left` and `right`.
    ///
    /// Each node renders the whole chunk on its own: its notes start and
    /// end on their exact frames and its parameters update on the control
    /// grid, following its own copy of the transport. Nodes of a level
    /// render in parallel; a folder starts from its children's sum, added
    /// in render order, so the result is the same on any number of threads.
    #[allow(
        unsafe_code,
        reason = "parallel tasks share the players; see the SAFETY note"
    )]
    fn render(&mut self, frames: usize, deadline: Option<Instant>) {
        let start = self.transport.clone();
        let plugin = PluginRun {
            deadline,
            offline: self.offline,
            tempo_bpm: self.state.tempo_bpm,
        };
        let start_frames = self.frames_processed;
        let players = &self.players;
        let nodes = &self.state.nodes;
        let slots = &self.slots;
        let (child_list, child_ranges) = (&self.child_list, &self.child_ranges);
        let mut begin = 0;
        for &end in &self.level_ends {
            let level = &self.order[begin..end];
            let task = |k: usize| {
                let slot = level[k];
                // SAFETY: in a level each task changes only the player of its
                // own node (no node is in a level twice, and each node has
                // its own player). It reads its children's players, which
                // finished in an earlier level and no task of this level
                // changes.
                let player = unsafe { players.task_mut(slots[slot]) };
                player.clear_bus(frames);
                player.render_chunk(&nodes[slot], &start, start_frames, frames, plugin);
                let (from, len) = child_ranges[slot];
                for &child in &child_list[from..from + len] {
                    let child = unsafe { players.task_ref(slots[child]) };
                    player.add_bus(child, frames);
                }
                player.finish_chunk(&nodes[slot], frames);
            };
            self.pool.run(level.len(), &task);
            begin = end;
        }
        self.advance_transport(frames as u64);
        self.frames_processed += frames as u64;
        for (slot, &index) in self.slots.iter().enumerate() {
            let player = self.players.get_mut(index);
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

    /// Moves the transport through a chunk exactly as each node's copy
    /// moved: segment by segment, jumping back at the loop end.
    fn advance_transport(&mut self, frames: u64) {
        let mut done = 0;
        while done < frames && self.transport.is_playing() {
            let (segment, jumps) = self.transport.next_segment(frames - done);
            self.transport.advance(segment);
            done += segment;
            if jumps {
                self.transport.jump_to_loop_start();
            }
        }
    }

    /// Points every node's cursor at its first note at or after now.
    fn seek(&mut self) {
        for (node, &index) in self.state.nodes.iter().zip(&self.slots) {
            self.players.get_mut(index).seek(node, &self.transport);
        }
    }

    fn release_all(&mut self) {
        for &index in &self.slots {
            self.players.get_mut(index).release_all();
        }
    }
}

/// The last guard before the speakers: anything that is not a number
/// (a broken calculation) is silence, and nothing goes past full scale.
#[inline]
pub(crate) fn safe(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Writes stereo output into interleaved `out`, through [`safe`]. Mono
/// output sums both sides at equal power; channels beyond two are silent.
fn write_output(left: &[f32], right: &[f32], out: &mut [f32], channels: usize) {
    for ((frame, &l), &r) in out.chunks_exact_mut(channels).zip(left).zip(right) {
        let (l, r) = (safe(l), safe(r));
        match frame {
            [only] => *only = safe((l + r) * std::f32::consts::FRAC_1_SQRT_2),
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
    /// Its channel to the host process, once the control side sends it.
    Plugin(Option<Box<DawBlock>>),
}

impl EffectInstance {
    fn new(kind: EffectKind, sample_rate: f32) -> Self {
        match kind {
            EffectKind::Filter => Self::Filter(FilterEffect::new(sample_rate)),
            EffectKind::Plugin => Self::Plugin(None),
        }
    }

    fn kind(&self) -> EffectKind {
        match self {
            Self::Filter(_) => EffectKind::Filter,
            Self::Plugin(_) => EffectKind::Plugin,
        }
    }

    /// Applies real-unit parameter values in the kind's parameter order.
    fn set_params(&mut self, plain: &[f64]) {
        match self {
            Self::Filter(filter) => filter.set_params(filter_params(plain)),
            Self::Plugin(_) => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self {
            // Plugins run over whole chunks (`run_plugin`).
            Self::Plugin(_) => {}
            Self::Filter(filter) => filter.process(left, right),
        }
    }
}

/// A running instrument. Holds no heap memory, so creating and dropping one
/// on the audio thread is allocation-free.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "instances are made on the audio thread, where boxing would allocate"
)]
enum InstrumentInstance {
    Synth(Synth),
    Sampler(Sampler),
    Plugin(PluginInstrument),
}

/// Notes sounding on one plugin instrument at once (more are not started).
const MAX_PLUGIN_NOTES: usize = 64;
/// Note starts and ends one plugin instrument takes in a chunk.
const MAX_CHUNK_NOTES: usize = 256;

/// A plugin instrument: its channel to the host process (once the control
/// side sends it) and the notes sounding on it, so an end, or ending them
/// all, can name their pitch.
#[derive(Debug)]
struct PluginInstrument {
    block: Option<Box<DawBlock>>,
    sounding: [Option<(u32, u8)>; MAX_PLUGIN_NOTES],
}

/// A plugin instrument's note starts and ends for the chunk being played,
/// at their frames within it.
#[derive(Debug)]
struct ChunkNotes {
    events: [NoteEvent; MAX_CHUNK_NOTES],
    count: usize,
}

impl ChunkNotes {
    fn new() -> Self {
        Self {
            events: [NoteEvent::default(); MAX_CHUNK_NOTES],
            count: 0,
        }
    }

    fn push(&mut self, event: NoteEvent) {
        if let Some(slot) = self.events.get_mut(self.count) {
            *slot = event;
            self.count += 1;
        }
    }
}

impl PluginInstrument {
    fn note_on(&mut self, note: u32, pitch: u8, velocity: u8, at: usize, notes: &mut ChunkNotes) {
        let Some(slot) = self.sounding.iter_mut().find(|s| s.is_none()) else {
            return;
        };
        *slot = Some((note, pitch));
        notes.push(NoteEvent {
            offset: at as u32,
            note: note as i32,
            pitch: u16::from(pitch),
            on: 1,
            velocity: f32::from(velocity) / 127.0,
        });
    }

    fn note_off(&mut self, note: u32, at: usize, notes: &mut ChunkNotes) {
        for slot in &mut self.sounding {
            if let Some((id, pitch)) = *slot
                && id == note
            {
                *slot = None;
                notes.push(NoteEvent {
                    offset: at as u32,
                    note: id as i32,
                    pitch: u16::from(pitch),
                    on: 0,
                    velocity: 0.0,
                });
            }
        }
    }

    fn release_all(&mut self, at: usize, notes: &mut ChunkNotes) {
        for slot in &mut self.sounding {
            if let Some((id, pitch)) = slot.take() {
                notes.push(NoteEvent {
                    offset: at as u32,
                    note: id as i32,
                    pitch: u16::from(pitch),
                    on: 0,
                    velocity: 0.0,
                });
            }
        }
    }
}

impl InstrumentInstance {
    fn new(kind: InstrumentKind, sample_rate: f32) -> Self {
        match kind {
            InstrumentKind::Synth => Self::Synth(Synth::new(sample_rate)),
            InstrumentKind::SingleSampler => Self::Sampler(Sampler::new(sample_rate)),
            InstrumentKind::Plugin => Self::Plugin(PluginInstrument {
                block: None,
                sounding: [None; MAX_PLUGIN_NOTES],
            }),
        }
    }

    fn kind(&self) -> InstrumentKind {
        match self {
            Self::Synth(_) => InstrumentKind::Synth,
            Self::Sampler(_) => InstrumentKind::SingleSampler,
            Self::Plugin(_) => InstrumentKind::Plugin,
        }
    }

    fn take_block(&mut self) -> Option<Box<DawBlock>> {
        match self {
            Self::Plugin(plugin) => plugin.block.take(),
            Self::Synth(_) | Self::Sampler(_) => None,
        }
    }

    /// Applies real-unit parameter values in the kind's parameter order.
    fn set_params(&mut self, plain: &[f64]) {
        match self {
            Self::Synth(synth) => synth.set_params(synth_params(plain)),
            Self::Sampler(sampler) => sampler.set_level(sampler_level(plain)),
            // A plugin takes its parameters per chunk (`play_plugins`).
            Self::Plugin(_) => {}
        }
    }

    /// Starts a note `at` a frame of the chunk (built-in instruments start
    /// it now, at the current span; a plugin hears it at that frame).
    fn note_on(&mut self, note: u32, pitch: u8, velocity: u8, at: usize, notes: &mut ChunkNotes) {
        match self {
            Self::Synth(synth) => synth.note_on(note, pitch, velocity),
            Self::Sampler(sampler) => sampler.note_on(note, pitch, velocity),
            Self::Plugin(plugin) => plugin.note_on(note, pitch, velocity, at, notes),
        }
    }

    fn note_off(&mut self, note: u32, at: usize, notes: &mut ChunkNotes) {
        match self {
            Self::Synth(synth) => synth.note_off(note),
            Self::Sampler(sampler) => sampler.note_off(note),
            Self::Plugin(plugin) => plugin.note_off(note, at, notes),
        }
    }

    fn release_all(&mut self, at: usize, notes: &mut ChunkNotes) {
        match self {
            Self::Synth(synth) => synth.release_all(),
            Self::Sampler(sampler) => sampler.release_all(),
            Self::Plugin(plugin) => plugin.release_all(at, notes),
        }
    }

    /// Renders into `left` and `right`, overwriting them. The synth is a
    /// mono source, heard in both sides.
    fn render(&mut self, sample: Option<&SampleData>, left: &mut [f32], right: &mut [f32]) {
        match self {
            Self::Synth(synth) => {
                synth.render(left);
                right.copy_from_slice(left);
            }
            Self::Sampler(sampler) => sampler.render(sample.map(SampleData::view), left, right),
            // Plays over the whole chunk (`play_plugins`).
            Self::Plugin(_) => {
                left.fill(0.0);
                right.fill(0.0);
            }
        }
    }
}

/// The most control points one chunk can hold: one per control period,
/// plus the chunk's start.
const MAX_CONTROL_POINTS: usize = CHUNK_FRAMES / CONTROL_FRAMES as usize + 1;

/// Where in a chunk parameters update, and the song position there.
#[derive(Debug, Clone, Copy, Default)]
struct ControlPoint {
    offset: usize,
    beat: f64,
}

impl EffectInstance {
    fn take_block(&mut self) -> Option<Box<DawBlock>> {
        match self {
            Self::Plugin(block) => block.take(),
            Self::Filter(_) => None,
        }
    }
}

/// A plugin effect over a whole chunk, in place. Silent when it has no
/// channel yet, misses its deadline or its process is gone.
fn run_plugin(
    block: Option<&mut DawBlock>,
    left: &mut [f32],
    right: &mut [f32],
    params: &[ParamChange],
    context: PluginContext,
    deadline: Option<Instant>,
) {
    let done = block.is_some_and(|block| {
        block.process_in_place(&mut [&mut *left, &mut *right], params, context, deadline)
            == Outcome::Done
    });
    if !done {
        left.fill(0.0);
        right.fill(0.0);
    }
}

/// A plugin's parameter values for a chunk, at its control points: each
/// parameter's first point, then a point wherever its value changed. A
/// parameter at rest costs one change per chunk, so the plugin always
/// follows the song. Returns how many were written; changes past `out`'s
/// room are dropped.
fn plugin_changes(
    params: &[(u32, Automation)],
    controls: &[ControlPoint],
    behind: f64,
    out: &mut [ParamChange],
) -> usize {
    let mut count = 0;
    for (id, automation) in params {
        let mut last = None;
        for point in controls {
            let value = automation.value_at(point.beat - behind);
            if last == Some(value) {
                continue;
            }
            last = Some(value);
            let Some(slot) = out.get_mut(count) else {
                return count;
            };
            *slot = ParamChange {
                id: *id,
                offset: point.offset as u32,
                value,
            };
            count += 1;
        }
    }
    count
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
    sample_rate: f32,
    /// The running instruments, in the order of the node's instruments.
    /// Capacity is reserved up front so rebuilding never allocates.
    instruments: Vec<(DeviceId, InstrumentInstance)>,
    spare_instruments: Vec<(DeviceId, InstrumentInstance)>,
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
    /// Plays its audio clips, while it has some (lent by the processor).
    clips: Option<Box<ClipPlayer>>,
    /// Where an instrument renders before joining the bus. Each player has
    /// its own, so players render on any thread.
    scratch_left: Box<[f32; CHUNK_FRAMES]>,
    scratch_right: Box<[f32; CHUNK_FRAMES]>,
    /// This chunk's control points, so the effect chain and the channel
    /// can run over the whole chunk after the instruments (a plugin needs
    /// whole blocks, not control periods).
    controls: [ControlPoint; MAX_CONTROL_POINTS],
    control_count: usize,
    /// A plugin effect's parameter changes for the chunk.
    plugin_changes: Box<[ParamChange; MAX_PARAM_CHANGES]>,
    /// Each plugin instrument's notes for the chunk, by the instrument's
    /// place in `instruments`.
    plugin_notes: Box<[ChunkNotes; MAX_INSTRUMENTS]>,
    /// The frame within the chunk being rendered, for notes sent to
    /// plugins.
    at: usize,
    /// Its delay lines, once it needs some (sent by the control side).
    delays: Option<Box<Delays>>,
    /// Which plugin effects (by place) were last run bypassed.
    bypassed: [bool; MAX_EFFECTS],
    /// The chunk being rendered, for its second half (`finish_chunk`).
    chunk: ChunkRun,
}

/// What the second half of a node's chunk needs from the first.
#[derive(Debug, Clone, Copy, Default)]
struct ChunkRun {
    context: PluginContext,
    deadline: Option<Instant>,
    beats_per_frame: f64,
}

impl NodePlayer {
    fn new(sample_rate: f32) -> Self {
        Self {
            node: None,
            fresh: false,
            sample_rate,
            instruments: Vec::with_capacity(MAX_INSTRUMENTS),
            spare_instruments: Vec::with_capacity(MAX_INSTRUMENTS),
            effects: Vec::with_capacity(MAX_EFFECTS),
            spare_effects: Vec::with_capacity(MAX_EFFECTS),
            gain_left: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            gain_right: Smoothed::new(0.0, CONTROL_FRAMES as u32),
            cursor: 0,
            pending_ends: [None; MAX_PENDING_ENDS],
            next_note_id: 0,
            clips: None,
            bus_left: Box::new([0.0; CHUNK_FRAMES]),
            bus_right: Box::new([0.0; CHUNK_FRAMES]),
            scratch_left: Box::new([0.0; CHUNK_FRAMES]),
            scratch_right: Box::new([0.0; CHUNK_FRAMES]),
            controls: [ControlPoint::default(); MAX_CONTROL_POINTS],
            control_count: 0,
            plugin_changes: Box::new([ParamChange::default(); MAX_PARAM_CHANGES]),
            plugin_notes: Box::new(std::array::from_fn(|_| ChunkNotes::new())),
            at: 0,
            delays: None,
            bypassed: [false; MAX_EFFECTS],
            chunk: ChunkRun::default(),
        }
    }

    /// Takes on `node`, starting from silence. Allocation-free.
    fn start(&mut self, node: NodeId, meter: &PlayerMeter, retired: &mut Retired) {
        self.node = Some(node);
        self.fresh = true;
        self.retire_instruments(retired);
        self.retire_effects(retired);
        if let Some(delays) = self.delays.take() {
            retire_delays(retired, delays);
        }
        self.cursor = 0;
        self.pending_ends = [None; MAX_PENDING_ENDS];
        meter.node.store(node.0 + 1, Ordering::Relaxed);
        meter.peaks.take();
    }

    /// Goes back to the pool.
    fn free(&mut self, meter: &PlayerMeter, retired: &mut Retired) {
        self.node = None;
        self.release_all();
        self.retire_instruments(retired);
        self.retire_effects(retired);
        if let Some(delays) = self.delays.take() {
            retire_delays(retired, delays);
        }
        meter.node.store(0, Ordering::Relaxed);
        meter.peaks.take();
    }

    fn clear_bus(&mut self, frames: usize) {
        self.bus_left[..frames].fill(0.0);
        self.bus_right[..frames].fill(0.0);
    }

    /// Adds a child's finished sound into this bus.
    fn add_bus(&mut self, child: &NodePlayer, frames: usize) {
        for (to, from) in self.bus_left[..frames]
            .iter_mut()
            .zip(&child.bus_left[..frames])
        {
            *to += *from;
        }
        for (to, from) in self.bus_right[..frames]
            .iter_mut()
            .zip(&child.bus_right[..frames])
        {
            *to += *from;
        }
    }

    /// Renders the node through a chunk of `frames`, from the transport as
    /// it stood at the chunk's start: notes on their exact frames,
    /// parameters on the control grid (counted from `start_frames`), the
    /// loop jumping back at its end. The bus already holds the children.
    /// Clips and instruments render span by span; the effect chain, then
    /// volume and pan, then run over the whole chunk, each taking its
    /// parameters at the same control points.
    fn render_chunk(
        &mut self,
        node: &NodeRender,
        start: &Transport,
        start_frames: u64,
        frames: usize,
        plugin: PluginRun,
    ) {
        let mut transport = start.clone();
        let chunk = frames;
        let frames = frames as u64;
        let mut processed = start_frames;
        self.control_count = 0;
        let mut done = 0;
        while done < frames {
            let playing = transport.is_playing();
            let (segment, jumps) = if playing {
                transport.next_segment(frames - done)
            } else {
                (frames - done, false)
            };
            let segment_end = done + segment;
            while done < segment_end {
                self.at = done as usize;
                let into_control = processed % CONTROL_FRAMES;
                if into_control == 0 {
                    let beat = transport.position();
                    self.update_instrument_params(node, beat);
                    if self.control_count < MAX_CONTROL_POINTS {
                        self.controls[self.control_count] = ControlPoint {
                            offset: done as usize,
                            beat,
                        };
                        self.control_count += 1;
                    }
                }
                let mut next = (CONTROL_FRAMES - into_control).min(segment_end - done);
                if playing {
                    self.fire_due_events(node, &transport);
                    let to_event = self.frames_to_next_event(node, &transport).max(1) as u64;
                    let to_edge = frames_to_clip_edge(&node.clips, &transport).max(1) as u64;
                    next = next.min(to_event).min(to_edge);
                }
                self.render_span(node, done as usize..(done + next) as usize, &transport);
                if playing {
                    transport.advance(next);
                }
                processed += next;
                done += next;
            }
            if jumps {
                transport.jump_to_loop_start();
                self.at = done as usize;
                self.release_all();
                self.seek(node, &transport);
            }
        }
        // Notes from outside the chunk (previews) come at its start.
        self.at = 0;
        let context = PluginContext {
            tempo_bpm: plugin.tempo_bpm,
            position_beats: start.position(),
            playing: start.is_playing(),
            offline: plugin.offline,
        };
        self.chunk = ChunkRun {
            context,
            deadline: plugin.deadline,
            beats_per_frame: start.beats_per_frame(),
        };
        // The built-in sources wait for the node's slowest.
        if let Some(delays) = &mut self.delays {
            delays.sources.process(
                &mut self.bus_left[..chunk],
                &mut self.bus_right[..chunk],
                node.comp.sources as usize,
            );
        }
        self.play_plugins(node, chunk, context, plugin.deadline);
    }

    /// The chunk's second half, once the children's sound is in the bus:
    /// the effect chain, volume and pan, then the delay that lines the node
    /// up at its parent.
    fn finish_chunk(&mut self, node: &NodeRender, frames: usize) {
        let ChunkRun {
            context, deadline, ..
        } = self.chunk;
        self.process_effects(node, frames, context, deadline);
        self.apply_channel(node, frames);
        if let Some(delays) = &mut self.delays {
            delays.output.process(
                &mut self.bus_left[..frames],
                &mut self.bus_right[..frames],
                node.comp.output as usize,
            );
        }
    }

    /// A span with steady parameters: its audio clips and every instrument
    /// into the bus (their sounds add up).
    fn render_span(
        &mut self,
        node: &NodeRender,
        span: std::ops::Range<usize>,
        transport: &Transport,
    ) {
        if let Some(clips) = &mut self.clips {
            clips.render(
                node.id,
                &node.clips,
                transport.is_playing(),
                transport.position(),
                transport.beats_per_frame(),
                &mut self.bus_left[span.clone()],
                &mut self.bus_right[span.clone()],
            );
        }
        for ((_, instrument), state) in self.instruments.iter_mut().zip(&node.instruments) {
            let left = &mut self.scratch_left[span.clone()];
            let right = &mut self.scratch_right[span.clone()];
            instrument.render(state.sample.as_deref(), left, right);
            if node.instrument_on && state.enabled {
                for (to, from) in self.bus_left[span.clone()].iter_mut().zip(left.iter()) {
                    *to += *from;
                }
                for (to, from) in self.bus_right[span.clone()].iter_mut().zip(right.iter()) {
                    *to += *from;
                }
            }
        }
    }

    fn settle_gains(&mut self) {
        self.gain_left = Smoothed::new(self.gain_left.target(), CONTROL_FRAMES as u32);
        self.gain_right = Smoothed::new(self.gain_right.target(), CONTROL_FRAMES as u32);
    }

    /// Rebuilds the running chain to match `wanted`, keeping existing effects
    /// (and their filter memory) by id. Allocation-free.
    /// Drops every instrument, handing plugin channels back.
    fn retire_instruments(&mut self, retired: &mut Retired) {
        for (_, mut instrument) in self.instruments.drain(..) {
            if let Some(block) = instrument.take_block() {
                retire(retired, block);
            }
        }
        for notes in self.plugin_notes.iter_mut() {
            notes.count = 0;
        }
    }

    /// Empties the chain, handing plugin channels back.
    fn retire_effects(&mut self, retired: &mut Retired) {
        for (_, mut effect) in self.effects.drain(..) {
            if let Some(block) = effect.take_block() {
                retire(retired, block);
            }
        }
    }

    fn sync_effects(&mut self, wanted: &[EffectState], sample_rate: f32, retired: &mut Retired) {
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
        self.retire_effects(retired);
        std::mem::swap(&mut self.effects, &mut self.spare_effects);
    }

    /// Rebuilds the running instruments to match `wanted`, keeping existing
    /// ones (and their sounding voices) by id and kind. Allocation-free: an
    /// instrument holds no heap memory.
    fn sync_instruments(&mut self, wanted: &[InstrumentState], retired: &mut Retired) {
        debug_assert!(wanted.len() <= MAX_INSTRUMENTS);
        self.spare_instruments.clear();
        for state in wanted {
            let existing = self
                .instruments
                .iter()
                .position(|(id, i)| *id == state.id && i.kind() == state.kind);
            let instance = match existing {
                Some(index) => self.instruments.swap_remove(index).1,
                None => InstrumentInstance::new(state.kind, self.sample_rate),
            };
            self.spare_instruments.push((state.id, instance));
        }
        // Whatever is left was removed.
        for (_, mut instrument) in self.instruments.drain(..) {
            if let Some(block) = instrument.take_block() {
                retire(retired, block);
            }
        }
        std::mem::swap(&mut self.instruments, &mut self.spare_instruments);
        // Notes queued for the old order no longer line up.
        for notes in self.plugin_notes.iter_mut() {
            notes.count = 0;
        }
    }

    /// Evaluates every parameter's layers at `beat` and applies the results.
    fn update_params(&mut self, node: &NodeRender, beat: f64) {
        self.update_instrument_params(node, beat);
        let mut plain = [0.0; MAX_DEVICE_PARAMS];
        for ((_, instance), effect) in self.effects.iter_mut().zip(&node.effects) {
            evaluate(&effect.params, effect_params(effect.kind), beat, &mut plain);
            instance.set_params(&plain);
        }
        self.update_channel(node, beat);
    }

    fn update_instrument_params(&mut self, node: &NodeRender, beat: f64) {
        let mut plain = [0.0; MAX_DEVICE_PARAMS];
        for ((_, instance), instrument) in self.instruments.iter_mut().zip(&node.instruments) {
            evaluate(
                &instrument.params,
                instrument_params(instrument.kind),
                beat,
                &mut plain,
            );
            instance.set_params(&plain);
        }
    }

    /// Volume and pan targets from the channel's layers at `beat`.
    fn update_channel(&mut self, node: &NodeRender, beat: f64) {
        let mut plain = [0.0; MAX_DEVICE_PARAMS];
        evaluate(&node.channel, &CHANNEL_PARAMS, beat, &mut plain);
        let (gain, pan) = channel_settings(&plain);
        let gain = if node.output_on { gain } else { 0.0 };
        // An instrument is a mono source: pan it with equal power. Anything
        // else is a stereo bus: balance it, so a centred folder passes its
        // sound unchanged.
        let (left, right) = if !node.instruments.is_empty() {
            equal_power(pan)
        } else {
            balance(pan)
        };
        self.gain_left.set_target(gain * left);
        self.gain_right.set_target(gain * right);
    }

    /// The effect chain over the chunk's bus, one effect after another; each
    /// takes its parameters at the chunk's control points. Effects switched
    /// off still follow their parameters, so they come back in step.
    fn process_effects(
        &mut self,
        node: &NodeRender,
        frames: usize,
        context: PluginContext,
        deadline: Option<Instant>,
    ) {
        let controls = &self.controls[..self.control_count];
        let (left, right) = (&mut self.bus_left[..frames], &mut self.bus_right[..frames]);
        let mut plain = [0.0; MAX_DEVICE_PARAMS];
        let beats_per_frame = self.chunk.beats_per_frame;
        for (index, (((_, effect), state), &late)) in self
            .effects
            .iter_mut()
            .zip(&node.effects)
            .zip(node.comp.effects.iter().chain(std::iter::repeat(&0)))
            .enumerate()
        {
            // Its layers act on the sound reaching it, `late` frames behind.
            let behind = f64::from(late) * beats_per_frame;
            if let EffectInstance::Plugin(block) = effect {
                let was_bypassed = &mut self.bypassed[index.min(MAX_EFFECTS - 1)];
                match (state.enabled, state.bypass_param) {
                    // Switched off without a bypass of its own: not run, and
                    // the sound waits as long as it would have taken.
                    (false, None) => {
                        if let Some(delays) = &mut self.delays {
                            let wait = node.comp.bypass.get(index).copied().unwrap_or(0);
                            delays.effects[index.min(MAX_EFFECTS - 1)].process(
                                left,
                                right,
                                wait as usize,
                            );
                        }
                    }
                    // Run, its own bypass switched to match (on while off;
                    // back off once, when switched on again, so its own
                    // bypass button stays the user's otherwise).
                    (enabled, bypass) => {
                        let mut count = plugin_changes(
                            &state.plugin_params,
                            controls,
                            behind,
                            &mut *self.plugin_changes,
                        );
                        let set = match (enabled, *was_bypassed) {
                            (false, _) => Some(1.0),
                            (true, true) => Some(0.0),
                            (true, false) => None,
                        };
                        if let (Some(id), Some(value)) = (bypass, set)
                            && let Some(slot) = self.plugin_changes.get_mut(count)
                        {
                            *slot = ParamChange {
                                id,
                                offset: 0,
                                value,
                            };
                            count += 1;
                        }
                        *was_bypassed = !enabled;
                        let changes = &self.plugin_changes[..count];
                        run_plugin(
                            block.as_deref_mut(),
                            left,
                            right,
                            changes,
                            context,
                            deadline,
                        );
                    }
                }
                continue;
            }
            let mut from = 0;
            for point in controls {
                if state.enabled {
                    effect.process(
                        &mut left[from..point.offset],
                        &mut right[from..point.offset],
                    );
                }
                evaluate(
                    &state.params,
                    effect_params(state.kind),
                    point.beat - behind,
                    &mut plain,
                );
                effect.set_params(&plain);
                from = point.offset;
            }
            if state.enabled {
                effect.process(&mut left[from..], &mut right[from..]);
            }
        }
    }

    /// Volume and pan over the chunk's bus, their targets moving at the
    /// control points.
    fn apply_channel(&mut self, node: &NodeRender, frames: usize) {
        let mut from = 0;
        for index in 0..=self.control_count {
            let point = self.controls[..self.control_count].get(index).copied();
            let to = point.map_or(frames, |p| p.offset);
            for i in from..to {
                self.bus_left[i] *= self.gain_left.next_value();
                self.bus_right[i] *= self.gain_right.next_value();
            }
            if let Some(point) = point {
                let behind = f64::from(node.comp.channel) * self.chunk.beats_per_frame;
                self.update_channel(node, point.beat - behind);
            }
            from = to;
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
            self.start_note(node, note.end_beats, note.pitch, note.velocity);
        }
        // Notes shorter than a frame end on the frame they start.
        self.end_due_notes(transport);
    }

    fn end_due_notes(&mut self, transport: &Transport) {
        for slot in &mut self.pending_ends {
            if let Some(pending) = *slot
                && transport.frames_until(pending.end_beats) <= 0
            {
                for ((_, instrument), notes) in self
                    .instruments
                    .iter_mut()
                    .zip(self.plugin_notes.iter_mut())
                {
                    instrument.note_off(pending.note_id, self.at, notes);
                }
                *slot = None;
            }
        }
    }

    fn start_note(&mut self, node: &NodeRender, end_beats: f64, pitch: u8, velocity: u8) {
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
                for ((_, instrument), notes) in self
                    .instruments
                    .iter_mut()
                    .zip(self.plugin_notes.iter_mut())
                {
                    instrument.note_off(soonest.note_id, self.at, notes);
                }
                slot
            }
        };
        self.pending_ends[slot] = Some(PendingEnd { note_id, end_beats });
        // Each instrument plays only its own notes.
        for (((_, instrument), state), notes) in self
            .instruments
            .iter_mut()
            .zip(&node.instruments)
            .zip(self.plugin_notes.iter_mut())
        {
            if state.plays.contains(pitch) {
                instrument.note_on(note_id, pitch, velocity, self.at, notes);
            }
        }
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
        for ((_, instrument), notes) in self
            .instruments
            .iter_mut()
            .zip(self.plugin_notes.iter_mut())
        {
            instrument.release_all(self.at, notes);
        }
        self.pending_ends = [None; MAX_PENDING_ENDS];
    }

    /// Plays the plugin instruments over the chunk, after the built-in
    /// ones: each takes its notes at their frames and its parameters at the
    /// chunk's control points, and joins the bus. Silent while it has no
    /// channel, is late or its process is gone.
    fn play_plugins(
        &mut self,
        node: &NodeRender,
        frames: usize,
        context: PluginContext,
        deadline: Option<Instant>,
    ) {
        let controls = &self.controls[..self.control_count];
        for (index, (((_, instrument), state), notes)) in self
            .instruments
            .iter_mut()
            .zip(&node.instruments)
            .zip(self.plugin_notes.iter_mut())
            .enumerate()
        {
            let InstrumentInstance::Plugin(plugin) = instrument else {
                continue;
            };
            let count = plugin_changes(
                &state.plugin_params,
                controls,
                0.0,
                &mut *self.plugin_changes,
            );
            let left = &mut self.scratch_left[..frames];
            let right = &mut self.scratch_right[..frames];
            let done = plugin.block.as_deref_mut().is_some_and(|block| {
                block.process_with_notes(
                    &[],
                    &self.plugin_changes[..count],
                    &notes.events[..notes.count],
                    context,
                    &mut [&mut *left, &mut *right],
                    deadline,
                ) == Outcome::Done
            });
            notes.count = 0;
            if !done {
                left.fill(0.0);
                right.fill(0.0);
            }
            // It waits for the node's slowest source.
            if let Some(delays) = &mut self.delays {
                let wait = node.comp.instruments.get(index).copied().unwrap_or(0);
                delays.instruments[index].process(left, right, wait as usize);
            }
            if done && node.instrument_on && state.enabled {
                for (to, from) in self.bus_left[..frames].iter_mut().zip(left.iter()) {
                    *to += *from;
                }
                for (to, from) in self.bus_right[..frames].iter_mut().zip(right.iter()) {
                    *to += *from;
                }
            }
        }
    }
}

/// Frames until the next start or end of a clip, among clips sorted by
/// start (which never overlap).
fn frames_to_clip_edge(clips: &[ClipRender], transport: &Transport) -> i64 {
    let beat = transport.position();
    let from = clips.partition_point(|c| c.end_beats <= beat);
    clips[from..]
        .iter()
        .flat_map(|c| [c.start_beats, c.end_beats])
        .map(|edge| transport.frames_until(edge))
        .find(|&frames| frames > 0)
        .unwrap_or(i64::MAX)
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
            1,
            true,
            state(100.0),
            AudioQueues {
                from_control,
                garbage: garbage_tx,
                retired: rtrb::RingBuffer::new(1).0,
            },
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
            1,
            true,
            state(120.0),
            AudioQueues {
                from_control,
                garbage: garbage_tx,
                retired: rtrb::RingBuffer::new(1).0,
            },
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
        assert_eq!(processor.players.get_mut(folder).node, Some(NodeId(7)));
    }

    #[test]
    fn the_output_never_goes_past_full_scale_or_carries_broken_numbers() {
        assert_eq!(safe(0.5), 0.5);
        assert_eq!(safe(3.0), 1.0);
        assert_eq!(safe(-7.0), -1.0);
        assert_eq!(safe(f32::NAN), 0.0);
        assert_eq!(safe(f32::INFINITY), 0.0);
        let mut out = [9.0; 4];
        write_output(&[2.0, f32::NAN], &[-2.0, 0.25], &mut out, 2);
        assert_eq!(out, [1.0, -1.0, 0.0, 0.25]);
    }

    #[test]
    fn plugin_parameters_send_their_first_value_then_only_changes() {
        use daw_protocol::{BlendMode, Block, BlockId, Layer, LayerId, ParamState, Shape, Ticks};
        let resting = ParamState {
            base: 0.25,
            layers: Vec::new(),
        };
        let mut layer = Layer::new(BlendMode::Add);
        // Over beats 1..2 the layer ramps from the centre to the top.
        layer.blocks.push((
            BlockId(1),
            Block::new(
                Ticks::from_beats(1),
                Ticks::from_beats(1),
                Shape::ramp(0.5, 1.0),
            ),
        ));
        let moving = ParamState {
            base: 0.0,
            layers: vec![(LayerId(1), layer)],
        };
        let signals = daw_automation::Signals::default();
        let params = [
            (3, Automation::compile_with(&resting, &signals)),
            (9, Automation::compile_with(&moving, &signals)),
        ];
        let controls: Vec<ControlPoint> = [0.0, 0.5, 1.25, 1.5]
            .iter()
            .enumerate()
            .map(|(i, &beat)| ControlPoint {
                offset: i * 32,
                beat,
            })
            .collect();
        let mut out = [ParamChange::default(); 8];
        let count = plugin_changes(&params, &controls, 0.0, &mut out);
        let sent: Vec<(u32, u32)> = out[..count].iter().map(|c| (c.id, c.offset)).collect();
        // The resting one once; the moving one at its start, then where it
        // moves (beats 0 and 0.5 are both before the block).
        assert_eq!(sent, [(3, 0), (9, 0), (9, 64), (9, 96)]);
        assert_eq!(out[0].value, 0.25);
        assert!(out[3].value > out[2].value);
        // Out of room, the rest is dropped.
        assert_eq!(plugin_changes(&params, &controls, 0.0, &mut out[..2]), 2);
    }
}
