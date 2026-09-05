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
// A job's handle is the same RaskTaskHandle Thread.spawn hands back, so join /
// detach / cancel all work unchanged. The difference is that a pooled job owns
// no thread, so join waits for the job's status rather than pthread_join'ing —
// that's the `pooled` flag in thread.c.

#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <pthread.h>
#include <stdatomic.h>
#include <unistd.h>

// ─── From thread.c ─────────────────────────────────────────

typedef struct RaskTaskState RaskTaskState;

extern RaskTaskState *rask_task_state_new_pooled(void);
extern RaskTaskHandle *rask_task_handle_for(RaskTaskState *state);
extern void rask_task_run_body(RaskTaskState *state, RaskTaskFn func, void *env);
extern void rask_task_state_release(RaskTaskState *state);

// ─── Job queue ─────────────────────────────────────────────

typedef struct PoolJob {
    RaskTaskFn      func;
    void           *env;
    void           *alloc_base;   // closure allocation, freed after the job
    RaskTaskState  *state;
    struct PoolJob *next;
} PoolJob;

static struct {
    pthread_t       *workers;
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

static void *pool_worker(void *arg) {
    (void)arg;
    for (;;) {
        pthread_mutex_lock(&g_pool.lock);
        while (!g_pool.head && !g_pool.shutting_down) {
            pthread_cond_wait(&g_pool.work_ready, &g_pool.lock);
        }
        // Drain the queue before exiting: shutdown runs at the end of the
        // `using` block, and a job already enqueued there still has to run.
        PoolJob *job = dequeue_locked();
        if (!job && g_pool.shutting_down) {
            pthread_mutex_unlock(&g_pool.lock);
            return NULL;
        }
        pthread_mutex_unlock(&g_pool.lock);

        if (!job) continue;

        rask_task_run_body(job->state, job->func, job->env);
        if (job->alloc_base) rask_closure_free(job->alloc_base);
        rask_task_state_release(job->state);   // the worker's ref
        rask_free(job);
    }
}

// ─── Public API ────────────────────────────────────────────

void rask_threadpool_init(int64_t worker_count) {
    if (g_pool.started) return;

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

    for (int i = 0; i < g_pool.worker_count; i++) {
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
    pthread_cond_broadcast(&g_pool.work_ready);
    pthread_mutex_unlock(&g_pool.lock);

    for (int i = 0; i < g_pool.worker_count; i++) {
        pthread_join(g_pool.workers[i], NULL);
    }

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
RaskTaskHandle *rask_threadpool_spawn(void *closure_ptr, int64_t result_owned) {
    if (!g_pool.started) {
        return rask_closure_spawn(closure_ptr, result_owned);
    }

    RaskTaskFn func = *(RaskTaskFn *)(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    RaskTaskState *state = rask_task_state_new_pooled();
    rask_task_state_set_result_owned(state, result_owned);

    PoolJob *job = (PoolJob *)rask_alloc(sizeof(PoolJob));
    job->func = func;
    job->env = env;
    job->alloc_base = closure_ptr;
    job->state = state;
    job->next = NULL;

    pthread_mutex_lock(&g_pool.lock);
    if (g_pool.tail) {
        g_pool.tail->next = job;
        g_pool.tail = job;
    } else {
        g_pool.head = job;
        g_pool.tail = job;
    }
    pthread_cond_signal(&g_pool.work_ready);
    pthread_mutex_unlock(&g_pool.lock);

    return rask_task_handle_for(state);
}
