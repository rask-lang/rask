// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Phase A thread primitives (conc.strategy/A1).
//
// One OS thread per spawn. Panics in spawned tasks are caught via
// setjmp/longjmp and propagated as JoinError on join.
//
// TaskHandle lifecycle:
//   spawn → [running] → join/detach/cancel → [consumed]
//
// The shared TaskState is refcounted: one ref for the handle, one for
// the running thread. Last one to drop frees it.

#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <pthread.h>
#include <stdatomic.h>
#include <setjmp.h>

// ─── Internal declarations from panic.c ────────────────────

extern jmp_buf *rask_panic_jmpbuf(void);
extern void     rask_panic_activate(void);
extern char    *rask_panic_take_message(void);

// ─── Task state (shared between handle and thread) ─────────

#define RASK_TASK_RUNNING   0
#define RASK_TASK_OK        1
#define RASK_TASK_PANICKED  2
#define RASK_TASK_CANCELLED 3

typedef struct {
    atomic_int   refcount;
    atomic_int   status;
    atomic_int   cancel_flag;
    char        *panic_msg;     // set on panic, owned by state
    pthread_t    thread;
} RaskTaskState;

struct RaskTaskHandle {
    RaskTaskState *state;
};

// Per-thread cancel flag pointer (points into the task's state).
static __thread atomic_int *current_cancel_flag;

static RaskTaskState *state_new(void) {
    RaskTaskState *s = (RaskTaskState *)rask_alloc(sizeof(RaskTaskState));
    atomic_init(&s->refcount, 2);  // handle + thread
    atomic_init(&s->status, RASK_TASK_RUNNING);
    atomic_init(&s->cancel_flag, 0);
    s->panic_msg = NULL;
    return s;
}

static void state_release(RaskTaskState *s) {
    if (atomic_fetch_sub_explicit(&s->refcount, 1, memory_order_acq_rel) == 1) {
        if (s->panic_msg) rask_free(s->panic_msg);
        rask_free(s);
    }
}

// ─── Thread entry point ────────────────────────────────────

typedef struct {
    RaskTaskFn     func;
    void          *env;
    RaskTaskState *state;
} TaskEntry;

static void *task_thread_entry(void *arg) {
    TaskEntry *entry = (TaskEntry *)arg;
    RaskTaskState *state = entry->state;
    RaskTaskFn func = entry->func;
    void *env = entry->env;
    rask_free(entry);

    // Set up cancel flag for this thread
    current_cancel_flag = &state->cancel_flag;

    // Install panic handler
    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();

    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        func(env);
        atomic_store_explicit(&state->status, RASK_TASK_OK, memory_order_release);
    } else {
        // Returned via longjmp from rask_panic
        state->panic_msg = rask_panic_take_message();
        atomic_store_explicit(&state->status, RASK_TASK_PANICKED,
                              memory_order_release);
    }

    rask_panic_remove();
    current_cancel_flag = NULL;
    state_release(state);
    return NULL;
}

// ─── Public API ────────────────────────────────────────────

RaskTaskHandle *rask_task_spawn(RaskTaskFn func, void *env) {
    RaskTaskState *state = state_new();

    TaskEntry *entry = (TaskEntry *)rask_alloc(sizeof(TaskEntry));
    entry->func  = func;
    entry->env   = env;
    entry->state = state;

    int err = pthread_create(&state->thread, NULL, task_thread_entry, entry);
    if (err != 0) {
        rask_free(entry);
        state_release(state);
        state_release(state); // drop both refs
        rask_panic_fmt("spawn failed: pthread_create returned %d", err);
    }

    RaskTaskHandle *h = (RaskTaskHandle *)rask_alloc(sizeof(RaskTaskHandle));
    h->state = state;
    return h;
}

int64_t rask_task_join(RaskTaskHandle *h, char **msg_out) {
    if (!h || !h->state) {
        rask_panic("join on consumed TaskHandle");
    }

    RaskTaskState *state = h->state;
    pthread_join(state->thread, NULL);

    int status = atomic_load_explicit(&state->status, memory_order_acquire);
    int64_t result = 0;

    if (status == RASK_TASK_PANICKED) {
        if (msg_out) {
            *msg_out = state->panic_msg;
            state->panic_msg = NULL; // transfer ownership
        }
        result = -1;
    } else if (msg_out) {
        *msg_out = NULL;
    }

    state_release(state);
    rask_free(h);
    return result;
}

void rask_task_detach(RaskTaskHandle *h) {
    if (!h || !h->state) {
        rask_panic("detach on consumed TaskHandle");
    }

    RaskTaskState *state = h->state;
    pthread_detach(state->thread);
    state_release(state);
    rask_free(h);
}

int64_t rask_task_cancel(RaskTaskHandle *h, char **msg_out) {
    if (!h || !h->state) {
        rask_panic("cancel on consumed TaskHandle");
    }

    // Set cancel flag — task checks via rask_task_cancelled()
    atomic_store_explicit(&h->state->cancel_flag, 1, memory_order_release);

    // Wait for completion
    return rask_task_join(h, msg_out);
}

int8_t rask_task_cancelled(void) {
    if (!current_cancel_flag) return 0;
    return atomic_load_explicit(current_cancel_flag, memory_order_acquire) ? 1 : 0;
}

void rask_sleep_ns(int64_t ns) {
    if (ns <= 0) return;
    struct timespec ts;
    ts.tv_sec  = ns / 1000000000LL;
    ts.tv_nsec = ns % 1000000000LL;
    nanosleep(&ts, NULL);
}

// Sleep for the given number of milliseconds.
int64_t rask_time_sleep_ms(int64_t ms) {
    rask_sleep_ns(ms * 1000000LL);
    return 0;
}

// ─── Codegen wrappers ──────────────────────────────────────
// Closure-aware spawn for the MIR codegen layer.
// Closure layout: [func_ptr(8) | captures...]
// The wrapper extracts func/env, runs the task, and frees the closure.

typedef struct {
    RaskTaskFn     func;
    void          *env;
    void          *alloc_base;  // closure allocation to free after task
} RaskSpawnCtx;

static void closure_spawn_entry(void *arg) {
    RaskSpawnCtx *ctx = (RaskSpawnCtx *)arg;
    RaskTaskFn func = ctx->func;
    void *env = ctx->env;
    void *alloc_base = ctx->alloc_base;
    rask_free(ctx);

    func(env);
    rask_free(alloc_base);
}

RaskTaskHandle *rask_closure_spawn(void *closure_ptr) {
    void (*func)(void *) = *(void (**)(void *))(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskSpawnCtx *ctx = (RaskSpawnCtx *)rask_alloc(sizeof(RaskSpawnCtx));
    ctx->func = func;
    ctx->env = env;
    ctx->alloc_base = closure_ptr;

    return rask_task_spawn(closure_spawn_entry, ctx);
}

int64_t rask_task_join_simple(void *h) {
    return rask_task_join((RaskTaskHandle *)h, NULL);
}
