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
extern int64_t  rask_next_task_id(void);
extern void     rask_panic_set_task_id(int64_t id);

// ─── Task state (shared between handle and thread) ─────────

#define RASK_TASK_RUNNING   0
#define RASK_TASK_OK        1
#define RASK_TASK_PANICKED  2
#define RASK_TASK_CANCELLED 3

typedef struct RaskTaskState {
    atomic_int   refcount;
    atomic_int   status;
    atomic_int   cancel_flag;
    char        *panic_msg;     // set on panic, owned by state
    int64_t      result;        // task body's return value, read by join
    pthread_t    thread;        // valid only when !pooled

    // A pooled job shares a worker with other jobs, so there is no thread of
    // its own to pthread_join. join() waits for the status to leave RUNNING
    // instead, and the worker signals done_cond when it sets it.
    int              pooled;
    pthread_cond_t   done_cond;

    // O4: guards `detached` and the decision to print an unjoined panic to
    // stderr. Only the panic path and detach() ever touch this — the normal
    // success path never contends on it. Also the mutex done_cond waits on.
    pthread_mutex_t report_lock;
    int              detached;
    int              counted_detached;  // in detached_outstanding

    int64_t      task_id;        // ctrl.panic/F1
} RaskTaskState;

struct RaskTaskHandle {
    RaskTaskState *state;
};

// Per-thread cancel flag pointer (points into the task's state).
static __thread atomic_int *current_cancel_flag;

// O4: detached tasks still running. A detached task's panic *must* reach
// stderr, and a task racing process exit doesn't satisfy that — the report just
// vanishes, which is the failure O4 exists to prevent. `main` waits for this to
// reach zero before returning (rask_await_detached_tasks).
static atomic_int detached_outstanding;

static RaskTaskState *state_new(void) {
    RaskTaskState *s = (RaskTaskState *)rask_alloc(sizeof(RaskTaskState));
    atomic_init(&s->refcount, 2);  // handle + thread
    atomic_init(&s->status, RASK_TASK_RUNNING);
    atomic_init(&s->cancel_flag, 0);
    s->panic_msg = NULL;
    s->result = 0;
    s->pooled = 0;
    pthread_cond_init(&s->done_cond, NULL);
    pthread_mutex_init(&s->report_lock, NULL);
    s->detached = 0;
    s->counted_detached = 0;
    s->task_id = rask_next_task_id();
    return s;
}

static void state_release(RaskTaskState *s) {
    if (atomic_fetch_sub_explicit(&s->refcount, 1, memory_order_acq_rel) == 1) {
        if (s->panic_msg) rask_free(s->panic_msg);
        pthread_cond_destroy(&s->done_cond);
        pthread_mutex_destroy(&s->report_lock);
        rask_free(s);
    }
}

// ─── Thread entry point ────────────────────────────────────

typedef struct {
    RaskTaskFn     func;
    void          *env;
    RaskTaskState *state;
} TaskEntry;

// Run one task body to completion and record how it ended. Shared by the
// one-thread-per-spawn path below and by the pool workers in threadpool.c,
// which run many of these back to back on the same thread.
void rask_task_run_body(RaskTaskState *state, RaskTaskFn func, void *env) {
    // Set up cancel flag for this thread
    current_cancel_flag = &state->cancel_flag;

    // Install panic handler
    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();
    rask_panic_set_task_id(state->task_id); // F1

    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        state->result = func(env);
        atomic_store_explicit(&state->status, RASK_TASK_OK, memory_order_release);
    } else {
        // Returned via longjmp from rask_panic
        state->panic_msg = rask_panic_take_message();
        atomic_store_explicit(&state->status, RASK_TASK_PANICKED,
                              memory_order_release);

        // O4: a detached task's panic must reach stderr — nobody is going to
        // join this handle and read the message otherwise. F1: task id
        // prefix, since a runtime task is what's panicking here.
        pthread_mutex_lock(&state->report_lock);
        if (state->detached && state->panic_msg) {
            fprintf(stderr, "task %lld panic at %s\n",
                    (long long)state->task_id, state->panic_msg);
            rask_free(state->panic_msg);
            state->panic_msg = NULL;
        }
        pthread_mutex_unlock(&state->report_lock);
    }

    // O4: this task is done reporting either way, so `main` no longer has to
    // wait for it. Outside the panic branch — a detached task that returns
    // normally has to clear its count too, or the wait never ends.
    pthread_mutex_lock(&state->report_lock);
    if (state->counted_detached) {
        state->counted_detached = 0;
        atomic_fetch_sub_explicit(&detached_outstanding, 1, memory_order_release);
    }
    pthread_mutex_unlock(&state->report_lock);

    rask_panic_set_task_id(0);
    rask_panic_remove();
    current_cancel_flag = NULL;

    // A pooled job has no thread for join() to wait on, so waking the waiter
    // is what "finished" means for it.
    if (state->pooled) {
        pthread_mutex_lock(&state->report_lock);
        pthread_cond_broadcast(&state->done_cond);
        pthread_mutex_unlock(&state->report_lock);
    }
}

static void *task_thread_entry(void *arg) {
    TaskEntry *entry = (TaskEntry *)arg;
    RaskTaskState *state = entry->state;
    RaskTaskFn func = entry->func;
    void *env = entry->env;
    rask_free(entry);

    rask_task_run_body(state, func, env);

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
    if (state->pooled) {
        // No thread of its own — wait for the worker to finish this job.
        pthread_mutex_lock(&state->report_lock);
        while (atomic_load_explicit(&state->status, memory_order_acquire)
               == RASK_TASK_RUNNING) {
            pthread_cond_wait(&state->done_cond, &state->report_lock);
        }
        pthread_mutex_unlock(&state->report_lock);
    } else {
        pthread_join(state->thread, NULL);
    }

    int status = atomic_load_explicit(&state->status, memory_order_acquire);
    int64_t result;

    if (status == RASK_TASK_PANICKED) {
        if (msg_out) {
            *msg_out = state->panic_msg;
            state->panic_msg = NULL; // transfer ownership
        }
        result = -1;
    } else {
        result = state->result;
        if (msg_out) *msg_out = NULL;
    }

    state_release(state);
    rask_free(h);
    return result;
}

// Join, splitting "how it ended" from "what it produced". The old shape folded
// both into one int64_t, so a task returning -1 read back as a panic and a task
// returning 42 read back as 0 (the value was never captured at all).
int64_t rask_task_join_outcome(void *handle, int64_t *value_out, RaskStr *msg_out) {
    RaskTaskHandle *h = (RaskTaskHandle *)handle;
    if (!h || !h->state) {
        rask_panic("join on consumed TaskHandle");
    }

    int cancelled = atomic_load_explicit(&h->state->cancel_flag, memory_order_acquire);

    char *msg = NULL;
    int64_t value = rask_task_join(h, &msg);

    if (msg) {
        rask_string_from(msg_out, msg);
        rask_free(msg);
        if (value_out) *value_out = 0;
        return RASK_JOIN_PANICKED;
    }

    rask_string_new(msg_out);
    if (cancelled) {
        if (value_out) *value_out = 0;
        return RASK_JOIN_CANCELLED;
    }
    if (value_out) *value_out = value;
    return RASK_JOIN_OK;
}

void rask_task_detach(RaskTaskHandle *h) {
    if (!h || !h->state) {
        rask_panic("detach on consumed TaskHandle");
    }

    RaskTaskState *state = h->state;

    pthread_mutex_lock(&state->report_lock);
    state->detached = 1;
    if (atomic_load_explicit(&state->status, memory_order_acquire) == RASK_TASK_RUNNING) {
        atomic_fetch_add_explicit(&detached_outstanding, 1, memory_order_relaxed);
        state->counted_detached = 1;
    }
    // O4: the task may have already panicked and finished before detach()
    // ran — same "report now, nobody will join" rule applies.
    if (atomic_load_explicit(&state->status, memory_order_acquire) == RASK_TASK_PANICKED
        && state->panic_msg) {
        fprintf(stderr, "task %lld panic at %s\n",
                (long long)state->task_id, state->panic_msg);
        rask_free(state->panic_msg);
        state->panic_msg = NULL;
    }
    pthread_mutex_unlock(&state->report_lock);

    // A pooled job's thread belongs to the pool and outlives the job, so there
    // is nothing to detach — dropping the handle's ref is the whole of it.
    if (!state->pooled) {
        pthread_detach(state->thread);
    }
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

int64_t rask_sleep_ns(int64_t ns) {
    if (ns <= 0) return 0;
    struct timespec ts;
    ts.tv_sec  = ns / 1000000000LL;
    ts.tv_nsec = ns % 1000000000LL;
    nanosleep(&ts, NULL);
    return 0;
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

static int64_t closure_spawn_entry(void *arg) {
    RaskSpawnCtx *ctx = (RaskSpawnCtx *)arg;
    RaskTaskFn func = ctx->func;
    void *env = ctx->env;
    void *alloc_base = ctx->alloc_base;
    rask_free(ctx);

    int64_t result = func(env);
    rask_free(alloc_base);
    return result;
}

RaskTaskHandle *rask_closure_spawn(void *closure_ptr) {
    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskSpawnCtx *ctx = (RaskSpawnCtx *)rask_alloc(sizeof(RaskSpawnCtx));
    ctx->func = func;
    ctx->env = env;
    ctx->alloc_base = closure_ptr;

    return rask_task_spawn(closure_spawn_entry, ctx);
}

// ─── Hooks for the worker pool (threadpool.c) ──────────────
// A pooled job needs a task state and a handle without a thread behind them.
// These keep RaskTaskState private to this file while letting the pool build
// jobs whose handles join/detach/cancel like any other.

RaskTaskState *rask_task_state_new_pooled(void) {
    RaskTaskState *s = state_new();
    s->pooled = 1;
    return s;
}

RaskTaskHandle *rask_task_handle_for(RaskTaskState *state) {
    RaskTaskHandle *h = (RaskTaskHandle *)rask_alloc(sizeof(RaskTaskHandle));
    h->state = state;
    return h;
}

void rask_task_state_release(RaskTaskState *state) {
    state_release(state);
}

int64_t rask_task_join_simple(void *h) {
    return rask_task_join((RaskTaskHandle *)h, NULL);
}

// O4: wait for detached tasks to finish reporting. Called from `main` after
// rask_main returns, so a detached panic can't be lost to process exit. Only
// waits for tasks that were still running when they were detached, so a program
// with none pays nothing.
void rask_await_detached_tasks(void) {
    // A detached task can't be joined, so poll. The wait is bounded by the
    // task's own runtime, not by this interval.
    while (atomic_load_explicit(&detached_outstanding, memory_order_acquire) > 0) {
        struct timespec ts = { .tv_sec = 0, .tv_nsec = 200000 };  // 0.2 ms
        nanosleep(&ts, NULL);
    }
}
