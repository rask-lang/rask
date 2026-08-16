// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tree-walk interpreter for the Rask language.
//!
//! Executes the AST directly without compilation.

mod value;
mod env;
mod resource;
mod interp;
mod builtins;
mod stdlib;
pub mod build_context;

/// Spawn a thread to run interpreted Rask code.
///
/// One interpreted call costs roughly 30 KB of Rust stack — `eval_expr` is a
/// single match over 80 expression kinds, and a frame is sized for the union of
/// every arm's locals, so every call pays for all of them. Measured by
/// recursing a Rask function until it aborts:
///
/// | stack  | max Rask recursion depth |
/// |--------|--------------------------|
/// |  2 MiB | ~65                      |
/// |  8 MiB | ~275                     |
/// | 16 MiB | ~525                     |
///
/// A bare `thread::spawn` gives 2 MiB, so an interpreted thread got ~65 frames
/// of headroom while the main thread — running the same interpreter — got ~275.
/// CI overflowed on four threads printing in a loop.
///
/// 16 MiB is a stopgap, not a fix: the frame size is the actual problem, and no
/// stack setting makes ordinary recursive code work (#759). Erring high is right
/// meanwhile, because exceeding it aborts the process with no Rask-level
/// diagnostic at all. The reservation is lazily committed, so the headroom costs
/// address space rather than memory.
///
/// `RUST_MIN_STACK` can still raise this; it can no longer lower it.
pub(crate) fn spawn_interp_thread<F, T>(f: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    const INTERP_STACK_BYTES: usize = 16 * 1024 * 1024;
    std::thread::Builder::new()
        .stack_size(INTERP_STACK_BYTES)
        .spawn(f)
        // `thread::spawn` panics on failure too — same behaviour, clearer text.
        .expect("failed to spawn interpreter thread")
}

pub use build_context::BuildState;
pub use interp::{BenchmarkResult, Interpreter, RuntimeDiagnostic, RuntimeError, SourceInfo, TestResult};

#[cfg(test)]
mod drift;
