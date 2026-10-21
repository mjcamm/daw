//! The per-instance audio channel: one shared memory block and two events.
//!
//! Hand-off for each block: the DAW writes inputs, parameter changes and
//! the context, publishes a new sequence number (`issued`, release) and sets
//! `go`. The host wakes, processes, publishes the same number in `finished`
//! (release) and sets `done`. Each side touches the inputs, parameters and
//! outputs only while it holds the block (DAW: `finished == issued`; host:
//! `issued > finished`), so the regions are never shared at once.
//!
//! The DAW waits for `done` until an absolute deadline. If the host is late
//! the block is dropped (the caller plays silence) and the next blocks are
//! skipped (`Busy`) until the host catches up, so the DAW never writes into
//! a block the host is still reading.

#![allow(
    unsafe_code,
    reason = "typed views over shared memory, exclusive by the protocol above"
)]

use crate::sys::{Event, SharedMemory, Timer};
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAGIC: u32 = u32::from_le_bytes(*b"DAWP");
/// Bump when the layout changes; both sides check it.
pub const LAYOUT_VERSION: u32 = 1;
/// Parameter changes one block can carry.
pub const MAX_PARAM_CHANGES: usize = 512;
/// Audio channels per direction.
pub const MAX_CHANNELS: usize = 16;

/// The block's size and channels, fixed while an instance is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub max_frames: u32,
    pub inputs: u32,
    pub outputs: u32,
}

/// Where an instance's block lives: the names the host opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Names {
    pub memory: String,
    pub go: String,
    pub done: String,
}

impl Names {
    /// Fresh names, unique in this DAW process: every activation gets its
    /// own, so a new channel never opens an old one still mapped.
    pub fn unique() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let base = format!("Local\\daw-plugins-{}-{n}", std::process::id());
        Self {
            memory: format!("{base}-memory"),
            go: format!("{base}-go"),
            done: format!("{base}-done"),
        }
    }
}

/// One parameter value at a frame within the block.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParamChange {
    pub id: u32,
    pub offset: u32,
    pub value: f64,
}

/// Song time and transport for the block.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Context {
    pub tempo_bpm: f64,
    /// Song position of the block's first frame.
    pub position_beats: f64,
    pub playing: bool,
    /// Offline render: the plugin may take as long as it needs.
    pub offline: bool,
}

#[repr(C, align(64))]
struct Header {
    magic: AtomicU32,
    version: AtomicU32,
    max_frames: AtomicU32,
    inputs: AtomicU32,
    outputs: AtomicU32,
    shutdown: AtomicU32,
    issued: AtomicU64,
    finished: AtomicU64,
    frames: AtomicU32,
    param_count: AtomicU32,
    flags: AtomicU32,
    tempo_bpm: AtomicU64,
    position_beats: AtomicU64,
}

const PLAYING: u32 = 1;
const OFFLINE: u32 = 2;

impl Layout {
    fn params_offset() -> usize {
        size_of::<Header>().next_multiple_of(64)
    }

    fn audio_offset() -> usize {
        (Self::params_offset() + MAX_PARAM_CHANGES * size_of::<ParamChange>()).next_multiple_of(64)
    }

    fn channel_bytes(self) -> usize {
        (self.max_frames as usize * size_of::<f32>()).next_multiple_of(64)
    }

    fn size(self) -> usize {
        Self::audio_offset() + (self.inputs + self.outputs) as usize * self.channel_bytes()
    }

    fn check(self) -> io::Result<()> {
        if self.max_frames == 0
            || self.inputs as usize > MAX_CHANNELS
            || self.outputs as usize > MAX_CHANNELS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bad layout {self:?}"),
            ));
        }
        Ok(())
    }
}

/// The mapped block with typed access.
#[derive(Debug)]
struct Shared {
    memory: SharedMemory,
    layout: Layout,
}

impl Shared {
    fn header(&self) -> &Header {
        // Aligned (mapping views are page aligned) and only atomics.
        unsafe { &*self.memory.as_ptr().cast::<Header>() }
    }

    fn params(&self) -> *mut ParamChange {
        unsafe { self.memory.as_ptr().add(Layout::params_offset()).cast() }
    }

    /// Input channels first, then outputs.
    fn channel(&self, index: usize) -> *mut f32 {
        let offset = Layout::audio_offset() + index * self.layout.channel_bytes();
        unsafe { self.memory.as_ptr().add(offset).cast() }
    }
}

/// The result of handing a block to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The outputs hold the processed block.
    Done,
    /// The deadline passed; play silence for this block.
    Late,
    /// The host is still on an earlier late block; play silence.
    Busy,
    /// The host process is gone; play silence until it is reloaded.
    Dead,
}

/// The DAW's end of an instance's channel. Moves to the audio thread;
/// [`DawBlock::process`] never allocates.
#[derive(Debug)]
pub struct DawBlock {
    shared: Shared,
    go: Event,
    done: Event,
    timer: Timer,
    issued: u64,
    alive: Arc<AtomicBool>,
    stats: Arc<BlockStats>,
    instance: u64,
}

impl DawBlock {
    /// Creates the shared memory and events for `instance`. `alive` turns
    /// false when the host process is known to be gone.
    pub fn create(
        instance: u64,
        names: &Names,
        layout: Layout,
        alive: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        layout.check()?;
        let shared = Shared {
            memory: SharedMemory::create(&names.memory, layout.size())?,
            layout,
        };
        let h = shared.header();
        h.magic.store(MAGIC, Ordering::Relaxed);
        h.version.store(LAYOUT_VERSION, Ordering::Relaxed);
        h.max_frames.store(layout.max_frames, Ordering::Relaxed);
        h.inputs.store(layout.inputs, Ordering::Relaxed);
        h.outputs.store(layout.outputs, Ordering::Relaxed);
        h.shutdown.store(0, Ordering::Relaxed);
        h.issued.store(0, Ordering::Relaxed);
        h.finished.store(0, Ordering::Release);
        Ok(Self {
            shared,
            go: Event::create(&names.go)?,
            done: Event::create(&names.done)?,
            timer: Timer::new()?,
            issued: 0,
            alive,
            stats: Arc::default(),
            instance,
        })
    }

    pub fn layout(&self) -> Layout {
        self.shared.layout
    }

    /// The host instance this channel feeds.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Counts of this instance's blocks, shared with the control side.
    pub fn stats(&self) -> Arc<BlockStats> {
        self.stats.clone()
    }

    /// Hands one block to the host and waits for it until `deadline`
    /// (`None`: no deadline, as for offline render). Missing input channels
    /// are silent; `outputs` are written only on `Done`. Parameter changes
    /// past [`MAX_PARAM_CHANGES`] are dropped.
    pub fn process(
        &mut self,
        inputs: &[&[f32]],
        params: &[ParamChange],
        context: Context,
        outputs: &mut [&mut [f32]],
        deadline: Option<Instant>,
    ) -> Outcome {
        let frames = outputs.first().map_or(0, |o| o.len());
        let outcome = self.exchange(
            frames,
            |ch| inputs.get(ch).copied(),
            params,
            context,
            deadline,
        );
        if outcome == Outcome::Done {
            self.read_outputs(outputs, frames);
        }
        outcome
    }

    /// As [`DawBlock::process`], with `audio` as both the inputs and the
    /// outputs (an effect on a bus). Left as it was unless `Done`.
    pub fn process_in_place(
        &mut self,
        audio: &mut [&mut [f32]],
        params: &[ParamChange],
        context: Context,
        deadline: Option<Instant>,
    ) -> Outcome {
        let frames = audio.first().map_or(0, |a| a.len());
        let outcome = self.exchange(
            frames,
            |ch| audio.get(ch).map(|a| &**a),
            params,
            context,
            deadline,
        );
        if outcome == Outcome::Done {
            self.read_outputs(audio, frames);
        }
        outcome
    }

    /// Writes a block, issues it and waits for it.
    fn exchange<'a>(
        &mut self,
        frames: usize,
        input: impl Fn(usize) -> Option<&'a [f32]>,
        params: &[ParamChange],
        context: Context,
        deadline: Option<Instant>,
    ) -> Outcome {
        if !self.alive.load(Ordering::Acquire) {
            return Outcome::Dead;
        }
        let layout = self.shared.layout;
        let h = self.shared.header();
        if h.finished.load(Ordering::Acquire) != self.issued {
            self.stats.missed();
            return Outcome::Busy;
        }
        let frames = frames.min(layout.max_frames as usize);
        for ch in 0..layout.inputs as usize {
            // The host is not reading: it finished every block issued.
            let dst = unsafe { std::slice::from_raw_parts_mut(self.shared.channel(ch), frames) };
            match input(ch) {
                Some(src) if src.len() >= frames => dst.copy_from_slice(&src[..frames]),
                _ => dst.fill(0.),
            }
        }
        let count = params.len().min(MAX_PARAM_CHANGES);
        let dst = unsafe { std::slice::from_raw_parts_mut(self.shared.params(), count) };
        dst.copy_from_slice(&params[..count]);
        h.param_count.store(count as u32, Ordering::Relaxed);
        h.frames.store(frames as u32, Ordering::Relaxed);
        h.flags.store(
            (u32::from(context.playing) * PLAYING) | (u32::from(context.offline) * OFFLINE),
            Ordering::Relaxed,
        );
        h.tempo_bpm
            .store(context.tempo_bpm.to_bits(), Ordering::Relaxed);
        h.position_beats
            .store(context.position_beats.to_bits(), Ordering::Relaxed);

        self.issued += 1;
        h.issued.store(self.issued, Ordering::Release);
        self.go.set();

        loop {
            if h.finished.load(Ordering::Acquire) == self.issued {
                self.stats.done();
                return Outcome::Done;
            }
            match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        self.stats.missed();
                        return Outcome::Late;
                    }
                    self.timer.arm(deadline - now);
                    // A stale `done` from an earlier late block may wake us
                    // early: the loop checks `finished` again.
                    self.done.wait_or(&self.timer);
                }
                None => {
                    if !self.alive.load(Ordering::Acquire) {
                        return Outcome::Dead;
                    }
                    self.done.wait(Some(Duration::from_millis(50)));
                }
            }
        }
    }

    /// Copies the host's finished outputs (call only after `Done`).
    fn read_outputs(&self, outputs: &mut [&mut [f32]], frames: usize) {
        let layout = self.shared.layout;
        let frames = frames.min(layout.max_frames as usize);
        for (ch, out) in outputs.iter_mut().enumerate().take(layout.outputs as usize) {
            let src = unsafe {
                std::slice::from_raw_parts(self.shared.channel(layout.inputs as usize + ch), frames)
            };
            out[..frames].copy_from_slice(src);
        }
    }
}

/// How an instance's blocks went, read by the control side.
#[derive(Debug, Default)]
pub struct BlockStats {
    missed_in_a_row: AtomicU32,
    missed: AtomicU64,
    done: AtomicU64,
}

impl BlockStats {
    fn done(&self) {
        self.missed_in_a_row.store(0, Ordering::Relaxed);
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    fn missed(&self) {
        self.missed_in_a_row.fetch_add(1, Ordering::Relaxed);
        self.missed.fetch_add(1, Ordering::Relaxed);
    }

    /// Blocks late or skipped since the last one done.
    pub fn missed_in_a_row(&self) -> u32 {
        self.missed_in_a_row.load(Ordering::Relaxed)
    }

    pub fn missed_total(&self) -> u64 {
        self.missed.load(Ordering::Relaxed)
    }

    pub fn done_total(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
}

impl Drop for DawBlock {
    fn drop(&mut self) {
        self.shared.header().shutdown.store(1, Ordering::Release);
        self.go.set();
    }
}

/// One block as the host sees it while processing.
#[derive(Debug)]
pub struct HostBlock<'a> {
    shared: &'a Shared,
    frames: usize,
    pub context: Context,
}

impl HostBlock<'_> {
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn inputs(&self) -> usize {
        self.shared.layout.inputs as usize
    }

    pub fn outputs(&self) -> usize {
        self.shared.layout.outputs as usize
    }

    pub fn input(&self, channel: usize) -> &[f32] {
        assert!(channel < self.inputs(), "input {channel} out of range");
        unsafe { std::slice::from_raw_parts(self.shared.channel(channel), self.frames) }
    }

    pub fn output(&mut self, channel: usize) -> &mut [f32] {
        assert!(channel < self.outputs(), "output {channel} out of range");
        let index = self.inputs() + channel;
        unsafe { std::slice::from_raw_parts_mut(self.shared.channel(index), self.frames) }
    }

    pub fn params(&self) -> &[ParamChange] {
        let count = self.shared.header().param_count.load(Ordering::Relaxed) as usize;
        unsafe { std::slice::from_raw_parts(self.shared.params(), count.min(MAX_PARAM_CHANGES)) }
    }
}

/// What ended [`HostEnd::serve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    Processed,
    /// The DAW closed the channel.
    Shutdown,
}

/// The host's end of an instance's channel.
#[derive(Debug)]
pub struct HostEnd {
    shared: Shared,
    go: Event,
    done: Event,
    finished: u64,
}

impl HostEnd {
    /// A second handle on `go`, to wake [`HostEnd::serve`] from another
    /// thread.
    pub fn waker(names: &Names) -> io::Result<Event> {
        Event::open(&names.go)
    }

    pub fn open(names: &Names, layout: Layout) -> io::Result<Self> {
        layout.check()?;
        let shared = Shared {
            memory: SharedMemory::open(&names.memory, layout.size())?,
            layout,
        };
        let h = shared.header();
        let matches = h.magic.load(Ordering::Relaxed) == MAGIC
            && h.version.load(Ordering::Relaxed) == LAYOUT_VERSION
            && h.max_frames.load(Ordering::Relaxed) == layout.max_frames
            && h.inputs.load(Ordering::Relaxed) == layout.inputs
            && h.outputs.load(Ordering::Relaxed) == layout.outputs;
        if !matches {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "block layout does not match",
            ));
        }
        let finished = h.finished.load(Ordering::Acquire);
        Ok(Self {
            shared,
            go: Event::open(&names.go)?,
            done: Event::open(&names.done)?,
            finished,
        })
    }

    /// Waits for the next block and runs `process` on it, or returns
    /// `Shutdown` once the DAW closes the channel or `stop` is set (set it,
    /// then [`HostEnd::waker`]'s event, to stop promptly). Allocates nothing
    /// itself; `process` must not either (it is the plugin's audio thread).
    pub fn serve(
        &mut self,
        stop: &AtomicBool,
        mut process: impl FnMut(&mut HostBlock<'_>),
    ) -> Served {
        loop {
            let h = self.shared.header();
            if h.shutdown.load(Ordering::Acquire) != 0 || stop.load(Ordering::Acquire) {
                return Served::Shutdown;
            }
            let issued = h.issued.load(Ordering::Acquire);
            if issued == self.finished {
                // Wake now and then even without `go`, to see `shutdown`.
                self.go.wait(Some(Duration::from_secs(1)));
                continue;
            }
            let flags = h.flags.load(Ordering::Relaxed);
            let mut block = HostBlock {
                shared: &self.shared,
                frames: h.frames.load(Ordering::Relaxed) as usize,
                context: Context {
                    tempo_bpm: f64::from_bits(h.tempo_bpm.load(Ordering::Relaxed)),
                    position_beats: f64::from_bits(h.position_beats.load(Ordering::Relaxed)),
                    playing: flags & PLAYING != 0,
                    offline: flags & OFFLINE != 0,
                },
            };
            process(&mut block);
            self.finished = issued;
            h.finished.store(issued, Ordering::Release);
            self.done.set();
            return Served::Processed;
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    #![allow(
        clippy::disallowed_methods,
        reason = "test hosts sleep to stand in for slow plugins"
    )]
    #![allow(clippy::unwrap_used, reason = "tests fail on error")]

    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::thread;

    const LAYOUT: Layout = Layout {
        max_frames: 256,
        inputs: 2,
        outputs: 2,
    };

    fn names() -> Names {
        Names::unique()
    }

    /// A DAW end and a host thread running `process` on every block until
    /// the DAW end is dropped.
    fn pair(
        process: impl FnMut(&mut HostBlock<'_>) + Send + 'static,
    ) -> (DawBlock, Arc<AtomicBool>, thread::JoinHandle<()>) {
        let names = names();
        let alive = Arc::new(AtomicBool::new(true));
        let daw = DawBlock::create(0, &names, LAYOUT, alive.clone()).unwrap();
        let mut host = HostEnd::open(&names, LAYOUT).unwrap();
        let mut process = process;
        let stop = AtomicBool::new(false);
        let thread =
            thread::spawn(move || while host.serve(&stop, &mut process) == Served::Processed {});
        (daw, alive, thread)
    }

    fn doubling(block: &mut HostBlock<'_>) {
        for ch in 0..block.outputs() {
            let input: Vec<f32> = block.input(ch).iter().map(|s| s * 2.).collect();
            block.output(ch).copy_from_slice(&input);
        }
    }

    fn soon(ms: u64) -> Option<Instant> {
        Some(Instant::now() + Duration::from_millis(ms))
    }

    #[test]
    fn a_block_goes_to_the_host_and_back_without_allocating() {
        let (mut daw, _, _host) = pair(doubling);
        let left: Vec<f32> = (0..128).map(|i| i as f32).collect();
        let right = vec![0.5; 128];
        let (mut out_l, mut out_r) = (vec![0.; 128], vec![0.; 128]);
        let outcome = daw_rt::realtime(|| {
            daw.process(
                &[&left, &right],
                &[],
                Context::default(),
                &mut [&mut out_l, &mut out_r],
                soon(500),
            )
        });
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(out_l[5], 10.);
        assert_eq!(out_r[0], 1.);
    }

    #[test]
    fn parameters_and_the_context_reach_the_host() {
        let seen = Arc::new(AtomicU64::new(0));
        let report = seen.clone();
        let (mut daw, _, _host) = pair(move |block| {
            let p = block.params();
            let ok = p.len() == 2
                && p[1]
                    == ParamChange {
                        id: 7,
                        offset: 32,
                        value: 0.25,
                    }
                && block.context.tempo_bpm == 128.
                && block.context.position_beats == 4.5
                && block.context.playing
                && block.frames() == 64;
            report.store(u64::from(ok), Ordering::Relaxed);
        });
        let params = [
            ParamChange {
                id: 7,
                offset: 0,
                value: 0.1,
            },
            ParamChange {
                id: 7,
                offset: 32,
                value: 0.25,
            },
        ];
        let context = Context {
            tempo_bpm: 128.,
            position_beats: 4.5,
            playing: true,
            offline: false,
        };
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        let outcome = daw.process(&[], &params, context, &mut [&mut l, &mut r], soon(500));
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(seen.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_late_host_costs_only_until_the_deadline_then_catches_up() {
        let slow = Arc::new(AtomicBool::new(true));
        let flag = slow.clone();
        let (mut daw, _, _host) = pair(move |block| {
            if flag.swap(false, Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(80));
            }
            doubling(block);
        });
        let input = vec![1.; 64];
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        let started = Instant::now();
        let outcome = daw.process(
            &[&input],
            &[],
            Context::default(),
            &mut [&mut l, &mut r],
            soon(5),
        );
        let waited = started.elapsed();
        assert_eq!(outcome, Outcome::Late);
        assert!(
            waited < Duration::from_millis(20),
            "waited {waited:?} for a 5 ms deadline"
        );
        assert_eq!(l[0], 0., "outputs untouched");

        let next = daw.process(
            &[&input],
            &[],
            Context::default(),
            &mut [&mut l, &mut r],
            soon(5),
        );
        assert_eq!(next, Outcome::Busy, "the host still has the late block");

        thread::sleep(Duration::from_millis(120));
        let later = daw.process(
            &[&input],
            &[],
            Context::default(),
            &mut [&mut l, &mut r],
            soon(500),
        );
        assert_eq!(later, Outcome::Done);
        assert_eq!(l[0], 2.);
    }

    #[test]
    fn offline_waits_as_long_as_the_host_needs() {
        let (mut daw, _, _host) = pair(|block| {
            thread::sleep(Duration::from_millis(30));
            doubling(block);
        });
        let input = vec![1.; 64];
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        let context = Context {
            offline: true,
            ..Context::default()
        };
        assert_eq!(
            daw.process(&[&input], &[], context, &mut [&mut l, &mut r], None),
            Outcome::Done
        );
        assert_eq!(l[0], 2.);
        assert_eq!(r[0], 0., "a missing input is silence");
    }

    #[test]
    fn a_dead_host_is_reported_at_once() {
        let (mut daw, alive, _host) = pair(doubling);
        alive.store(false, Ordering::Release);
        let (mut l, mut r) = (vec![0.; 64], vec![0.; 64]);
        assert_eq!(
            daw.process(&[], &[], Context::default(), &mut [&mut l, &mut r], None),
            Outcome::Dead
        );
    }

    #[test]
    fn dropping_the_daw_end_stops_the_host() {
        let (daw, _, host) = pair(doubling);
        drop(daw);
        host.join().unwrap();
    }

    #[test]
    fn the_host_refuses_a_different_layout() {
        let names = names();
        let _daw = DawBlock::create(0, &names, LAYOUT, Arc::new(AtomicBool::new(true))).unwrap();
        let other = Layout {
            outputs: 1,
            ..LAYOUT
        };
        assert!(HostEnd::open(&names, other).is_err());
    }
}
