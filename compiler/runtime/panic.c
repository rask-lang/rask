// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Structured panic handler.
//
// Main thread: panics print message + optional backtrace, then abort.
// Spawned tasks: panics longjmp back to the task entry point, storing the
// message for propagation as JoinError::Panicked(msg) on join.
//
// Thread-local storage holds the per-task panic context (jmp_buf + message).

#include "rask_runtime.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdarg.h>
#include <setjmp.h>
#include <stdatomic.h>

#ifdef __linux__
#include <execinfo.h>
#define RASK_HAS_BACKTRACE 1
#else
#define RASK_HAS_BACKTRACE 0
#endif

// ─── Per-thread panic context ──────────────────────────────

struct RaskPanicCtx {
    jmp_buf buf;
    int     active;       // handler installed?
    char   *message;      // heap-allocated on panic
};

static __thread struct RaskPanicCtx panic_ctx;

RaskPanicCtx *rask_panic_install(void) {
    panic_ctx.active  = 0;
    panic_ctx.message = NULL;
    return &panic_ctx;
}

void rask_panic_remove(void) {
    if (panic_ctx.message) {
        free(panic_ctx.message);
        panic_ctx.message = NULL;
    }
    panic_ctx.active = 0;
}

// Called by thread.c task entry — returns the stored jmp_buf for setjmp.
jmp_buf *rask_panic_jmpbuf(void) {
    return &panic_ctx.buf;
}

// Mark the handler as active (called after setjmp returns 0).
void rask_panic_activate(void) {
    panic_ctx.active = 1;
}

// ─── FFI boundary (ctrl.panic/A1) ──────────────────────────
//
// An `extern "C"` function with a body is called from outside Rask, so the C
// frames between the caller and the panic are frames the runtime doesn't own.
// The task-unwind path would longjmp straight over them to the enclosing task's
// setjmp — skipping whatever the C caller had on the stack. A1 says don't:
// abort at the boundary instead.
//
// A depth counter rather than a saved/restored jmp_buf, because there is no
// resuming here — the panic never unwinds past this point, it ends the process.
// Nesting is counted so a Rask → C → Rask → C → Rask chain still aborts on the
// innermost export, and a normal return unwinds the count one level at a time.
static __thread int ffi_boundary_depth;

void rask_ffi_boundary_enter(void) {
    ffi_boundary_depth++;
}

void rask_ffi_boundary_exit(void) {
    if (ffi_boundary_depth > 0) ffi_boundary_depth--;
}

int rask_in_ffi_boundary(void) {
    return ffi_boundary_depth > 0;
}

// Report and abort. Called from both panic entry points before either decides
// to longjmp.
static void ffi_boundary_abort(const char *msg) {
    rask_print_unlock_all();
    fflush(stdout);
    fprintf(stderr,
            "panic crossed an FFI boundary: %s\n"
            "  an `extern \"C\"` function cannot unwind into its C caller's frames, "
            "so the process aborts here (ctrl.panic/A1)\n",
            msg ? msg : "(unknown panic)");
    fflush(stderr);
    abort();
}

// Retrieve the panic message after longjmp. Transfers ownership to caller.
char *rask_panic_take_message(void) {
    char *msg = panic_ctx.message;
    panic_ctx.message = NULL;
    return msg;
}

// ─── Task ids (ctrl.panic/F1) ───────────────────────────────
//
// One counter shared by both task backends (thread.c OS threads, green.c
// fibers) so ids stay unique regardless of which spawns which. 0 means
// "no task" — main never has one. thread.c/green.c set the current thread's
// id around each task invocation; panic output prepends it when set.

static atomic_llong next_task_id = 1;
static __thread int64_t tl_task_id = 0;

int64_t rask_next_task_id(void) {
    return atomic_fetch_add_explicit(&next_task_id, 1, memory_order_relaxed);
}

void rask_panic_set_task_id(int64_t id) {
    tl_task_id = id;
}

// ─── Ensure hooks (LIFO cleanup stack) ─────────────────────
//
// Per-thread linked list of scheduled cleanups. Codegen pushes one per
// `ensure` and pops it on normal scope exit; `rask_ensure_run_all` drains
// what's left when the stack unwinds on panic (ctrl.panic/U1). Lives here,
// in the always-linked TU, so both the main thread and every backend
// (thread.c OS tasks, green.c fibers) share one stack. green.c reaches it
// through the take/set accessors below instead of owning its own copy.

typedef struct EnsureHook {
    RaskEnsureFn       fn;
    void              *ctx;
    struct EnsureHook *next;
} EnsureHook;

static __thread EnsureHook *tl_ensure_stack = NULL;

// Set while draining hooks, so a panic raised by the unwind machinery
// itself (A1) doesn't recursively re-drain the stack.
static __thread int tl_in_unwind = 0;

void rask_ensure_push(RaskEnsureFn fn, void *ctx) {
    EnsureHook *hook = (EnsureHook *)malloc(sizeof(EnsureHook));
    if (!hook) return;
    hook->fn   = fn;
    hook->ctx  = ctx;
    hook->next = tl_ensure_stack;
    tl_ensure_stack = hook;
}

void rask_ensure_pop(void) {
    EnsureHook *hook = tl_ensure_stack;
    if (!hook) return;
    tl_ensure_stack = hook->next;
    free(hook);
}

// Save/restore the current thread's stack head. Lets a worker thread that
// multiplexes fibers (green.c) park one task's hooks and resume another's
// without knowing the EnsureHook layout.
void *rask_ensure_stack_take(void) {
    void *head = tl_ensure_stack;
    tl_ensure_stack = NULL;
    return head;
}

void rask_ensure_stack_set(void *head) {
    tl_ensure_stack = (EnsureHook *)head;
}

// Run every scheduled ensure in LIFO order during unwind. Each body runs
// even if an earlier one panicked (E2); the first panic is already the
// task's panic, so a panic raised by a body here is contained and reported
// to stderr as a secondary panic (E3).
void rask_ensure_run_all(void) {
    if (tl_in_unwind) {
        // A1: panic inside the unwind machinery — don't recurse.
        return;
    }
    tl_in_unwind = 1;
    while (tl_ensure_stack) {
        EnsureHook *hook = tl_ensure_stack;
        tl_ensure_stack = hook->next;
        RaskEnsureFn fn = hook->fn;
        void *ctx = hook->ctx;
        free(hook);
        if (!fn) continue;

        // Contain a panic thrown by this ensure body: install a local
        // handler so rask_panic longjmps back here instead of escaping.
        struct RaskPanicCtx saved = panic_ctx;
        if (setjmp(panic_ctx.buf) == 0) {
            panic_ctx.active  = 1;
            panic_ctx.message = NULL;
            fn(ctx);
        } else {
            char *m = panic_ctx.message;
            // F1: task id prefix when a runtime task is active.
            if (tl_task_id > 0) {
                fprintf(stderr, "task %lld secondary panic during unwind: %s\n",
                        (long long)tl_task_id, m ? m : "(unknown panic)");
            } else {
                fprintf(stderr, "secondary panic during unwind: %s\n",
                        m ? m : "(unknown panic)");
            }
            free(m);
        }
        panic_ctx = saved;
    }
    tl_in_unwind = 0;
}

// ─── Held access (ctrl.panic/U3, U4) ───────────────────────
//
// The release half of every `with`-block and inline sync access. Codegen emits
// the release inline, so a panic in the middle of the block jumped past it and
// left the lock held: the next acquire blocked forever, and the first thing to
// try one is usually an ensure body running during the same unwind. Each
// acquire registers its release here; the matching release deregisters it; the
// panic path drains what's left, before the ensures, so a cleanup that touches
// the same box can take the lock.
//
// Keyed by handle rather than popping blindly off the top: acquire/release pairs
// nest LIFO in practice (conc.sync/DL1 rejects nested `with` on sync boxes
// outright), but a release that didn't match the top would otherwise drop
// somebody else's entry and leave a real lock held.

typedef struct HeldAccess {
    RaskReleaseFn      fn;
    int64_t            handle;
    struct HeldAccess *next;
} HeldAccess;

static __thread HeldAccess *tl_held_access = NULL;

void rask_access_push(RaskReleaseFn fn, int64_t handle) {
    HeldAccess *held = (HeldAccess *)rask_alloc(sizeof(HeldAccess));
    if (!held) return;
    held->fn     = fn;
    held->handle = handle;
    held->next   = tl_held_access;
    tl_held_access = held;
}

void rask_access_pop(int64_t handle) {
    HeldAccess **link = &tl_held_access;
    while (*link) {
        if ((*link)->handle == handle) {
            HeldAccess *dead = *link;
            *link = dead->next;
            rask_free(dead);
            return;
        }
        link = &(*link)->next;
    }
}

void rask_access_release_all(void) {
    while (tl_held_access) {
        HeldAccess *held = tl_held_access;
        tl_held_access = held->next;
        RaskReleaseFn fn = held->fn;
        int64_t handle = held->handle;
        rask_free(held);
        if (fn) fn(handle);
    }
}

void *rask_access_stack_take(void) {
    void *head = tl_held_access;
    tl_held_access = NULL;
    return head;
}

void rask_access_stack_set(void *head) {
    tl_held_access = (HeldAccess *)head;
}

// ─── Backtrace ─────────────────────────────────────────────

static void print_backtrace(void) {
    // F2: backtraces are opt-in and not part of the deterministic surface.
    if (!getenv("RASK_BACKTRACE")) {
        return;
    }
#if RASK_HAS_BACKTRACE
    void *frames[64];
    int n = backtrace(frames, 64);
    if (n > 0) {
        fprintf(stderr, "backtrace:\n");
        backtrace_symbols_fd(frames, n, 2); // fd 2 = stderr
    }
#endif
}

// ─── Thread-local source location for runtime panics ──────
// Codegen calls rask_set_panic_location() before any runtime function
// that can panic. rask_panic() checks these and includes file:line:col.

static __thread const char *panic_loc_file;
static __thread int32_t     panic_loc_line;
static __thread int32_t     panic_loc_col;

void rask_set_panic_location(const char *file, int32_t line, int32_t col) {
    panic_loc_file = file;
    panic_loc_line = line;
    panic_loc_col  = col;
}

// ─── Panic entry points ────────────────────────────────────

_Noreturn void rask_panic(const char *msg) {
    // If codegen set a source location, use rask_panic_at instead
    if (panic_loc_file && panic_loc_line > 0) {
        const char *f = panic_loc_file;
        int32_t l = panic_loc_line;
        int32_t c = panic_loc_col;
        panic_loc_file = NULL;
        panic_loc_line = 0;
        panic_loc_col  = 0;
        rask_panic_at(f, l, c, msg);
    }

    // A1: inside an exported symbol there are C frames between here and any
    // handler, so there is nothing to unwind to. Abort before the ensures run —
    // they'd be running during an unwind that isn't going to happen.
    if (ffi_boundary_depth > 0) {
        ffi_boundary_abort(msg);
    }

    // A panic raised mid-line — from inside a print argument, or from an
    // ensure that prints — would longjmp past the unlock that balances
    // codegen's print lock and deadlock every later print on other threads.
    rask_print_unlock_all();

    // U3/U4: release the locks and borrows this task still holds. Before the
    // ensures, not after — a cleanup that touches the same box has to be able to
    // take the lock, and an ensure blocking on it is a hang, not a failure.
    rask_access_release_all();

    // Unwind: run scheduled ensures for the dying task (U1/E2/E3). The primary
    // panic message is set afterward, so it wins over any secondary.
    rask_ensure_run_all();

    if (panic_ctx.active) {
        // Spawned task — store message and longjmp back to task entry
        panic_ctx.message = msg ? strdup(msg) : strdup("(unknown panic)");
        panic_ctx.active = 0;
        longjmp(panic_ctx.buf, 1);
    }

    // Main task — a panic escaping main exits 101 after unwind (P4).
    fprintf(stderr, "panic: %s\n", msg ? msg : "(unknown panic)");
    print_backtrace();
    exit(101);
}

_Noreturn void rask_panic_at(const char *file, int32_t line, int32_t col,
                             const char *msg) {
    // `file:line:` — no column. The message is stored and handed to user code
    // through `JoinError.Panicked(msg)`, so both backends have to agree on it,
    // and they point their columns at different sub-expressions: codegen's is
    // the last MIR statement (the panic's argument), the interpreter's is the
    // call. The line is the useful half either way, and ER15's `origin`
    // strings are already file:line (#748).
    (void)col;
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf), "%s:%d: %s",
             file ? file : "<unknown>", line,
             msg ? msg : "(unknown panic)");

    if (ffi_boundary_depth > 0) {
        ffi_boundary_abort(buf);
    }

    rask_print_unlock_all();
    rask_access_release_all();
    rask_ensure_run_all();

    if (panic_ctx.active) {
        panic_ctx.message = strdup(buf);
        panic_ctx.active = 0;
        longjmp(panic_ctx.buf, 1);
    }

    fprintf(stderr, "panic at %s\n", buf);
    print_backtrace();
    exit(101);
}

_Noreturn void rask_panic_fmt(const char *fmt, ...) {
    char buf[RASK_PANIC_MSG_MAX];
    va_list args;
    va_start(args, fmt);
    vsnprintf(buf, sizeof(buf), fmt, args);
    va_end(args);
    rask_panic(buf);
}
