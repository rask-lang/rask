// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! M:N work-stealing scheduler (conc.runtime/S1-S4).
//!
//! N worker threads each own a local queue. When idle, workers steal
//! from peers or the global injection queue, then poll the reactor.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use super::queue::{InjectorQueue, LocalQueue};
use super::reactor::Reactor;
use super::task::{RawTask, TaskState};

/// Scheduler runtime. Created by `using Multitasking { }` block entry.
///
/// Owns worker threads, global queue, and reactor. Shuts down when
/// the `using` block exits (waits for all non-detached tasks).
pub struct Scheduler {
    /// Worker handles for join-on-shutdown.
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    /// Shared state visible to all workers.
    shared: Arc<SharedState>,
}

/// State shared between workers, reactor thread, and external spawners.
pub(crate) struct SharedState {
    /// Per-worker local queues. Index = worker id.
    pub local_queues: Vec<Arc<LocalQueue>>,
    /// Overflow / external spawn queue.
    pub global_queue: Arc<InjectorQueue>,
    /// Central I/O reactor.
    pub reactor: Arc<Reactor>,
    /// Number of active (non-completed, non-detached) tasks.
    pub active_tasks: AtomicUsize,
    /// Signal for block exit waiting on active tasks.
    pub all_done: (Mutex<bool>, Condvar),
    /// Shutdown flag for workers.
    pub shutdown: AtomicBool,
    /// Number of workers.
    pub worker_count: usize,
    /// Notify idle workers that new work is available.
    pub work_available: (Mutex<bool>, Condvar),
}

impl Scheduler {
    /// Start the scheduler with `n` worker threads and a reactor thread.
    ///
    /// If `n` is 0, defaults to the number of available CPU cores.
    pub fn new(n: usize) -> Self {
        let worker_count = if n == 0 {
            thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(4)
        } else {
            n
        };

        let local_queues: Vec<Arc<LocalQueue>> =
            (0..worker_count).map(|_| Arc::new(LocalQueue::new())).collect();

        let reactor = Arc::new(Reactor::new().expect("failed to create epoll reactor"));

        let shared = Arc::new(SharedState {
            local_queues,
            global_queue: Arc::new(InjectorQueue::new()),
            reactor,
            active_tasks: AtomicUsize::new(0),
            all_done: (Mutex::new(false), Condvar::new()),
            shutdown: AtomicBool::new(false),
            worker_count,
            work_available: (Mutex::new(false), Condvar::new()),
        });

        let mut worker_handles = Vec::with_capacity(worker_count + 1);

        // Spawn reactor thread (R2: dedicated thread for consistent I/O latency).
        {
            let shared = shared.clone();
            worker_handles.push(
                thread::Builder::new()
                    .name("rask-reactor".to_string())
                    .spawn(move || reactor_loop(&shared))
                    .expect("failed to spawn reactor thread"),
            );
        }

        // Spawn worker threads (S1).
        for id in 0..worker_count {
            let shared = shared.clone();
            worker_handles.push(
                thread::Builder::new()
                    .name(format!("rask-worker-{}", id))
                    .spawn(move || worker_loop(id, &shared))
                    .expect("failed to spawn worker thread"),
            );
        }

        Self {
            workers: Mutex::new(worker_handles),
            shared,
        }
    }

    /// Schedule a task for execution.
    ///
    /// Sets the schedule callback on the task so wakers can re-enqueue it.
    pub(crate) fn schedule(&self, task: Arc<RawTask>) {
        self.shared.active_tasks.fetch_add(1, Ordering::AcqRel);

        // Set the re-enqueue callback the waker uses.
        let shared = self.shared.clone();
        *task.header.schedule_fn.lock().unwrap() = Some(Arc::new(move |t: Arc<RawTask>| {
            inject_task(&shared, t);
            // Notify a sleeping worker.
            let (lock, cvar) = &shared.work_available;
            let mut ready = lock.lock().unwrap();
            *ready = true;
            cvar.notify_one();
        }));

        // Push to global injection queue.
        inject_task(&self.shared, task);

        // Wake a worker.
        let (lock, cvar) = &self.shared.work_available;
        let mut ready = lock.lock().unwrap();
        *ready = true;
        cvar.notify_one();
    }

    /// Notify the scheduler that a task was detached.
    ///
    /// Per conc.async/C4, block exit still waits for detached tasks to
    /// complete. Detach only means the caller doesn't care about the
    /// result — the runtime still tracks the task. So this is a no-op
    /// for the active count; `run_task` decrements on actual completion.
    pub(crate) fn task_detached(&self) {
        // Intentionally does NOT decrement active_tasks.
        // The task still runs; run_task decrements when it completes.
    }

    /// Get a reference to the reactor for I/O registration.
    pub fn reactor(&self) -> &Arc<Reactor> {
        &self.shared.reactor
    }

    /// Get shared state (for green task handles).
    #[allow(dead_code)]
    pub(crate) fn shared(&self) -> &Arc<SharedState> {
        &self.shared
    }

    /// Shut down the scheduler. Waits for all active tasks, then stops
    /// workers and reactor (C4: block exit waits for tasks).
    pub fn shutdown(&self) {
        // Wait for all tasks to complete.
        {
            let (lock, cvar) = &self.shared.all_done;
            let mut done = lock.lock().unwrap();
            while !*done && self.shared.active_tasks.load(Ordering::Acquire) > 0 {
                done = cvar.wait(done).unwrap();
            }
        }

        // Signal workers + reactor to exit.
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.reactor.request_shutdown();

        // Wake all sleeping workers.
        let (lock, cvar) = &self.shared.work_available;
        let mut ready = lock.lock().unwrap();
        *ready = true;
        cvar.notify_all();
        drop(ready);

        // Join all threads.
        let mut workers = self.workers.lock().unwrap();
        for handle in workers.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        if !self.shared.shutdown.load(Ordering::Acquire) {
            self.shutdown();
        }
    }
}

/// Push a task to the global injection queue.
fn inject_task(shared: &SharedState, task: Arc<RawTask>) {
    shared.global_queue.push(task);
}

/// Simple xorshift64 for random victim selection.
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Worker main loop (S2).
fn worker_loop(id: usize, shared: &SharedState) {
    let local = &shared.local_queues[id];
    let mut rng = (id as u64).wrapping_add(0x9E3779B97F4A7C15); // Golden ratio hash

    loop {
        // 1. Try local queue (fast path).
        if let Some(task) = local.pop() {
            run_task(task, shared);
            continue;
        }

        // 2. Try stealing from a random victim (S3).
        if shared.worker_count > 1 {
            let victim = (xorshift64(&mut rng) as usize) % shared.worker_count;
            if victim != id {
                let stolen = shared.local_queues[victim].steal_batch();
                if !stolen.is_empty() {
                    // Push all but the first to our local queue; run the first.
                    let mut iter = stolen.into_iter();
                    let first = iter.next().unwrap();
                    for task in iter {
                        let _ = local.push(task);
                    }
                    run_task(first, shared);
                    continue;
                }
            }
        }

        // 3. Try global injection queue.
        if let Some(task) = shared.global_queue.pop() {
            run_task(task, shared);
            continue;
        }

        // 4. Check shutdown before sleeping.
        if shared.shutdown.load(Ordering::Acquire) {
            // Drain local queue before exiting.
            while let Some(task) = local.pop() {
                run_task(task, shared);
            }
            break;
        }

        // 5. Park until new work arrives (avoids busy-spinning).
        let (lock, cvar) = &shared.work_available;
        let mut ready = lock.lock().unwrap();
        // Re-check: work may have arrived between step 3 and locking.
        if !shared.global_queue.is_empty() || !local.is_empty() {
            continue;
        }
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        // Wait with timeout so we periodically check for shutdown / steals.
        let result = cvar
            .wait_timeout(ready, std::time::Duration::from_millis(5))
            .unwrap();
        ready = result.0;
        *ready = false;
    }
}

/// Poll a single task. Handles completion, re-parking, and cancellation.
fn run_task(task: Arc<RawTask>, shared: &SharedState) {
    // Skip already-completed tasks.
    if task.state() == TaskState::Complete {
        return;
    }

    task.header
        .state
        .store(TaskState::Running as u8, Ordering::Release);

    let completed = task.poll();

    if completed {
        task.mark_complete();
        // Decrement active count.
        let prev = shared.active_tasks.fetch_sub(1, Ordering::AcqRel);
        if prev == 1 {
            let (lock, cvar) = &shared.all_done;
            let mut done = lock.lock().unwrap();
            *done = true;
            cvar.notify_all();
        }
    } else {
        // Future returned Pending. Use CAS to transition Running→Waiting.
        // If the waker already fired during poll() (changed Running→Ready
        // via schedule_fn), the CAS fails — re-enqueue immediately so the
        // task isn't lost.
        let prev = task.header.state.compare_exchange(
            TaskState::Running as u8,
            TaskState::Waiting as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if prev.is_err() {
            // Waker fired while we were polling — it changed
            // Running→Ready. Re-enqueue so the task gets polled again.
            inject_task(shared, task);
            let (lock, cvar) = &shared.work_available;
            let mut ready = lock.lock().unwrap();
            *ready = true;
            cvar.notify_one();
        }
    }
}

/// Reactor thread loop (R2).
fn reactor_loop(shared: &SharedState) {
    while !shared.reactor.should_shutdown() {
        // Poll with 1ms timeout (S2 step 4).
        let _ = shared.reactor.poll_once(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;

    #[test]
    fn scheduler_spawn_and_shutdown() {
        let sched = Scheduler::new(2);
        let counter = Arc::new(AtomicI32::new(0));

        for _ in 0..10 {
            let c = counter.clone();
            let task = RawTask::new(
                Box::pin(async move {
                    c.fetch_add(1, Ordering::Relaxed);
                }),
                Arc::new(crate::cancel::CancelToken::new()),
                "test",
                0,
            );
            sched.schedule(task);
        }

        sched.shutdown();
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn scheduler_default_workers() {
        // Verify it starts without panicking with 0 (auto-detect).
        let sched = Scheduler::new(0);
        sched.shutdown();
    }
}
