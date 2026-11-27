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
    struct PoolJob *next;
} PoolJob;

static struct {
    pthread_t       *workers;
#ifdef RASK_SIM
    void           **sim_workers; // the workers' sim tasks, NULL outside sim
#endif
    int              worker_count;
    pthread_mutex_t  lock;
    pthread_cond_t   work_ready;
    pthread_cond_t   idle;        // `pending` reached 0
    PoolJob         *head;
    PoolJob         *tail;
    int64_t          pending;     // jobs spawned and not finished, under `lock`
    int              shutting_down;
    int              started;     // workers exist
    int              depth;       // `using ThreadPool` blocks open, atomic
} g_pool = {
    .lock = PTHREAD_MUTEX_INITIALIZER,
    .work_ready = PTHREAD_COND_INITIALIZER,
    .idle = PTHREAD_COND_INITIALIZER,
};

// Serializes opening and closing blocks. The pool is one per process, and two
// threads may each open a block over it; without this, one thread's init
// reset the queue and flags that the other's shutdown was still using.
static pthread_mutex_t g_life = PTHREAD_MUTEX_INITIALIZER;

static void job_done(void) {
    pthread_mutex_lock(&g_pool.lock);
    if (--g_pool.pending == 0) rask_task_cond_broadcast(&g_pool.idle);
    pthread_mutex_unlock(&g_pool.lock);
}

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

        // Its joiner may have taken it back and run it already.
        if (rask_task_pool_claim(job->task)) {
            rask_task_mark_on_pool(job->task);
            rask_task_run_body(job->task, job->func, job->env);
        }
        rask_task_release(job->task);   // the worker's ref
        rask_free(job);
        job_done();
    }
}

static void *pool_worker(void *arg) {
    (void)arg;
    rask_fp_reset();
    rask_stack_guard_thread_init();
    rask_outside_thread_start();
    pool_worker_loop();
    rask_outside_thread_exit();
    rask_thread_end();
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
    g_pool.pending++;
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

// Being inside a `using ThreadPool` block and having workers are separate:
// a default-sized pool under sim, and a pool none of whose threads could be
// created, are in the block with no workers, and spawn gives each job its own
// task. A block opened while another is open, on this thread or another,
// shares the pool; the last one to close tears it down.
static void start_workers(int64_t worker_count) {
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
        return;
    }
    g_pool.started = 1;
}

static void stop_workers(void) {
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
}

void rask_threadpool_init(int64_t worker_count) {
    rask_task_mutex_lock(&g_life, "opening a ThreadPool block");
    int depth = __atomic_load_n(&g_pool.depth, __ATOMIC_ACQUIRE);
    if (depth == 0) start_workers(worker_count);
    // Published after the workers exist, so a spawn that sees the block open
    // also sees whether it has workers.
    __atomic_store_n(&g_pool.depth, depth + 1, __ATOMIC_RELEASE);
    pthread_mutex_unlock(&g_life);
}

// The block is open until every job spawned in it has finished, detached ones
// included, so a job still running can spawn onto the pool. The wait happens
// without `g_life` held: a job may open and close a block of its own.
void rask_threadpool_shutdown(void) {
    rask_task_mutex_lock(&g_life, "closing a ThreadPool block");
    for (;;) {
        int depth = __atomic_load_n(&g_pool.depth, __ATOMIC_ACQUIRE);
        if (depth > 1) {
            __atomic_store_n(&g_pool.depth, depth - 1, __ATOMIC_RELEASE);
            pthread_mutex_unlock(&g_life);
            return;
        }
        pthread_mutex_lock(&g_pool.lock);
        int idle = g_pool.pending == 0;
        pthread_mutex_unlock(&g_pool.lock);
        if (idle) break;

        pthread_mutex_unlock(&g_life);
        pthread_mutex_lock(&g_pool.lock);
        while (g_pool.pending > 0) {
            rask_task_cond_wait(&g_pool.idle, &g_pool.lock, "the pool's jobs to finish");
        }
        pthread_mutex_unlock(&g_pool.lock);
        // Another block may have opened meanwhile and now owns the teardown.
        rask_task_mutex_lock(&g_life, "closing a ThreadPool block");
    }
    // Last block, nothing pending, and `g_life` keeps any block from opening:
    // no job can arrive while the workers stop.
    if (g_pool.started) stop_workers();
    __atomic_store_n(&g_pool.depth, 0, __ATOMIC_RELEASE);
    pthread_mutex_unlock(&g_life);
}

// A panic in a test's body skips the block's end (see rask_scope_close_if_open).
// The next test has to find the pool closed, however deep the panic left it.
//
// Most panics leave no block open. Checking first keeps those from taking the
// lock, which under sim is a scheduling point: a claim whose input panicked
// was reported as running under sampled schedules, and lost its full walk.
void rask_threadpool_close_if_open(void) {
    if (__atomic_load_n(&g_pool.depth, __ATOMIC_ACQUIRE) == 0) return;
    rask_task_mutex_lock(&g_life, "closing a ThreadPool block");
    int open = __atomic_load_n(&g_pool.depth, __ATOMIC_ACQUIRE) > 0;
    if (open) __atomic_store_n(&g_pool.depth, 1, __ATOMIC_RELEASE);
    pthread_mutex_unlock(&g_life);
    if (open) rask_threadpool_shutdown();
}

// ThreadPool.spawn (conc.async/S2). Outside a `using ThreadPool` block there
// is no pool. A spawn that said `requires_pool: false` runs right here, now:
// it may run beside what follows, and running it first is one of the orders
// it may run in. Any other panics (CC3): the checker rejects every pool spawn
// it can see outside the block, so this is a call it couldn't see (a stored
// closure, an interface call).
RaskTask *rask_threadpool_spawn(void *closure_ptr, int64_t result_owned, int8_t requires_pool) {
    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    if (__atomic_load_n(&g_pool.depth, __ATOMIC_ACQUIRE) == 0) {
        if (requires_pool) {
            rask_panic("ThreadPool.spawn requires `using ThreadPool` context");
        }
        RaskTask *task = rask_task_new();
        rask_task_adopt_closure(task, closure_ptr, result_owned);
        rask_task_run_inline(task, func, env);
        rask_task_release(task);   // the runner's ref; the handle keeps one
        return task;
    }
    if (!g_pool.started) {
        pthread_mutex_lock(&g_pool.lock);
        g_pool.pending++;
        pthread_mutex_unlock(&g_pool.lock);
        return rask_closure_spawn_then(closure_ptr, result_owned, job_done);
    }

    RaskTask *task = rask_task_new();
    rask_task_adopt_closure(task, closure_ptr, result_owned);
    rask_task_pool_queue(task, func, env);

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
