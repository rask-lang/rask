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

// Retrieve the panic message after longjmp. Transfers ownership to caller.
char *rask_panic_take_message(void) {
    char *msg = panic_ctx.message;
    panic_ctx.message = NULL;
    return msg;
}

// ─── Backtrace ─────────────────────────────────────────────

static void print_backtrace(void) {
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

    if (panic_ctx.active) {
        // Spawned task — store message and longjmp back to task entry
        panic_ctx.message = msg ? strdup(msg) : strdup("(unknown panic)");
        panic_ctx.active = 0;
        longjmp(panic_ctx.buf, 1);
    }

    // Main thread or no handler — print and abort
    fprintf(stderr, "panic: %s\n", msg ? msg : "(unknown panic)");
    print_backtrace();
    abort();
}

_Noreturn void rask_panic_at(const char *file, int32_t line, int32_t col,
                             const char *msg) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf), "%s:%d:%d: %s",
             file ? file : "<unknown>", line, col,
             msg ? msg : "(unknown panic)");

    if (panic_ctx.active) {
        panic_ctx.message = strdup(buf);
        panic_ctx.active = 0;
        longjmp(panic_ctx.buf, 1);
    }

    fprintf(stderr, "panic at %s\n", buf);
    print_backtrace();
    abort();
}

_Noreturn void rask_panic_fmt(const char *fmt, ...) {
    char buf[RASK_PANIC_MSG_MAX];
    va_list args;
    va_start(args, fmt);
    vsnprintf(buf, sizeof(buf), fmt, args);
    va_end(args);
    rask_panic(buf);
}
