// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Runtime context (conc.io-context/CTX1-CTX4).
//!
//! Phase A: marker type. I/O ignores it — all I/O blocks the thread.
//! Phase B: carries scheduler + reactor handles.

/// How the runtime executes tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMode {
    /// Phase A: OS threads, blocking I/O.
    ThreadBacked,
}

/// Opaque runtime context threaded via hidden parameters.
///
/// Phase A: just a marker. Validates the desugaring pass (conc.strategy/A5)
/// without carrying real scheduler state.
#[derive(Debug)]
pub struct RuntimeContext {
    pub mode: ContextMode,
}

impl RuntimeContext {
    /// Create a new thread-backed runtime context (Phase A).
    pub fn new() -> Self {
        Self {
            mode: ContextMode::ThreadBacked,
        }
    }

    /// Shut down the runtime. In Phase A this is a no-op — OS threads
    /// clean up on join/detach. Block exit semantics (conc.async/C4)
    /// are enforced by the caller tracking non-detached handles.
    pub fn shutdown(&self) {
        // Phase A: nothing to tear down
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new()
    }
}
