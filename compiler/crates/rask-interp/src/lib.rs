// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tree-walk interpreter for the Rask language.
//!
//! Executes the AST directly without compilation.

mod value;
mod ptr;
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
///
/// Answers `Err` rather than panicking when the target has no threads at all.
/// Every way a Rask program can ask for a thread funnels through here —
/// `using Multitasking`, `using ThreadPool`, `Thread.spawn`, `spawn_raw`, a
/// pool submission — so this is the one place that has to know, and the reason
/// it's fallible: on wasm32 `Builder::spawn` answers `Unsupported`, and the
/// `expect` this used to end with trapped the whole interpreter instead of
/// failing the call (#1172).
pub(crate) fn spawn_interp_thread<F, T>(f: F) -> Result<std::thread::JoinHandle<T>, RuntimeError>
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
        .map_err(|e| {
            if !HAS_THREADS {
                // The browser playground. Worded like the OS-backed modules'
                // refusals (`fs module not available in browser playground`),
                // because it is the same situation from the reader's side.
                RuntimeError::Generic(
                    "threads not available in browser playground".to_string(),
                )
            } else {
                RuntimeError::Generic(format!("could not start a thread: {e}"))
            }
        })
}


/// Stack for a thread running interpreted Rask code.
///
/// Sized so that both profiles reach a comparable Rask recursion depth (~465),
/// because a debug frame costs roughly 17× an optimized one.
#[cfg(all(not(debug_assertions), not(target_family = "wasm")))]
pub(crate) const INTERP_STACK_BYTES: usize = 16 * 1024 * 1024;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
pub(crate) const INTERP_STACK_BYTES: usize = 288 * 1024 * 1024;

/// On wasm the stack is whatever the linker reserved, so this is a measurement
/// rather than a request — wasm-ld's default is 1 MiB. Whoever passes
/// `-zstack-size` knows better and says so through `set_stack_bytes`; this is
/// the floor for anyone who doesn't. Reading it low is the safe direction: the
/// guard refuses to recurse slightly before the real stack runs out, which is a
/// diagnostic instead of a trap.
#[cfg(target_family = "wasm")]
pub(crate) const INTERP_STACK_BYTES: usize = 1024 * 1024;

/// How much stack the interpreter's entry point has to work with.
#[cfg(not(target_family = "wasm"))]
fn interp_stack_bytes() -> usize {
    INTERP_STACK_BYTES
}

#[cfg(target_family = "wasm")]
thread_local! {
    static WASM_STACK_BYTES: std::cell::Cell<usize> =
        const { std::cell::Cell::new(INTERP_STACK_BYTES) };
}

#[cfg(target_family = "wasm")]
fn interp_stack_bytes() -> usize {
    WASM_STACK_BYTES.get()
}

/// Can this target run a thread?
///
/// wasm32-unknown-unknown cannot: `thread::Builder::spawn` answers
/// `Unsupported`. Only `spawn_interp_thread` reads this, to tell "no threads
/// here" apart from an OS that ran out of them — the two want different
/// messages, and a Rask program can only tell the difference from the wording.
///
/// It deliberately isn't a guard at the places a program asks for a thread.
/// An earlier version of this put the check on the `using Multitasking` arm
/// and claimed that covered every route to `spawn`. It didn't:
/// `Thread.spawn`, `spawn_raw`, `using ThreadPool` and pool submissions each
/// reach the spawn on their own, and all four still trapped.
pub(crate) const HAS_THREADS: bool = !cfg!(target_arch = "wasm32");

/// Does this target have a clock?
///
/// wasm32-unknown-unknown does not: `Instant::now` and `SystemTime::now` panic
/// there, and in the browser playground a panic is a trap that takes the whole
/// interpreter with it rather than failing one call. So the clock reads a
/// program can observe check this first and refuse the way the OS-backed
/// modules already do — `fs module not available in browser playground` — while
/// anything that only wanted entropy uses `seed_entropy` instead.
pub(crate) const HAS_CLOCK: bool = !cfg!(target_arch = "wasm32");

/// Entropy for seeding a generator, on any target.
///
/// The clock where there is one; a call counter where there isn't. A playground
/// that answers with the same "random" number every single time is worse than
/// one seeded by how many numbers have been asked for.
pub(crate) fn seed_entropy() -> u64 {
    if HAS_CLOCK {
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            return d.as_nanos() as u64;
        }
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    // Odd multiplier so successive seeds don't land in the same neighbourhood.
    COUNTER
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(0x1234_5678_9abc_def1)
}

/// Tell the interpreter how much stack the linker reserved.
///
/// Only wasm needs this: every other target spawns its own thread and picks the
/// size itself. Call it before running anything, from whatever crate owns the
/// `-zstack-size` link argument — `rask-wasm`'s build script sets both from one
/// constant.
#[cfg(target_family = "wasm")]
pub fn set_stack_bytes(bytes: usize) {
    WASM_STACK_BYTES.set(bytes);
}

/// Stack left in reserve when the interpreter refuses to recurse further.
///
/// The refusal itself has work to do: unwind out of every frame, build the
/// diagnostic, format it with its source snippet. That has to fit in what's
/// left, or reporting the overflow overflows. Scaled with the profile for the
/// same reason the stack size is — a debug frame is ~17× an optimized one, so a
/// megabyte of headroom there is barely two frames.
#[cfg(all(not(debug_assertions), not(target_family = "wasm")))]
const STACK_RESERVE_BYTES: usize = 1024 * 1024;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
const STACK_RESERVE_BYTES: usize = 24 * 1024 * 1024;
/// Scaled to the 1 MiB wasm stack: a quarter of it, so the refusal has room to
/// unwind and format itself the way it does everywhere else.
#[cfg(target_family = "wasm")]
const STACK_RESERVE_BYTES: usize = 256 * 1024;

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
    used != 0 && used + STACK_RESERVE_BYTES >= interp_stack_bytes()
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
#[cfg(not(target_family = "wasm"))]
const MAX_STACK_SEGMENTS: usize = {
    let n = MAX_INTERP_STACK_BYTES / INTERP_STACK_BYTES;
    if n < 2 { 2 } else { n }
};

/// One, on wasm: a segment is a thread, and there are none. Running out of
/// stack there is `RecursionTooDeep` with nowhere to continue — which is the
/// answer `call_function` already gives once the budget is spent.
#[cfg(target_family = "wasm")]
const MAX_STACK_SEGMENTS: usize = 1;

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
#[cfg(not(target_family = "wasm"))]
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
/// A single-stack target has one answer: continue where we are. Unreachable in
/// practice — `MAX_STACK_SEGMENTS` is 1 on wasm, so `call_function` answers
/// `RecursionTooDeep` before it ever asks for another segment.
#[cfg(target_family = "wasm")]
pub(crate) fn grow_interp_stack<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    f()
}

#[cfg(not(target_family = "wasm"))]
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

/// On wasm there is one stack and its size was fixed by the linker, so the
/// entry point's job is only to record where it starts. Spawning a thread here
/// is what the browser playground used to do: `spawn_scoped` answers
/// `Unsupported`, the `expect` traps, and because a trap skips every
/// destructor, wasm-bindgen's borrow of `Playground` is never given back —
/// so the first `println` bricked the instance and every click afterwards
/// reported "recursive use of an object" instead (#1172).
#[cfg(target_family = "wasm")]
pub(crate) fn on_interp_stack<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    mark_stack_base();
    f()
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
