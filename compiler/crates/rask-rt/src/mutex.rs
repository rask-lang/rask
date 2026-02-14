// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Mutex (conc.sync/MX1-MX2).
//!
//! Closure-based access — no guard objects, no escaping references (CB1-CB2).

use std::sync;

/// Exclusive-access wrapper. Closure-based API prevents escaping references.
pub struct RaskMutex<T> {
    inner: sync::Mutex<T>,
}

impl<T> RaskMutex<T> {
    /// Create a new mutex wrapping `value`.
    pub fn new(value: T) -> Self {
        Self {
            inner: sync::Mutex::new(value),
        }
    }

    /// Acquire the lock and run `f` with exclusive access (MX1).
    pub fn lock<R, F: FnOnce(&mut T) -> R>(&self, f: F) -> R {
        let mut guard = self.inner.lock().unwrap();
        f(&mut guard)
    }

    /// Try to acquire the lock without blocking (MX2).
    pub fn try_lock<R, F: FnOnce(&mut T) -> R>(&self, f: F) -> Option<R> {
        match self.inner.try_lock() {
            Ok(mut guard) => Some(f(&mut guard)),
            Err(sync::TryLockError::WouldBlock) => None,
            Err(sync::TryLockError::Poisoned(e)) => {
                // Recover from poison — Rask doesn't expose poison state
                Some(f(&mut e.into_inner()))
            }
        }
    }
}

// Safety: T: Send required for Mutex to be Send+Sync
unsafe impl<T: Send> Send for RaskMutex<T> {}
unsafe impl<T: Send> Sync for RaskMutex<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_and_mutate() {
        let m = RaskMutex::new(0);
        m.lock(|v| *v += 1);
        let val = m.lock(|v| *v);
        assert_eq!(val, 1);
    }

    #[test]
    fn try_lock_succeeds() {
        let m = RaskMutex::new(42);
        assert_eq!(m.try_lock(|v| *v), Some(42));
    }

    #[test]
    fn concurrent_lock() {
        use std::sync::Arc;
        let m = Arc::new(RaskMutex::new(0));
        let mut handles = vec![];
        for _ in 0..10 {
            let m = m.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    m.lock(|v| *v += 1);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.lock(|v| *v), 1000);
    }
}
