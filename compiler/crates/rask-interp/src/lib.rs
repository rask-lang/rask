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
/// Tree-walking costs one Rust frame per nested expression, so an interpreted
/// call sits far deeper on the stack than the compiled equivalent. The 2 MiB a
/// bare `thread::spawn` gives is enough on a roomy dev box and not always
/// enough elsewhere — CI overflowed on four threads printing in a loop, which
/// is not a deep program. The main thread gets 8 MiB by default; spawned
/// threads running the same interpreter should not get a quarter of that.
///
/// `RUST_MIN_STACK` still wins if it asks for more.
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
