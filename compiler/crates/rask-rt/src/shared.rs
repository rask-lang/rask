// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared<T> (conc.sync/SY1, R1-R3).
//!
//! Read-heavy concurrent access via RwLock. Closure-based API.

use std::sync::RwLock;

/// Read-heavy shared state. Multiple readers concurrent, exclusive writer.
pub struct RaskShared<T> {
    inner: RwLock<T>,
}

impl<T> RaskShared<T> {
    /// Create a new shared value.
    pub fn new(value: T) -> Self {
        Self {
            inner: RwLock::new(value),
        }
    }

    /// Shared read access — multiple readers concurrent (R1).
    pub fn read<R, F: FnOnce(&T) -> R>(&self, f: F) -> R {
        let guard = self.inner.read().unwrap();
        f(&guard)
    }

    /// Exclusive write access — blocks until all readers finish (R2).
    pub fn write<R, F: FnOnce(&mut T) -> R>(&self, f: F) -> R {
        let mut guard = self.inner.write().unwrap();
        f(&mut guard)
    }

    /// Try shared read without blocking (R3).
    pub fn try_read<R, F: FnOnce(&T) -> R>(&self, f: F) -> Option<R> {
        match self.inner.try_read() {
            Ok(guard) => Some(f(&guard)),
            Err(_) => None,
        }
    }

    /// Try exclusive write without blocking (R3).
    pub fn try_write<R, F: FnOnce(&mut T) -> R>(&self, f: F) -> Option<R> {
        match self.inner.try_write() {
            Ok(mut guard) => Some(f(&mut guard)),
            Err(_) => None,
        }
    }
}

unsafe impl<T: Send + Sync> Send for RaskShared<T> {}
unsafe impl<T: Send + Sync> Sync for RaskShared<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn read_concurrent() {
        let s = Arc::new(RaskShared::new(42));
        let mut handles = vec![];
        for _ in 0..10 {
            let s = s.clone();
            handles.push(std::thread::spawn(move || {
                s.read(|v| assert_eq!(*v, 42));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn write_exclusive() {
        let s = RaskShared::new(0);
        s.write(|v| *v = 42);
        assert_eq!(s.read(|v| *v), 42);
    }

    #[test]
    fn concurrent_read_write() {
        let s = Arc::new(RaskShared::new(0));
        let mut handles = vec![];
        for i in 0..10 {
            let s = s.clone();
            handles.push(std::thread::spawn(move || {
                if i % 2 == 0 {
                    s.write(|v| *v += 1);
                } else {
                    s.read(|v| { let _ = *v; });
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 5 writers, each adding 1
        assert_eq!(s.read(|v| *v), 5);
    }
}
