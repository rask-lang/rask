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
/// One interpreted call costs tens of KB of Rust stack. `eval_expr` alone
/// reserves ~9 KB and `exec_stmt` ~5.6 KB, and a Rask frame goes through both
/// plus one `eval_expr` per level of expression nesting in the body, so a light
/// body costs ~30 KB and a heavy one several times that. Measured by recursing a
/// one-line Rask function until it dies:
///
/// | stack  | max Rask recursion depth |
/// |--------|--------------------------|
/// |  2 MiB | ~65                      |
/// |  8 MiB | ~245                     |
/// | 16 MiB | ~495                     |
///
/// A bare `thread::spawn` gives 2 MiB, so an interpreted thread got ~65 frames of
/// headroom while the main thread — running the same interpreter — got ~245. CI
/// overflowed on four threads printing in a loop. `on_interp_stack` now puts
/// `main` and the test runner on this same size, so the depth no longer depends
/// on which entry point ran the code.
///
/// 16 MiB is a stopgap, not a fix: the frame size is the actual problem (#759).
/// Erring high is right meanwhile. The reservation is lazily committed, so the
/// headroom costs address space rather than memory.
///
/// Outlining the biggest cold `eval_expr` arms was tried and barely moved it —
/// `eval_expr`'s frame went 9144 → 9064 bytes for the two largest, because LLVM
/// already overlaps slots that aren't live across a call. What the frame holds is
/// the state of the arms that *are* on a recursive path, so shrinking it means
/// restructuring those, not moving cold code out.
///
/// `RUST_MIN_STACK` can still raise this; it can no longer lower it.
pub(crate) fn spawn_interp_thread<F, T>(f: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(INTERP_STACK_BYTES)
        .spawn(move || {
            mark_stack_base();
            f()
        })
        // `thread::spawn` panics on failure too — same behaviour, clearer text.
        .expect("failed to spawn interpreter thread")
}

pub(crate) const INTERP_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Stack left in reserve when the interpreter refuses to recurse further.
///
/// The refusal itself has work to do: unwind out of every frame, build the
/// diagnostic, format it with its source snippet. That has to fit in what's
/// left, or reporting the overflow overflows.
const STACK_RESERVE_BYTES: usize = 1024 * 1024;

thread_local! {
    /// Address of a local in the frame that started interpreting on this thread.
    ///
    /// The stack grows down, so `base - current` is how much of it has been used.
    /// A depth counter can't answer this: one Rask frame costs anywhere from a few
    /// KB to tens of KB depending on how deeply nested the expressions in the body
    /// are, so a fixed limit is either wrong for heavy bodies or needlessly low for
    /// light ones (#759).
    static STACK_BASE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Record this frame as the interpreter's stack base for this thread.
fn mark_stack_base() {
    let here = 0u8;
    STACK_BASE.set(&here as *const u8 as usize);
}

/// How much stack the interpreter has used on this thread, in bytes.
///
/// Zero when no base was recorded — a caller that reached the interpreter without
/// going through one of the entry points above, in which case there's nothing to
/// measure against and the guard stays out of the way.
pub(crate) fn stack_used() -> usize {
    let here = 0u8;
    let current = &here as *const u8 as usize;
    let base = STACK_BASE.get();
    if base == 0 || base < current {
        return 0;
    }
    base - current
}

/// Is there too little stack left to safely recurse again?
pub(crate) fn stack_nearly_exhausted() -> bool {
    let used = stack_used();
    used != 0 && used + STACK_RESERVE_BYTES >= INTERP_STACK_BYTES
}

/// Run `f` on a thread with the interpreter's stack size, borrowing freely.
///
/// `main` used to run on whatever thread called in — the process main thread and
/// its 8 MiB — while every spawned task got 16 MiB. Same program, different
/// recursion depth depending on which thread ran it (#759). A scoped thread gets
/// the borrows through without requiring anything to be `'static`.
pub(crate) fn on_interp_stack<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(INTERP_STACK_BYTES)
            .spawn_scoped(s, || {
                mark_stack_base();
                f()
            })
            .expect("failed to spawn interpreter thread")
            .join()
            // The child's panic is the program's panic — resume it here rather
            // than turning it into a different one.
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

/// Reaper threads waiting on a detached task's result (ctrl.panic/O4).
///
/// O4 says a detached task's panic *must* reach stderr. A reaper racing process
/// exit doesn't satisfy that — the report just vanishes, which is exactly the
/// failure mode O4 exists to prevent. They're registered here and joined before
/// the program is done.
static DETACHED_REAPERS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());

/// Register a reaper so `join_detached_reapers` can wait for it.
pub(crate) fn register_detached_reaper(jh: std::thread::JoinHandle<()>) {
    if let Ok(mut v) = DETACHED_REAPERS.lock() {
        v.push(jh);
    }
}

/// Wait for every detached-task reaper to finish reporting.
///
/// Called once the program's own work is done. A reaper only blocks on a task
/// that was already spawned, so this waits for exactly as long as the slowest
/// detached task — which is what "the panic reaches stderr" costs.
pub fn join_detached_reapers() {
    let pending: Vec<_> = match DETACHED_REAPERS.lock() {
        Ok(mut v) => std::mem::take(&mut *v),
        Err(_) => return,
    };
    for jh in pending {
        let _ = jh.join();
    }
}

pub use build_context::BuildState;
pub use interp::{BenchmarkResult, Interpreter, RuntimeDiagnostic, RuntimeError, SourceInfo, TestResult};

#[cfg(test)]
mod drift;
