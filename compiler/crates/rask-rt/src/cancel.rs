// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Cooperative cancellation (conc.async/CN1-CN3).
//!
//! AtomicBool flag + join. Task checks `cancelled()` at I/O boundaries.

use std::sync::atomic::{AtomicBool, Ordering};

/// Cancellation token shared between parent and child task.
#[derive(Debug)]
pub struct CancelToken {
    flag: AtomicBool,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    /// Set the cancellation flag.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Check if cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
