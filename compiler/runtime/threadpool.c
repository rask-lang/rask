// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Bounded worker pool for `using ThreadPool(workers: n)`.
//
// conc.io-context/IO2 settles the semantics: a pool worker is an OS thread that
// runs a job to completion — no parking, no reactor. So this is a plain thread
// pool, not a second green scheduler.
//
// Before this, `ThreadPool.spawn` was `pthread_create` every time and the
// worker count was accepted, stored, and ignored: eight jobs were eight
// threads, a thousand jobs were a thousand threads. `using ThreadPool` also
// started the *green* scheduler, which the spawn never looked at.
//
// A job is a `RaskTask` like any other, and its handle joins, detaches and
// cancels the same way. It just has no thread of its own to join.
//
// Under sim the pool is the same pool: each worker is a sim task, so the
// worker count and the queue's order hold exactly as they do in production,
// and a job waiting on a job still queued behind it deadlocks the same way.

#include "rask_runtime.h"
#include "sim.h"

#include <stdlib.h>
#include <stdio.h>
#include <pthread.h>
#include <stdatomic.h>
#include <unistd.h>

// ─── Job queue ─────────────────────────────────────────────

typedef struct PoolJob {
    RaskTaskFn      func;
    void           *env;
    RaskTask       *task;
    RaskForkSide   *fork;   // a fork block instead of a task, when set
    struct PoolJob *next;
} PoolJob;

static void fork_side_run_queued(RaskForkSide *side);

static struct {
    pthread_t       *workers;
#ifdef RASK_SIM
    void           **sim_workers; // the workers' sim tasks, NULL outside sim
#endif
    int              worker_count;
    pthread_mutex_t  lock;
    pthread_cond_t   work_ready;
    PoolJob         *head;
    PoolJob         *tail;
    int              shutting_down;
    int              started;
} g_pool;

static PoolJob *dequeue_locked(void) {
    PoolJob *job = g_pool.head;
    if (!job) return NULL;
    g_pool.head = job->next;
    if (!g_pool.head) g_pool.tail = NULL;
    job->next = NULL;
    return job;
}

// Run jobs until shutdown and the queue is empty.
static void pool_worker_loop(void) {
    for (;;) {
        pthread_mutex_lock(&g_pool.lock);
        while (!g_pool.head && !g_pool.shutting_down) {
            rask_task_cond_wait(&g_pool.work_ready, &g_pool.lock, "a pool job");
        }
        // Drain the queue before exiting: shutdown runs at the end of the
        // `using` block, and a job already enqueued there still has to run.
        PoolJob *job = dequeue_locked();
        if (!job && g_pool.shutting_down) {
            pthread_mutex_unlock(&g_pool.lock);
            return;
        }
        pthread_mutex_unlock(&g_pool.lock);

        if (!job) continue;

        if (job->fork) {
            fork_side_run_queued(job->fork);
            rask_free(job);
            continue;
        }
        rask_task_run_body(job->task, job->func, job->env);
        rask_task_release(job->task);   // the worker's ref
        rask_free(job);
    }
}

static void *pool_worker(void *arg) {
    (void)arg;
    rask_outside_thread_start();
    pool_worker_loop();
    rask_outside_thread_exit();
    return NULL;
}

#ifdef RASK_SIM
// Under sim a worker is a fiber the seeded scheduler switches to (sim.c).
static void pool_worker_fiber(void *arg) {
    (void)arg;
    pool_worker_loop();
}
#endif

static void enqueue(PoolJob *job) {
    pthread_mutex_lock(&g_pool.lock);
    if (g_pool.tail) {
        g_pool.tail->next = job;
        g_pool.tail = job;
    } else {
        g_pool.head = job;
        g_pool.tail = job;
    }
    rask_task_cond_signal(&g_pool.work_ready);
    pthread_mutex_unlock(&g_pool.lock);
}

// ─── Public API ────────────────────────────────────────────

void rask_threadpool_init(int64_t worker_count) {
    if (g_pool.started) return;
#ifdef RASK_SIM
    // A pool sized to the machine can't be replayed (determinism/D1), so a
    // default-sized pool under sim has no bound: every job gets its own task,
    // the same as a default `using Multitasking`.
    if (worker_count <= 0 && rask_sim_active()) return;
#endif

    if (worker_count <= 0) {
        worker_count = sysconf(_SC_NPROCESSORS_ONLN);
        if (worker_count <= 0) worker_count = 4;
    }

    pthread_mutex_init(&g_pool.lock, NULL);
    pthread_cond_init(&g_pool.work_ready, NULL);
    g_pool.head = NULL;
    g_pool.tail = NULL;
    g_pool.shutting_down = 0;
    g_pool.worker_count = (int)worker_count;
    g_pool.workers = (pthread_t *)rask_alloc(sizeof(pthread_t) * (size_t)worker_count);

#ifdef RASK_SIM
    g_pool.sim_workers = NULL;
    if (rask_sim_active()) {
        g_pool.sim_workers = (void **)rask_alloc(sizeof(void *) * (size_t)worker_count);
    }
#endif

    for (int i = 0; i < g_pool.worker_count; i++) {
#ifdef RASK_SIM
        if (g_pool.sim_workers) {
            g_pool.sim_workers[i] = rask_sim_worker_spawn(pool_worker_fiber, NULL);
            continue;
        }
#endif
        if (pthread_create(&g_pool.workers[i], NULL, pool_worker, NULL) != 0) {
            // Fewer workers than asked for is survivable — none is not, since
            // every later spawn would enqueue into a queue nobody drains.
            g_pool.worker_count = i;
            break;
        }
    }

    if (g_pool.worker_count == 0) {
        rask_free(g_pool.workers);
        g_pool.workers = NULL;
#ifdef RASK_SIM
        if (g_pool.sim_workers) rask_free(g_pool.sim_workers);
        g_pool.sim_workers = NULL;
#endif
        pthread_cond_destroy(&g_pool.work_ready);
        pthread_mutex_destroy(&g_pool.lock);
        return;   // spawn falls back to one thread per job
    }

    g_pool.started = 1;
}

void rask_threadpool_shutdown(void) {
    if (!g_pool.started) return;

    pthread_mutex_lock(&g_pool.lock);
    g_pool.shutting_down = 1;
    rask_task_cond_broadcast(&g_pool.work_ready);
    pthread_mutex_unlock(&g_pool.lock);

    for (int i = 0; i < g_pool.worker_count; i++) {
#ifdef RASK_SIM
        if (g_pool.sim_workers) {
            rask_sim_task_join(g_pool.sim_workers[i]);
            continue;
        }
#endif
        pthread_join(g_pool.workers[i], NULL);
    }

#ifdef RASK_SIM
    if (g_pool.sim_workers) rask_free(g_pool.sim_workers);
    g_pool.sim_workers = NULL;
#endif
    rask_free(g_pool.workers);
    g_pool.workers = NULL;
    g_pool.worker_count = 0;
    g_pool.started = 0;
    g_pool.shutting_down = 0;
    pthread_cond_destroy(&g_pool.work_ready);
    pthread_mutex_destroy(&g_pool.lock);
}

// ThreadPool.spawn. Outside a `using ThreadPool` block there is no pool, so
// this falls back to a dedicated thread rather than enqueueing into a queue
// nothing drains.
RaskTask *rask_threadpool_spawn(void *closure_ptr, int64_t result_owned) {
    if (!g_pool.started) {
        return rask_closure_spawn(closure_ptr, result_owned);
    }

    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskTask *task = rask_task_new();
    rask_task_adopt_closure(task, closure_ptr, result_owned);

    PoolJob *job = (PoolJob *)rask_alloc(sizeof(PoolJob));
    *job = (PoolJob){
        .func = func,
        .env = env,
        .task = task,
        .next = NULL,
    };
    enqueue(job);
    return task;
}

// ─── Fork (conc.fork) ──────────────────────────────────────
//
// A fork block borrows from the forking frame, so the frame can't go on until
// the block is done (FK1). The block goes on the ordinary queue; whoever gets
// to it first runs it: a worker that dequeues it, or the forking thread when
// it comes to join it and finds it still unclaimed. Taking it back is what
// keeps nested forks from deadlocking a small pool: a thread only ever waits
// for a block another thread is already running, and that one finishes.

typedef void (*RaskForkThunk)(void *closure, void *out);

enum { FORK_QUEUED, FORK_CLAIMED, FORK_DONE };

struct RaskForkSide {
    RaskForkThunk    thunk;
    void            *closure;
    void            *out;
    atomic_int       state;
    atomic_int       refs;    // the joiner's, plus the queue's while queued
    RaskCaughtPanic *panic;
    pthread_mutex_t  lock;
    pthread_cond_t   done;
};

static void fork_side_release(RaskForkSide *side) {
    if (atomic_fetch_sub_explicit(&side->refs, 1, memory_order_acq_rel) == 1) {
        pthread_cond_destroy(&side->done);
        pthread_mutex_destroy(&side->lock);
        rask_free(side);
    }
}

static int fork_side_claim(RaskForkSide *side) {
    int expected = FORK_QUEUED;
    return atomic_compare_exchange_strong_explicit(
        &side->state, &expected, FORK_CLAIMED, memory_order_acq_rel, memory_order_acquire);
}

// A worker took the job off the queue. The joiner may have run it already.
static void fork_side_run_queued(RaskForkSide *side) {
    if (fork_side_claim(side)) {
        side->panic = rask_panic_catch(side->thunk, side->closure, side->out);
        pthread_mutex_lock(&side->lock);
        atomic_store_explicit(&side->state, FORK_DONE, memory_order_release);
        rask_task_cond_broadcast(&side->done);
        pthread_mutex_unlock(&side->lock);
    }
    fork_side_release(side);
}

RaskForkSide *rask_fork_start(void *thunk, void *closure, void *out) {
    RaskForkSide *side = (RaskForkSide *)rask_alloc(sizeof(RaskForkSide));
    side->thunk = (RaskForkThunk)thunk;
    side->closure = closure;
    side->out = out;
    side->panic = NULL;
    atomic_init(&side->state, FORK_QUEUED);
    pthread_mutex_init(&side->lock, NULL);
    pthread_cond_init(&side->done, NULL);
    // No pool: nothing to hand it to. The join runs it, in written order (FK5).
    if (!g_pool.started) {
        atomic_init(&side->refs, 1);
        return side;
    }
    atomic_init(&side->refs, 2);
    PoolJob *job = (PoolJob *)rask_alloc(sizeof(PoolJob));
    *job = (PoolJob){ .fork = side };
    enqueue(job);
    return side;
}

RaskCaughtPanic *rask_fork_inline(void *thunk, void *closure, void *out) {
    return rask_panic_catch((RaskForkThunk)thunk, closure, out);
}

// The first panic in written order wins (FK6); a later one is dropped.
static RaskCaughtPanic *fork_first(RaskCaughtPanic *first, RaskCaughtPanic *next) {
    if (!first) return next;
    rask_panic_discard(next);
    return first;
}

RaskCaughtPanic *rask_fork_join(RaskForkSide *side, RaskCaughtPanic *first) {
    RaskCaughtPanic *caught;
    if (fork_side_claim(side)) {
        caught = rask_panic_catch(side->thunk, side->closure, side->out);
    } else {
        pthread_mutex_lock(&side->lock);
        while (atomic_load_explicit(&side->state, memory_order_acquire) != FORK_DONE) {
            rask_task_cond_wait(&side->done, &side->lock, "a fork block");
        }
        pthread_mutex_unlock(&side->lock);
        caught = side->panic;
    }
    fork_side_release(side);
    return fork_first(first, caught);
}

void rask_fork_end(RaskCaughtPanic *first) {
    if (first) rask_panic_resume(first);
}
