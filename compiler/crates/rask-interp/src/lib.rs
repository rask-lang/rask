// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tree-walk interpreter for the Rask language.
//!
//! Executes the AST directly without compilation.

mod value;
mod rack;
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
/// | stack  | max Rask recursion depth (release) |
/// |--------|-----------------------------------|
/// |  2 MiB | ~65                               |
/// |  8 MiB | ~245                              |
/// | 16 MiB | ~495                              |
///
/// A bare `thread::spawn` gives 2 MiB, so an interpreted thread got ~65 frames of
/// headroom while the main thread — running the same interpreter — got ~245. CI
/// overflowed on four threads printing in a loop. `on_interp_stack` now puts
/// `main` and the test runner on this same size, so the depth no longer depends
/// on which entry point ran the code.
///
/// Unoptimized, a frame costs about 550 KB rather than 30 KB — nothing is
/// overlapped or inlined — so 16 MiB gets a debug build only ~27 frames. That is
/// why `INTERP_STACK_BYTES` is profile-dependent: the same program has to be able
/// to recurse as deep whichever way the compiler was built, or a test passes
/// locally under `--release` and dies in a debug CI job.
///
/// The size no longer decides how deep a program can recurse — `grow_interp_stack`
/// continues on a fresh stack when this one runs out — but it does decide how
/// often that costs a thread spawn, so erring high is still right. The
/// reservation is lazily committed, so the headroom costs address space rather
/// than memory until it's used.
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


/// Stack for a thread running interpreted Rask code.
///
/// Sized so that both profiles reach a comparable Rask recursion depth (~465),
/// because a debug frame costs roughly 17× an optimized one.
#[cfg(not(debug_assertions))]
pub(crate) const INTERP_STACK_BYTES: usize = 16 * 1024 * 1024;
#[cfg(debug_assertions)]
pub(crate) const INTERP_STACK_BYTES: usize = 288 * 1024 * 1024;

/// Stack left in reserve when the interpreter refuses to recurse further.
///
/// The refusal itself has work to do: unwind out of every frame, build the
/// diagnostic, format it with its source snippet. That has to fit in what's
/// left, or reporting the overflow overflows. Scaled with the profile for the
/// same reason the stack size is — a debug frame is ~17× an optimized one, so a
/// megabyte of headroom there is barely two frames.
#[cfg(not(debug_assertions))]
const STACK_RESERVE_BYTES: usize = 1024 * 1024;
#[cfg(debug_assertions)]
const STACK_RESERVE_BYTES: usize = 24 * 1024 * 1024;

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

thread_local! {
    /// How many stacks deep this evaluation already is (see `grow_interp_stack`).
    ///
    /// Thread-local, and a fresh thread starts at zero — so the count is handed
    /// across explicitly when a segment is added, or an infinite recursion would
    /// grow forever.
    static STACK_SEGMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Live host stack one interpreted program may chain together.
///
/// A segment is fully spent before the next is added, so this is committed
/// memory, not just reserved address space — the cap is what keeps a runaway
/// recursion a diagnostic instead of a machine swapping itself to death. A
/// gigabyte buys around 30,000 Rask frames in release, which covers the things
/// that legitimately recurse: a descent over nested JSON, a quicksort on a
/// nearly-sorted list, a naive fibonacci.
const MAX_INTERP_STACK_BYTES: usize = 1024 * 1024 * 1024;

/// How many stacks that works out to.
///
/// Expressed as a budget rather than a count so the two profiles agree on the
/// memory rather than on the number of threads — a debug frame is ~17× an
/// optimized one, so a debug segment is correspondingly larger and there are
/// correspondingly fewer of them.
const MAX_STACK_SEGMENTS: usize = {
    let n = MAX_INTERP_STACK_BYTES / INTERP_STACK_BYTES;
    if n < 2 { 2 } else { n }
};

/// Has the chain of stacks reached its cap?
pub(crate) fn stack_segments_exhausted() -> bool {
    STACK_SEGMENTS.get() + 1 >= MAX_STACK_SEGMENTS
}

/// Continue evaluating on a fresh stack.
///
/// The interpreter spends one host frame per Rask call and those frames are
/// large — around 30 KB, because `eval_expr` is a single match over 80 kinds and
/// Rust sizes a frame for the union of every arm's locals. 16 MiB therefore
/// buys only ~465 Rask calls, and a program that recursed deeper than that used
/// to die: first as a SIGABRT with no message, then (once the guard landed) as
/// an R0023 diagnostic. Both are wrong answers — the same program compiled
/// natively recurses into the millions, and the interpreter is supposed to be
/// the reference for what the answer is.
///
/// So instead of refusing, the call continues on a thread with a whole new
/// stack, and the old one waits in `join`. The recursion is unchanged as far as
/// the program can tell — same interpreter, same environment, same values,
/// which travel because they're already `Arc`-backed for concurrency. What
/// changes is which host stack the frames land on.
///
/// The cost lands once per ~465 Rask frames: one thread spawn, and one OS
/// thread parked in `join` per live segment. Frame size is still worth
/// shrinking (#759) — it decides how often this happens — but it's no longer
/// the difference between running and not.
pub(crate) fn grow_interp_stack<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    let next = STACK_SEGMENTS.get() + 1;
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(INTERP_STACK_BYTES)
            .spawn_scoped(s, || {
                mark_stack_base();
                STACK_SEGMENTS.set(next);
                f()
            })
            .expect("failed to spawn interpreter thread")
            .join()
            // The child's panic is the program's panic — resume it here rather
            // than turning it into a different one.
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
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
