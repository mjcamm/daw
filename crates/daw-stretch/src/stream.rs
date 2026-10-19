//! Real-time `stem`. Pulls from a clip already in memory, so lookahead costs nothing and a seek
//! starts producing correct output on the very next block (no latency). Tempo can change at any
//! time and takes effect within one window. `process` never allocates or locks.
//!
//! At exactly ratio 1 the input passes straight through (bit-exact, almost no CPU). Going in,
//! each bin's phase glides from the stretched phase to the input's over `GLIDE` frames (an
//! abrupt switch would cancel bins whose phases had drifted half a turn), then frames stop and
//! the raw input fills in exactly as their tails fade. Coming out, frames restart from the
//! input's own phases while the raw input covers what they don't yet. A `cover` ring tracks
//! how much of each output sample the frames provide, so the fill is exact.
//!
//! CPU arrives in bursts: one frame every `hs` output samples, and four at once after a seek.
//! Streams stagger their frames automatically (override with `set_stagger`), and `prime` moves
//! a seek's burst to a block of the host's choosing. See docs/stretcher.md (Streaming).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::hann;
use crate::fastmath::princarg;
use crate::stem::{rotations, Analysis, Pghi, Stem};

const ONSET_BLOCK: i64 = 128;
/// Onsets kept / spans considered per frame. Generous; never reached by music.
const MAX_ONSETS: usize = 1024;
const MAX_SPANS: usize = 64;
/// Automatic stagger: each new stream takes the next of this many evenly spaced frame phases.
const AUTO_SLOTS: usize = 16;
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
/// Frames over which phases glide to the input's when entering bypass (~190 ms at hs 512).
const GLIDE: f32 = 16.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Process,
    /// Ratio is exactly 1: phases glide to the input's; once there, `settle` more frames make
    /// every frame overlapping future output identical to the input. Raw index = t + offset.
    Drain { offset: i64, settle: u32 },
    /// No frames: output = remaining frame tails + raw input · (1 − cover).
    Bypass { offset: i64 },
    /// Ratio moved off 1: frames restart; raw input fills what they don't cover until `until`.
    Resume { offset: i64, until: i64 },
}

#[derive(Clone)]
pub struct StemStream {
    n: usize,
    hs: usize,
    half: usize,
    bins: usize,
    chans: usize,
    max_block: usize,
    looping: bool,
    source_origin: i64,
    scale_group_delay: bool,
    tol_db: f32,
    lock: bool,
    bypass_enabled: bool,
    mode: Mode,
    /// 0 = stretched phases, 1 = the input's phases (see `GLIDE`).
    glide: f32,
    /// Per-frame cover weight: Hann² normalised so a full overlap sums to 1.
    cover_win: Vec<f32>,
    /// Per output sample, the share of a full reconstruction the frames so far provide.
    cover: Vec<f32>,
    synth_phase: Vec<f32>,

    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// FFT scratch, preallocated: the libraries' plain `process` allocates on every call.
    fwd_scratch: Vec<Complex32>,
    inv_scratch: Vec<Complex32>,
    /// Hann synthesis window with the overlap-add and inverse-FFT normalisation folded in.
    win: Vec<f32>,
    time: Vec<f32>,
    raw: Vec<Vec<Complex32>>,
    mid_raw: Vec<Complex32>,
    chan_a: Vec<Analysis>,
    mid_a: Analysis,
    pghi: Pghi,
    synth: Vec<Complex32>,
    rot: Vec<Complex32>,
    /// One channel's windowed synthesis frame, before overlap-adding into the ring(s).
    frame_out: Vec<f32>,

    /// Overlap-add accumulator per channel, indexed by output sample & mask.
    ring: Vec<Vec<f32>>,
    mask: usize,
    /// Next output sample to emit, counted from the last seek.
    out_pos: i64,
    /// Next synthesis frame; frame k is centred on output sample k·hs + grid_offset.
    next_k: i64,
    /// Shifts this stream's frame grid so streams compute frames in different blocks.
    grid_offset: i64,

    ratio: f64,
    /// Nominal input position of frame `next_k`: integrates hs/ratio, the musical grid.
    p_nom: f64,
    prev_pos: f64,
    first_frame: bool,
    onsets: Onsets,
}

impl StemStream {
    /// Checked host constructor. Scale the analysis window and onset clock together.
    /// Construction allocates and must happen outside the audio callback.
    pub fn for_sample_rate(chans: usize, max_block: usize, sample_rate: f64) -> Result<Self, &'static str> {
        if !(1..=64).contains(&chans) || !(1..=65536).contains(&max_block)
            || !sample_rate.is_finite() || !(8000.0..=192000.0).contains(&sample_rate) {
            return Err("invalid stream configuration");
        }
        let mut params = Stem::default();
        params.frame = 1usize << ((2048.0 * sample_rate / 44100.0).log2().round() as u32);
        let mut stream = Self::new(&params, chans, max_block);
        stream.onsets.block = (128.0 * sample_rate / 44100.0).round().max(1.0) as i64;
        Ok(stream)
    }

    /// Whole-clip looping, including analysis lookahead/lookbehind and onset detection.
    /// Configure before seek/prime; changing the source layout during playback is invalid.
    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// `max_block`: largest output block `process` will be asked for.
    pub fn new(params: &Stem, chans: usize, max_block: usize) -> Self {
        let n = params.frame;
        let hs = n / params.overlap;
        let bins = n / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        // Hann² overlap-add sums to 3/8 · overlap; the inverse FFT scales by n.
        let norm = 1.0 / (0.375 * params.overlap as f32 * n as f32);
        let ring_len = (2 * n + max_block).next_power_of_two();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let mut s = Self {
            n,
            hs,
            half: n / 2,
            bins,
            chans,
            max_block,
            looping: false,
            source_origin: 0,
            scale_group_delay: params.scale_group_delay,
            tol_db: params.tol_db,
            lock: params.lock,
            bypass_enabled: params.bypass,
            mode: Mode::Process,
            glide: 0.0,
            cover_win: hann(n).into_iter().map(|w| w * w / (0.375 * params.overlap as f32)).collect(),
            cover: vec![0.0; ring_len],
            synth_phase: vec![0.0; bins],
            fwd_scratch: vec![Complex32::default(); fwd.get_scratch_len()],
            inv_scratch: vec![Complex32::default(); inv.get_scratch_len()],
            fwd,
            inv,
            win: hann(n).into_iter().map(|w| w * norm).collect(),
            time: vec![0.0; n],
            raw: vec![vec![Complex32::default(); bins]; chans],
            mid_raw: vec![Complex32::default(); bins],
            chan_a: (0..chans).map(|_| Analysis::new(bins)).collect(),
            mid_a: Analysis::new(bins),
            pghi: Pghi::new(bins),
            synth: vec![Complex32::default(); bins],
            rot: vec![Complex32::default(); bins],
            frame_out: vec![0.0; n],
            ring: vec![vec![0.0; ring_len]; chans],
            mask: ring_len - 1,
            out_pos: 0,
            next_k: 0,
            // Successive streams compute frames in different blocks without any host setup.
            grid_offset: (hs * (NEXT_SLOT.fetch_add(1, Ordering::Relaxed) % AUTO_SLOTS) / AUTO_SLOTS) as i64,
            ratio: 1.0,
            p_nom: 0.0,
            prev_pos: 0.0,
            first_frame: true,
            onsets: Onsets::new(params.onset_db),
        };
        s.prefault();
        s
    }

    /// Zeroed allocations are mapped lazily by the OS, so the first write to each page faults.
    /// Touch every page now so the first `process` call doesn't pay for it.
    fn prefault(&mut self) {
        fn touch<T: Copy>(v: &mut [T]) {
            for x in v.iter_mut().step_by(512) {
                // Write each value back unchanged (some buffers, like the window, hold data).
                // Volatile, so the compiler can't drop a write it can prove is a no-op.
                unsafe { std::ptr::write_volatile(x, *x) };
            }
        }
        touch(&mut self.win);
        touch(&mut self.time);
        touch(&mut self.mid_raw);
        touch(&mut self.synth);
        touch(&mut self.rot);
        touch(&mut self.frame_out);
        touch(&mut self.cover);
        touch(&mut self.cover_win);
        touch(&mut self.synth_phase);
        touch(&mut self.fwd_scratch);
        touch(&mut self.inv_scratch);
        for v in &mut self.raw {
            touch(v);
        }
        for v in &mut self.ring {
            touch(v);
        }
        self.mid_a.prefault();
        self.pghi.prefault();
        self.onsets.prefault();
        for a in &mut self.chan_a {
            a.prefault();
        }
    }

    /// Output length / input length (> 1 = slower). Applies from the next frame computed.
    pub fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(0.25, 4.0);
    }

    /// Stream `slot` of `slots` offsets its frame grid by slot/slots of a hop, so many streams
    /// don't all compute a frame in the same block. New streams already stagger themselves;
    /// this overrides that (e.g. `set_stagger(0, 1)` for reproducible output). Takes effect at
    /// the next `seek`.
    pub fn set_stagger(&mut self, slot: usize, slots: usize) {
        self.grid_offset = (self.hs * (slot % slots.max(1)) / slots.max(1)) as i64;
    }

    /// The next output sample will correspond to input sample `pos`. Frames reaching back
    /// before `pos` are built from the clip, so the first block is already correct (call
    /// `prime` to build them now rather than inside the first `process`).
    pub fn seek(&mut self, pos: f64) {
        for c in &mut self.ring {
            c.fill(0.0);
        }
        self.cover.fill(0.0);
        self.glide = 0.0;
        if self.unity() {
            // Nothing to prime: the raw input is the output.
            self.out_pos = 0;
            self.mode = Mode::Bypass { offset: pos.round() as i64 };
            return;
        }
        self.mode = Mode::Process;
        let (hs, half) = (self.hs as i64, self.half as i64);
        // First frame whose window reaches output sample 0.
        let k_min = (-self.grid_offset - half).div_euclid(hs) + 1;
        self.out_pos = 0;
        self.next_k = k_min;
        self.p_nom = pos + (k_min * hs + self.grid_offset) as f64 / self.ratio;
        self.first_frame = true;
        self.pghi.started = false;
        self.reset_onsets();
    }

    fn reset_onsets(&mut self) {
        let lookback = 9.0 * self.half as f64 / self.ratio + 10.0 * self.onsets.block as f64;
        self.onsets.reset((self.p_nom - lookback).floor() as i64, self.looping);
    }

    fn unity(&self) -> bool {
        self.bypass_enabled && self.ratio == 1.0
    }

    /// Mode changes happen between blocks, driven by the current ratio.
    fn update_mode(&mut self) {
        let unity = self.unity();
        match self.mode {
            Mode::Process if unity => {
                // At ratio 1 the lock deviation is zero, so frame k reads input round(p_nom(k))
                // and p_nom advances by exactly hs: every frame has the same input offset.
                let offset = self.p_nom.round() as i64 - self.frame_centre(self.next_k);
                self.mode = Mode::Drain { offset, settle: 0 };
            }
            Mode::Drain { .. } if !unity => self.mode = Mode::Process,
            Mode::Bypass { offset } if !unity => self.resume(offset),
            _ => {}
        }
    }

    /// Leaves bypass: the first frame is the first whose window starts at the next output
    /// sample or later, so it touches nothing already emitted.
    fn resume(&mut self, offset: i64) {
        let (hs, half) = (self.hs as i64, self.half as i64);
        let k = (self.out_pos + half - self.grid_offset + hs - 1).div_euclid(hs);
        self.next_k = k;
        let here = (self.out_pos + offset) as f64;
        self.p_nom = here + (self.frame_centre(k) - self.out_pos) as f64 / self.ratio;
        self.first_frame = true;
        self.pghi.started = false;
        self.glide = 0.0;
        self.reset_onsets();
        let until = self.frame_centre(k) - half + self.n as i64;
        self.mode = Mode::Resume { offset, until };
    }

    /// A snapshot of this stream seeked to `pos` and primed, for instant launch with
    /// `launch`. Building it allocates and computes the start-up frames, so do it off the
    /// audio thread or ahead of time (hot cues, quantised launch points).
    pub fn cue(&self, source: &[&[f32]], pos: f64) -> Cue {
        let mut s = self.clone();
        s.seek(pos);
        s.prime(source);
        Cue(s)
    }

    /// Starts playback from a cue by copying its state: no frames computed, no allocation.
    /// The cue's start-up frames were built at its ratio; the current ratio applies from the
    /// next frame, exactly like a tempo change during playback.
    pub fn launch(&mut self, cue: &Cue) {
        let c = &cue.0;
        assert!(c.n == self.n && c.chans == self.chans && c.ring[0].len() == self.ring[0].len(), "cue from a stream with a different configuration");
        for (dst, src) in self.ring.iter_mut().zip(&c.ring) {
            dst.copy_from_slice(src);
        }
        self.pghi.copy_state_from(&c.pghi);
        self.cover.copy_from_slice(&c.cover);
        self.mode = c.mode;
        self.glide = c.glide;
        self.onsets.copy_state_from(&c.onsets);
        self.out_pos = c.out_pos;
        self.next_k = c.next_k;
        self.grid_offset = c.grid_offset;
        self.p_nom = c.p_nom;
        self.prev_pos = c.prev_pos;
        self.first_frame = c.first_frame;
    }

    /// True while the input passes straight through (ratio exactly 1, hand-over complete).
    pub fn is_bypassed(&self) -> bool {
        matches!(self.mode, Mode::Bypass { .. })
    }

    /// Input position on the musical grid for the next output sample (playheads, sync).
    pub fn input_position(&self) -> f64 {
        match self.mode {
            Mode::Bypass { offset } => (self.out_pos + offset) as f64,
            _ => self.p_nom - (self.frame_centre(self.next_k) - self.out_pos) as f64 / self.ratio,
        }
    }

    fn frame_centre(&self, k: i64) -> i64 {
        k * self.hs as i64 + self.grid_offset
    }

    /// Builds every frame the next `max_block` of output needs, without emitting anything.
    /// After a seek this moves the start-up burst (four frames) to a block of the host's
    /// choosing, e.g. spread over the blocks before a quantised launch.
    pub fn prime(&mut self, source: &[&[f32]]) {
        if !matches!(self.mode, Mode::Bypass { .. }) {
            self.frames_through(source, self.out_pos + self.max_block as i64 - 1);
        }
    }

    /// Computes frames until output sample `last` is complete: every frame starting at or
    /// before it has been added.
    fn frames_through(&mut self, source: &[&[f32]], last: i64) {
        while self.frame_centre(self.next_k) - self.half as i64 <= last {
            self.frame(source);
        }
    }

    /// Fills every channel of `out` (equal lengths, at most `max_block`) from `source`.
    /// `source` must be the same clip on every call (until the next seek); reads past either
    /// end are silence.
    pub fn process(&mut self, source: &[&[f32]], out: &mut [&mut [f32]]) {
        let b = out[0].len();
        self.render(source, out, b);
    }

    /// Absolute source interval needed by the next block, including onset lookahead.
    /// Apply ratio/seek first. Calling this does not advance output; it may enter a
    /// bypass transition. The host must prepare this interval before process_window.
    pub fn source_bounds(&mut self, frames: usize) -> Result<(i64, i64), &'static str> {
        if frames > self.max_block { return Err("block too large"); }
        self.update_mode();
        if let Mode::Bypass { offset } = self.mode {
            let start = self.out_pos + offset;
            return Ok((start, start + frames as i64));
        }
        let ui = self.half as f64 / self.ratio;
        let mut lo = (self.p_nom - 9.0 * ui - self.n as f64 - 2.0).floor() as i64;
        let mut hi = (self.p_nom + (frames + self.n) as f64 / self.ratio
            + 9.0 * ui + self.n as f64 + 2.0).ceil() as i64;
        if self.lock { lo = lo.min(self.onsets.scanned - 1); }
        if let Mode::Resume { offset, .. } = self.mode {
            lo = lo.min(self.out_pos + offset);
            hi = hi.max(self.out_pos + offset + frames as i64);
        }
        Ok((lo, hi))
    }

    /// Process a resident window at absolute `origin`. The host has already
    /// applied whole-clip looping/zero padding to this window. No I/O or allocation.
    /// Missing coverage returns an error before consuming any output samples.
    pub fn process_window(&mut self, source: &[&[f32]], origin: i64,
        out: &mut [&mut [f32]]) -> Result<(), &'static str> {
        if source.len() != self.chans || out.len() != self.chans || out.is_empty() {
            return Err("channel count mismatch");
        }
        let frames = out[0].len();
        if out.iter().any(|c| c.len() != frames) { return Err("output lengths differ"); }
        let (lo, hi) = self.source_bounds(frames)?;
        if origin > lo || source.iter().any(|c| (c.len() as i64) < hi - origin) {
            return Err("source window incomplete");
        }
        let looping = self.looping;
        self.source_origin = origin;
        self.looping = false;
        self.render(source, out, frames);
        self.source_origin = 0;
        self.looping = looping;
        Ok(())
    }

    /// Like `process`, writing the first `len` samples of each channel buffer. Lets a host
    /// (or `StemPool`) keep per-stream buffers without building slice lists every block.
    pub fn process_into(&mut self, source: &[&[f32]], out: &mut [Vec<f32>], len: usize) {
        self.render(source, out, len);
    }

    fn render<O: AsMut<[f32]>>(&mut self, source: &[&[f32]], out: &mut [O], b: usize) {
        assert!(b <= self.max_block, "block of {b} exceeds max_block {}", self.max_block);
        assert!(source.len() == self.chans && out.len() == self.chans, "channel count mismatch");
        self.update_mode();
        if !matches!(self.mode, Mode::Bypass { .. }) {
            self.frames_through(source, self.out_pos + b as i64 - 1);
        }
        let fill = match self.mode {
            Mode::Bypass { offset } | Mode::Resume { offset, .. } => Some(offset),
            _ => None,
        };
        for ((ring, o), x) in self.ring.iter_mut().zip(out.iter_mut()).zip(source) {
            for (i, v) in o.as_mut()[..b].iter_mut().enumerate() {
                let idx = (self.out_pos as usize + i) & self.mask;
                *v = ring[idx];
                ring[idx] = 0.0;
                if let Some(off) = fill {
                    // The raw input supplies exactly the share the frames don't.
                    let missing = 1.0 - self.cover[idx];
                    if missing > 0.0 {
                        let j = self.out_pos + i as i64 + off;
                        let raw = source_sample(x, j - self.source_origin, self.looping);
                        *v += missing * raw;
                    }
                }
            }
        }
        for i in 0..b {
            self.cover[(self.out_pos as usize + i) & self.mask] = 0.0;
        }
        self.out_pos += b as i64;
        match self.mode {
            // Frames from the glide's end onwards equal the input; once every frame overlapping
            // future output is one of them, stop computing and let the raw input take over.
            Mode::Drain { offset, settle } if settle > self.n as u32 / self.hs as u32 => self.mode = Mode::Bypass { offset },
            Mode::Resume { until, .. } if self.out_pos >= until => self.mode = Mode::Process,
            _ => {}
        }
    }

    fn frame(&mut self, source: &[&[f32]]) {
        let (n, hs, half, bins) = (self.n, self.hs, self.half, self.bins);
        let r = self.ratio;
        let k = self.next_k;
        let p_nom = self.p_nom;
        self.next_k += 1;
        self.p_nom += hs as f64 / r;

        let (pos, locked) = if self.lock {
            let ui = half as f64 / r;
            self.onsets.scan_to(source, (p_nom + 9.0 * ui + half as f64).ceil() as i64, self.looping, self.source_origin);
            self.onsets.drop_before((p_nom - 9.0 * ui).floor() as i64);
            lock_map(&self.onsets.found, p_nom, r, ui)
        } else {
            (p_nom, false)
        };
        // Local stretch for the group-delay scaling: synthesis hop over analysis hop.
        let r_local = if locked {
            1.0
        } else if self.first_frame {
            r
        } else {
            hs as f64 / (pos - self.prev_pos).max(hs as f64 / 8.0)
        };
        self.prev_pos = pos;
        self.first_frame = false;
        let r_gd = if self.scale_group_delay { r_local as f32 } else { 1.0 };

        let centre = pos.round() as isize;
        // Mono material stored as stereo (common for kick, bass, vocal stems): when every
        // channel's frame is identical, analyse and synthesise once and copy. Exact.
        let dup = self.chans > 1 && frames_identical(source, centre - self.source_origin as isize, half, self.looping);
        let analysed = if dup { 1 } else { self.chans };
        let mut silent = true;
        let centre = centre - self.source_origin as isize;
        for (c, x) in source.iter().enumerate().take(analysed) {
            // Zero-phase layout: offset τ from the centre goes to index τ mod n.
            let (lo, hi) = (centre - half as isize, centre + half as isize);
            if lo >= 0 && hi as usize <= x.len() {
                let (lo, centre) = (lo as usize, centre as usize);
                self.time[..half].copy_from_slice(&x[centre..centre + half]);
                self.time[half..].copy_from_slice(&x[lo..centre]);
            } else {
                let get = |j: isize| source_sample(x, j as i64, self.looping);
                for i in 0..half {
                    self.time[i] = get(centre + i as isize);
                    self.time[i + half] = get(lo + i as isize);
                }
            }
            // Below −110 dBFS RMS counts as silence: skip the FFT.
            if self.time.iter().map(|v| v * v).sum::<f32>() < 1e-11 * n as f32 {
                self.raw[c].fill(Complex32::default());
            } else {
                silent = false;
                self.fwd.process_with_scratch(&mut self.time, &mut self.raw[c], &mut self.fwd_scratch).expect("fft");
            }
        }
        if silent {
            // Nothing to synthesise; restart phase tracking when sound returns.
            self.pghi.started = false;
            return;
        }
        for m in 0..bins {
            // With duplicated channels the mid is just one channel (scaling doesn't matter:
            // only its phase and relative level are used).
            self.mid_raw[m] = self.raw[..analysed].iter().map(|r| r[m]).sum();
        }
        for c in 0..analysed {
            self.chan_a[c].compute(&self.raw[c], n, false);
        }
        self.mid_a.compute(&self.mid_raw, n, true);
        self.pghi.step(&self.mid_a, hs as f32, r_gd, self.tol_db);

        // Bypass glide: draining moves the synthesis phases towards the input's, processing
        // moves them back. PGHI's own state is untouched, so it carries on unaware.
        if let Mode::Drain { settle, .. } = &mut self.mode {
            if self.glide >= 1.0 {
                *settle += 1;
            }
            self.glide = (self.glide + 1.0 / GLIDE).min(1.0);
        } else if self.glide > 0.0 {
            self.glide = (self.glide - 1.0 / GLIDE).max(0.0);
        }
        let phase: &[f32] = if self.glide > 0.0 {
            let keep = 1.0 - self.glide;
            for m in 0..bins {
                let a = self.mid_a.phase[m];
                self.synth_phase[m] = a + keep * princarg(self.pghi.phase[m] - a);
            }
            &self.synth_phase
        } else {
            &self.pghi.phase
        };

        // Each channel keeps its phase offset from the mid, preserving the stereo image:
        // Y_c = X_c · e^{i(ψ − φ_mid)}, one unit rotation per bin shared by all channels.
        rotations(phase, &self.mid_a.spec, &self.mid_a.mag, &mut self.rot);
        let start = self.frame_centre(k) - half as i64;
        // Skip samples already emitted (only happens for frames primed by a seek).
        let skip = (self.out_pos - start).clamp(0, n as i64) as usize;
        for c in 0..analysed {
            for m in 0..bins {
                self.synth[m] = self.chan_a[c].spec[m] * self.rot[m];
            }
            self.synth[0].im = 0.0;
            self.synth[bins - 1].im = 0.0;
            self.inv.process_with_scratch(&mut self.synth, &mut self.time, &mut self.inv_scratch).expect("ifft");
            // Undo the zero-phase layout while windowing: output j comes from time[j ± half].
            for j in 0..half {
                self.frame_out[j] = self.time[j + half] * self.win[j];
                self.frame_out[j + half] = self.time[j] * self.win[j + half];
            }
            // Duplicated channels all receive channel 0's frame.
            let targets = if dup { 0..self.chans } else { c..c + 1 };
            let base = start as usize;
            for ring in &mut self.ring[targets] {
                for j in skip..n {
                    ring[(base + j) & self.mask] += self.frame_out[j];
                }
            }
        }
        let base = start as usize;
        for j in skip..n {
            self.cover[(base + j) & self.mask] += self.cover_win[j];
        }
    }
}

/// A primed stream state for instant launch (see `StemStream::cue`).
#[derive(Clone)]
pub struct Cue(StemStream);

impl Cue {
    /// Input position the cue starts at.
    pub fn position(&self) -> f64 {
        self.0.input_position()
    }
}

/// Whether every channel holds the same samples over the frame centred on `centre`.
fn frames_identical(source: &[&[f32]], centre: isize, half: usize, looping: bool) -> bool {
    let (lo, hi) = (centre - half as isize, centre + half as isize);
    let first = source[0];
    if lo >= 0 && hi as usize <= first.len() {
        let r = lo as usize..hi as usize;
        source[1..].iter().all(|c| c.len() == first.len() && c[r.clone()] == first[r.clone()])
    } else {
        let get = |c: &[f32], j: isize| source_sample(c, j as i64, looping);
        source[1..].iter().all(|c| (lo..hi).all(|j| get(c, j) == get(first, j)))
    }
}

/// Input position for nominal position `p_nom` at ratio `r`, and whether the frame is locked.
/// Around each cluster of onsets the map has slope 1 (no stretch) through (q, q) in nominal
/// terms for the cluster's first onset q, so hits land exactly on the grid and are rebuilt
/// sample-exact; linear ramps return to the nominal map. `ui`: half window in input samples.
/// Equivalent to the offline map at constant ratio, expressed in input time so the ratio can
/// change while playing.
fn lock_map(onsets: &VecDeque<i64>, p_nom: f64, r: f64, ui: f64) -> (f64, bool) {
    let mut spans = [(0.0f64, 0.0f64, 0.0f64); MAX_SPANS];
    let mut ns = 0;
    let (lo, hi) = (p_nom - 9.0 * ui, p_nom + 9.0 * ui);
    for &q in onsets.iter().filter(|&&q| (q as f64) >= lo && (q as f64) <= hi) {
        let q = q as f64;
        if ns > 0 && q - spans[ns - 1].1 <= ui {
            spans[ns - 1].1 = q + ui;
        } else if ns < MAX_SPANS {
            spans[ns] = (q - ui, q + ui, q);
            ns += 1;
        }
    }
    // Deviation from the nominal map along a lock: Δ = (p − anchor)(r − 1).
    let dev = |x: f64, a: f64| (x - a) * (r - 1.0);
    let mut out = (p_nom, false);
    for i in 0..ns {
        let (s, e, a) = spans[i];
        if p_nom >= s && p_nom <= e {
            return (p_nom + dev(p_nom, a), true);
        }
        let before = if i > 0 { (s - spans[i - 1].1) / 2.0 } else { 2.0 * ui }.clamp(0.0, 2.0 * ui);
        let after = if i + 1 < ns { (spans[i + 1].0 - e) / 2.0 } else { 2.0 * ui }.clamp(0.0, 2.0 * ui);
        if p_nom < s && s - p_nom < before {
            out = (p_nom + dev(s, a) * (1.0 - (s - p_nom) / before), false);
        } else if p_nom > e && p_nom - e < after {
            out = (p_nom + dev(e, a) * (1.0 - (p_nom - e) / after), false);
        }
    }
    out
}

/// Incremental onset detector over the clip: 128-sample blocks of first-difference
/// (high-passed) energy, an onset where it rises `thresh_db` over the mean of the previous
/// ten blocks (~30 ms at 44.1 kHz). Fixed-size state; scans only as far as frames need.
#[derive(Clone)]
struct Onsets {
    block: i64,
    thresh_db: f32,
    found: VecDeque<i64>,
    scanned: i64,
    hist: [f32; 10],
    hist_len: usize,
    hist_pos: usize,
    prev: f32,
    prev_valid: bool,
    last: Option<i64>,
}

impl Onsets {
    fn new(thresh_db: f32) -> Self {
        Self {
            thresh_db,
            block: ONSET_BLOCK,
            found: VecDeque::with_capacity(MAX_ONSETS),
            scanned: 0,
            hist: [0.0; 10],
            hist_len: 0,
            hist_pos: 0,
            prev: 0.0,
            prev_valid: false,
            last: None,
        }
    }

    /// Fill the reserved capacity once so its pages are mapped before real-time use.
    fn prefault(&mut self) {
        while self.found.len() < MAX_ONSETS {
            self.found.push_back(0);
        }
        self.found.clear();
    }

    /// Copies another detector's state without allocating (capacity is fixed).
    fn copy_state_from(&mut self, o: &Onsets) {
        self.found.clear();
        self.found.extend(o.found.iter().copied());
        self.scanned = o.scanned;
        self.hist = o.hist;
        self.hist_len = o.hist_len;
        self.hist_pos = o.hist_pos;
        self.prev = o.prev;
        self.prev_valid = o.prev_valid;
        self.last = o.last;
    }

    fn reset(&mut self, from: i64, looping: bool) {
        self.found.clear();
        self.scanned = (if looping { from } else { from.max(0) }).div_euclid(self.block) * self.block;
        self.hist_len = 0;
        self.hist_pos = 0;
        self.prev_valid = false;
        self.last = None;
    }

    fn scan_to(&mut self, source: &[&[f32]], to: i64, looping: bool, origin: i64) {
        let mono = |i: i64| -> f32 {
            source.iter().map(|c| source_sample(c, i - origin, looping)).sum()
        };
        if !self.prev_valid {
            self.prev = mono(self.scanned - 1);
            self.prev_valid = true;
        }
        while self.scanned + self.block <= to {
            let b0 = self.scanned;
            let mut sum = 0.0;
            for i in b0..b0 + self.block {
                let x = mono(i);
                sum += (x - self.prev) * (x - self.prev);
                self.prev = x;
            }
            let e = sum / self.block as f32;
            if self.hist_len > 0 {
                let mean = self.hist[..self.hist_len].iter().sum::<f32>() / self.hist_len as f32;
                // About −70 dB floor: ignore rises out of near-silence noise.
                let rise = e > 1e-7 && 10.0 * (e / (mean + 1e-12)).log10() > self.thresh_db;
                if rise && self.last.map_or(true, |l| b0 - l > 4 * self.block) {
                    if self.found.len() == MAX_ONSETS {
                        self.found.pop_front();
                    }
                    self.found.push_back(b0);
                    self.last = Some(b0);
                }
            }
            self.hist[self.hist_pos] = e;
            self.hist_pos = (self.hist_pos + 1) % 10;
            self.hist_len = (self.hist_len + 1).min(10);
            self.scanned += self.block;
        }
    }

    fn drop_before(&mut self, pos: i64) {
        while self.found.front().is_some_and(|&q| q < pos) {
            self.found.pop_front();
        }
    }
}

/// Periodic reads avoid resetting the phase/onset state at a loop boundary.
#[inline]
fn source_sample(x: &[f32], i: i64, looping: bool) -> f32 {
    if looping && !x.is_empty() {
        x[i.rem_euclid(x.len() as i64) as usize]
    } else if i >= 0 {
        x.get(i as usize).copied().unwrap_or(0.0)
    } else {
        0.0
    }
}
