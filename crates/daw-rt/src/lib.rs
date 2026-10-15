//! Real-time safety tooling.
//!
//! The audio thread must never allocate or free memory. Every binary and test
//! harness installs [`GuardedAllocator`] as its global allocator, and all code
//! that runs on the audio thread runs inside [`realtime`]. In debug builds any
//! allocation or deallocation inside [`realtime`] prints an error and aborts the
//! process. In release builds the checks compile away.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;
//! ```

#![allow(unsafe_code, reason = "implementing GlobalAlloc requires unsafe")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    static REALTIME_DEPTH: Cell<u32> = const { Cell::new(0) };
    static PERMIT_DEPTH: Cell<u32> = const { Cell::new(0) };
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    static VIOLATIONS: Cell<u32> = const { Cell::new(0) };
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(debug_assertions)]
const VIOLATION_MESSAGE: &[u8] =
    b"daw-rt: memory was allocated or freed inside a realtime section; aborting\n";

/// Global allocator that enforces the no-allocation rule inside [`realtime`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GuardedAllocator;

// SAFETY: every method forwards to `System` unchanged; the extra checks only
// touch const-initialised thread-locals and never allocate.
unsafe impl GlobalAlloc for GuardedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        check();
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[inline]
fn check() {
    if !INSTALLED.load(Ordering::Relaxed) {
        INSTALLED.store(true, Ordering::Relaxed);
    }
    #[cfg(debug_assertions)]
    {
        let in_realtime = REALTIME_DEPTH.try_with(Cell::get).unwrap_or(0) > 0;
        let permitted = PERMIT_DEPTH.try_with(Cell::get).unwrap_or(0) > 0;
        if in_realtime && !permitted {
            violation();
        }
    }
}

#[cfg(debug_assertions)]
#[cold]
fn violation() {
    if RECORDING.try_with(Cell::get).unwrap_or(false) {
        let _ = VIOLATIONS.try_with(|v| v.set(v.get() + 1));
        return;
    }
    // Reporting must not re-enter the check.
    let _ = PERMIT_DEPTH.try_with(|p| p.set(p.get() + 1));
    let _ = std::io::Write::write_all(&mut std::io::stderr(), VIOLATION_MESSAGE);
    std::process::abort();
}

/// Restores a thread-local counter when dropped, so panics cannot leave a
/// section marked as entered.
#[cfg(debug_assertions)]
struct DepthGuard(&'static std::thread::LocalKey<Cell<u32>>);

#[cfg(debug_assertions)]
impl DepthGuard {
    fn enter(key: &'static std::thread::LocalKey<Cell<u32>>) -> Self {
        key.with(|d| d.set(d.get() + 1));
        Self(key)
    }
}

#[cfg(debug_assertions)]
impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.0.with(|d| d.set(d.get() - 1));
    }
}

/// Runs `f` as audio-thread code: in debug builds it must not allocate or free.
#[inline]
pub fn realtime<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(debug_assertions)]
    let _guard = DepthGuard::enter(&REALTIME_DEPTH);
    f()
}

/// Allows allocation inside a [`realtime`] section. Every use needs a comment
/// explaining why it cannot glitch audio.
#[inline]
pub fn permit_alloc<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(debug_assertions)]
    let _guard = DepthGuard::enter(&PERMIT_DEPTH);
    f()
}

/// Runs `f`, counting realtime violations on this thread instead of aborting.
///
/// Intended for tests. Always reports zero in release builds, where the checks
/// are compiled out.
pub fn count_violations<T>(f: impl FnOnce() -> T) -> (T, u32) {
    let was_recording = RECORDING.with(|r| r.replace(true));
    let before = VIOLATIONS.with(Cell::get);
    let result = f();
    let after = VIOLATIONS.with(Cell::get);
    RECORDING.with(|r| r.set(was_recording));
    (result, after - before)
}

/// Whether [`GuardedAllocator`] is this process's global allocator.
///
/// Test harnesses assert this so the guard can never be silently inactive.
pub fn is_installed() -> bool {
    // Make sure at least one allocation has gone through the allocator.
    drop(std::hint::black_box(Box::new(0u8)));
    INSTALLED.load(Ordering::Relaxed)
}

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: GuardedAllocator = GuardedAllocator;

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[test]
    fn guard_is_installed() {
        assert!(is_installed());
    }

    #[test]
    fn allocating_and_freeing_inside_realtime_are_violations() {
        let ((), violations) = count_violations(|| realtime(|| drop(black_box(Box::new(1)))));
        assert_eq!(violations, 2);
    }

    #[test]
    fn allocating_outside_realtime_is_allowed() {
        let ((), violations) = count_violations(|| drop(black_box(Box::new(1))));
        assert_eq!(violations, 0);
    }

    #[test]
    fn permit_alloc_allows_allocation_inside_realtime() {
        let ((), violations) =
            count_violations(|| realtime(|| permit_alloc(|| drop(black_box(Box::new(1))))));
        assert_eq!(violations, 0);
    }

    #[test]
    fn nested_sections_stay_realtime_until_the_outermost_exits() {
        let ((), violations) = count_violations(|| {
            realtime(|| {
                realtime(|| {});
                drop(black_box(Box::new(1)));
            });
            drop(black_box(Box::new(1)));
        });
        assert_eq!(violations, 2);
    }

    #[test]
    fn realtime_section_ends_when_it_panics() {
        // Panicking allocates, so count those violations rather than abort.
        let (result, _) =
            count_violations(|| std::panic::catch_unwind(|| realtime(|| panic!("boom"))));
        assert!(result.is_err());
        let ((), violations) = count_violations(|| drop(black_box(Box::new(1))));
        assert_eq!(violations, 0);
    }

    const ABORT_CHILD_ENV: &str = "DAW_RT_ABORT_CHILD";

    /// Runs only as a child process of `violation_aborts_the_process`.
    #[test]
    #[ignore = "spawned by violation_aborts_the_process"]
    fn abort_child() {
        if std::env::var_os(ABORT_CHILD_ENV).is_some() {
            realtime(|| drop(black_box(Box::new(1))));
        }
    }

    #[test]
    fn violation_aborts_the_process() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::abort_child", "--ignored", "--nocapture"])
            .env(ABORT_CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(!output.status.success(), "child process should abort");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("realtime section"), "stderr was: {stderr}");
    }
}
