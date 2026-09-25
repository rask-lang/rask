// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Phase A thread primitives (conc.strategy/A1).
//
// One OS thread per spawn. Panics in spawned tasks are caught via
// setjmp/longjmp and propagated as JoinError on join.
//
// Handle lifecycle (a thread or pooled job):
//   spawn → [running] → join/detach/cancel → [consumed]
//
// The shared TaskState is refcounted: one ref for the handle, one for
// the running thread. Last one to drop frees it.

#include "rask_runtime.h"
#include "sim.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <pthread.h>
#include <stdatomic.h>
#include <setjmp.h>
#include <unistd.h>

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
    // Non-zero when `result` is a heap box this task owns rather than a plain
    // value — a payload wider than a machine word, or one that comes back in a
    // float register. Freed here when no join ever came for it (#963).
    int64_t      result_owned;
    // The closure allocation the task body runs out of. The task owns it for
    // the same reason it owns `result_owned` above: it is the only party
    // present at every ending. The body used to free it on the line after its
    // own call, which a panicking body longjmps past (#1223).
    void        *closure_base;
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

    // The task's place in the sim scheduler (sim.c), or NULL outside sim.
    void        *sim;
} RaskTaskState;

// `kind` first: `rask_handle_*` below reads it to route a handle.
struct RaskTaskHandle {
    int64_t        kind;
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
    // Assigned whole rather than field by field, which is what stops a field
    // added later from being read as whatever the allocator left there. C99
    // zero-fills every member this literal doesn't name, so "nobody wrote a
    // line for it" means NULL and 0 instead of a pointer `state_release` would
    // free. That is #1223: `closure_base` was added to this struct and the
    // pooled path never set it, so the release freed garbage and ran drop glue
    // on it. `thread` is in the same position today — only the spawn that
    // creates a real thread writes it — and needs no line here now.
    *s = (RaskTaskState){
        .panic_msg = NULL,
        .result = 0,
        .task_id = rask_next_task_id(),
    };
    atomic_init(&s->refcount, 2);  // handle + thread
    atomic_init(&s->status, RASK_TASK_RUNNING);
    atomic_init(&s->cancel_flag, 0);
    pthread_cond_init(&s->done_cond, NULL);
    pthread_mutex_init(&s->report_lock, NULL);
    return s;
}

static void state_release(RaskTaskState *s) {
    if (atomic_fetch_sub_explicit(&s->refcount, 1, memory_order_acq_rel) == 1) {
        if (s->panic_msg) rask_free(s->panic_msg);
        // The task body's closure allocation, whichever way the body ended.
        if (s->closure_base) rask_closure_free(s->closure_base);
        // Still set means nobody took it — a detached thread whose value no
        // join ever came for.
        if (s->result_owned && s->result) rask_free((void *)(intptr_t)s->result);
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

// ─── Task slots ────────────────────────────────────────────
//
// `using Multitasking(workers: n)` bounds how many tasks run at once. Where
// there is a green scheduler that bound is its worker count; where there isn't,
// a task is an OS thread and nothing counted them — `workers: 2` with six
// spawns ran six bodies at once (#1111). So the scope installs a count here and
// a body waits for one of the slots.
//
// Installed only by `rask_runtime_init` in green_threads.c, which exists only
// on a build without the scheduler, so `slots_total` is 0 and all of this is
// inert otherwise. Inside such a scope the bound covers every task body,
// `Thread.spawn` and a pooled job included — they aren't Multitasking tasks and
// the green build wouldn't count them, which is the one place the two differ.

static pthread_mutex_t slot_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t  slot_freed = PTHREAD_COND_INITIALIZER;
static int64_t         slots_total;   // 0 = no bound installed
static int64_t         slots_free;
static __thread int    slot_held;

// `n <= 0` is `using Multitasking` with no count, which the green scheduler
// reads as one worker per CPU. It used to install a single slot here, so a
// default scope on this path ran one task body at a time.
void rask_task_slots_install(int64_t n) {
    if (n <= 0) {
#ifdef RASK_SIM
        // The machine's CPU count can't be an input to a replay (determinism/D1),
        // so a default scope under sim has no bound at all.
        if (rask_sim_active()) return;
#endif
        n = (int64_t)sysconf(_SC_NPROCESSORS_ONLN);
        if (n <= 0) n = 1;
    }
    pthread_mutex_lock(&slot_lock);
    slots_total = n;
    slots_free = slots_total;
    pthread_mutex_unlock(&slot_lock);
}

void rask_task_slots_clear(void) {
    pthread_mutex_lock(&slot_lock);
    slots_total = 0;
    pthread_mutex_unlock(&slot_lock);
}

static void slot_take(void) {
    pthread_mutex_lock(&slot_lock);
    if (slots_total == 0) {
        pthread_mutex_unlock(&slot_lock);
        return;
    }
    // The bound is observable — at most n bodies in flight — so sim keeps it
    // too, and waiting for a slot is a scheduling point like any other wait.
    while (slots_free == 0) {
        rask_task_cond_wait(&slot_freed, &slot_lock, "a free worker slot");
    }
    slots_free--;
    pthread_mutex_unlock(&slot_lock);
    slot_held = 1;
}

// Returns 1 when a slot was given back.
static int slot_give(void) {
    if (!slot_held) return 0;
    slot_held = 0;
    pthread_mutex_lock(&slot_lock);
    slots_free++;
    rask_task_cond_signal(&slot_freed);
    pthread_mutex_unlock(&slot_lock);
    return 1;
}

// A task blocked in `join` isn't running anything, so it gives its slot up for
// the duration — without which `workers: 1` could not run a task that joins
// another. The green build answers the same case by starting a replacement
// worker. Only a slot that was given up is taken back: the block's own body
// isn't a task and never held one, and taking one after its join starved a
// task of the only slot (#1346).
int  rask_task_slot_release(void) { return slot_give(); }
void rask_task_slot_retake(int released) { if (released) slot_take(); }

// Run one task body to completion and record how it ended. Shared by the
// one-thread-per-spawn path below and by the pool workers in threadpool.c,
// which run many of these back to back on the same thread.
void rask_task_run_body(RaskTaskState *state, RaskTaskFn func, void *env) {
    slot_take();

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
#ifdef RASK_SIM
        rask_sim_notify(&detached_outstanding);
#endif
    }
    pthread_mutex_unlock(&state->report_lock);

    rask_panic_set_task_id(0);
    rask_panic_remove();
    current_cancel_flag = NULL;

    // A pooled job has no thread for join() to wait on, so waking the waiter
    // is what "finished" means for it.
    if (state->pooled) {
        pthread_mutex_lock(&state->report_lock);
        rask_task_cond_broadcast(&state->done_cond);
        pthread_mutex_unlock(&state->report_lock);
    }

    // Last, so a joiner waiting for this task's slot doesn't start before the
    // task has finished reporting.
    slot_give();
}

static void *task_thread_entry(void *arg) {
    TaskEntry *entry = (TaskEntry *)arg;
    RaskTaskState *state = entry->state;
    rask_outside_thread_start();
#ifdef RASK_SIM
    // Before anything else: under sim this thread may not run until picked.
    void *sim = state->sim;
    if (sim) rask_sim_task_enter(sim);
#endif
    RaskTaskFn func = entry->func;
    void *env = entry->env;
    rask_free(entry);

    rask_task_run_body(state, func, env);

    state_release(state);
#ifdef RASK_SIM
    if (sim) rask_sim_task_exit();
#endif
    rask_outside_thread_exit();
    return NULL;
}

// ─── Public API ────────────────────────────────────────────

RaskTaskHandle *rask_task_spawn(RaskTaskFn func, void *env) {
    RaskTaskState *state = state_new();

    TaskEntry *entry = (TaskEntry *)rask_alloc(sizeof(TaskEntry));
    *entry = (TaskEntry){ .func = func, .env = env, .state = state };

#ifdef RASK_SIM
    if (rask_sim_active()) state->sim = rask_sim_task_new(state->task_id);
#endif
    int err = pthread_create(&state->thread, NULL, task_thread_entry, entry);
    if (err != 0) {
#ifdef RASK_SIM
        // The task was registered as runnable; with no thread behind it the
        // baton would be handed to nobody.
        if (state->sim) rask_sim_task_abandon(state->sim);
#endif
        rask_free(entry);
        state_release(state);
        state_release(state); // drop both refs
        rask_panic_fmt("spawn failed: pthread_create returned %d", err);
    }

    RaskTaskHandle *h = (RaskTaskHandle *)rask_alloc(sizeof(RaskTaskHandle));
    *h = (RaskTaskHandle){ .kind = RASK_HANDLE_THREAD, .state = state };
    RASK_SIM_POINT();
    return h;
}

static int64_t task_join(RaskTaskHandle *h, char **msg_out) {
    if (!h || !h->state) {
        rask_panic("join on a consumed Handle");
    }

    RaskTaskState *state = h->state;
    // Waiting isn't running: a joiner that kept its slot would leave
    // `workers: 1` with nothing free to run the task it waits for.
    int released = rask_task_slot_release();
    if (state->pooled) {
        // No thread of its own — wait for the worker to finish this job.
        pthread_mutex_lock(&state->report_lock);
        while (atomic_load_explicit(&state->status, memory_order_acquire)
               == RASK_TASK_RUNNING) {
            rask_task_cond_wait(&state->done_cond, &state->report_lock, "a pooled job");
        }
        pthread_mutex_unlock(&state->report_lock);
    } else {
#ifdef RASK_SIM
        // The thread is about to exit once its task is done, and holds no lock
        // on the way out, so the real join after this doesn't wait on anyone.
        if (state->sim) rask_sim_task_join(state->sim);
#endif
        pthread_join(state->thread, NULL);
    }
    rask_task_slot_retake(released);

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
        // Ownership of a boxed result moves to the caller — clearing it stops
        // `state_release` from freeing what the caller is about to read. Same
        // handover the green path makes (#963).
        state->result = 0;
        if (msg_out) *msg_out = NULL;
    }

    state_release(state);
    rask_free(h);
    return result;
}

// Join, splitting "how it ended" from "what it produced". The old shape folded
// both into one int64_t, so a task returning -1 read back as a panic and a task
// returning 42 read back as 0 (the value was never captured at all).
static int64_t task_join_outcome(void *handle, int64_t *value_out, RaskStr *msg_out) {
    RaskTaskHandle *h = (RaskTaskHandle *)handle;
    if (!h || !h->state) {
        rask_panic("join on a consumed Handle");
    }

    int cancelled = atomic_load_explicit(&h->state->cancel_flag, memory_order_acquire);

    char *msg = NULL;
    int64_t value = task_join(h, &msg);

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

static void task_detach(RaskTaskHandle *h) {
    if (!h || !h->state) {
        rask_panic("detach on a consumed Handle");
    }

    RaskTaskState *state = h->state;

    RASK_SIM_POINT();
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

static void task_request_cancel(void *handle) {
    RaskTaskHandle *h = (RaskTaskHandle *)handle;
    if (!h || !h->state) {
        rask_panic("cancel on a consumed Handle");
    }
    RASK_SIM_POINT();
    atomic_store_explicit(&h->state->cancel_flag, 1, memory_order_release);
}

int8_t rask_task_cancelled(void) {
    RASK_SIM_POINT();
    if (!current_cancel_flag) return 0;
    return atomic_load_explicit(current_cancel_flag, memory_order_acquire) ? 1 : 0;
}

// ─── Handle (conc.async/H5) ────────────────────────────────
//
// Every spawn form hands back one `Handle<T>`. A green task's handle and a
// thread's are different structs, both starting with `kind`, so these read it
// and pass the handle on. A build with no green scheduler never makes a green
// handle: its `spawn` starts a thread.

#if RASK_HAS_GREEN
static int64_t handle_kind(void *h, const char *op) {
    if (!h) rask_panic_fmt("%s on a consumed Handle", op);
    return *(int64_t *)h;
}
#endif

int64_t rask_handle_join(void *h, int64_t *value_out, RaskStr *msg_out) {
#if RASK_HAS_GREEN
    if (handle_kind(h, "join") == RASK_HANDLE_GREEN) {
        return rask_green_join_outcome(h, value_out, msg_out);
    }
#endif
    return task_join_outcome(h, value_out, msg_out);
}

int64_t rask_handle_cancel(void *h, int64_t *value_out, RaskStr *msg_out) {
#if RASK_HAS_GREEN
    if (handle_kind(h, "cancel") == RASK_HANDLE_GREEN) {
        return rask_green_cancel_outcome(h, value_out, msg_out);
    }
#endif
    task_request_cancel(h);
    return task_join_outcome(h, value_out, msg_out);
}

void rask_handle_detach(void *h) {
#if RASK_HAS_GREEN
    if (handle_kind(h, "detach") == RASK_HANDLE_GREEN) {
        rask_green_detach(h);
        return;
    }
#endif
    task_detach((RaskTaskHandle *)h);
}

int8_t rask_handle_cancelled(void) {
#if RASK_HAS_GREEN
    if (rask_green_task_is_cancelled()) return 1;
#endif
    return rask_task_cancelled();
}

int64_t rask_sleep_ns(int64_t ns) {
#ifdef RASK_SIM
    if (rask_sim_active()) {
        rask_sim_sleep(ns);
        return 0;
    }
#endif
    if (ns <= 0) return 0;
    // A green task parks and leaves its worker to the others.
    if (rask_fiber_active()) {
        rask_fiber_sleep_ns(ns);
        return 0;
    }
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
} RaskSpawnCtx;

static int64_t closure_spawn_entry(void *arg) {
    RaskSpawnCtx *ctx = (RaskSpawnCtx *)arg;
    RaskTaskFn func = ctx->func;
    void *env = ctx->env;
    rask_free(ctx);

    // The closure allocation is the state's to free, not this frame's. It used
    // to be freed right here, on the line after the call — which a panicking
    // body longjmps straight past, one frame up into `rask_task_run_body`,
    // taking the local that held the pointer with it. Handing it to the state
    // is what makes the two endings agree; freeing it here *as well* would
    // need this frame to tell the state, and the thread is already running by
    // the time the spawner could have said which state that is.
    return func(env);
}

// `result_owned`: the closure hands back a heap box rather than a plain value.
// See `RaskTaskState::result_owned`.
RaskTaskHandle *rask_closure_spawn(void *closure_ptr, int64_t result_owned) {
    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskSpawnCtx *ctx = (RaskSpawnCtx *)rask_alloc(sizeof(RaskSpawnCtx));
    *ctx = (RaskSpawnCtx){ .func = func, .env = env };

    RaskTaskHandle *h = rask_task_spawn(closure_spawn_entry, ctx);
    if (h && h->state) {
        h->state->result_owned = result_owned;
        h->state->closure_base = closure_ptr;
    }
    return h;
}

// `Thread.spawn` — a raw OS thread, which sim can't schedule (sim/B1). Task
// spawns reach `rask_closure_spawn` directly, so this is the only entry that
// refuses.
RaskTaskHandle *rask_thread_spawn(void *closure_ptr, int64_t result_owned) {
#ifdef RASK_SIM
    if (rask_sim_active()) {
        rask_panic("Thread.spawn is not simulated: sim picks which task runs "
                   "next from the seed, and a raw OS thread would run outside "
                   "that (sim/B1). Use `spawn` in `using Multitasking { }`, or "
                   "`using ThreadPool { }`, which sim schedules like tasks");
    }
#endif
    return rask_closure_spawn(closure_ptr, result_owned);
}

// ─── Hooks for the worker pool (threadpool.c) ──────────────
// A pooled job needs a task state and a handle without a thread behind them.
// These keep RaskTaskState private to this file while letting the pool build
// jobs whose handles join/detach/cancel like any other.

// The pool builds its own states, so it needs a way to say the same thing
// `rask_closure_spawn` says — keeps `RaskTaskState` private to this file.
void rask_task_state_set_result_owned(struct RaskTaskState *state, int64_t owned) {
    if (state) state->result_owned = owned;
}

RaskTaskState *rask_task_state_new_pooled(void) {
    RaskTaskState *s = state_new();
    s->pooled = 1;
    return s;
}

RaskTaskHandle *rask_task_handle_for(RaskTaskState *state) {
    RaskTaskHandle *h = (RaskTaskHandle *)rask_alloc(sizeof(RaskTaskHandle));
    *h = (RaskTaskHandle){ .kind = RASK_HANDLE_THREAD, .state = state };
    return h;
}

void rask_task_state_release(RaskTaskState *state) {
    state_release(state);
}

// O4: wait for detached tasks to finish reporting. Called from `main` after
// rask_main returns, so a detached panic can't be lost to process exit. Only
// waits for tasks that were still running when they were detached, so a program
// with none pays nothing.
void rask_await_detached_tasks(void) {
#ifdef RASK_SIM
    if (rask_sim_active()) {
        while (atomic_load_explicit(&detached_outstanding, memory_order_acquire) > 0) {
            rask_sim_park(&detached_outstanding, "detached tasks to finish");
        }
        return;
    }
#endif
    // A detached task can't be joined, so poll. The wait is bounded by the
    // task's own runtime, not by this interval.
    while (atomic_load_explicit(&detached_outstanding, memory_order_acquire) > 0) {
        struct timespec ts = { .tv_sec = 0, .tv_nsec = 200000 };  // 0.2 ms
        nanosleep(&ts, NULL);
    }
}

// ─── Threads that could wake a task ────────────────────────
//
// The green scheduler reports a deadlock when every task is parked and nothing
// can wake one (green.c). A parked task can be woken from outside the fibers
// too: by the scope's own thread, a `Thread.spawn` thread or a pool worker. So
// the scheduler also needs to know that each of those is itself stuck in a
// wait, and not running code that might still send or unlock.
//
// `outside_running` counts program threads outside the scheduler that aren't
// in an untimed runtime wait. The scope's thread counts from the start; the
// threads this runtime creates count from when they start. A thread in a
// blocking syscall, a sleep or a timed wait stays counted, which only ever
// errs towards not reporting.

#define OUTSIDE_SLOTS 32

static atomic_int  outside_running = 1;
static atomic_long outside_waits_done;
static pthread_mutex_t outside_lock = PTHREAD_MUTEX_INITIALIZER;
static const char *outside_what[OUTSIDE_SLOTS];
static int outside_waiting;
static __thread int tl_outside_slot = -1;

void rask_outside_thread_start(void) {
    atomic_fetch_add_explicit(&outside_running, 1, memory_order_seq_cst);
}

void rask_outside_thread_exit(void) {
    atomic_fetch_sub_explicit(&outside_running, 1, memory_order_seq_cst);
}

void rask_thread_wait_begin(const char *what) {
    pthread_mutex_lock(&outside_lock);
    outside_waiting++;
    for (int i = 0; i < OUTSIDE_SLOTS; i++) {
        if (!outside_what[i]) {
            outside_what[i] = what ? what : "a wakeup";
            tl_outside_slot = i;
            break;
        }
    }
    pthread_mutex_unlock(&outside_lock);
    atomic_fetch_sub_explicit(&outside_running, 1, memory_order_seq_cst);
}

void rask_thread_wait_end(void) {
    atomic_fetch_add_explicit(&outside_running, 1, memory_order_seq_cst);
    atomic_fetch_add_explicit(&outside_waits_done, 1, memory_order_relaxed);
    pthread_mutex_lock(&outside_lock);
    outside_waiting--;
    if (tl_outside_slot >= 0) {
        outside_what[tl_outside_slot] = NULL;
        tl_outside_slot = -1;
    }
    pthread_mutex_unlock(&outside_lock);
}

int64_t rask_outside_running(void) {
    return atomic_load_explicit(&outside_running, memory_order_seq_cst);
}

// Changes whenever an outside thread comes out of a wait.
int64_t rask_outside_progress(void) {
    return atomic_load_explicit(&outside_waits_done, memory_order_relaxed);
}

// One line per outside thread that is waiting, for the deadlock report.
void rask_outside_report(FILE *out) {
    pthread_mutex_lock(&outside_lock);
    int named = 0;
    for (int i = 0; i < OUTSIDE_SLOTS; i++) {
        if (outside_what[i]) {
            fprintf(out, "  a thread outside the tasks, waiting on %s\n", outside_what[i]);
            named++;
        }
    }
    if (outside_waiting > named) {
        fprintf(out, "  %d more thread(s) outside the tasks, waiting\n", outside_waiting - named);
    }
    pthread_mutex_unlock(&outside_lock);
}
