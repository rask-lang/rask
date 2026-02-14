// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Sleep and timeout (conc.runtime/TM5, conc.strategy/A table).
//!
//! Phase A: `std::thread::sleep`. Timeout via thread + channel race.

use std::time::Duration;

use crate::channel::{self, Receiver};

/// Sleep the current thread/task for the given duration.
///
/// Phase A: blocks the OS thread (std::thread::sleep).
/// Phase B: registers with timer wheel, parks the green task.
pub fn rask_sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// Create a one-shot timer that fires after `duration`.
///
/// Returns a `Receiver<()>` that receives `()` after the delay.
/// Dropping the receiver cancels the timer (the thread finishes
/// but its send fails silently).
pub fn timer_after(duration: Duration) -> Receiver<()> {
    let (tx, rx) = channel::buffered(1);
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = tx.send(());
    });
    rx
}

/// Run a closure with a timeout. Returns `Err(TimedOut)` if the closure
/// doesn't complete within `duration`.
///
/// Phase A: spawns a thread for the work, races against a timer.
pub fn with_timeout<T, F>(duration: Duration, f: F) -> Result<T, TimedOut>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = channel::buffered(1);
    std::thread::spawn(move || {
        let result = f();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(duration) {
        Ok(val) => Ok(val),
        Err(_) => Err(TimedOut),
    }
}

/// Timeout error.
#[derive(Debug, Clone, Copy)]
pub struct TimedOut;

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation timed out")
    }
}

impl std::error::Error for TimedOut {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_short() {
        let start = std::time::Instant::now();
        rask_sleep(Duration::from_millis(10));
        assert!(start.elapsed() >= Duration::from_millis(9));
    }

    #[test]
    fn timer_after_fires() {
        let rx = timer_after(Duration::from_millis(10));
        assert!(rx.recv().is_ok());
    }

    #[test]
    fn timeout_completes() {
        let result = with_timeout(Duration::from_secs(1), || 42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn timeout_expires() {
        let result = with_timeout(Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_secs(10));
            42
        });
        assert!(result.is_err());
    }
}
