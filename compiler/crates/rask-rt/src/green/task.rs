// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Green task representation (conc.runtime/T1-T2).
//!
//! Stackless coroutine tasks: ~120 bytes base + closure captures.
//! State machine driven by poll(). Scheduler owns the polling loop.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use crate::cancel::CancelToken;

/// Task lifecycle states (T2).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Queued, waiting to be polled.
    Ready = 0,
    /// Currently being polled by a worker.
    Running = 1,
    /// Parked on I/O or timer — waiting for waker.
    Waiting = 2,
    /// Finished (result stored).
    Complete = 3,
}

impl TaskState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Ready,
            1 => Self::Running,
            2 => Self::Waiting,
            3 => Self::Complete,
            _ => Self::Complete,
        }
    }
}

/// Type-erased future for the scheduler. All tasks store a boxed future
/// that produces `()` — the typed result is captured by the closure and
/// written to a shared slot.
pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Header shared between scheduler and handle. Holds state, result
/// slot, and wake/completion signaling.
#[allow(dead_code)]
pub(crate) struct TaskHeader {
    pub state: AtomicU8,
    pub cancel_token: Arc<CancelToken>,
    /// Signal for join() blocking on completion.
    pub complete_notify: (Mutex<bool>, Condvar),
    /// Scheduler re-enqueue callback. Set by the scheduler after spawn.
    pub schedule_fn: Mutex<Option<Arc<dyn Fn(Arc<RawTask>) + Send + Sync>>>,
    /// Debug: where was this task spawned?
    pub spawn_file: &'static str,
    pub spawn_line: u32,
}

/// The actual task object owned by the scheduler.
pub(crate) struct RawTask {
    pub header: TaskHeader,
    pub future: Mutex<Option<BoxFuture>>,
}

impl std::fmt::Debug for RawTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawTask")
            .field("state", &self.state())
            .field("file", &self.header.spawn_file)
            .field("line", &self.header.spawn_line)
            .finish()
    }
}

// RawTask is Send+Sync: the future is Send, header fields are atomic/mutex.
unsafe impl Send for RawTask {}
unsafe impl Sync for RawTask {}

impl RawTask {
    pub fn new(
        future: BoxFuture,
        cancel_token: Arc<CancelToken>,
        file: &'static str,
        line: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            header: TaskHeader {
                state: AtomicU8::new(TaskState::Ready as u8),
                cancel_token,
                complete_notify: (Mutex::new(false), Condvar::new()),
                schedule_fn: Mutex::new(None),
                spawn_file: file,
                spawn_line: line,
            },
            future: Mutex::new(Some(future)),
        })
    }

    pub fn state(&self) -> TaskState {
        TaskState::from_u8(self.header.state.load(Ordering::Acquire))
    }

    /// Transition to a new state. Returns false if already Complete.
    pub fn transition(&self, new: TaskState) -> bool {
        let current = self.state();
        if current == TaskState::Complete {
            return false;
        }
        self.header
            .state
            .store(new as u8, Ordering::Release);
        true
    }

    /// Mark complete and notify any thread blocking on join().
    pub fn mark_complete(&self) {
        self.header
            .state
            .store(TaskState::Complete as u8, Ordering::Release);
        let (lock, cvar) = &self.header.complete_notify;
        let mut done = lock.lock().unwrap();
        *done = true;
        cvar.notify_all();
    }

    /// Poll the future once. Returns true if the task completed.
    pub fn poll(self: &Arc<Self>) -> bool {
        let waker = task_waker(self.clone());
        let mut cx = Context::from_waker(&waker);

        let mut fut_slot = self.future.lock().unwrap();
        let Some(fut) = fut_slot.as_mut() else {
            // Already completed or taken.
            return true;
        };

        // SAFETY: We hold the mutex, so no one else can poll concurrently.
        // The future is pinned inside the Box.
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(()) => {
                // Drop the future now that it's done.
                *fut_slot = None;
                true
            }
            Poll::Pending => false,
        }
    }
}

/// Waker that re-enqueues a task with the scheduler.
struct TaskWaker {
    task: Arc<RawTask>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // Only wake if the task is currently Waiting.
        let prev = self.task.header.state.compare_exchange(
            TaskState::Waiting as u8,
            TaskState::Ready as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if prev.is_err() {
            return; // Not in Waiting state — nothing to do.
        }

        // Re-enqueue via the schedule callback.
        let sched = self.task.header.schedule_fn.lock().unwrap();
        if let Some(ref f) = *sched {
            f(self.task.clone());
        }
    }
}

fn task_waker(task: Arc<RawTask>) -> Waker {
    Waker::from(Arc::new(TaskWaker { task }))
}

/// Typed result slot shared between the spawned future and the TaskHandle.
/// The future writes to it; join() reads from it.
pub(crate) struct ResultSlot<T> {
    inner: Mutex<Option<Result<T, String>>>,
}

impl<T> ResultSlot<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn set(&self, result: Result<T, String>) {
        *self.inner.lock().unwrap() = Some(result);
    }

    pub fn take(&self) -> Option<Result<T, String>> {
        self.inner.lock().unwrap().take()
    }
}
