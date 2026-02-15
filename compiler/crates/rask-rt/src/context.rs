// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime context (conc.io-context/CTX1-CTX4).
//!
//! Phase A: marker type. I/O blocks the thread.
//! Phase B: carries scheduler + reactor handles for green tasks.

use std::sync::Arc;

use crate::green::reactor::Reactor;
use crate::green::scheduler::Scheduler;

/// How the runtime executes tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMode {
    /// Phase A: OS threads, blocking I/O.
    ThreadBacked,
    /// Phase B: M:N green tasks, non-blocking I/O via reactor.
    GreenTask,
}

/// Opaque runtime context threaded via hidden parameters.
///
/// Phase A: just a marker (ThreadBacked). Validates the desugaring pass
/// (conc.strategy/A5) without carrying real scheduler state.
///
/// Phase B: carries scheduler and reactor handles. I/O functions detect
/// GreenTask mode and use non-blocking syscalls + reactor registration
/// instead of blocking (conc.io-context/IO1-IO3).
pub struct RuntimeContext {
    pub mode: ContextMode,
    /// Phase B scheduler (None in Phase A).
    scheduler: Option<Arc<Scheduler>>,
}

impl std::fmt::Debug for RuntimeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeContext")
            .field("mode", &self.mode)
            .field("has_scheduler", &self.scheduler.is_some())
            .finish()
    }
}

impl RuntimeContext {
    /// Create a thread-backed runtime context (Phase A).
    pub fn new() -> Self {
        Self {
            mode: ContextMode::ThreadBacked,
            scheduler: None,
        }
    }

    /// Create a green-task runtime context (Phase B).
    ///
    /// Spawns worker threads and reactor. The returned context carries
    /// scheduler + reactor handles for I/O dispatching.
    pub fn with_green_tasks(worker_count: usize) -> Self {
        let scheduler = Arc::new(Scheduler::new(worker_count));
        Self {
            mode: ContextMode::GreenTask,
            scheduler: Some(scheduler),
        }
    }

    /// Get the scheduler (Phase B only).
    pub fn scheduler(&self) -> Option<&Arc<Scheduler>> {
        self.scheduler.as_ref()
    }

    /// Get the reactor (Phase B only).
    pub fn reactor(&self) -> Option<&Arc<Reactor>> {
        self.scheduler.as_ref().map(|s| s.reactor())
    }

    /// Shut down the runtime. Phase A: no-op. Phase B: waits for all
    /// tasks to complete, then stops workers and reactor (conc.async/C4).
    pub fn shutdown(&self) {
        if let Some(ref sched) = self.scheduler {
            sched.shutdown();
        }
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new()
    }
}
