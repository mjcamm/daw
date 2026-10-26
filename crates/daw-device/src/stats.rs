//! Live callback timing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Timing of live audio callbacks, gathered on the audio thread with atomics
/// only (no allocation or locking).
#[derive(Debug, Default)]
pub struct CallbackStats {
    sample_rate: AtomicU64,
    callbacks: AtomicU64,
    total_process_ns: AtomicU64,
    max_process_ns: AtomicU64,
    max_gap_ns: AtomicU64,
    /// Callbacks that arrived much later than their buffer length implies:
    /// the device likely ran dry (an underrun).
    late: AtomicU64,
    min_frames: AtomicU64,
    max_frames: AtomicU64,
    /// Since the load was last read: time spent processing, audio time
    /// those callbacks covered, and late callbacks.
    window_process_ns: AtomicU64,
    window_audio_ns: AtomicU64,
    window_late: AtomicU64,
}

/// How hard the audio thread worked since last asked.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Load {
    /// Processing time over the time the audio lasted: 1.0 is the limit.
    pub fraction: f32,
    /// Callbacks that came late (likely dropouts).
    pub late: u64,
}

/// Seconds of audio a buffer of `frames` frames holds.
fn buffer_seconds(frames: u64, sample_rate: u32) -> f64 {
    frames as f64 / f64::from(sample_rate)
}

impl CallbackStats {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let stats = Self::default();
        stats
            .sample_rate
            .store(u64::from(sample_rate), Ordering::Relaxed);
        stats
    }

    pub(crate) fn record(
        &self,
        now: Instant,
        took: Duration,
        previous: Option<Instant>,
        frames: usize,
    ) {
        let frames = frames as u64;
        let count = self.callbacks.fetch_add(1, Ordering::Relaxed);
        let took = took.as_nanos() as u64;
        self.total_process_ns.fetch_add(took, Ordering::Relaxed);
        self.window_process_ns.fetch_add(took, Ordering::Relaxed);
        let rate = self.sample_rate.load(Ordering::Relaxed).max(1);
        self.window_audio_ns
            .fetch_add(frames * 1_000_000_000 / rate, Ordering::Relaxed);
        self.max_process_ns.fetch_max(took, Ordering::Relaxed);
        if count == 0 {
            self.min_frames.store(frames, Ordering::Relaxed);
        }
        self.min_frames.fetch_min(frames, Ordering::Relaxed);
        self.max_frames.fetch_max(frames, Ordering::Relaxed);
        // Skip the first few callbacks while the stream settles.
        if let Some(previous) = previous.filter(|_| count > 4) {
            let gap = now.duration_since(previous).as_nanos() as u64;
            self.max_gap_ns.fetch_max(gap, Ordering::Relaxed);
            // The smallest buffer is the device period: the normal spacing.
            let sample_rate = self.sample_rate.load(Ordering::Relaxed).max(1);
            let period = self.min_frames.load(Ordering::Relaxed) * 1_000_000_000 / sample_rate;
            if gap > period * 2 + 1_000_000 {
                self.late.fetch_add(1, Ordering::Relaxed);
                self.window_late.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// The load since the last call, starting a new window.
    pub fn take_load(&self) -> Load {
        let process = self.window_process_ns.swap(0, Ordering::Relaxed);
        let audio = self.window_audio_ns.swap(0, Ordering::Relaxed);
        Load {
            fraction: if audio == 0 {
                0.0
            } else {
                process as f32 / audio as f32
            },
            late: self.window_late.swap(0, Ordering::Relaxed),
        }
    }

    /// The smallest buffer the device has asked for: its period.
    pub fn buffer_frames(&self) -> u64 {
        self.min_frames.load(Ordering::Relaxed)
    }

    /// Prints a summary: buffer sizes, processing time against the budget,
    /// and likely dropouts.
    pub fn report(&self) {
        let sample_rate = self.sample_rate.load(Ordering::Relaxed) as u32;
        let callbacks = self.callbacks.load(Ordering::Relaxed).max(1);
        let ms = |ns: u64| ns as f64 / 1e6;
        let min_frames = self.min_frames.load(Ordering::Relaxed);
        let max_frames = self.max_frames.load(Ordering::Relaxed);
        let budget_ms = buffer_seconds(min_frames, sample_rate) * 1e3;
        let max_process = ms(self.max_process_ns.load(Ordering::Relaxed));
        println!(
            "Audio callbacks: {callbacks}, buffers of {min_frames}..={max_frames} frames \
             ({budget_ms:.1} ms or more each)."
        );
        println!(
            "  Processing: {:.3} ms average, {max_process:.3} ms worst ({:.0}% of the budget).",
            ms(self.total_process_ns.load(Ordering::Relaxed)) / callbacks as f64,
            max_process / budget_ms * 100.0
        );
        println!(
            "  Longest gap between callbacks: {:.1} ms. Late callbacks (likely dropouts): {}.",
            ms(self.max_gap_ns.load(Ordering::Relaxed)),
            self.late.load(Ordering::Relaxed)
        );
    }
}
