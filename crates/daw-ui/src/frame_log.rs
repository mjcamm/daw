//! Frame times of the running app, for measuring speed work. Set
//! `DAW_FRAME_LOG=1` and every 2 seconds the app prints to stderr:
//! - **gap**: time between frames (median, 90th percentile, worst). At the
//!   display's refresh rate every frame is on time; more is a missed frame.
//! - **work**: CPU time from the start of the project view's render to the
//!   end of the frame's painting (the live layer paints last). A short work
//!   time with long gaps means the time goes elsewhere (GPU, presenting).
//!
//! `DAW_FRAME_LOG=always` also draws frames continuously, as if playing, to
//! measure a page's steady cost while stopped.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Log {
    last: Option<Instant>,
    since: Option<Instant>,
    started: Option<Instant>,
    gaps: Vec<f32>,
    work: Vec<f32>,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::default();
}

fn setting() -> Option<&'static str> {
    static ON: OnceLock<Option<String>> = OnceLock::new();
    ON.get_or_init(|| std::env::var("DAW_FRAME_LOG").ok())
        .as_deref()
}

/// Whether frames should run continuously (`DAW_FRAME_LOG=always`).
pub fn always() -> bool {
    setting() == Some("always")
}

/// Called as the project view starts rendering a frame.
pub fn frame() {
    if setting().is_none() {
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
        log.started = Some(now);
        let since = *log.since.get_or_insert(now);
        if now - since >= EVERY && !log.gaps.is_empty() {
            let count = log.gaps.len();
            let gaps = summary(&mut std::mem::take(&mut log.gaps));
            let work = summary(&mut std::mem::take(&mut log.work));
            eprintln!(
                "frames: {count} in {:.1} s; gap {gaps}; work {work} (at {})",
                (now - since).as_secs_f32(),
                clock(),
            );
            log.since = Some(now);
        }
    });
}

/// Called when the frame's painting is done.
pub fn painted() {
    if setting().is_none() {
        return;
    }
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        if let Some(started) = log.started.take() {
            let work = started.elapsed().as_secs_f32() * 1000.;
            log.work.push(work);
        }
    });
}

/// The time of day as HH:MM:SS, UTC, to line the log up with other logs.
fn clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "{:02}:{:02}:{:02} UTC",
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    )
}

/// "median a, p90 b, worst c" in ms.
fn summary(values: &mut [f32]) -> String {
    if values.is_empty() {
        return "-".into();
    }
    values.sort_by(f32::total_cmp);
    format!(
        "median {:.1}, p90 {:.1}, worst {:.1} ms",
        values[values.len() / 2],
        values[values.len() * 9 / 10],
        values[values.len() - 1],
    )
}
