// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Green task handle (conc.async/H1-H4, conc.runtime/S4).
//!
//! Same API as Phase A `TaskHandle` — spawn/join/detach/cancel.
//! Backed by green tasks instead of OS threads.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::cancel::CancelToken;
use crate::spawn::JoinError;

use super::scheduler::Scheduler;
use super::task::{RawTask, ResultSlot, TaskState};

/// Affine handle to a green task (conc.async/H1-H4).
///
/// Must be consumed via `join()`, `detach()`, or `cancel()`.
/// Dropping without consuming panics (runtime affine check).
pub struct GreenTaskHandle<T> {
    raw: Arc<RawTask>,
    result: Arc<ResultSlot<T>>,
    scheduler: Arc<Scheduler>,
    consumed: AtomicBool,
}

impl<T: Send + 'static> GreenTaskHandle<T> {
    /// Wait for the task to complete, returning its result (H2).
    ///
    /// Blocks the calling OS thread (via condvar). In a green task context,
    /// callers should use `green_join()` future instead.
    pub fn join(self) -> Result<T, JoinError> {
        self.consumed.store(true, Ordering::Release);

        // Block until the task is done.
        let (lock, cvar) = &self.raw.header.complete_notify;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = cvar.wait(done).unwrap();
        }

        self.take_result()
    }

    /// Fire-and-forget (H3).
    pub fn detach(self) {
        self.consumed.store(true, Ordering::Release);
        self.scheduler.task_detached();
    }

    /// Request cooperative cancellation, then wait for exit (H4).
    pub fn cancel(self) -> Result<T, JoinError> {
        self.consumed.store(true, Ordering::Release);
        self.raw.header.cancel_token.cancel();

        // Wait for completion.
        let (lock, cvar) = &self.raw.header.complete_notify;
        let mut done = lock.lock().unwrap();
        while !*done {
            done = cvar.wait(done).unwrap();
        }

        self.take_result()
    }

    /// Check if the task has completed (non-blocking).
    pub fn is_complete(&self) -> bool {
        self.raw.state() == TaskState::Complete
    }

    /// Returns a future that resolves when the task completes.
    /// For use inside green tasks (avoids blocking the worker thread).
    pub fn green_join(self) -> GreenJoinFuture<T> {
        self.consumed.store(true, Ordering::Release);
        GreenJoinFuture {
            raw: self.raw.clone(),
            result: self.result.clone(),
        }
    }

    fn take_result(&self) -> Result<T, JoinError> {
        match self.result.take() {
            Some(Ok(val)) => Ok(val),
            Some(Err(msg)) => Err(JoinError::Panicked(msg)),
            None => Err(JoinError::Panicked(
                "task completed without producing a result".to_string(),
            )),
        }
    }
}

impl<T> Drop for GreenTaskHandle<T> {
    fn drop(&mut self) {
        if !self.consumed.load(Ordering::Acquire) {
            if !std::thread::panicking() {
                panic!(
                    "GreenTaskHandle dropped without being joined, detached, or cancelled \
                     [conc.async/H1]"
                );
            }
        }
    }
}

/// Future for joining a green task from within another green task.
pub struct GreenJoinFuture<T> {
    raw: Arc<RawTask>,
    result: Arc<ResultSlot<T>>,
}

impl<T> Future for GreenJoinFuture<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.raw.state() == TaskState::Complete {
            let result = match self.result.take() {
                Some(Ok(val)) => Ok(val),
                Some(Err(msg)) => Err(JoinError::Panicked(msg)),
                None => Err(JoinError::Panicked(
                    "task completed without result".to_string(),
                )),
            };
            return Poll::Ready(result);
        }

        // Register our waker with the target task. When the target
        // completes (mark_complete), it wakes all registered join wakers,
        // which re-enqueues us via the scheduler.
        self.raw.register_join_waker(cx.waker().clone());
        Poll::Pending
    }
}

/// Spawn a green task on the given scheduler (S4).
///
/// The closure runs as a stackless coroutine on a worker thread.
/// Returns an affine handle that must be consumed.
pub fn green_spawn<T, F>(
    scheduler: &Arc<Scheduler>,
    f: F,
    file: &'static str,
    line: u32,
) -> GreenTaskHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let cancel_token = Arc::new(CancelToken::new());
    let result_slot = Arc::new(ResultSlot::<T>::new());

    // Wrap the closure in a future that captures the result.
    let result_ref = result_slot.clone();
    let token_ref = cancel_token.clone();
    let future = async move {
        // Check cancellation before starting.
        if token_ref.is_cancelled() {
            result_ref.set(Err("cancelled before start".to_string()));
            return;
        }

        // Run the closure, catching panics.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(val) => result_ref.set(Ok(val)),
            Err(e) => {
                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                result_ref.set(Err(msg));
            }
        }
    };

    let raw = RawTask::new(
        Box::pin(future),
        cancel_token,
        file,
        line,
    );

    scheduler.schedule(raw.clone());

    GreenTaskHandle {
        raw,
        result: result_slot,
        scheduler: scheduler.clone(),
        consumed: AtomicBool::new(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;

    fn make_scheduler() -> Arc<Scheduler> {
        Arc::new(Scheduler::new(2))
    }

    #[test]
    fn green_spawn_and_join() {
        let sched = make_scheduler();
        let h = green_spawn(&sched, || 42, file!(), line!());
        assert_eq!(h.join().unwrap(), 42);
        sched.shutdown();
    }

    #[test]
    fn green_spawn_and_detach() {
        let sched = make_scheduler();
        let counter = Arc::new(AtomicI32::new(0));
        let c = counter.clone();
        let h = green_spawn(&sched, move || { c.fetch_add(1, Ordering::Relaxed); }, file!(), line!());
        h.detach();
        sched.shutdown();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn green_spawn_panic_returns_join_error() {
        let sched = make_scheduler();
        let h = green_spawn(&sched, || -> i32 { panic!("boom") }, file!(), line!());
        match h.join() {
            Err(JoinError::Panicked(msg)) => assert!(msg.contains("boom")),
            other => panic!("expected Panicked, got {:?}", other),
        }
        sched.shutdown();
    }

    #[test]
    fn green_spawn_many_tasks() {
        let sched = make_scheduler();
        let counter = Arc::new(AtomicI32::new(0));
        let mut handles = Vec::new();

        for _ in 0..100 {
            let c = counter.clone();
            handles.push(green_spawn(&sched, move || {
                c.fetch_add(1, Ordering::Relaxed);
            }, file!(), line!()));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), 100);
        sched.shutdown();
    }
}
