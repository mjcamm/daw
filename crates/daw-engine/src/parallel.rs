//! Rendering nodes on several CPU cores at once.
//!
//! A [`Pool`] holds worker threads made up front (on the control side).
//! [`Pool::run`] hands out the tasks `0..count` to the workers and the
//! calling audio thread, and returns once every task has finished. Nothing
//! is allocated, locked or waited on with the operating system while it
//! runs: tasks are claimed from one atomic word, workers are woken with
//! `unpark` (which never blocks), and the audio thread spins on a counter
//! for the last tasks to finish while helping with the rest.
//!
//! [`Shards`] lets the tasks of one run each change their own item of a
//! shared list and read items finished in earlier runs.

#![allow(
    unsafe_code,
    reason = "sharing the running task and per-node state across threads without allocating"
)]

use std::cell::UnsafeCell;
use std::hint::spin_loop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{JoinHandle, Thread};

/// How long an idle worker keeps looking for work before it sleeps, in
/// spins. Short: the next block's work arrives with a wake-up anyway.
const SPINS_BEFORE_SLEEP: u32 = 20_000;

/// The task of the current run. Lifetimes are erased: the pointer is only
/// used while [`Pool::run`] waits for the tasks, so the closure outlives
/// every use.
type TaskPtr = *const (dyn Fn(usize) + Sync);

/// The claim word: run generation (32 bits), task count (16), next task (16).
fn pack(generation: u64, count: usize, next: usize) -> u64 {
    (generation << 32) | ((count as u64 & 0xFFFF) << 16) | (next as u64 & 0xFFFF)
}

fn unpack(word: u64) -> (u64, usize, usize) {
    (
        word >> 32,
        ((word >> 16) & 0xFFFF) as usize,
        (word & 0xFFFF) as usize,
    )
}

struct Shared {
    claim: AtomicU64,
    done: AtomicUsize,
    stop: AtomicBool,
    task: UnsafeCell<Option<TaskPtr>>,
}

// SAFETY: `task` is written only by the thread in `Pool::run` before it
// publishes a new generation through `claim` (Release), and read by workers
// only after they claim a task of that generation (Acquire). It is not
// written again until every claimed task has finished.
unsafe impl Sync for Shared {}
// SAFETY: as above; the pointer is only dereferenced during `Pool::run`.
unsafe impl Send for Shared {}

impl Shared {
    /// Claims the next task of the current run, if any is left.
    fn try_claim(&self) -> Option<(u64, usize)> {
        let mut word = self.claim.load(Ordering::Acquire);
        loop {
            let (generation, count, next) = unpack(word);
            if generation == 0 || next >= count {
                return None;
            }
            match self.claim.compare_exchange_weak(
                word,
                pack(generation, count, next + 1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some((generation, next)),
                Err(current) => word = current,
            }
        }
    }

    /// Runs a claimed task and counts it done.
    fn run_claimed(&self, index: usize) {
        // SAFETY: the task was published before the claim word that let us
        // claim `index`, and `Pool::run` keeps it alive until `done` counts
        // this task.
        if let Some(task) = unsafe { *self.task.get() } {
            let task = unsafe { &*task };
            task(index);
        }
        self.done.fetch_add(1, Ordering::AcqRel);
    }
}

/// Worker threads for rendering in parallel.
pub(crate) struct Pool {
    shared: Arc<Shared>,
    workers: Vec<(Thread, Option<JoinHandle<()>>)>,
    generation: u64,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("workers", &self.workers.len())
            .finish()
    }
}

impl Pool {
    /// A pool with `workers` extra threads (0: everything runs on the
    /// caller). `prepare` runs once on each worker as it starts, e.g. to
    /// raise its priority.
    pub fn new(workers: usize, prepare: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            claim: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            task: UnsafeCell::new(None),
        });
        let prepare = Arc::new(prepare);
        let workers = (0..workers)
            .filter_map(|n| {
                let shared = Arc::clone(&shared);
                let prepare = Arc::clone(&prepare);
                std::thread::Builder::new()
                    .name(format!("daw-render-{n}"))
                    .spawn(move || {
                        prepare();
                        work(&shared);
                    })
                    .ok()
                    .map(|handle| (handle.thread().clone(), Some(handle)))
            })
            .collect();
        Self {
            shared,
            workers,
            generation: 0,
        }
    }

    /// Threads that work on a run, the caller included.
    pub fn threads(&self) -> usize {
        self.workers.len() + 1
    }

    /// Runs `task(i)` for every `i` in `0..count` across the pool and this
    /// thread, returning when all have finished. Real-time safe.
    pub fn run(&mut self, count: usize, task: &(dyn Fn(usize) + Sync)) {
        if count == 0 {
            return;
        }
        if self.workers.is_empty() || count == 1 {
            for index in 0..count {
                task(index);
            }
            return;
        }
        debug_assert!(count <= 0xFFFF);
        let shared = &*self.shared;
        self.generation += 1;
        // SAFETY: no task of an earlier run is running (each run waits for
        // all of its tasks), so nothing reads `task` while it changes. The
        // lifetime is erased; the pointer is cleared before returning.
        unsafe {
            let erased: TaskPtr = std::mem::transmute::<*const (dyn Fn(usize) + Sync + '_), TaskPtr>(
                task as *const _,
            );
            *shared.task.get() = Some(erased);
        }
        shared.done.store(0, Ordering::Relaxed);
        shared
            .claim
            .store(pack(self.generation, count, 0), Ordering::Release);
        for (thread, _) in &self.workers {
            thread.unpark();
        }
        while let Some((_, index)) = shared.try_claim() {
            shared.run_claimed(index);
        }
        while shared.done.load(Ordering::Acquire) < count {
            spin_loop();
        }
        // SAFETY: every task of this run has finished and no more can be
        // claimed (all were), so no worker holds the pointer.
        unsafe {
            *shared.task.get() = None;
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        for (thread, handle) in &mut self.workers {
            thread.unpark();
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// A worker: takes tasks while there are any, spins a while for more, then
/// sleeps until woken.
fn work(shared: &Shared) {
    loop {
        let mut idle = 0;
        loop {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            if let Some((_, index)) = shared.try_claim() {
                daw_rt::realtime(|| daw_rt::without_denormals(|| shared.run_claimed(index)));
                idle = 0;
                continue;
            }
            idle += 1;
            if idle >= SPINS_BEFORE_SLEEP {
                break;
            }
            spin_loop();
        }
        std::thread::park();
    }
}

/// A list whose items the tasks of one [`Pool::run`] change in parallel.
pub(crate) struct Shards<T>(Box<[UnsafeCell<T>]>);

// SAFETY: access goes through the `unsafe` methods below, whose callers
// promise that no item is changed by one thread while another uses it.
unsafe impl<T: Send> Sync for Shards<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for Shards<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shards")
            .field("len", &self.0.len())
            .finish()
    }
}

impl<T> Shards<T> {
    pub fn new(items: impl IntoIterator<Item = T>) -> Self {
        Self(items.into_iter().map(UnsafeCell::new).collect())
    }

    /// An item, through exclusive access to the whole list.
    pub fn get_mut(&mut self, index: usize) -> &mut T {
        self.0[index].get_mut()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.iter_mut().map(UnsafeCell::get_mut)
    }

    /// An item to change from a parallel task.
    ///
    /// # Safety
    ///
    /// No other thread may use item `index` until the task ends.
    #[allow(clippy::mut_from_ref, reason = "the caller guarantees exclusive use")]
    pub unsafe fn task_mut(&self, index: usize) -> &mut T {
        unsafe { &mut *self.0[index].get() }
    }

    /// An item to read from a parallel task.
    ///
    /// # Safety
    ///
    /// No thread may change item `index` while it is read.
    pub unsafe fn task_ref(&self, index: usize) -> &T {
        unsafe { &*self.0[index].get() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn every_task_runs_exactly_once_on_any_number_of_threads() {
        for workers in [0, 1, 3, 7] {
            let mut pool = Pool::new(workers, || {});
            for count in [0, 1, 2, 5, 64, 300] {
                let hits: Vec<AtomicU32> = (0..count).map(|_| AtomicU32::new(0)).collect();
                for _ in 0..20 {
                    pool.run(count, &|i| {
                        hits[i].fetch_add(1, Ordering::Relaxed);
                    });
                }
                assert!(
                    hits.iter().all(|h| h.load(Ordering::Relaxed) == 20),
                    "{workers} workers, {count} tasks"
                );
            }
        }
    }

    #[test]
    fn runs_change_their_own_items_and_read_earlier_ones() {
        let mut pool = Pool::new(3, || {});
        let mut items = Shards::new(0..100u64);
        // Level 1: each item doubles itself; level 2: each adds the item
        // before it, as settled in level 1.
        pool.run(100, &|i| unsafe { *items.task_mut(i) *= 2 });
        let next = Shards::new((0..100u64).map(|_| 0));
        pool.run(100, &|i| unsafe {
            *next.task_mut(i) = items
                .task_ref(i)
                .wrapping_add(*items.task_ref(i.saturating_sub(1)));
        });
        let mut next = next;
        assert_eq!(*next.get_mut(10), 20 + 18);
        assert_eq!(*items.get_mut(99), 198);
    }

    #[test]
    fn a_run_allocates_nothing() {
        let mut pool = Pool::new(2, || {});
        let sum = AtomicU64::new(0);
        let ((), violations) = daw_rt::count_violations(|| {
            daw_rt::realtime(|| {
                pool.run(50, &|i| {
                    sum.fetch_add(i as u64, Ordering::Relaxed);
                });
            })
        });
        assert_eq!(violations, 0);
        assert_eq!(sum.load(Ordering::Relaxed), (0..50).sum::<u64>());
    }
}
