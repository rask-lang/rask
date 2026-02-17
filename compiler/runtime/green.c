// SPDX-License-Identifier: (MIT OR Apache-2.0)

// M:N green task scheduler with work-stealing.
//
// Core design:
//   - N worker threads (default: CPU count), each with a local Chase-Lev deque
//   - Global injection queue for cross-thread spawns
//   - I/O engine (io_uring or epoll) polled by idle workers
//   - Tasks are stackless state machines: poll_fn(state, ctx) → READY/PENDING
//
// Worker loop: local pop → steal from peer → global pop → poll I/O → park
//
// Task lifecycle: Spawned → Running → (Waiting ↔ Running) → Complete
// Handles are refcounted: one for the handle holder, one for the scheduler.

#include "io_engine.h"
#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <pthread.h>
#include <stdatomic.h>
#include <setjmp.h>
#include <unistd.h>
#include <sched.h>

// ─── Constants ──────────────────────────────────────────────

#define RASK_POLL_READY   0
#define RASK_POLL_PENDING 1

#define TASK_STATE_READY    0
#define TASK_STATE_RUNNING  1
#define TASK_STATE_WAITING  2
#define TASK_STATE_COMPLETE 3

#define DEQUE_CAP 1024
#define MAX_EVENTS_PER_POLL 64

// ─── Green task ─────────────────────────────────────────────

typedef int (*rask_poll_fn)(void *state, void *task_ctx);

typedef struct GreenTask {
    rask_poll_fn    poll_fn;
    void           *state;
    int64_t         state_size;    // for deallocation
    atomic_int      task_state;
    atomic_int      cancel_flag;
    int64_t         result;
    char           *panic_msg;

    // Completion signaling
    pthread_mutex_t done_lock;
    pthread_cond_t  done_cond;
    int             done;

    // Refcount: handle(1) + scheduler(1)
    atomic_int      refcount;

    // I/O result staging (set by I/O callback before re-enqueue)
    int64_t         io_result;
    int             io_err;
} GreenTask;

// ─── Task handle (returned to user code) ────────────────────

typedef struct GreenHandle {
    GreenTask *task;
} GreenHandle;

// ─── Chase-Lev work-stealing deque ──────────────────────────
//
// Owner: push_bottom / pop_bottom (LIFO, no CAS needed for single owner)
// Stealer: steal_top (FIFO, CAS for contention)
// Bounded fixed-size for simplicity.

typedef struct {
    GreenTask  *buf[DEQUE_CAP];
    atomic_long top;
    atomic_long bottom;
} WorkDeque;

static void deque_init(WorkDeque *d) {
    memset(d->buf, 0, sizeof(d->buf));
    atomic_init(&d->top, 0);
    atomic_init(&d->bottom, 0);
}

static void deque_push(WorkDeque *d, GreenTask *task) {
    long b = atomic_load_explicit(&d->bottom, memory_order_relaxed);
    long t = atomic_load_explicit(&d->top, memory_order_acquire);
    if (b - t >= DEQUE_CAP) {
        // Deque full — shouldn't happen with reasonable task counts.
        // Drop on floor rather than crash; task leaks but runtime stays alive.
        fprintf(stderr, "rask: work deque overflow\n");
        return;
    }
    d->buf[b % DEQUE_CAP] = task;
    atomic_store_explicit(&d->bottom, b + 1, memory_order_release);
}

static GreenTask *deque_pop(WorkDeque *d) {
    long b = atomic_load_explicit(&d->bottom, memory_order_relaxed) - 1;
    atomic_store_explicit(&d->bottom, b, memory_order_relaxed);
    atomic_thread_fence(memory_order_seq_cst);
    long t = atomic_load_explicit(&d->top, memory_order_relaxed);

    if (t <= b) {
        GreenTask *task = d->buf[b % DEQUE_CAP];
        if (t == b) {
            // Last element — race with stealers
            if (!atomic_compare_exchange_strong_explicit(
                    &d->top, &t, t + 1,
                    memory_order_seq_cst, memory_order_relaxed)) {
                task = NULL;
            }
            atomic_store_explicit(&d->bottom, b + 1, memory_order_relaxed);
        }
        return task;
    }

    // Empty
    atomic_store_explicit(&d->bottom, b + 1, memory_order_relaxed);
    return NULL;
}

static GreenTask *deque_steal(WorkDeque *d) {
    long t = atomic_load_explicit(&d->top, memory_order_acquire);
    atomic_thread_fence(memory_order_seq_cst);
    long b = atomic_load_explicit(&d->bottom, memory_order_acquire);

    if (t >= b) return NULL;

    GreenTask *task = d->buf[t % DEQUE_CAP];
    if (!atomic_compare_exchange_strong_explicit(
            &d->top, &t, t + 1,
            memory_order_seq_cst, memory_order_relaxed)) {
        return NULL; // lost race to another stealer
    }
    return task;
}

// ─── Global injection queue (mutex-protected) ───────────────

typedef struct {
    GreenTask  *buf[DEQUE_CAP * 4];
    int         head;
    int         tail;
    int         cap;
    pthread_mutex_t lock;
} GlobalQueue;

static void gq_init(GlobalQueue *gq) {
    gq->head = 0;
    gq->tail = 0;
    gq->cap = DEQUE_CAP * 4;
    pthread_mutex_init(&gq->lock, NULL);
}

static void gq_destroy(GlobalQueue *gq) {
    pthread_mutex_destroy(&gq->lock);
}

static void gq_push(GlobalQueue *gq, GreenTask *task) {
    pthread_mutex_lock(&gq->lock);
    int next = (gq->tail + 1) % gq->cap;
    if (next == gq->head) {
        // Global queue full
        pthread_mutex_unlock(&gq->lock);
        fprintf(stderr, "rask: global queue overflow\n");
        return;
    }
    gq->buf[gq->tail] = task;
    gq->tail = next;
    pthread_mutex_unlock(&gq->lock);
}

static GreenTask *gq_pop(GlobalQueue *gq) {
    pthread_mutex_lock(&gq->lock);
    if (gq->head == gq->tail) {
        pthread_mutex_unlock(&gq->lock);
        return NULL;
    }
    GreenTask *task = gq->buf[gq->head];
    gq->head = (gq->head + 1) % gq->cap;
    pthread_mutex_unlock(&gq->lock);
    return task;
}

// ─── Scheduler ──────────────────────────────────────────────

typedef struct {
    pthread_t       *workers;
    int              worker_count;
    WorkDeque       *local;        // local[worker_id]
    GlobalQueue      global;
    RaskIoEngine    *io;
    atomic_int       active_tasks;
    atomic_int       shutdown;

    // Parking: workers sleep here when no work found
    pthread_mutex_t  park_lock;
    pthread_cond_t   park_cond;

    // Shutdown barrier: main thread waits here
    pthread_mutex_t  done_lock;
    pthread_cond_t   done_cond;
} GreenScheduler;

// Singleton scheduler
static GreenScheduler *g_sched = NULL;

// Per-worker thread-local state
static __thread int tl_worker_id = -1;
static __thread GreenTask *tl_current_task = NULL;

// XorShift RNG for steal target selection
static __thread uint32_t tl_rng_state = 0;

static uint32_t xorshift32(void) {
    uint32_t x = tl_rng_state;
    if (x == 0) x = 1; // seed
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    tl_rng_state = x;
    return x;
}

// ─── Task lifecycle ─────────────────────────────────────────

static GreenTask *task_new(rask_poll_fn fn, void *state, int64_t state_size) {
    GreenTask *t = (GreenTask *)calloc(1, sizeof(GreenTask));
    if (!t) {
        fprintf(stderr, "rask: green task alloc failed\n");
        abort();
    }
    t->poll_fn    = fn;
    t->state      = state;
    t->state_size = state_size;
    atomic_init(&t->task_state, TASK_STATE_READY);
    atomic_init(&t->cancel_flag, 0);
    t->result     = 0;
    t->panic_msg  = NULL;
    t->done       = 0;
    atomic_init(&t->refcount, 2); // handle + scheduler
    t->io_result  = 0;
    t->io_err     = 0;
    pthread_mutex_init(&t->done_lock, NULL);
    pthread_cond_init(&t->done_cond, NULL);
    return t;
}

static void task_release(GreenTask *t) {
    if (atomic_fetch_sub_explicit(&t->refcount, 1, memory_order_acq_rel) == 1) {
        pthread_mutex_destroy(&t->done_lock);
        pthread_cond_destroy(&t->done_cond);
        if (t->panic_msg) free(t->panic_msg);
        if (t->state) free(t->state);
        free(t);
    }
}

static void task_mark_complete(GreenTask *t) {
    pthread_mutex_lock(&t->done_lock);
    t->done = 1;
    pthread_cond_broadcast(&t->done_cond);
    pthread_mutex_unlock(&t->done_lock);
}

// Enqueue task to the scheduler.
// If called from a worker thread, push to local deque.
// Otherwise, push to global queue.
static void sched_enqueue(GreenScheduler *s, GreenTask *t) {
    atomic_store_explicit(&t->task_state, TASK_STATE_READY, memory_order_release);

    if (tl_worker_id >= 0 && tl_worker_id < s->worker_count) {
        deque_push(&s->local[tl_worker_id], t);
    } else {
        gq_push(&s->global, t);
    }

    // Wake a parked worker
    pthread_mutex_lock(&s->park_lock);
    pthread_cond_signal(&s->park_cond);
    pthread_mutex_unlock(&s->park_lock);
}

// I/O completion callback: re-enqueue the task.
static void io_completion_cb(void *userdata, int64_t result, int err) {
    GreenTask *t = (GreenTask *)userdata;
    t->io_result = result;
    t->io_err    = err;
    if (g_sched) {
        sched_enqueue(g_sched, t);
    }
}

// ─── Panic handling for green tasks ─────────────────────────

extern jmp_buf *rask_panic_jmpbuf(void);
extern void     rask_panic_activate(void);
extern char    *rask_panic_take_message(void);

// ─── Execute a single task ──────────────────────────────────

// Forward declaration — ensure hooks run LIFO on task cancel/panic
static void run_ensure_hooks(void);

static void execute_task(GreenScheduler *s, GreenTask *t) {
    atomic_store_explicit(&t->task_state, TASK_STATE_RUNNING,
                          memory_order_release);
    tl_current_task = t;

    // Install panic handler for this task invocation
    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();

    int poll_result;
    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        poll_result = t->poll_fn(t->state, t);
    } else {
        // Panicked — run cleanup hooks before completing
        run_ensure_hooks();
        t->panic_msg = rask_panic_take_message();
        poll_result = RASK_POLL_READY;
        t->result = -1;
    }

    rask_panic_remove();
    tl_current_task = NULL;

    if (poll_result == RASK_POLL_READY) {
        // Task complete — run remaining ensure hooks
        run_ensure_hooks();
        atomic_store_explicit(&t->task_state, TASK_STATE_COMPLETE,
                              memory_order_release);
        task_mark_complete(t);
        atomic_fetch_sub_explicit(&s->active_tasks, 1, memory_order_relaxed);
        task_release(t); // scheduler's ref

        // Signal shutdown waiter if all tasks done
        if (atomic_load_explicit(&s->active_tasks, memory_order_acquire) == 0) {
            pthread_mutex_lock(&s->done_lock);
            pthread_cond_signal(&s->done_cond);
            pthread_mutex_unlock(&s->done_lock);
        }
    } else {
        // Task yielded (PENDING) — it will be re-enqueued by I/O callback
        // or immediately if it self-enqueued before returning PENDING
        atomic_store_explicit(&t->task_state, TASK_STATE_WAITING,
                              memory_order_release);
    }
}

// ─── Worker loop ────────────────────────────────────────────

static void *worker_entry(void *arg) {
    GreenScheduler *s = (GreenScheduler *)arg;
    // Compute worker_id from thread identity — use global queue push order
    // Actually, we pack the id into the arg pointer for simplicity.
    // Re-encode: we store worker_id in upper bits. Nah — use a proper struct.
    // Simpler: use a static counter.
    static atomic_int next_id = ATOMIC_VAR_INIT(0);
    int my_id = atomic_fetch_add_explicit(&next_id, 1, memory_order_relaxed);
    tl_worker_id = my_id;
    tl_rng_state = (uint32_t)(my_id + 1) * 2654435761U;

    int idle_spins = 0;

    while (!atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
        GreenTask *task = NULL;

        // 1. Pop from local deque
        task = deque_pop(&s->local[my_id]);

        // 2. Steal from a random peer
        if (!task) {
            int target = (int)(xorshift32() % (uint32_t)s->worker_count);
            if (target != my_id) {
                task = deque_steal(&s->local[target]);
            }
        }

        // 3. Pop from global queue
        if (!task) {
            task = gq_pop(&s->global);
        }

        if (task) {
            idle_spins = 0;
            execute_task(s, task);
            continue;
        }

        // 4. Poll I/O (non-blocking)
        if (s->io) {
            int fired = s->io->poll(s->io, 0);
            if (fired > 0) {
                idle_spins = 0;
                continue;
            }
        }

        // 5. No work — spin briefly before parking
        idle_spins++;
        if (idle_spins < 64) {
            sched_yield();
            continue;
        }

        // 6. Park on condvar (with timeout to recheck I/O)
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_nsec += 1000000; // 1ms
        if (ts.tv_nsec >= 1000000000L) {
            ts.tv_sec += 1;
            ts.tv_nsec -= 1000000000L;
        }
        pthread_mutex_lock(&s->park_lock);
        pthread_cond_timedwait(&s->park_cond, &s->park_lock, &ts);
        pthread_mutex_unlock(&s->park_lock);
        idle_spins = 0;
    }

    return NULL;
}

// ─── Public API ─────────────────────────────────────────────

void rask_runtime_init(int64_t worker_count) {
    if (g_sched) return; // already initialized

    GreenScheduler *s = (GreenScheduler *)calloc(1, sizeof(GreenScheduler));
    if (!s) {
        fprintf(stderr, "rask: scheduler alloc failed\n");
        abort();
    }

    if (worker_count <= 0) {
        worker_count = sysconf(_SC_NPROCESSORS_ONLN);
        if (worker_count <= 0) worker_count = 4;
    }

    s->worker_count = (int)worker_count;
    s->workers = (pthread_t *)calloc((size_t)worker_count, sizeof(pthread_t));
    s->local   = (WorkDeque *)calloc((size_t)worker_count, sizeof(WorkDeque));
    if (!s->workers || !s->local) {
        fprintf(stderr, "rask: scheduler arrays alloc failed\n");
        abort();
    }

    for (int i = 0; i < s->worker_count; i++) {
        deque_init(&s->local[i]);
    }

    gq_init(&s->global);
    atomic_init(&s->active_tasks, 0);
    atomic_init(&s->shutdown, 0);
    pthread_mutex_init(&s->park_lock, NULL);
    pthread_cond_init(&s->park_cond, NULL);
    pthread_mutex_init(&s->done_lock, NULL);
    pthread_cond_init(&s->done_cond, NULL);

    // Create I/O engine
    s->io = rask_io_create();
    // NULL is acceptable — scheduler works without I/O, tasks just can't yield on I/O

    g_sched = s;

    // Spawn worker threads
    for (int i = 0; i < s->worker_count; i++) {
        int err = pthread_create(&s->workers[i], NULL, worker_entry, s);
        if (err != 0) {
            fprintf(stderr, "rask: failed to create worker thread %d: %d\n",
                    i, err);
            abort();
        }
    }
}

void rask_runtime_shutdown(void) {
    GreenScheduler *s = g_sched;
    if (!s) return;

    // Wait for all active tasks to complete
    pthread_mutex_lock(&s->done_lock);
    while (atomic_load_explicit(&s->active_tasks, memory_order_acquire) > 0) {
        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_nsec += 10000000; // 10ms
        if (ts.tv_nsec >= 1000000000L) {
            ts.tv_sec += 1;
            ts.tv_nsec -= 1000000000L;
        }
        pthread_cond_timedwait(&s->done_cond, &s->done_lock, &ts);
    }
    pthread_mutex_unlock(&s->done_lock);

    // Signal shutdown and wake all workers
    atomic_store_explicit(&s->shutdown, 1, memory_order_release);

    pthread_mutex_lock(&s->park_lock);
    pthread_cond_broadcast(&s->park_cond);
    pthread_mutex_unlock(&s->park_lock);

    // Join worker threads
    for (int i = 0; i < s->worker_count; i++) {
        pthread_join(s->workers[i], NULL);
    }

    // Cleanup
    if (s->io) s->io->destroy(s->io);
    gq_destroy(&s->global);
    pthread_mutex_destroy(&s->park_lock);
    pthread_cond_destroy(&s->park_cond);
    pthread_mutex_destroy(&s->done_lock);
    pthread_cond_destroy(&s->done_cond);
    free(s->local);
    free(s->workers);
    free(s);
    g_sched = NULL;
}

// ─── Spawn / Join / Detach / Cancel ─────────────────────────

void *rask_green_spawn(void *poll_fn, void *state, int64_t state_size) {
    GreenScheduler *s = g_sched;
    if (!s) {
        rask_panic("spawn outside `using Multitasking {}` block");
    }

    GreenTask *t = task_new((rask_poll_fn)poll_fn, state, state_size);
    atomic_fetch_add_explicit(&s->active_tasks, 1, memory_order_relaxed);
    sched_enqueue(s, t);

    GreenHandle *h = (GreenHandle *)malloc(sizeof(GreenHandle));
    if (!h) {
        fprintf(stderr, "rask: green handle alloc failed\n");
        abort();
    }
    h->task = t;
    return h;
}

int64_t rask_green_join(void *handle) {
    GreenHandle *h = (GreenHandle *)handle;
    if (!h || !h->task) {
        rask_panic("join on consumed TaskHandle");
    }

    GreenTask *t = h->task;

    // Block until task completes
    pthread_mutex_lock(&t->done_lock);
    while (!t->done) {
        pthread_cond_wait(&t->done_cond, &t->done_lock);
    }
    pthread_mutex_unlock(&t->done_lock);

    int64_t result = t->result;

    // If task panicked, propagate
    if (t->panic_msg) {
        char *msg = t->panic_msg;
        t->panic_msg = NULL;
        task_release(t); // handle's ref
        free(h);
        // Re-panic in the joining context
        rask_panic(msg);
    }

    task_release(t); // handle's ref
    free(h);
    return result;
}

void rask_green_detach(void *handle) {
    GreenHandle *h = (GreenHandle *)handle;
    if (!h || !h->task) {
        rask_panic("detach on consumed TaskHandle");
    }

    task_release(h->task); // drop handle's ref
    free(h);
}

int64_t rask_green_cancel(void *handle) {
    GreenHandle *h = (GreenHandle *)handle;
    if (!h || !h->task) {
        rask_panic("cancel on consumed TaskHandle");
    }

    // Set cancel flag
    atomic_store_explicit(&h->task->cancel_flag, 1, memory_order_release);

    // Wait for completion
    return rask_green_join(handle);
}

// ─── Yield helpers (called by state machines) ───────────────
//
// These submit an I/O op with a callback that re-enqueues the current task,
// then the state machine returns PENDING. On next poll, it checks io_result.

void rask_yield_read(int fd, void *buf, size_t len) {
    GreenScheduler *s = g_sched;
    GreenTask *t = tl_current_task;
    if (!s || !s->io || !t) return;

    s->io->submit_read(s->io, fd, buf, len, io_completion_cb, t);
}

void rask_yield_write(int fd, const void *buf, size_t len) {
    GreenScheduler *s = g_sched;
    GreenTask *t = tl_current_task;
    if (!s || !s->io || !t) return;

    s->io->submit_write(s->io, fd, buf, len, io_completion_cb, t);
}

void rask_yield_accept(int listen_fd) {
    GreenScheduler *s = g_sched;
    GreenTask *t = tl_current_task;
    if (!s || !s->io || !t) return;

    s->io->submit_accept(s->io, listen_fd, io_completion_cb, t);
}

void rask_yield_timeout(uint64_t ns) {
    GreenScheduler *s = g_sched;
    GreenTask *t = tl_current_task;
    if (!s || !s->io || !t) return;

    s->io->submit_timeout(s->io, ns, io_completion_cb, t);
}

void rask_yield(void) {
    // Cooperative yield: re-enqueue via zero-timeout so the task gets
    // polled again on the next I/O sweep. Falls back to direct re-enqueue
    // if no I/O engine is available.
    GreenScheduler *s = g_sched;
    GreenTask *t = tl_current_task;
    if (!s || !t) return;

    if (s->io) {
        s->io->submit_timeout(s->io, 0, io_completion_cb, t);
    } else {
        // No I/O engine — direct re-enqueue
        sched_enqueue(s, t);
    }
}

int rask_green_task_is_cancelled(void) {
    GreenTask *t = tl_current_task;
    if (!t) return 0;
    return atomic_load_explicit(&t->cancel_flag, memory_order_acquire);
}

// ─── Closure-based spawn adapter ────────────────────────────
//
// For Phase A compatibility: wraps a closure (func_ptr | captures) as
// a single-state poll function that calls the closure once and returns READY.
// This is the bridge until the compiler generates state machines (Task 2).

typedef struct {
    void (*func)(void *env);
    void *env;
    void *alloc_base; // closure allocation to free after task
} ClosurePollState;

static int closure_poll_fn(void *state, void *task_ctx) {
    (void)task_ctx;
    ClosurePollState *s = (ClosurePollState *)state;
    s->func(s->env);
    rask_free(s->alloc_base);
    s->alloc_base = NULL; // prevent double-free in task_release
    return RASK_POLL_READY;
}

void *rask_green_closure_spawn(void *closure_ptr) {
    void (*func)(void *) = *(void (**)(void *))(closure_ptr);
    void *env = (char *)closure_ptr + 8;

    ClosurePollState *ps = (ClosurePollState *)malloc(sizeof(ClosurePollState));
    if (!ps) {
        fprintf(stderr, "rask: closure poll state alloc failed\n");
        abort();
    }
    ps->func = func;
    ps->env  = env;
    ps->alloc_base = closure_ptr;

    return rask_green_spawn(closure_poll_fn, ps, sizeof(ClosurePollState));
}

// ─── Async I/O wrappers (dual-path) ─────────────────────────
//
// Inside a green task: submit async I/O, result staged in GreenTask.
// Outside a green task: fall back to blocking syscalls.

#include <unistd.h>
#include <sys/socket.h>
#include <errno.h>

int64_t rask_async_read(int fd, void *buf, int64_t len) {
    GreenTask *t = tl_current_task;
    if (t && g_sched && g_sched->io) {
        // Green task path: submit async read, result in t->io_result/io_err
        rask_yield_read(fd, buf, (size_t)len);
        // On resume, io_result has bytes read (or -1 on error)
        return t->io_result;
    }
    // Blocking fallback
    ssize_t n = read(fd, buf, (size_t)len);
    return (int64_t)n;
}

int64_t rask_async_write(int fd, const void *buf, int64_t len) {
    GreenTask *t = tl_current_task;
    if (t && g_sched && g_sched->io) {
        rask_yield_write(fd, buf, (size_t)len);
        return t->io_result;
    }
    ssize_t n = write(fd, buf, (size_t)len);
    return (int64_t)n;
}

int64_t rask_async_accept(int listen_fd) {
    GreenTask *t = tl_current_task;
    if (t && g_sched && g_sched->io) {
        rask_yield_accept(listen_fd);
        return t->io_result;
    }
    int client = accept(listen_fd, NULL, NULL);
    return (int64_t)client;
}

// ─── Green-aware sleep ──────────────────────────────────────

void rask_green_sleep_ns(int64_t ns) {
    GreenTask *t = tl_current_task;
    if (t && g_sched) {
        rask_yield_timeout((uint64_t)ns);
        return;
    }
    // Blocking fallback
    struct timespec ts;
    ts.tv_sec  = ns / 1000000000LL;
    ts.tv_nsec = ns % 1000000000LL;
    nanosleep(&ts, NULL);
}

// ─── Ensure hooks (LIFO cleanup stack) ──────────────────────
//
// Per-task linked list of cleanup callbacks. Run LIFO on cancel.

typedef struct EnsureHook {
    void (*fn)(void *ctx);
    void *ctx;
    struct EnsureHook *next;
} EnsureHook;

static __thread EnsureHook *tl_ensure_stack = NULL;

void rask_ensure_push(void (*fn)(void *), void *ctx) {
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

// Run all ensure hooks LIFO (called on cancel/panic before task completes).
static void run_ensure_hooks(void) {
    while (tl_ensure_stack) {
        EnsureHook *hook = tl_ensure_stack;
        tl_ensure_stack = hook->next;
        if (hook->fn) {
            hook->fn(hook->ctx);
        }
        free(hook);
    }
}
