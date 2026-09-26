// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Phase A thread primitives (conc.strategy/A1).
//
// One OS thread per spawn. Panics in spawned tasks are caught via
// setjmp/longjmp and propagated as JoinError on join.
//
// Every spawn form — a green task, a thread, a pooled job — records how it
// ended in a `RaskTask`, and the `Handle<T>` user code holds is a reference to
// one (conc.async/H5). So join, detach and cancel are written once, here.
//
//   spawn → [running] → join/detach/cancel → [consumed]
//
// A task is refcounted: one ref for the handle, one for whatever runs the
// body. Last one to drop frees it.

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
#include <poll.h>
#include <errno.h>
#include <fcntl.h>

// ─── Internal declarations from panic.c ────────────────────

extern jmp_buf *rask_panic_jmpbuf(void);
extern void     rask_panic_activate(void);
extern char    *rask_panic_take_message(void);
extern int64_t  rask_next_task_id(void);
extern void     rask_panic_set_task_id(int64_t id);

// ─── The task record ───────────────────────────────────────

#define RASK_TASK_RUNNING   0
#define RASK_TASK_OK        1
#define RASK_TASK_PANICKED  2

struct RaskTask {
    atomic_int   refcount;
    atomic_int   status;
    atomic_int   cancel_flag;
    char        *panic_msg;     // set on panic, owned by the task
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

    // A `Thread.spawn` thread is the task's own and has to be joined or
    // detached along with it. A green task and a pooled job run on a thread
    // that outlives them.
    int          own_thread;
    pthread_t    thread;

    // `done_cond` is signalled when `status` leaves RUNNING, under
    // `report_lock`. The lock also guards `detached` and the decision to
    // print an unjoined panic to stderr (O4).
    pthread_mutex_t report_lock;
    pthread_cond_t  done_cond;
    int             detached;
    int             counted_detached;  // in detached_outstanding

    int64_t      task_id;        // ctrl.panic/F1

    // The task's place in the sim scheduler (sim.c), or NULL outside sim.
    void        *sim;

    // How to wake the body out of the wait it is in, so a cancel reaches a
    // task parked in a receive, a sleep or a socket (conc.async/CN3). Set and
    // cleared by the body around the wait; `waking` is the canceller using
    // it, and the body doesn't leave the wait until it's done.
    pthread_mutex_t   wait_lock;
    pthread_cond_t    wait_idle;
    RaskCancelWake   *wake;
    int               waking;
    // A thread blocked in poll can't be woken through a condvar, so it polls
    // this pipe too. Made the first time one is needed.
    int               wake_pipe[2];
};

// A thread blocking in the runtime says so, for the green scheduler's deadlock
// check (defined at the end of this file).
void rask_thread_wait_begin(const char *what);
void rask_thread_wait_end(void);

// What `cancelled()` reads: the task whose body this thread is running.
static __thread RaskTask *current_task;

// O4: detached tasks still running. A detached task's panic *must* reach
// stderr, and a task racing process exit doesn't satisfy that — the report just
// vanishes, which is the failure O4 exists to prevent. `main` waits for this to
// reach zero before returning (rask_await_detached_tasks).
static atomic_int detached_outstanding;

RaskTask *rask_task_new(void) {
    RaskTask *t = (RaskTask *)rask_alloc(sizeof(RaskTask));
    // Assigned whole rather than field by field, which is what stops a field
    // added later from being read as whatever the allocator left there. C99
    // zero-fills every member this literal doesn't name, so "nobody wrote a
    // line for it" means NULL and 0 instead of a pointer `rask_task_release`
    // would free. That is #1223: `closure_base` was added to this struct and
    // the pooled path never set it, so the release freed garbage and ran drop
    // glue on it.
    *t = (RaskTask){ .task_id = rask_next_task_id(), .wake_pipe = { -1, -1 } };
    atomic_init(&t->refcount, 2);  // handle + runner
    atomic_init(&t->status, RASK_TASK_RUNNING);
    atomic_init(&t->cancel_flag, 0);
    pthread_cond_init(&t->done_cond, NULL);
    pthread_mutex_init(&t->report_lock, NULL);
    pthread_mutex_init(&t->wait_lock, NULL);
    pthread_cond_init(&t->wait_idle, NULL);
    return t;
}

void rask_task_adopt_closure(RaskTask *t, void *closure_base, int64_t result_owned) {
    t->closure_base = closure_base;
    t->result_owned = result_owned;
}

int64_t rask_task_id(RaskTask *t) {
    return t->task_id;
}

void rask_task_set_current(RaskTask *t) {
    current_task = t;
}

void rask_task_release(RaskTask *t) {
    if (atomic_fetch_sub_explicit(&t->refcount, 1, memory_order_acq_rel) == 1) {
        // `strdup`'d by panic.c, so not ours to hand to `rask_free`.
        free(t->panic_msg);
        // The task body's closure allocation, whichever way the body ended.
        if (t->closure_base) rask_closure_free(t->closure_base);
        // Still set means nobody took it — a detached task whose value no
        // join ever came for.
        if (t->result_owned && t->result) rask_free((void *)(intptr_t)t->result);
        pthread_cond_destroy(&t->done_cond);
        pthread_mutex_destroy(&t->report_lock);
        pthread_mutex_destroy(&t->wait_lock);
        pthread_cond_destroy(&t->wait_idle);
        if (t->wake_pipe[0] >= 0) {
            close(t->wake_pipe[0]);
            close(t->wake_pipe[1]);
        }
        rask_free(t);
    }
}

// Record how the body ended and wake whoever waits for it. The one ending for
// every kind of task.
void rask_task_finish(RaskTask *t, int64_t result, char *panic_msg) {
    pthread_mutex_lock(&t->report_lock);
    t->result = result;
    t->panic_msg = panic_msg;
    atomic_store_explicit(&t->status, panic_msg ? RASK_TASK_PANICKED : RASK_TASK_OK,
                          memory_order_release);
    // O4: already detached — no join is coming to read the message, so report
    // it now. F1: task id prefix, since a runtime task is what panicked.
    if (t->detached && t->panic_msg) {
        fprintf(stderr, "task %lld panic at %s\n", (long long)t->task_id, t->panic_msg);
        free(t->panic_msg);
        t->panic_msg = NULL;
    }
    // O4: done reporting either way, so `main` no longer has to wait for it.
    if (t->counted_detached) {
        t->counted_detached = 0;
        atomic_fetch_sub_explicit(&detached_outstanding, 1, memory_order_release);
#ifdef RASK_SIM
        rask_sim_notify(&detached_outstanding);
#endif
    }
    rask_task_cond_broadcast(&t->done_cond);
    pthread_mutex_unlock(&t->report_lock);
}

// ─── Thread entry point ────────────────────────────────────

typedef struct {
    RaskTaskFn  func;
    void       *env;
    RaskTask   *task;
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
// which run many of these back to back on the same thread. A green task runs
// its body in green.c, which has its own panic frame per fiber.
void rask_task_run_body(RaskTask *t, RaskTaskFn func, void *env) {
    slot_take();
    current_task = t;

    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();
    rask_panic_set_task_id(t->task_id); // F1

    int64_t result = 0;
    char *panic_msg = NULL;
    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        result = func(env);
    } else {
        // Returned via longjmp from rask_panic
        panic_msg = rask_panic_take_message();
    }
    rask_panic_set_task_id(0);
    rask_panic_remove();
    current_task = NULL;

    rask_task_finish(t, result, panic_msg);

    // Last, so a joiner waiting for this task's slot doesn't start before the
    // task has finished reporting.
    slot_give();
}

static void *task_thread_entry(void *arg) {
    TaskEntry *entry = (TaskEntry *)arg;
    RaskTask *t = entry->task;
    rask_outside_thread_start();
#ifdef RASK_SIM
    // Before anything else: under sim this thread may not run until picked.
    void *sim = t->sim;
    if (sim) rask_sim_task_enter(sim);
#endif
    RaskTaskFn func = entry->func;
    void *env = entry->env;
    rask_free(entry);

    rask_task_run_body(t, func, env);

    rask_task_release(t);
#ifdef RASK_SIM
    if (sim) rask_sim_task_exit();
#endif
    rask_outside_thread_exit();
    return NULL;
}

// ─── Threads ───────────────────────────────────────────────

static RaskTask *task_spawn_thread(RaskTaskFn func, void *env, void *closure_base,
                                   int64_t result_owned) {
    RaskTask *t = rask_task_new();
    rask_task_adopt_closure(t, closure_base, result_owned);
    t->own_thread = 1;

    TaskEntry *entry = (TaskEntry *)rask_alloc(sizeof(TaskEntry));
    *entry = (TaskEntry){ .func = func, .env = env, .task = t };

#ifdef RASK_SIM
    if (rask_sim_active()) t->sim = rask_sim_task_new(t->task_id);
#endif
    int err = pthread_create(&t->thread, NULL, task_thread_entry, entry);
    if (err != 0) {
#ifdef RASK_SIM
        // The task was registered as runnable; with no thread behind it the
        // baton would be handed to nobody.
        if (t->sim) rask_sim_task_abandon(t->sim);
#endif
        rask_free(entry);
        // The closure was never run, and the caller still thinks it's theirs.
        t->closure_base = NULL;
        rask_task_release(t);
        rask_task_release(t); // drop both refs
        rask_panic_fmt("spawn failed: pthread_create returned %d", err);
    }
    RASK_SIM_POINT();
    return t;
}

// ─── Handle (conc.async/H1–H5) ─────────────────────────────

static RaskTask *handle_task(void *h, const char *op) {
    if (!h) rask_panic_fmt("%s on a consumed Handle", op);
    return (RaskTask *)h;
}

// Wait for the body to finish. A green joiner parks rather than holding its
// worker; `rask_task_cond_wait` knows which it is.
static void wait_done(RaskTask *t) {
    // Waiting isn't running: a joiner that kept its slot would leave
    // `workers: 1` with nothing free to run the task it waits for.
    int released = rask_task_slot_release();
    // Read by the deadlock report while this waits, so it names the task.
    char what[48];
    snprintf(what, sizeof(what), "join(task %lld)", (long long)t->task_id);
    pthread_mutex_lock(&t->report_lock);
    while (atomic_load_explicit(&t->status, memory_order_acquire) == RASK_TASK_RUNNING) {
        rask_task_cond_wait(&t->done_cond, &t->report_lock, what);
    }
    pthread_mutex_unlock(&t->report_lock);
    if (t->own_thread) {
#ifdef RASK_SIM
        // The thread is about to exit once its task is done, and holds no lock
        // on the way out, so the real join after this doesn't wait on anyone.
        if (t->sim) rask_sim_task_join(t->sim);
#endif
        pthread_join(t->thread, NULL);
    }
    rask_task_slot_retake(released);
}

// Join, splitting "how it ended" from "what it produced". Folding both into one
// int64_t read a task returning -1 back as a panic. `*msg_out` is always left a
// valid string. Consumes the handle.
int64_t rask_handle_join(void *h, int64_t *value_out, RaskStr *msg_out) {
    RaskTask *t = handle_task(h, "join");
    wait_done(t);

    int64_t outcome;
    if (atomic_load_explicit(&t->status, memory_order_acquire) == RASK_TASK_PANICKED) {
        rask_string_from(msg_out, t->panic_msg ? t->panic_msg : "");
        if (value_out) *value_out = 0;
        outcome = RASK_JOIN_PANICKED;
    } else {
        rask_string_new(msg_out);
        if (value_out) *value_out = t->result;
        // Ownership of a boxed result moves to the caller — clearing it stops
        // `rask_task_release` from freeing what the caller is about to read.
        t->result = 0;
        outcome = RASK_JOIN_OK;
    }
    rask_task_release(t);
    return outcome;
}

// CN1: raise the flag, then wait. What the body returned is the answer, even
// if it stopped early because it was asked to: it may be holding a value only
// the caller can consume (CN4).
int64_t rask_handle_cancel(void *h, int64_t *value_out, RaskStr *msg_out) {
    RaskTask *t = handle_task(h, "cancel");
    RASK_SIM_POINT();
    atomic_store_explicit(&t->cancel_flag, 1, memory_order_seq_cst);
    // Wake it if it's parked. The body registers its wake before it last
    // reads the flag, and this reads the wake after raising the flag, so one
    // of the two always sees the other.
    pthread_mutex_lock(&t->wait_lock);
    RaskCancelWake *w = t->wake;
    if (w) t->waking = 1;
    pthread_mutex_unlock(&t->wait_lock);
    if (w) {
        w->wake(w);
        pthread_mutex_lock(&t->wait_lock);
        t->waking = 0;
        rask_task_cond_broadcast(&t->wait_idle);
        pthread_mutex_unlock(&t->wait_lock);
    }
    return rask_handle_join(h, value_out, msg_out);
}

void rask_handle_detach(void *h) {
    RaskTask *t = handle_task(h, "detach");
    RASK_SIM_POINT();
    pthread_mutex_lock(&t->report_lock);
    t->detached = 1;
    int status = atomic_load_explicit(&t->status, memory_order_acquire);
    if (status == RASK_TASK_RUNNING) {
        atomic_fetch_add_explicit(&detached_outstanding, 1, memory_order_relaxed);
        t->counted_detached = 1;
    }
    // O4: the task may have already panicked and finished before detach()
    // ran — same "report now, nobody will join" rule applies.
    if (status == RASK_TASK_PANICKED && t->panic_msg) {
        fprintf(stderr, "task %lld panic at %s\n", (long long)t->task_id, t->panic_msg);
        free(t->panic_msg);
        t->panic_msg = NULL;
    }
    pthread_mutex_unlock(&t->report_lock);

    if (t->own_thread) pthread_detach(t->thread);
    rask_task_release(t);
}

int8_t rask_handle_cancelled(void) {
    RASK_SIM_POINT();
    RaskTask *t = current_task;
    if (!t) return 0;
    return atomic_load_explicit(&t->cancel_flag, memory_order_acquire) ? 1 : 0;
}

// ─── Waits a cancel ends (conc.async/CN3) ──────────────────
//
// A wait that a cancel should end registers how to wake it, then loops on its
// own condition and on `rask_cancel_requested()`. The waker lives on the
// waiter's stack, so `rask_cancel_wait_end` doesn't return while a canceller
// is still calling it.

int rask_cancel_requested(void) {
    RaskTask *t = current_task;
    return t && atomic_load_explicit(&t->cancel_flag, memory_order_seq_cst);
}

int rask_cancel_wait_begin(RaskCancelWake *w) {
    RaskTask *t = current_task;
    if (!t) return 0;
    pthread_mutex_lock(&t->wait_lock);
    t->wake = w;
    pthread_mutex_unlock(&t->wait_lock);
    return atomic_load_explicit(&t->cancel_flag, memory_order_seq_cst);
}

void rask_cancel_wait_end(void) {
    RaskTask *t = current_task;
    if (!t) return;
    pthread_mutex_lock(&t->wait_lock);
    while (t->waking) {
        rask_task_cond_wait(&t->wait_idle, &t->wait_lock, "a cancel to finish");
    }
    t->wake = NULL;
    pthread_mutex_unlock(&t->wait_lock);
}

// The waker for a condvar wait: `a` is the mutex, `b` the condvar.
static void wake_cond(RaskCancelWake *w) {
    pthread_mutex_lock((pthread_mutex_t *)w->a);
    rask_task_cond_broadcast((pthread_cond_t *)w->b);
    pthread_mutex_unlock((pthread_mutex_t *)w->a);
}

RaskCancelWake rask_cancel_wake_cond(void *m, void *c) {
    return (RaskCancelWake){ .wake = wake_cond, .a = m, .b = c };
}

// ─── A thread waiting on a socket ──────────────────────────

static void wake_pipe_write(RaskCancelWake *w) {
    char one = 1;
    ssize_t ignored = write((int)(intptr_t)w->a, &one, 1);
    (void)ignored;
}

// Block until `fd` is ready or the task is cancelled; 1 means cancelled. The
// thread polls the task's wake pipe beside the socket, since a condvar can't
// interrupt poll.
int rask_thread_io_wait(int64_t fd, int64_t want_write) {
    short events = want_write ? POLLOUT : POLLIN;
    RaskTask *t = current_task;
    if (!t) {
        struct pollfd p = { .fd = (int)fd, .events = events };
        while (poll(&p, 1, -1) < 0 && errno == EINTR) {}
        return 0;
    }
    if (t->wake_pipe[0] < 0) {
        if (pipe(t->wake_pipe) < 0) {
            rask_panic_fmt("cancellable wait: pipe failed: %s", strerror(errno));
        }
        // Drained without blocking; a byte left from an earlier cancel only
        // wakes a wait that would see the flag anyway.
        fcntl(t->wake_pipe[0], F_SETFL, fcntl(t->wake_pipe[0], F_GETFL) | O_NONBLOCK);
    }
    RaskCancelWake w = { .wake = wake_pipe_write, .a = (void *)(intptr_t)t->wake_pipe[1] };
    int cancelled = rask_cancel_wait_begin(&w);
    struct pollfd p[2] = {
        { .fd = (int)fd, .events = events },
        { .fd = t->wake_pipe[0], .events = POLLIN },
    };
    rask_thread_wait_begin(want_write ? "a socket to take a write" : "a socket to be readable");
    while (!cancelled) {
        int n = poll(p, 2, -1);
        if (n < 0 && errno == EINTR) continue;
        if (p[1].revents) {
            char buf[16];
            while (read(t->wake_pipe[0], buf, sizeof(buf)) == sizeof(buf)) {}
        }
        cancelled = rask_cancel_requested();
        if (n < 0 || p[0].revents) break;
    }
    rask_thread_wait_end();
    rask_cancel_wait_end();
    return cancelled;
}

#ifdef RASK_SIM
static void wake_sim_sleeper(RaskCancelWake *w) {
    rask_sim_wake(w->a);
}
#endif

// A task's thread asleep: a timed wait a cancel can broadcast.
static int thread_sleep_cancellable(int64_t ns) {
    pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_t c = PTHREAD_COND_INITIALIZER;
    struct timespec now, until;
    clock_gettime(CLOCK_REALTIME, &now);
    int64_t end_ns = (int64_t)now.tv_sec * 1000000000LL + now.tv_nsec + ns;
    until.tv_sec = end_ns / 1000000000LL;
    until.tv_nsec = end_ns % 1000000000LL;

    RaskCancelWake w = rask_cancel_wake_cond(&m, &c);
    int cancelled = rask_cancel_wait_begin(&w);
    pthread_mutex_lock(&m);
    rask_thread_wait_begin("a sleep");
    while (!cancelled) {
        if (pthread_cond_timedwait(&c, &m, &until) == ETIMEDOUT) break;
        cancelled = rask_cancel_requested();
    }
    rask_thread_wait_end();
    pthread_mutex_unlock(&m);
    rask_cancel_wait_end();
    pthread_cond_destroy(&c);
    pthread_mutex_destroy(&m);
    return cancelled;
}

// 0, or RASK_CANCELLED when a cancel ended the sleep early (conc.async/CN3).
int64_t rask_sleep_ns(int64_t ns) {
    int cancelled = 0;
#ifdef RASK_SIM
    if (rask_sim_active()) {
        RaskCancelWake w = { .wake = wake_sim_sleeper, .a = rask_sim_self() };
        cancelled = rask_cancel_wait_begin(&w);
        if (!cancelled) {
            rask_sim_sleep(ns);
            cancelled = rask_cancel_requested();
        }
        rask_cancel_wait_end();
        return cancelled ? RASK_CANCELLED : 0;
    }
#endif
    if (ns <= 0) return 0;
    if (rask_fiber_active()) {
        // A green task parks and leaves its worker to the others.
        cancelled = rask_fiber_sleep_ns(ns);
    } else if (current_task) {
        cancelled = thread_sleep_cancellable(ns);
    } else {
        // Nothing can cancel the scope's own thread.
        struct timespec ts = { .tv_sec = ns / 1000000000LL, .tv_nsec = ns % 1000000000LL };
        nanosleep(&ts, NULL);
    }
    return cancelled ? RASK_CANCELLED : 0;
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
// See `RaskTask::result_owned`.
RaskTask *rask_closure_spawn(void *closure_ptr, int64_t result_owned) {
    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskSpawnCtx *ctx = (RaskSpawnCtx *)rask_alloc(sizeof(RaskSpawnCtx));
    *ctx = (RaskSpawnCtx){ .func = func, .env = env };
    return task_spawn_thread(closure_spawn_entry, ctx, closure_ptr, result_owned);
}

// `Thread.spawn` — a raw OS thread, which sim can't schedule (sim/B1). Task
// spawns reach `rask_closure_spawn` directly, so this is the only entry that
// refuses.
RaskTask *rask_thread_spawn(void *closure_ptr, int64_t result_owned) {
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
