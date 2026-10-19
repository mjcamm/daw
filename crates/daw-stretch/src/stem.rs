//! The project's stretcher. Phase-gradient heap integration (Průša & Holighaus, "Phase
//! Vocoder Done Right", 2017) with the synthesis gradient taken as the push-forward of the
//! analysis gradient under the time-frequency map (t, f) -> (r t, f). See docs/stretcher.md.
//! This file holds the parameters and per-frame DSP; `stream.rs` runs it in real time.

#![allow(
    clippy::needless_range_loop,
    reason = "per-bin loops index several arrays in step; kept plain so they vectorise"
)]

use std::f32::consts::{PI, TAU};

use realfft::num_complex::Complex32;

use crate::fastmath::{self, princarg};
use crate::out_len;
use crate::stream::StemStream;

#[derive(Debug, Clone)]
pub struct Stem {
    pub frame: usize,
    /// Synthesis hop = frame / overlap.
    pub overlap: usize,
    /// Scale the frequency-direction phase gradient (local group delay) by the stretch ratio.
    /// `false` is plain PGHI time-stretching, kept for ablation.
    pub scale_group_delay: bool,
    /// Bins quieter than this relative to the frame peak get the analysis phase, untracked.
    pub tol_db: f32,
    /// Stop stretching (ratio 1) for frames whose window holds a strong onset, so hits are
    /// rebuilt exactly; the lost stretch is made up over the following frames.
    pub lock: bool,
    /// Onset threshold: rise of high-passed energy over the preceding ~30 ms, in dB.
    pub onset_db: f32,
    /// At exactly ratio 1, pass the input straight through (bit-exact, almost free), with
    /// seamless hand-overs in and out (see `StemStream`).
    pub bypass: bool,
    /// Parameter string this was built from, kept in the name so results stay separate.
    pub params: String,
}

impl Default for Stem {
    fn default() -> Self {
        Self {
            frame: 2048,
            overlap: 4,
            scale_group_delay: true,
            tol_db: -70.0,
            lock: true,
            onset_db: 9.0,
            bypass: true,
            params: String::new(),
        }
    }
}

impl Stem {
    /// Parses `key=value,...` over the defaults. Unknown keys or bad values give `None`.
    pub fn with_params(params: &str) -> Option<Self> {
        let mut s = Self {
            params: params.into(),
            ..Default::default()
        };
        for kv in params.split(',') {
            let (k, v) = kv.split_once('=')?;
            match k {
                "frame" => s.frame = v.parse().ok()?,
                "overlap" => s.overlap = v.parse().ok()?,
                "scale" => s.scale_group_delay = v.parse().ok()?,
                "tol" => s.tol_db = v.parse().ok()?,
                "lock" => s.lock = v.parse().ok()?,
                "onset" => s.onset_db = v.parse().ok()?,
                "bypass" => s.bypass = v.parse().ok()?,
                _ => return None,
            }
        }
        Some(s)
    }
}

/// Max-priority queue over log-magnitude buckets: O(1) push/pop, order exact to 1/4 dB.
/// Entries are `bin`, or `bin | PREV` for the previous frame. Buckets are linked lists in a
/// preallocated node pool: no allocation or capacity checks in the hot loop.
#[derive(Clone)]
struct Buckets {
    head: Vec<u32>,
    next: Vec<u32>,
    val: Vec<u32>,
    used: usize,
    top: usize,
}

const PREV: u32 = 1 << 31;
const NIL: u32 = u32::MAX;
const BUCKETS_PER_DB: f32 = 4.0;
const BUCKET_RANGE_DB: f32 = 200.0;
const BUCKETS: usize = (BUCKET_RANGE_DB * BUCKETS_PER_DB) as usize + 2;

impl Buckets {
    /// `capacity`: most pushes in one frame.
    fn new(capacity: usize) -> Self {
        Self {
            head: vec![NIL; BUCKETS],
            next: vec![NIL; capacity],
            val: vec![0; capacity],
            used: 0,
            top: 0,
        }
    }

    fn reset(&mut self) {
        self.head.fill(NIL);
        self.used = 0;
        self.top = 0;
    }

    #[inline(always)]
    fn push(&mut self, bucket: u16, entry: u32) {
        let (b, k) = (bucket as usize, self.used);
        self.val[k] = entry;
        self.next[k] = self.head[b];
        self.head[b] = k as u32;
        self.used += 1;
        self.top = self.top.max(b);
    }

    #[inline(always)]
    fn pop(&mut self) -> Option<u32> {
        loop {
            let h = self.head[self.top];
            if h != NIL {
                self.head[self.top] = self.next[h as usize];
                return Some(self.val[h as usize]);
            }
            if self.top == 0 {
                return None;
            }
            self.top -= 1;
        }
    }
}

/// Hann-windowed spectrum of one frame. Magnitude, dB, phase and reassigned frequency
/// (radians/sample) are filled only for the mid, which drives the phase integration.
#[derive(Clone)]
pub(crate) struct Analysis {
    pub(crate) spec: Vec<Complex32>,
    pub(crate) mag: Vec<f32>,
    db: Vec<f32>,
    pub(crate) phase: Vec<f32>,
    omega: Vec<f32>,
    /// Raw spectrum mirrored one bin past each end, so the 3-tap filters run branch-free.
    ext: Vec<Complex32>,
}

impl Analysis {
    pub(crate) fn new(bins: usize) -> Self {
        Self {
            spec: vec![Complex32::default(); bins],
            mag: vec![0.0; bins],
            db: vec![0.0; bins],
            phase: vec![0.0; bins],
            omega: vec![0.0; bins],
            ext: vec![Complex32::default(); bins + 2],
        }
    }

    /// Touch every page of the buffers (see `StemStream::prefault`).
    pub(crate) fn prefault(&mut self) {
        for v in [
            &mut self.mag,
            &mut self.db,
            &mut self.phase,
            &mut self.omega,
        ] {
            for x in v.iter_mut().step_by(512) {
                *x = std::hint::black_box(*x);
            }
        }
        for v in [&mut self.spec, &mut self.ext] {
            for x in v.iter_mut().step_by(256) {
                *x = std::hint::black_box(*x);
            }
        }
    }

    /// From the spectrum `r` of the unwindowed zero-phase frame. A centred Hann window and its
    /// derivative are 3-tap filters in frequency, so one FFT gives both:
    ///   X_w(m)  = r(m)/2 + (r(m-1) + r(m+1))/4
    ///   X_dw(m) = i·(π/n)·(r(m-1) - r(m+1))/2
    /// and the reassigned frequency is ω(m) = 2πm/n − Im(X_dw / X_w).
    pub(crate) fn compute(&mut self, r: &[Complex32], n: usize, full: bool) {
        let bins = r.len();
        // Real signal: r(−m) = r(m)*, and past Nyquist r(n/2+1) = r(n/2−1)*.
        self.ext[0] = r[1].conj();
        self.ext[1..=bins].copy_from_slice(r);
        self.ext[bins + 1] = r[bins - 2].conj();
        let ext = &self.ext;
        for (m, xw) in self.spec.iter_mut().enumerate() {
            *xw = ext[m + 1] * 0.5 + (ext[m] + ext[m + 2]) * 0.25;
        }
        if !full {
            return;
        }
        let k = PI / (2.0 * n as f32);
        let w0 = TAU / n as f32;
        for m in 0..bins {
            let xw = self.spec[m];
            let d = ext[m] - ext[m + 2];
            let p = xw.norm_sqr();
            self.mag[m] = p.sqrt();
            self.db[m] = fastmath::power_db(p);
            self.phase[m] = fastmath::atan2(xw.im, xw.re);
            // With X_dw = i·(π/n)·d/2: Im(X_dw·conj(X_w)) = (π/2n)·Re(d·conj(X_w)).
            let corr = k * (d.re * xw.re + d.im * xw.im) / p.max(1e-20);
            let corr = if p > 1e-20 { corr } else { 0.0 };
            self.omega[m] = w0 * m as f32 - corr;
        }
    }
}

/// Phase-gradient integration state for the reference (mid) signal.
#[derive(Clone)]
pub(crate) struct Pghi {
    prev_phase: Vec<f32>,
    prev_omega: Vec<f32>,
    prev_db: Vec<f32>,
    pub(crate) phase: Vec<f32>,
    todo: Vec<bool>,
    /// Bucket of each bin's current / previous-frame level, for this frame's floor.
    bucket: Vec<u16>,
    prev_bucket: Vec<u16>,
    queue: Buckets,
    /// False until the first non-silent frame (and after silence or a seek): that frame takes
    /// its analysis phases as the starting point.
    pub(crate) started: bool,
}

impl Pghi {
    /// Copies the frame-to-frame state (not per-frame scratch) without allocating.
    pub(crate) fn copy_state_from(&mut self, o: &Pghi) {
        self.prev_phase.copy_from_slice(&o.prev_phase);
        self.prev_omega.copy_from_slice(&o.prev_omega);
        self.prev_db.copy_from_slice(&o.prev_db);
        self.phase.copy_from_slice(&o.phase);
        self.started = o.started;
    }

    /// Touch every page of the state and queue pool (see `StemStream::prefault`).
    pub(crate) fn prefault(&mut self) {
        fn touch<T: Copy>(v: &mut [T]) {
            for x in v.iter_mut().step_by(256) {
                *x = std::hint::black_box(*x);
            }
        }
        for v in [
            &mut self.prev_phase,
            &mut self.prev_omega,
            &mut self.prev_db,
            &mut self.phase,
        ] {
            touch(v);
        }
        touch(&mut self.todo);
        touch(&mut self.bucket);
        touch(&mut self.prev_bucket);
        touch(&mut self.queue.head);
        touch(&mut self.queue.next);
        touch(&mut self.queue.val);
    }

    pub(crate) fn new(bins: usize) -> Self {
        Self {
            prev_phase: vec![0.0; bins],
            prev_omega: vec![0.0; bins],
            prev_db: vec![-600.0; bins],
            phase: vec![0.0; bins],
            todo: vec![false; bins],
            bucket: vec![0; bins],
            prev_bucket: vec![0; bins],
            // Each bin is pushed at most twice (previous frame, then current), plus seeds.
            queue: Buckets::new(3 * bins),
            started: false,
        }
    }

    /// Synthesis phases for the current frame. `hs`: synthesis hop; `r`: local stretch ratio
    /// applied to the frequency-direction gradient.
    pub(crate) fn step(&mut self, a: &Analysis, hs: f32, r: f32, tol_db: f32) {
        let bins = a.mag.len();
        let db = &a.db;
        let peak = db.iter().fold(f32::MIN, |m, &v| m.max(v));
        let tol = peak + tol_db;
        // power_db floors at 1e-30, i.e. −300 dB: a silent frame.
        let silent = peak < -290.0;

        if !self.started || silent {
            self.phase.copy_from_slice(&a.phase);
            self.started = !silent;
        } else {
            // Bucket 0 sits well below tolerance so previous-frame entries still order.
            let floor = tol - 60.0;
            let top = (BUCKETS - 1) as f32;
            for m in 0..bins {
                self.bucket[m] = ((db[m] - floor) * BUCKETS_PER_DB).clamp(0.0, top) as u16;
                self.prev_bucket[m] =
                    ((self.prev_db[m] - floor) * BUCKETS_PER_DB).clamp(0.0, top) as u16;
            }
            self.queue.reset();
            let mut left = 0;
            for m in 0..bins {
                let t = db[m] > tol;
                self.todo[m] = t;
                if t {
                    left += 1;
                    self.queue.push(self.prev_bucket[m], m as u32 | PREV);
                } else {
                    self.phase[m] = a.phase[m];
                }
            }
            while left > 0 {
                while let Some(e) = self.queue.pop() {
                    let m = (e & !PREV) as usize;
                    if e & PREV != 0 {
                        if self.todo[m] {
                            // Time direction: trapezoidal integration of instantaneous frequency.
                            self.phase[m] =
                                self.prev_phase[m] + hs * 0.5 * (self.prev_omega[m] + a.omega[m]);
                            self.todo[m] = false;
                            left -= 1;
                            self.queue.push(self.bucket[m], m as u32);
                        }
                    } else {
                        // Frequency direction: the analysis phase step between neighbouring
                        // bins encodes group delay; scaling it by r moves events to r·t.
                        for nb in [m.wrapping_sub(1), m + 1] {
                            if nb < bins && self.todo[nb] {
                                self.phase[nb] =
                                    self.phase[m] + r * princarg(a.phase[nb] - a.phase[m]);
                                self.todo[nb] = false;
                                left -= 1;
                                self.queue.push(self.bucket[nb], nb as u32);
                            }
                        }
                    }
                    if left == 0 {
                        break;
                    }
                }
                if left > 0 {
                    // Isolated region: seed it from its loudest bin's analysis phase.
                    let m = (0..bins)
                        .filter(|&m| self.todo[m])
                        .max_by(|&x, &y| a.mag[x].total_cmp(&a.mag[y]))
                        .expect("`left` counts bins still to do");
                    self.phase[m] = a.phase[m];
                    self.todo[m] = false;
                    left -= 1;
                    self.queue.push(self.bucket[m], m as u32);
                }
            }
        }
        self.finish(a);
    }

    fn finish(&mut self, a: &Analysis) {
        for p in &mut self.phase {
            *p = princarg(*p);
        }
        self.prev_phase.copy_from_slice(&self.phase);
        self.prev_omega.copy_from_slice(&a.omega);
        self.prev_db.copy_from_slice(&a.db);
    }
}

/// rot = e^{iψ}·conj(X)/|X| per bin (1 where the mid is silent, e.g. antiphase content).
/// Branch-free so it vectorises: both results are computed and one is selected.
pub(crate) fn rotations(phase: &[f32], spec: &[Complex32], mag: &[f32], rot: &mut [Complex32]) {
    for (((r, &ph), x), &m) in rot.iter_mut().zip(phase).zip(spec).zip(mag) {
        let (sn, cs) = fastmath::sincos(ph);
        let inv = 1.0 / m.max(1e-20);
        let live = m > 1e-20;
        let re = (cs * x.re + sn * x.im) * inv;
        let im = (sn * x.re - cs * x.im) * inv;
        *r = Complex32::new(if live { re } else { 1.0 }, if live { im } else { 0.0 });
    }
}

impl Stem {
    /// Stretches a whole clip offline: exactly `out_len(input_len, ratio)` samples, pitch
    /// unchanged. A loop over the real-time engine, so it measures what ships.
    pub fn stretch(&self, input: &[Vec<f32>], ratio: f64) -> Vec<Vec<f32>> {
        const BLOCK: usize = 4096;
        let len = out_len(input[0].len(), ratio);
        let mut stream = StemStream::new(self, input.len(), BLOCK);
        // Fixed frame phase so offline results don't depend on how many streams came before.
        stream.set_stagger(0, 1);
        stream.set_ratio(ratio);
        stream.seek(0.0);
        let source: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
        let mut out = vec![vec![0.0f32; len]; input.len()];
        let mut done = 0;
        while done < len {
            let b = BLOCK.min(len - done);
            let mut blocks: Vec<&mut [f32]> =
                out.iter_mut().map(|c| &mut c[done..done + b]).collect();
            stream.process(&source, &mut blocks);
            done += b;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reassigned frequency recovers an off-bin sinusoid's true frequency.
    #[test]
    fn reassigned_frequency_is_exact_for_sinusoid() {
        let n = 1024;
        let f = 0.0731f32; // cycles/sample, off-bin
        let mut time: Vec<f32> = (0..n)
            .map(|i| (TAU * f * ((i + n / 2) % n) as f32 + 0.3).sin())
            .collect();
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let mut spec = vec![Complex32::default(); n / 2 + 1];
        planner
            .plan_fft_forward(n)
            .process(&mut time, &mut spec)
            .unwrap();
        let mut a = Analysis::new(n / 2 + 1);
        a.compute(&spec, n, true);
        let peak = (0..a.mag.len())
            .max_by(|&x, &y| a.mag[x].total_cmp(&a.mag[y]))
            .unwrap();
        for m in peak - 1..=peak + 1 {
            let got = a.omega[m] / TAU;
            assert!((got - f).abs() < 1e-4, "bin {m}: {got} vs {f}");
        }
    }
}
