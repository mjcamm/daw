//! Frame times of the running app, for measuring speed work. Set
//! `DAW_FRAME_LOG=1` and every 2 seconds the app prints the time between
//! frames to stderr (median, 90th percentile, worst). At the display's
//! refresh rate every frame is on time; anything above it is a missed frame.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Log {
    last: Option<Instant>,
    since: Option<Instant>,
    gaps: Vec<f32>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::default();
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("DAW_FRAME_LOG").is_some())
}

/// Called once per frame.
pub fn frame() {
    if !enabled() {
        return;
    }
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        let now = Instant::now();
        if let Some(last) = log.last {
            let gap = (now - last).as_secs_f32() * 1000.;
            log.gaps.push(gap);
        }
        log.last = Some(now);
        let since = *log.since.get_or_insert(now);
        if now - since >= EVERY && !log.gaps.is_empty() {
            let mut gaps = std::mem::take(&mut log.gaps);
            gaps.sort_by(f32::total_cmp);
            eprintln!(
                "frames: {} in {:.1} s, median {:.1} ms, p90 {:.1} ms, worst {:.1} ms",
                gaps.len(),
                (now - since).as_secs_f32(),
                gaps[gaps.len() / 2],
                gaps[gaps.len() * 9 / 10],
                gaps[gaps.len() - 1],
            );
            log.since = Some(now);
        }
    });
}
