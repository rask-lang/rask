// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Spawn/join/detach (conc.async/S1-S4, H1-H4).
//!
//! Phase A: `spawn` creates an OS thread. `TaskHandle` wraps `JoinHandle`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::cancel::CancelToken;

/// Error returned by `join()` when the task failed.
#[derive(Debug)]
pub enum JoinError {
    /// Task panicked with the given message.
    Panicked(String),
    /// Task was cancelled.
    Cancelled,
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JoinError::Panicked(msg) => write!(f, "task panicked: {}", msg),
            JoinError::Cancelled => write!(f, "task was cancelled"),
        }
    }
}

impl std::error::Error for JoinError {}

/// Affine task handle (conc.async/H1-H4).
///
/// Must be consumed via `join()`, `detach()`, or `cancel()`.
/// Phase A: wraps a real OS thread `JoinHandle`.
pub struct TaskHandle<T> {
    handle: Mutex<Option<JoinHandle<Result<T, String>>>>,
    cancel_token: Arc<CancelToken>,
    consumed: AtomicBool,
}

impl<T> TaskHandle<T> {
    /// Wait for the task to complete, returning its result (H2).
    pub fn join(self) -> Result<T, JoinError> {
        self.consumed.store(true, Ordering::Release);
        let jh = self
            .handle
            .lock()
            .unwrap()
            .take()
            .expect("handle already consumed");

        match jh.join() {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(msg)) => Err(JoinError::Panicked(msg)),
            Err(_) => Err(JoinError::Panicked("thread panicked".to_string())),
        }
    }

    /// Fire-and-forget — detach the task (H3).
    pub fn detach(self) {
        self.consumed.store(true, Ordering::Release);
        // Drop the JoinHandle; thread continues running independently
        let _ = self.handle.lock().unwrap().take();
    }

    /// Request cooperative cancellation, wait for exit (H4).
    pub fn cancel(self) -> Result<T, JoinError> {
        self.consumed.store(true, Ordering::Release);
        self.cancel_token.cancel();
        let jh = self
            .handle
            .lock()
            .unwrap()
            .take()
            .expect("handle already consumed");

        match jh.join() {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => Err(JoinError::Cancelled),
            Err(_) => Err(JoinError::Cancelled),
        }
    }
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) {
        if !self.consumed.load(Ordering::Acquire) {
            // Affine violation (H1): handle dropped without join/detach/cancel.
            // Phase A: runtime panic. Phase B (or linear type pass): compile error.
            if !std::thread::panicking() {
                panic!(
                    "TaskHandle dropped without being joined, detached, or cancelled \
                     [conc.async/H1]"
                );
            }
        }
    }
}

/// Spawn a new task on an OS thread (S1, conc.strategy/A1).
///
/// Returns an affine `TaskHandle` that must be consumed.
pub fn rask_spawn<T, F>(f: F) -> TaskHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let cancel_token = Arc::new(CancelToken::new());
    let token_clone = cancel_token.clone();

    // Store the cancel token in a thread-local so `cancelled()` works inside
    CANCEL_TOKEN.with(|cell| {
        // Parent's token — we set the child's below
        let _ = cell;
    });

    let handle = thread::spawn(move || {
        CANCEL_TOKEN.with(|cell| {
            *cell.borrow_mut() = Some(token_clone);
        });
        // Catch panics and convert to Result
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(val) => Ok(val),
            Err(e) => {
                let msg = if let Some(s) = e.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = e.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Err(msg)
            }
        }
    });

    TaskHandle {
        handle: Mutex::new(Some(handle)),
        cancel_token,
        consumed: AtomicBool::new(false),
    }
}

/// Spawn a raw OS thread (S3). No affine tracking — returns a raw handle.
pub fn rask_thread_spawn<F>(f: F) -> JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(f)
}

/// Check if the current task has been cancelled (CN1).
pub fn cancelled() -> bool {
    CANCEL_TOKEN.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|t| t.is_cancelled())
            .unwrap_or(false)
    })
}

thread_local! {
    static CANCEL_TOKEN: std::cell::RefCell<Option<Arc<CancelToken>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_and_join() {
        let h = rask_spawn(|| 42);
        assert_eq!(h.join().unwrap(), 42);
    }

    #[test]
    fn spawn_and_detach() {
        let h = rask_spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(10));
        });
        h.detach();
    }

    #[test]
    fn spawn_panic_returns_join_error() {
        let h = rask_spawn(|| -> i32 { panic!("boom") });
        match h.join() {
            Err(JoinError::Panicked(msg)) => assert!(msg.contains("boom")),
            other => panic!("expected Panicked, got {:?}", other),
        }
    }

    #[test]
    fn cancel_sets_flag() {
        let h = rask_spawn(|| {
            while !cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            "done"
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        match h.cancel() {
            Ok(val) => assert_eq!(val, "done"),
            Err(JoinError::Cancelled) => {} // also acceptable
            Err(e) => panic!("unexpected: {:?}", e),
        }
    }
}
