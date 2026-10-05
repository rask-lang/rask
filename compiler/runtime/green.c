// SPDX-License-Identifier: (MIT OR Apache-2.0)

// M:N scheduler: tasks are stackful fibers on N worker threads
// (conc.runtime, conc.strategy/B1-B2).
//
// A task runs on its own stack (fiber.c). When it waits — for a join, a
// channel, a lock, a sleep — it parks: its registers go onto its stack, and
// the worker thread picks up something else. Waking it puts it back in a run
// queue. So a blocked task costs its stack, not a thread, and `workers: n`
// means n OS threads however many tasks are waiting.
//
// Worker loop: resumed fibers (inbox) → own deque → steal → global queue →
// timers and sockets → sleep.
//
// A task that waits on a socket parks the same way (`rask_io_wait`): the fd
// goes into one epoll set, and whichever idle worker is the poller sleeps in
// `epoll_wait` instead of on its condvar, so a ready socket wakes its task
// without a thread of its own. The poller is woken for other work through an
// eventfd in the same set.
//
// A task that has started stays on the worker that started it. Only tasks that
// haven't run yet are stolen. Moving a running fiber to another thread would
// leave the C code under it looking at the old thread's `__thread` variables:
// the compiler may compute a thread-local's address once per function and
// reuse it across a call, and a park is a call. Pinning keeps every
// thread-local address a fiber has computed valid, and keeps a lock a task
// holds across a park being released by the thread that took it.
//
// Each task's share of the runtime's thread-local state (panic handler, ensure
// hooks, held locks — panic.c's `TaskTls`) is swapped onto the thread when the
// task switches on and off again when it switches off, since several fibers
// take turns on one thread.
//
// What a task produced, and its handle, live in thread.c's `RaskTask`, shared
// with threads and pooled jobs. This file owns only the fiber and its place in
// the scheduler, and frees it when the body is done.
//
// Under sim (sim.c) this is the same scheduler with the machine taken out. The
// workers are threads sim switches between on its one OS thread, not pthreads;
// a worker with nothing to do parks there instead of sleeping; the clock is
// sim's; and the choices timing would make here — which deque a steal tries,
// when a running fiber is cut off — come from the seed. Everything else, the
// queues and the parking and the pinning, is what ships (#1381).

#include "fiber.h"
#include "rask_runtime.h"
#include "sim.h"

#include <poll.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <pthread.h>
#include <stdatomic.h>
#include <setjmp.h>
#include <signal.h>
#include <unistd.h>
#include <sched.h>
#include <errno.h>
#include <time.h>

#define DEQUE_CAP 1024

// ─── Green task ─────────────────────────────────────────────

// Where a fiber is in going to sleep, read by the worker it switched off and
// by whoever wakes it — the two can race. A wake that arrives while the fiber
// is still `PARKING` (enqueued as a waiter, not yet switched off) must not put
// it in a run queue: it could be resumed before its registers are saved. So it
// flips the state to `WOKEN` instead, and the worker that switches it off sees
// that and queues it itself.
enum { PARK_RUNNING = 0, PARK_PARKING, PARK_PARKED, PARK_WOKEN };

// Why a fiber switched back to its worker.
enum { SWITCH_DONE = 1, SWITCH_PARKED, SWITCH_YIELD, SWITCH_PREEMPTED };

typedef struct GreenTask {
    // The body: the closure's function and its environment.
    int64_t       (*body)(void *);
    void           *body_arg;

    // How it ended and who waits for it (thread.c). The task holds one ref,
    // released when the fiber is done.
    RaskTask       *task;

    // ── Fiber ──
    RaskFiber       fiber;
    int             started;
    int             home;            // worker it runs on once started
    RaskFiber      *worker_fiber;    // what to switch back to
    int             switch_reason;
    atomic_int      park;
    void           *tls;             // panic.c's TaskTls while switched off

    // Run-queue link (global queue, inboxes) — a task is in one queue at most.
    struct GreenTask *qnext;
    // Wait-table link and key, and what the wait is for (the deadlock report).
    const void       *wait_key;
    const char       *wait_what;
    struct GreenTask *wait_next;
    // Sleep deadline, and the timer list link.
    int64_t           wake_at_ns;
    struct GreenTask *timer_next;
} GreenTask;

// ─── Chase-Lev work-stealing deque ──────────────────────────
//
// Owner: push_bottom / pop_bottom (LIFO, no CAS needed for single owner)
// Stealer: steal_top (FIFO, CAS for contention)
//
// Holds tasks that haven't started. It is bounded; a full one spills to the
// global queue, which isn't.

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

// Returns 0 when full.
static int deque_push(WorkDeque *d, GreenTask *task) {
    long b = atomic_load_explicit(&d->bottom, memory_order_relaxed);
    long t = atomic_load_explicit(&d->top, memory_order_acquire);
    if (b - t >= DEQUE_CAP) return 0;
    d->buf[b % DEQUE_CAP] = task;
    atomic_store_explicit(&d->bottom, b + 1, memory_order_release);
    return 1;
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

// ─── Locked FIFO (global queue, inboxes) ────────────────────

typedef struct {
    pthread_mutex_t lock;
    GreenTask      *head;
    GreenTask      *tail;
    atomic_int      len;
} TaskQueue;

static void tq_init(TaskQueue *q) {
    pthread_mutex_init(&q->lock, NULL);
    q->head = q->tail = NULL;
    atomic_init(&q->len, 0);
}

static void tq_destroy(TaskQueue *q) {
    pthread_mutex_destroy(&q->lock);
}

static void tq_push(TaskQueue *q, GreenTask *t) {
    t->qnext = NULL;
    pthread_mutex_lock(&q->lock);
    if (q->tail) q->tail->qnext = t; else q->head = t;
    q->tail = t;
    atomic_fetch_add_explicit(&q->len, 1, memory_order_release);
    pthread_mutex_unlock(&q->lock);
}

static GreenTask *tq_pop(TaskQueue *q) {
    if (atomic_load_explicit(&q->len, memory_order_acquire) == 0) return NULL;
    pthread_mutex_lock(&q->lock);
    GreenTask *t = q->head;
    if (t) {
        q->head = t->qnext;
        if (!q->head) q->tail = NULL;
        t->qnext = NULL;
        atomic_fetch_sub_explicit(&q->len, 1, memory_order_relaxed);
    }
    pthread_mutex_unlock(&q->lock);
    return t;
}

// ─── Scheduler ──────────────────────────────────────────────

typedef struct GreenScheduler GreenScheduler;

typedef struct {
    GreenScheduler *sched;
    int             id;
    // Under sim: the thread sim runs this worker as, and the safe points the
    // fiber on it has passed against the budget the seed gave it.
    void           *sim_thread;
    int64_t         sim_points;
    int64_t         sim_budget;
    WorkDeque       deque;      // unstarted tasks, stealable
    TaskQueue       inbox;      // started tasks resuming here
    // Tasks this worker took off the CPU for running past their budget. Run
    // after everything else it has, so a spinning task can't keep a woken or
    // unstarted one waiting (conc.runtime/P1). Not stealable: a started fiber
    // stays on its worker (S3a).
    TaskQueue       preempted;
    // When the fiber running here was switched in, or 0 while none is. The
    // preemption timer reads it from whatever thread the signal lands on.
    atomic_llong    running_since;
    // Set by the timer when that fiber is over budget; the next safe point
    // it reaches yields.
    atomic_int      preempt;
    RaskFiber       fiber;      // this worker thread's own stack
    // Sleeping: waits on `cond` until something lands in its queues.
    pthread_mutex_t sleep_lock;
    pthread_cond_t  sleep_cond;
    atomic_int      sleeping;
    // Sleeping in `epoll_wait` as the poller, not on `sleep_cond`: a wake
    // is an eventfd write then.
    atomic_int      polling;
    // Fibers this worker has switched to. Only its own thread writes it; the
    // deadlock check reads it to see whether anything ran.
    atomic_long     runs;
    pthread_t       thread;
} Worker;

struct GreenScheduler {
    Worker          *workers;
    int              worker_count;
    TaskQueue        global;
    atomic_int       active_tasks;

    // Sockets tasks are parked on (edge-triggered, both directions), the
    // eventfd that wakes the poller, whether some worker holds the poller
    // role, and how many tasks are parked on a socket.
    int              epfd;
    int              wakefd;
    atomic_int       poller_taken;
    atomic_int       io_waiters;
    atomic_int       shutdown;

    // Sleeping tasks, unsorted: a wake is a scan, and a program that sleeps
    // in thousands of tasks at once is not the case to optimise first.
    pthread_mutex_t  timers_lock;
    GreenTask       *timers;
    atomic_int       timer_count;

    // Shutdown barrier: the scope's thread waits here
    pthread_mutex_t  done_lock;
    pthread_cond_t   done_cond;

    // Deadlock check: when nothing could move was first seen, and the
    // progress count then. See `check_deadlock`.
    pthread_mutex_t  stuck_lock;
    int64_t          stuck_since;
    int64_t          stuck_progress;
};

// Singleton scheduler
static GreenScheduler *g_sched = NULL;

// Per-worker thread-local state
static __thread Worker    *tl_worker = NULL;
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

// Under sim the machine's choices come from the seed (sim.c).
static int under_sim(void) {
#ifdef RASK_SIM
    return rask_sim_active();
#else
    return 0;
#endif
}

static int64_t now_ns(void) {
#ifdef RASK_SIM
    if (under_sim()) return rask_sim_time_ns();
#endif
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static void worker_wake(Worker *w) {
    if (!atomic_load_explicit(&w->sleeping, memory_order_seq_cst)) return;
#ifdef RASK_SIM
    // Parked on its key, or asleep until a timer.
    if (under_sim()) {
        rask_sim_notify(&w->sleep_cond);
        rask_sim_wake(w->sim_thread);
        return;
    }
#endif
    if (atomic_load_explicit(&w->polling, memory_order_seq_cst)) {
        uint64_t one = 1;
        ssize_t ignored = write(w->sched->wakefd, &one, sizeof(one));
        (void)ignored;
        return;
    }
    pthread_mutex_lock(&w->sleep_lock);
    pthread_cond_signal(&w->sleep_cond);
    pthread_mutex_unlock(&w->sleep_lock);
}

// Something anyone can run arrived: wake one sleeping worker.
static void wake_any(GreenScheduler *s) {
    for (int i = 0; i < s->worker_count; i++) {
        Worker *w = &s->workers[i];
        if (atomic_load_explicit(&w->sleeping, memory_order_acquire)) {
            worker_wake(w);
            return;
        }
    }
}

// ─── Task lifecycle ─────────────────────────────────────────

static GreenTask *task_new(void) {
    GreenTask *t = (GreenTask *)calloc(1, sizeof(GreenTask));
    void *tls = calloc(1, rask_task_tls_size());
    if (!t || !tls) {
        fprintf(stderr, "rask: green task alloc failed\n");
        abort();
    }
    t->tls = tls;
    atomic_init(&t->park, PARK_RUNNING);
    t->home = -1;
    return t;
}

static void task_free(GreenTask *t) {
    rask_task_release(t->task); // the runner's ref
    free(t->tls);
    free(t);
}

// A task that hasn't started: onto this worker's deque if we are one, else
// the global queue.
static void sched_enqueue_new(GreenScheduler *s, GreenTask *t) {
    Worker *w = tl_worker;
    if (!(w && w->sched == s && deque_push(&w->deque, t))) {
        tq_push(&s->global, t);
    }
    wake_any(s);
}

// A started task that can run again: back to the worker it lives on.
static void sched_resume(GreenTask *t) {
    GreenScheduler *s = g_sched;
    Worker *w = &s->workers[t->home];
    tq_push(&w->inbox, t);
    worker_wake(w);
}

// Make a parked task runnable. See PARK_* for the race this settles.
static void task_wake(GreenTask *t) {
    for (;;) {
        int st = atomic_load_explicit(&t->park, memory_order_acquire);
        if (st == PARK_PARKING) {
            if (atomic_compare_exchange_weak_explicit(&t->park, &st, PARK_WOKEN,
                    memory_order_acq_rel, memory_order_acquire)) {
                return;
            }
        } else if (st == PARK_PARKED) {
            if (atomic_compare_exchange_weak_explicit(&t->park, &st, PARK_RUNNING,
                    memory_order_acq_rel, memory_order_acquire)) {
                sched_resume(t);
                return;
            }
        } else {
            return; // already awake
        }
    }
}

// ─── Panic handling for green tasks ─────────────────────────

extern jmp_buf *rask_panic_jmpbuf(void);
extern void     rask_panic_activate(void);
extern char    *rask_panic_take_message(void);
extern void     rask_panic_set_task_id(int64_t id);
extern void     rask_ensure_run_all(void);

// ─── Running a fiber ────────────────────────────────────────

// Switch off the running fiber back to its worker, for `reason`.
static void switch_to_worker(GreenTask *t, int reason) {
    t->switch_reason = reason;
    rask_fiber_switch(&t->fiber, t->worker_fiber);
}

// Let everything else that is runnable here go first.
static void fiber_yield(void) {
    GreenTask *t = tl_current_task;
    if (!t) return;
    switch_to_worker(t, SWITCH_YIELD);
}

static void fiber_main(void *arg) {
    rask_fiber_started();
    // A fresh fiber stack reads as zeros, which hides a slot codegen forgot to
    // write exactly the way a fresh thread stack does.
    rask_poison_stack();
    GreenTask *t = (GreenTask *)arg;

    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();
    rask_panic_set_task_id(rask_task_id(t->task)); // F1

    int64_t result = 0;
    char *panic_msg = NULL;
    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        result = t->body(t->body_arg);
    } else {
        // Panicked — the hooks ran before the longjmp; drain anything left.
        rask_ensure_run_all();
        panic_msg = rask_panic_take_message();
    }
    rask_panic_remove();
    rask_panic_set_task_id(0);
    // A normal return has popped its hooks; anything still here is owed.
    rask_ensure_run_all();

    rask_task_finish(t->task, result, panic_msg);
    t->switch_reason = SWITCH_DONE;
    rask_fiber_switch_final(&t->fiber, t->worker_fiber);
}

static void preempt_flag_recompute(GreenScheduler *s);

static void run_task(GreenScheduler *s, Worker *w, GreenTask *t) {
    if (!t->started) {
        t->started = 1;
        t->home = w->id;
        rask_fiber_init(&t->fiber, fiber_main, t);
    }
    t->worker_fiber = &w->fiber;
    tl_current_task = t;
    // The swap carries the running task too (thread.c's part of the blob), so
    // it goes first and the task is named after it.
    rask_task_tls_swap(t->tls);
    rask_task_set_current(t->task);
    atomic_store_explicit(&w->running_since, now_ns(), memory_order_release);
#ifdef RASK_SIM
    if (under_sim()) {
        w->sim_points = 0;
        w->sim_budget = rask_sim_preempt_budget();
    }
#endif
    rask_fiber_switch(&w->fiber, &t->fiber);
    atomic_store_explicit(&w->running_since, 0, memory_order_release);
    // A mark the fiber never reached a safe point for (it parked, finished, or
    // was inside a long C call) goes with it. The global flag has to follow,
    // or every check in every thread takes the slow path until some other
    // fiber happens to be preempted.
    if (atomic_exchange_explicit(&w->preempt, 0, memory_order_acq_rel)) {
        preempt_flag_recompute(w->sched);
    }
    rask_task_tls_swap(t->tls);
    tl_current_task = NULL;

    switch (t->switch_reason) {
    case SWITCH_DONE:
        rask_fiber_destroy(&t->fiber);
        if (atomic_fetch_sub_explicit(&s->active_tasks, 1, memory_order_acq_rel) == 1) {
            pthread_mutex_lock(&s->done_lock);
            rask_task_cond_broadcast(&s->done_cond);
            pthread_mutex_unlock(&s->done_lock);
        }
        task_free(t);
        break;
    case SWITCH_PARKED: {
        int expected = PARK_PARKING;
        if (!atomic_compare_exchange_strong_explicit(&t->park, &expected, PARK_PARKED,
                memory_order_acq_rel, memory_order_acquire)) {
            // Woken before it was off the stack; it runs again now.
            atomic_store_explicit(&t->park, PARK_RUNNING, memory_order_release);
            tq_push(&w->inbox, t);
        }
        break;
    }
    case SWITCH_YIELD:
        tq_push(&w->inbox, t);
        break;
    case SWITCH_PREEMPTED:
        tq_push(&w->preempted, t);
        break;
    }
}

// ─── Timers ─────────────────────────────────────────────────

// Wake every sleeper whose deadline has passed; returns the nearest deadline
// still pending, or 0.
static int64_t fire_timers(GreenScheduler *s) {
    if (atomic_load_explicit(&s->timer_count, memory_order_acquire) == 0) return 0;
    int64_t now = now_ns();
    int64_t next = 0;
    GreenTask *due = NULL;
    pthread_mutex_lock(&s->timers_lock);
    GreenTask **link = &s->timers;
    while (*link) {
        GreenTask *t = *link;
        if (t->wake_at_ns <= now) {
            *link = t->timer_next;
            t->timer_next = due;
            due = t;
            atomic_fetch_sub_explicit(&s->timer_count, 1, memory_order_relaxed);
        } else {
            if (!next || t->wake_at_ns < next) next = t->wake_at_ns;
            link = &t->timer_next;
        }
    }
    pthread_mutex_unlock(&s->timers_lock);
    while (due) {
        GreenTask *t = due;
        due = t->timer_next;
        t->timer_next = NULL;
        task_wake(t);
    }
    return next;
}

// ─── Preemption (conc.runtime/P1–P3) ────────────────────────
//
// A fiber that computes without parking would keep its worker until it
// finished. Every Rask function entry and every loop back-edge checks
// `rask_preempt_requested` (codegen); it is zero unless some fiber has run past
// its budget, so the check is a load and a branch not taken. A timer signal
// every PREEMPT_TICK_NS looks at each worker, and when the fiber on one has
// been running for PREEMPT_BUDGET_NS it marks that worker and raises the flag.
// The next check on that fiber calls `rask_preempt_point`, which yields.
//
// The signal only reads a clock and stores atomics, so it is safe wherever it
// lands, and no thread is added for it: the thread-count soak still holds.
//
// A loop that calls nothing is covered by its back-edge check. conc.runtime/P2.3
// describes a signal that switches the fiber from inside the handler instead;
// that can stop runtime C code holding a lock the next fiber needs, and the
// check can't, because it only switches where codegen put it.

#define PREEMPT_BUDGET_NS (10LL * 1000000LL)
#define PREEMPT_TICK_NS   (5LL * 1000000LL)

int32_t rask_preempt_requested;

static timer_t              preempt_timer;
static int                  preempt_timer_armed;
static _Atomic(GreenScheduler *) preempt_sched;
static atomic_int           preempt_in_handler;

static void preempt_tick(int sig) {
    (void)sig;
    int saved = errno;
    atomic_fetch_add_explicit(&preempt_in_handler, 1, memory_order_acq_rel);
    GreenScheduler *s = atomic_load_explicit(&preempt_sched, memory_order_acquire);
    if (s) {
        int64_t now = now_ns();
        for (int i = 0; i < s->worker_count; i++) {
            Worker *w = &s->workers[i];
            int64_t since = atomic_load_explicit(&w->running_since, memory_order_acquire);
            if (since && now - since >= PREEMPT_BUDGET_NS) {
                atomic_store_explicit(&w->preempt, 1, memory_order_release);
                __atomic_store_n(&rask_preempt_requested, 1, __ATOMIC_RELEASE);
            }
        }
    }
    atomic_fetch_sub_explicit(&preempt_in_handler, 1, memory_order_acq_rel);
    errno = saved;
}

static void preempt_timer_start(GreenScheduler *s) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = preempt_tick;
    // Restarted, so a read the tick interrupts carries on instead of failing.
    sa.sa_flags = SA_RESTART | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGURG, &sa, NULL) != 0) return;
    atomic_store_explicit(&preempt_sched, s, memory_order_release);
    struct sigevent sev;
    memset(&sev, 0, sizeof(sev));
    sev.sigev_notify = SIGEV_SIGNAL;
    sev.sigev_signo = SIGURG;
    if (timer_create(CLOCK_MONOTONIC, &sev, &preempt_timer) != 0) return;
    struct itimerspec its = {
        .it_interval = { 0, PREEMPT_TICK_NS },
        .it_value = { 0, PREEMPT_TICK_NS },
    };
    if (timer_settime(preempt_timer, 0, &its, NULL) != 0) {
        timer_delete(preempt_timer);
        return;
    }
    preempt_timer_armed = 1;
}

// Before the scheduler is freed: no tick may still be reading it.
static void preempt_timer_stop(void) {
    if (preempt_timer_armed) {
        timer_delete(preempt_timer);
        preempt_timer_armed = 0;
    }
    atomic_store_explicit(&preempt_sched, NULL, memory_order_release);
    while (atomic_load_explicit(&preempt_in_handler, memory_order_acquire) != 0) {
        sched_yield();
    }
    __atomic_store_n(&rask_preempt_requested, 0, __ATOMIC_RELEASE);
}

// The flag is process-wide, so it stays up while any worker is marked.
static void preempt_flag_recompute(GreenScheduler *s) {
    for (int i = 0; i < s->worker_count; i++) {
        if (atomic_load_explicit(&s->workers[i].preempt, memory_order_acquire)) return;
    }
    __atomic_store_n(&rask_preempt_requested, 0, __ATOMIC_RELEASE);
}

void rask_preempt_point(void) {
    Worker *w = tl_worker;
    GreenTask *t = tl_current_task;
    if (!w || !t) return;
#ifdef RASK_SIM
    // No timer: the flag stays up and the budget is counted in safe points,
    // so a seed cuts a fiber off at the same place every replay.
    if (under_sim()) {
        if (++w->sim_points < w->sim_budget) return;
        if (rask_preempt_unsafe()) return;
        switch_to_worker(t, SWITCH_PREEMPTED);
        return;
    }
#endif
    if (!atomic_exchange_explicit(&w->preempt, 0, memory_order_acq_rel)) {
        return;
    }
    preempt_flag_recompute(w->sched);
    // Not in the middle of something another fiber on this thread could walk
    // into: an unwind, a C caller's frames, or a line being printed.
    if (rask_preempt_unsafe()) return;
    switch_to_worker(t, SWITCH_PREEMPTED);
}

// ─── Stack overflow ─────────────────────────────────────────
//
// A fiber that runs into its guard page faults on a stack it can't push
// another frame on, so the handler runs on a stack of its own (sigaltstack,
// one per worker) and says what happened before the process dies. Any other
// fault goes to whatever handler was there before.

static struct sigaction prev_segv;
static struct sigaction prev_bus;

static void overflow_handler(int sig, siginfo_t *info, void *uctx) {
    GreenTask *t = tl_current_task;
    if (t && info && rask_fiber_in_guard(&t->fiber, info->si_addr)) {
        char msg[160];
        int n = snprintf(msg, sizeof(msg),
                         "task %lld overflowed its stack (1 MiB) — "
                         "unbounded recursion?\n",
                         (long long)rask_task_id(t->task));
        if (n > 0) {
            ssize_t ignored = write(2, msg, (size_t)n);
            (void)ignored;
        }
        signal(SIGABRT, SIG_DFL);
        abort();
    }
    struct sigaction *prev = sig == SIGSEGV ? &prev_segv : &prev_bus;
    if (prev->sa_flags & SA_SIGINFO) {
        if (prev->sa_sigaction) {
            prev->sa_sigaction(sig, info, uctx);
            return;
        }
    } else if (prev->sa_handler != SIG_IGN && prev->sa_handler != SIG_DFL) {
        prev->sa_handler(sig);
        return;
    }
    signal(sig, SIG_DFL);
    raise(sig);
}

static pthread_once_t overflow_once = PTHREAD_ONCE_INIT;

static void install_overflow_handler(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = overflow_handler;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, &prev_segv);
    sigaction(SIGBUS, &sa, &prev_bus);
}

// ─── Deadlock ───────────────────────────────────────────────
//
// Every task parked and nothing left that could wake one: no worker running a
// fiber, nothing queued, no sleeper whose timer will fire, and every program
// thread outside the scheduler itself blocked in a wait (thread.c). Once that
// holds, it holds forever. A worker about to sleep checks it; one that finds it
// true, and finds it still true with nothing having run a second later, reports
// the waits and ends the process rather than hang.
//
// The second is for the one gap the counts can't see: a thread outside the
// scheduler that has been signalled still counts as waiting until it gets a
// CPU and comes out of `pthread_cond_wait`. On a loaded machine, or under
// valgrind, that can take a while.
//
// A task parked on a socket rules a deadlock out, since the other end can wake
// it. A task in a blocking read of anything else keeps its worker busy, so it
// can't be mistaken for stuck either.

#define DEADLOCK_CONFIRM_NS (1000LL * 1000000LL)

void rask_outside_report(FILE *out);   // thread.c
static void report_waits(FILE *out);

static int nothing_can_move(GreenScheduler *s, Worker *self) {
    if (atomic_load_explicit(&s->active_tasks, memory_order_seq_cst) == 0) return 0;
    // A socket can be woken from outside the program.
    if (atomic_load_explicit(&s->io_waiters, memory_order_seq_cst) != 0) return 0;
    if (atomic_load_explicit(&s->timer_count, memory_order_seq_cst) != 0) return 0;
    if (rask_outside_running() != 0) return 0;
    if (atomic_load_explicit(&s->global.len, memory_order_seq_cst) != 0) return 0;
    for (int i = 0; i < s->worker_count; i++) {
        Worker *w = &s->workers[i];
        if (w != self && !atomic_load_explicit(&w->sleeping, memory_order_seq_cst)) return 0;
        if (atomic_load_explicit(&w->inbox.len, memory_order_seq_cst) != 0) return 0;
        if (atomic_load_explicit(&w->preempted.len, memory_order_seq_cst) != 0) return 0;
        long top = atomic_load_explicit(&w->deque.top, memory_order_seq_cst);
        long bottom = atomic_load_explicit(&w->deque.bottom, memory_order_seq_cst);
        if (bottom > top) return 0;
    }
    return 1;
}

static int64_t progress_count(GreenScheduler *s) {
    int64_t n = rask_outside_progress();
    for (int i = 0; i < s->worker_count; i++) {
        n += atomic_load_explicit(&s->workers[i].runs, memory_order_relaxed);
    }
    return n;
}

_Noreturn static void report_deadlock(void) {
    fflush(stdout);
    fprintf(stderr, "rask: deadlock: every task is waiting and nothing can wake one\n");
    report_waits(stderr);
    rask_outside_report(stderr);
    fflush(stderr);
    _exit(101);
}

// A failed check doesn't restart the clock: another worker is often awake for
// a moment between its timed sleeps, which says nothing. What restarts it is
// progress — a fiber running, or an outside thread coming out of a wait — and
// nothing can un-stick the program without one of those.
static void check_deadlock(GreenScheduler *s, Worker *self) {
    if (!nothing_can_move(s, self)) return;
    int64_t now = now_ns();
    int64_t progress = progress_count(s);
    pthread_mutex_lock(&s->stuck_lock);
    int confirmed = 0;
    if (s->stuck_since && s->stuck_progress == progress) {
        confirmed = now - s->stuck_since >= DEADLOCK_CONFIRM_NS;
    } else {
        s->stuck_since = now;
        s->stuck_progress = progress;
    }
    pthread_mutex_unlock(&s->stuck_lock);
    if (confirmed && nothing_can_move(s, self) && progress_count(s) == progress) {
        report_deadlock();
    }
}

// ─── Sockets ────────────────────────────────────────────────
//
// A task waiting on a socket parks on a key made from the fd and the
// direction. Fds sit in one epoll set, edge-triggered for both directions, and
// any event on one wakes both of its keys; a waiter re-checks with `poll` and
// parks again if the event wasn't for it. Edge-triggered is safe because the
// waiter checks readiness itself after becoming a waiter, so an edge that came
// before it doesn't need to come again.

#define WAKE_TAG UINT64_MAX

// Not an address: user space never has the top bits set, so these can't
// collide with a condvar or lock key in the wait table.
static const void *io_key(int fd, int want_write) {
    return (const void *)(uintptr_t)(0xF000000000000000ULL |
                                     ((uint64_t)(uint32_t)fd << 1) |
                                     (uint64_t)(want_write != 0));
}

// Wake whatever waits on the sockets that became ready. Returns how many
// socket events there were.
static int netpoll(GreenScheduler *s, int timeout_ms) {
    if (s->epfd < 0) return 0;
    struct epoll_event evs[64];
    int n = epoll_wait(s->epfd, evs, 64, timeout_ms);
    int fired = 0;
    for (int i = 0; i < n; i++) {
        if (evs[i].data.u64 == WAKE_TAG) {
            uint64_t drained;
            ssize_t ignored = read(s->wakefd, &drained, sizeof(drained));
            (void)ignored;
            continue;
        }
        int fd = (int)evs[i].data.u64;
        rask_fiber_notify(io_key(fd, 0), 1);
        rask_fiber_notify(io_key(fd, 1), 1);
        fired++;
    }
    return fired;
}

// ─── Worker loop ────────────────────────────────────────────

// Which deque a steal tries.
static uint32_t steal_draw(uint32_t n) {
#ifdef RASK_SIM
    if (under_sim()) return (uint32_t)rask_sim_draw(n);
#endif
    return xorshift32() % n;
}

static GreenTask *find_work(GreenScheduler *s, Worker *w) {
    GreenTask *t = tq_pop(&w->inbox);
    if (t) return t;
    t = deque_pop(&w->deque);
    if (t) return t;
    int n = s->worker_count;
    if (n > 1) {
        int target = (int)steal_draw((uint32_t)n);
        if (target != w->id) {
            t = deque_steal(&s->workers[target].deque);
            if (t) return t;
        }
    }
    t = tq_pop(&s->global);
    if (t) return t;
    return tq_pop(&w->preempted);
}

static void *worker_entry(void *arg) {
    Worker *w = (Worker *)arg;
    GreenScheduler *s = w->sched;
    tl_worker = w;
    tl_rng_state = (uint32_t)(w->id + 1) * 2654435761U;
    rask_poison_stack();
    rask_fiber_init_thread(&w->fiber);

    stack_t alt;
    alt.ss_size = 64 * 1024;
    alt.ss_sp = malloc(alt.ss_size);
    alt.ss_flags = 0;
    if (alt.ss_sp) sigaltstack(&alt, NULL);

    int idle_spins = 0;

    while (!atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
        GreenTask *task = find_work(s, w);
        if (task) {
            idle_spins = 0;
            atomic_fetch_add_explicit(&w->runs, 1, memory_order_relaxed);
            run_task(s, w, task);
            continue;
        }

        int64_t next_timer = fire_timers(s);

        if (netpoll(s, 0) > 0) {
            idle_spins = 0;
            continue;
        }

        // No work — spin briefly before sleeping
        idle_spins++;
        if (idle_spins < 64) {
            sched_yield();
            continue;
        }

        check_deadlock(s, w);

        // Sleep until woken, or at most 1ms, or the next timer.
        int64_t wait_ns = 1000000;
        if (next_timer) {
            int64_t until = next_timer - now_ns();
            if (until < wait_ns) wait_ns = until > 0 ? until : 0;
        }

        // One sleeping worker is the poller: it sleeps in epoll_wait, so a
        // socket becoming ready wakes its task at once. `polling` goes up
        // before `sleeping`, so a waker that sees `sleeping` knows which way
        // to wake it.
        if (s->epfd >= 0 && !atomic_exchange_explicit(&s->poller_taken, 1, memory_order_acq_rel)) {
            atomic_store_explicit(&w->polling, 1, memory_order_seq_cst);
            atomic_store_explicit(&w->sleeping, 1, memory_order_seq_cst);
            if (atomic_load_explicit(&w->inbox.len, memory_order_seq_cst) == 0 &&
                atomic_load_explicit(&s->global.len, memory_order_seq_cst) == 0 &&
                !atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
                netpoll(s, (int)((wait_ns + 999999) / 1000000));
            }
            atomic_store_explicit(&w->sleeping, 0, memory_order_release);
            atomic_store_explicit(&w->polling, 0, memory_order_release);
            atomic_store_explicit(&s->poller_taken, 0, memory_order_release);
            idle_spins = 0;
            continue;
        }

        struct timespec ts;
        clock_gettime(CLOCK_REALTIME, &ts);
        ts.tv_nsec += wait_ns;
        while (ts.tv_nsec >= 1000000000L) {
            ts.tv_sec += 1;
            ts.tv_nsec -= 1000000000L;
        }
        pthread_mutex_lock(&w->sleep_lock);
        atomic_store_explicit(&w->sleeping, 1, memory_order_seq_cst);
        // Re-check after announcing, so a push that saw `sleeping == 0` a
        // moment ago isn't missed.
        if (atomic_load_explicit(&w->inbox.len, memory_order_seq_cst) == 0 &&
            atomic_load_explicit(&s->global.len, memory_order_seq_cst) == 0 &&
            !atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
            pthread_cond_timedwait(&w->sleep_cond, &w->sleep_lock, &ts);
        }
        atomic_store_explicit(&w->sleeping, 0, memory_order_release);
        pthread_mutex_unlock(&w->sleep_lock);
        idle_spins = 0;
    }

    if (alt.ss_sp) {
        stack_t off = { .ss_flags = SS_DISABLE };
        sigaltstack(&off, NULL);
        free(alt.ss_sp);
    }
    tl_worker = NULL;
    return NULL;
}

#ifdef RASK_SIM
// A worker under sim. The same order of looking for work as `worker_entry`,
// with a scheduling point per turn so the seed decides which worker moves.
// Idle, it parks until something lands in its queues or, with a sleeper
// pending, until sim's clock reaches the first deadline. Between looking and
// parking nothing else runs, so a wake can't fall in the gap.
static void sim_worker_entry(void *arg) {
    Worker *w = (Worker *)arg;
    GreenScheduler *s = w->sched;
    tl_worker = w;
    rask_fiber_init_thread(&w->fiber);

    while (!atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
        rask_sim_point();
        GreenTask *task = find_work(s, w);
        if (task) {
            atomic_fetch_add_explicit(&w->runs, 1, memory_order_relaxed);
            run_task(s, w, task);
            continue;
        }
        int64_t next_timer = fire_timers(s);
        if (atomic_load_explicit(&w->inbox.len, memory_order_acquire) != 0 ||
            atomic_load_explicit(&s->global.len, memory_order_acquire) != 0 ||
            atomic_load_explicit(&s->shutdown, memory_order_acquire)) {
            continue;
        }
        atomic_store_explicit(&w->sleeping, 1, memory_order_seq_cst);
        if (next_timer) {
            rask_sim_sleep(next_timer - now_ns());
        } else {
            rask_sim_park(&w->sleep_cond, "work");
        }
        atomic_store_explicit(&w->sleeping, 0, memory_order_release);
    }
    tl_worker = NULL;
}
#endif

// ─── Public API ─────────────────────────────────────────────

void rask_runtime_init(int64_t worker_count) {
    if (g_sched) return; // already initialized

    GreenScheduler *s = (GreenScheduler *)calloc(1, sizeof(GreenScheduler));
    if (!s) {
        fprintf(stderr, "rask: scheduler alloc failed\n");
        abort();
    }

    // The machine's CPU count can't be an input to a replay (determinism/D1),
    // so under sim a default scope gets one to four workers, by seed.
#ifdef RASK_SIM
    if (worker_count <= 0 && under_sim()) worker_count = 1 + (int64_t)rask_sim_draw(4);
#endif
    if (worker_count <= 0) {
        worker_count = sysconf(_SC_NPROCESSORS_ONLN);
        if (worker_count <= 0) worker_count = 4;
    }

    s->worker_count = (int)worker_count;
    s->workers = (Worker *)calloc((size_t)s->worker_count, sizeof(Worker));
    if (!s->workers) {
        fprintf(stderr, "rask: scheduler arrays alloc failed\n");
        abort();
    }

    tq_init(&s->global);
    atomic_init(&s->active_tasks, 0);
    atomic_init(&s->shutdown, 0);
    atomic_init(&s->timer_count, 0);
    pthread_mutex_init(&s->timers_lock, NULL);
    pthread_mutex_init(&s->done_lock, NULL);
    pthread_cond_init(&s->done_cond, NULL);
    pthread_mutex_init(&s->stuck_lock, NULL);

    for (int i = 0; i < s->worker_count; i++) {
        Worker *w = &s->workers[i];
        w->sched = s;
        w->id = i;
        deque_init(&w->deque);
        tq_init(&w->inbox);
        tq_init(&w->preempted);
        atomic_init(&w->running_since, 0);
        atomic_init(&w->preempt, 0);
        pthread_mutex_init(&w->sleep_lock, NULL);
        pthread_cond_init(&w->sleep_cond, NULL);
        atomic_init(&w->sleeping, 0);
        atomic_init(&w->polling, 0);
        atomic_init(&w->runs, 0);
    }

    // Without epoll (or an eventfd to wake the poller) a task waiting on a
    // socket blocks its worker instead of parking; everything else still works.
    atomic_init(&s->poller_taken, 0);
    atomic_init(&s->io_waiters, 0);
    // Sim's sockets aren't the kernel's, so there is nothing to poll.
    s->epfd = under_sim() ? -1 : epoll_create1(EPOLL_CLOEXEC);
    s->wakefd = under_sim() ? -1 : eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    if (s->epfd >= 0 && s->wakefd >= 0) {
        struct epoll_event ev = { .events = EPOLLIN, .data.u64 = WAKE_TAG };
        if (epoll_ctl(s->epfd, EPOLL_CTL_ADD, s->wakefd, &ev) < 0) {
            close(s->epfd);
            s->epfd = -1;
        }
    } else if (s->epfd >= 0) {
        close(s->epfd);
        s->epfd = -1;
    }

    pthread_once(&overflow_once, install_overflow_handler);

    g_sched = s;
#ifdef RASK_SIM
    if (under_sim()) {
        __atomic_store_n(&rask_preempt_requested, 1, __ATOMIC_RELEASE);
        for (int i = 0; i < s->worker_count; i++) {
            Worker *w = &s->workers[i];
            w->sim_thread = rask_sim_green_worker_spawn(i, sim_worker_entry, w);
        }
        return;
    }
#endif
    preempt_timer_start(s);

    for (int i = 0; i < s->worker_count; i++) {
        int err = pthread_create(&s->workers[i].thread, NULL, worker_entry, &s->workers[i]);
        if (err != 0) {
            fprintf(stderr, "rask: failed to create worker thread %d: %d\n",
                    i, err);
            abort();
        }
    }
}

// The scope's end waits for every task (conc.async/C4). Timed only so a missed
// broadcast can't hang it; the thread can't wake anyone meanwhile, so it
// counts as waiting for the deadlock check. Under sim it parks like any other
// thread with nothing to do.
static void wait_tasks_done(GreenScheduler *s) {
    pthread_mutex_lock(&s->done_lock);
#ifdef RASK_SIM
    if (under_sim()) {
        while (atomic_load_explicit(&s->active_tasks, memory_order_acquire) > 0) {
            rask_task_cond_wait(&s->done_cond, &s->done_lock, "the end of `using Multitasking`");
        }
        pthread_mutex_unlock(&s->done_lock);
        return;
    }
#endif
    rask_thread_wait_begin("the end of `using Multitasking`");
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
    rask_thread_wait_end();
    pthread_mutex_unlock(&s->done_lock);
}

// Signal shutdown, wake every worker (the poller through its eventfd), and
// wait for them to leave their loops.
static void stop_workers(GreenScheduler *s) {
    atomic_store_explicit(&s->shutdown, 1, memory_order_release);
#ifdef RASK_SIM
    if (under_sim()) {
        for (int i = 0; i < s->worker_count; i++) {
            Worker *w = &s->workers[i];
            rask_sim_notify(&w->sleep_cond);
            rask_sim_wake(w->sim_thread);
        }
        for (int i = 0; i < s->worker_count; i++) {
            rask_sim_task_join(s->workers[i].sim_thread);
        }
        return;
    }
#endif
    if (s->wakefd >= 0) {
        uint64_t one = 1;
        ssize_t ignored = write(s->wakefd, &one, sizeof(one));
        (void)ignored;
    }
    for (int i = 0; i < s->worker_count; i++) {
        Worker *w = &s->workers[i];
        pthread_mutex_lock(&w->sleep_lock);
        pthread_cond_signal(&w->sleep_cond);
        pthread_mutex_unlock(&w->sleep_lock);
    }
    for (int i = 0; i < s->worker_count; i++) {
        pthread_join(s->workers[i].thread, NULL);
    }
}

void rask_runtime_shutdown(void) {
    GreenScheduler *s = g_sched;
    if (!s) return;

    wait_tasks_done(s);
    preempt_timer_stop();
    stop_workers(s);

    // Cleanup
    if (s->epfd >= 0) close(s->epfd);
    if (s->wakefd >= 0) close(s->wakefd);
    for (int i = 0; i < s->worker_count; i++) {
        Worker *w = &s->workers[i];
        tq_destroy(&w->inbox);
        tq_destroy(&w->preempted);
        pthread_mutex_destroy(&w->sleep_lock);
        pthread_cond_destroy(&w->sleep_cond);
    }
    tq_destroy(&s->global);
    pthread_mutex_destroy(&s->timers_lock);
    pthread_mutex_destroy(&s->done_lock);
    pthread_cond_destroy(&s->done_cond);
    pthread_mutex_destroy(&s->stuck_lock);
    free(s->workers);
    free(s);
    g_sched = NULL;
}

// ─── Parking ────────────────────────────────────────────────
//
// The primitive every wait is built on: park the running fiber on an address,
// and let a notify on that address make it runnable again. The same shape as
// sim mode's park/notify (sim.c), which is why sim.h's wrappers can route to
// either.
//
// Waiters live in a small hash table of buckets keyed by address. Each bucket
// counts its waiters so a notify with nobody waiting — every unlock of an
// uncontended lock — costs one atomic load.

#define WAIT_BUCKETS 64

typedef struct {
    pthread_mutex_t lock;
    atomic_int      waiters;
    GreenTask      *head;
} WaitBucket;

static WaitBucket g_wait[WAIT_BUCKETS] = {
    [0 ... WAIT_BUCKETS - 1] = { .lock = PTHREAD_MUTEX_INITIALIZER },
};

static WaitBucket *bucket_for(const void *key) {
    uintptr_t k = (uintptr_t)key;
    k ^= k >> 17;
    k *= 0x9E3779B97F4A7C15ULL;
    return &g_wait[(k >> 32) % WAIT_BUCKETS];
}

// Append `t` as a waiter on `key`. Caller holds the bucket lock.
static void bucket_add(WaitBucket *b, GreenTask *t, const void *key) {
    t->wait_key = key;
    t->wait_next = NULL;
    GreenTask **link = &b->head;
    while (*link) link = &(*link)->wait_next;
    *link = t;
    atomic_fetch_add_explicit(&b->waiters, 1, memory_order_seq_cst);
}

// Remove `t`; caller holds the bucket lock.
static void bucket_remove(WaitBucket *b, GreenTask *t) {
    GreenTask **link = &b->head;
    while (*link && *link != t) link = &(*link)->wait_next;
    if (*link) {
        *link = t->wait_next;
        t->wait_next = NULL;
        atomic_fetch_sub_explicit(&b->waiters, 1, memory_order_relaxed);
    }
}

int rask_fiber_active(void) {
    return tl_current_task != NULL;
}

void rask_fiber_notify(const void *key, int all) {
    WaitBucket *b = bucket_for(key);
    atomic_thread_fence(memory_order_seq_cst);
    if (atomic_load_explicit(&b->waiters, memory_order_seq_cst) == 0) return;

    GreenTask *woken = NULL;
    pthread_mutex_lock(&b->lock);
    GreenTask **link = &b->head;
    while (*link) {
        GreenTask *t = *link;
        if (t->wait_key == key) {
            *link = t->wait_next;
            atomic_fetch_sub_explicit(&b->waiters, 1, memory_order_relaxed);
            t->wait_next = woken;
            woken = t;
            if (!all) break;
        } else {
            link = &t->wait_next;
        }
    }
    pthread_mutex_unlock(&b->lock);

    while (woken) {
        GreenTask *t = woken;
        woken = t->wait_next;
        t->wait_next = NULL;
        task_wake(t);
    }
}

// Condition wait on a fiber: become a waiter on `c`, let go of `m`, park, and
// take `m` back once woken. Like pthread_cond_wait, a caller loops on its own
// condition — a wake can be spurious.
void rask_fiber_cond_wait(pthread_cond_t *c, pthread_mutex_t *m, const char *what) {
    GreenTask *t = tl_current_task;
    WaitBucket *b = bucket_for(c);
    t->wait_what = what;
    pthread_mutex_lock(&b->lock);
    atomic_store_explicit(&t->park, PARK_PARKING, memory_order_release);
    bucket_add(b, t, c);
    pthread_mutex_unlock(&b->lock);
    pthread_mutex_unlock(m);
    switch_to_worker(t, SWITCH_PARKED);
    pthread_mutex_lock(m);
}

// Park until `try_take(obj)` succeeds. The retry happens after becoming a
// waiter and before sleeping, so a release between the failed attempt and
// the park still wakes us.
static void park_until(const void *key, int (*try_take)(void *), void *obj,
                       const char *what) {
    GreenTask *t = tl_current_task;
    WaitBucket *b = bucket_for(key);
    t->wait_what = what;
    for (;;) {
        if (try_take(obj)) return;
        pthread_mutex_lock(&b->lock);
        atomic_store_explicit(&t->park, PARK_PARKING, memory_order_release);
        bucket_add(b, t, key);
        if (try_take(obj)) {
            bucket_remove(b, t);
            atomic_store_explicit(&t->park, PARK_RUNNING, memory_order_release);
            pthread_mutex_unlock(&b->lock);
            return;
        }
        pthread_mutex_unlock(&b->lock);
        switch_to_worker(t, SWITCH_PARKED);
    }
}

void rask_fiber_park(const void *key, const char *what) {
    GreenTask *t = tl_current_task;
    WaitBucket *b = bucket_for(key);
    t->wait_what = what;
    pthread_mutex_lock(&b->lock);
    atomic_store_explicit(&t->park, PARK_PARKING, memory_order_release);
    bucket_add(b, t, key);
    pthread_mutex_unlock(&b->lock);
    switch_to_worker(t, SWITCH_PARKED);
}

static int try_mutex(void *m) { return pthread_mutex_trylock((pthread_mutex_t *)m) == 0; }
static int try_rd(void *l) { return pthread_rwlock_tryrdlock((pthread_rwlock_t *)l) == 0; }
static int try_wr(void *l) { return pthread_rwlock_trywrlock((pthread_rwlock_t *)l) == 0; }

void rask_fiber_mutex_lock(pthread_mutex_t *m, const char *what) {
    park_until(m, try_mutex, m, what);
}
void rask_fiber_rwlock_rdlock(pthread_rwlock_t *l, const char *what) {
    park_until(l, try_rd, l, what);
}
void rask_fiber_rwlock_wrlock(pthread_rwlock_t *l, const char *what) {
    park_until(l, try_wr, l, what);
}

// ─── Waiting on a socket ────────────────────────────────────

typedef struct {
    int   fd;
    short events;
} IoWait;

static int io_ready(void *arg) {
    IoWait *w = (IoWait *)arg;
    struct pollfd p = { .fd = w->fd, .events = w->events };
    // An error or a hang-up counts: the syscall the caller retries reports it.
    return poll(&p, 1, 0) > 0;
}

// Ready, or the task was cancelled while waiting (conc.async/CN3).
static int io_ready_or_cancelled(void *arg) {
    return rask_cancel_requested() || io_ready(arg);
}

static void wake_io_waiter(RaskCancelWake *w) {
    rask_fiber_notify(w->a, 1);
}

// Wait until a non-blocking socket can be read (or accepted on) or written.
// On a fiber this parks and gives the worker to other tasks; anywhere else it
// blocks the thread in poll. 1 means a cancel ended the wait.
int rask_io_wait(int64_t fd, int64_t want_write) {
    GreenTask *t = tl_current_task;
    GreenScheduler *s = g_sched;
    short events = want_write ? POLLOUT : POLLIN;
    if (!t || !s || s->epfd < 0) {
        return rask_thread_io_wait(fd, want_write);
    }
    struct epoll_event ev = {
        .events = EPOLLIN | EPOLLOUT | EPOLLRDHUP | EPOLLET,
        .data.u64 = (uint64_t)(uint32_t)fd,
    };
    if (epoll_ctl(s->epfd, EPOLL_CTL_ADD, (int)fd, &ev) < 0 && errno != EEXIST) {
        // Not something epoll can watch.
        return rask_thread_io_wait(fd, want_write);
    }
    IoWait w = { .fd = (int)fd, .events = events };
    const void *key = io_key((int)fd, (int)want_write);
    RaskCancelWake wake = { .wake = wake_io_waiter, .a = (void *)key };
    rask_cancel_wait_begin(&wake);
    atomic_fetch_add_explicit(&s->io_waiters, 1, memory_order_seq_cst);
    park_until(key, io_ready_or_cancelled, &w,
               want_write ? "a socket to take a write" : "a socket to be readable");
    atomic_fetch_sub_explicit(&s->io_waiters, 1, memory_order_seq_cst);
    rask_cancel_wait_end();
    return rask_cancel_requested();
}

// What makes a thread a worker here, carried with the rest of a thread's
// state when sim switches threads on its one OS thread.
typedef struct {
    Worker    *worker;
    GreenTask *task;
    uint32_t   rng;
} GreenThreadTls;

size_t rask_green_thread_tls_size(void) {
    return sizeof(GreenThreadTls);
}

#define SWAP(a, b) do { __typeof__(a) tmp_ = (a); (a) = (b); (b) = tmp_; } while (0)
void rask_green_thread_tls_swap(void *blob) {
    GreenThreadTls *t = (GreenThreadTls *)blob;
    SWAP(tl_worker, t->worker);
    SWAP(tl_current_task, t->task);
    SWAP(tl_rng_state, t->rng);
}
#undef SWAP

#ifdef RASK_SIM
// Sim's stuck report lists threads; the tasks park here, so they come from here.
size_t rask_green_describe_waits(char *buf, size_t cap) {
    size_t used = 0;
    for (int i = 0; i < WAIT_BUCKETS && used < cap; i++) {
        for (GreenTask *t = g_wait[i].head; t && used < cap; t = t->wait_next) {
            char who[32];
            snprintf(who, sizeof(who), "task %lld", (long long)rask_task_id(t->task));
            int n = snprintf(buf + used, cap - used, "\n  %-18s waiting on %s", who,
                             t->wait_what ? t->wait_what : "a wakeup");
            if (n > 0) used += (size_t)n;
        }
    }
    return used < cap ? used : cap;
}
#endif

// Every parked task and what it waits on, for the deadlock report.
static void report_waits(FILE *out) {
    for (int i = 0; i < WAIT_BUCKETS; i++) {
        WaitBucket *b = &g_wait[i];
        pthread_mutex_lock(&b->lock);
        for (GreenTask *t = b->head; t; t = t->wait_next) {
            fprintf(out, "  task %lld waiting on %s\n", (long long)rask_task_id(t->task),
                    t->wait_what ? t->wait_what : "a wakeup");
        }
        pthread_mutex_unlock(&b->lock);
    }
}

// A cancel ending a sleep: take the fiber off the timer list, so the timer
// can't wake it a second time later, and make it runnable. If the timer got
// there first it's already on its way.
static void wake_sleeper(RaskCancelWake *w) {
    GreenTask *t = (GreenTask *)w->a;
    GreenScheduler *s = g_sched;
    int found = 0;
    pthread_mutex_lock(&s->timers_lock);
    for (GreenTask **link = &s->timers; *link; link = &(*link)->timer_next) {
        if (*link == t) {
            *link = t->timer_next;
            t->timer_next = NULL;
            atomic_fetch_sub_explicit(&s->timer_count, 1, memory_order_relaxed);
            found = 1;
            break;
        }
    }
    pthread_mutex_unlock(&s->timers_lock);
    if (found) task_wake(t);
}

int rask_fiber_sleep_ns(int64_t ns) {
    GreenTask *t = tl_current_task;
    GreenScheduler *s = g_sched;
    if (ns <= 0) {
        fiber_yield();
        return 0;
    }
    RaskCancelWake w = { .wake = wake_sleeper, .a = t };
    if (rask_cancel_wait_begin(&w)) {
        rask_cancel_wait_end();
        return 1;
    }
    t->wake_at_ns = now_ns() + ns;
    pthread_mutex_lock(&s->timers_lock);
    atomic_store_explicit(&t->park, PARK_PARKING, memory_order_release);
    t->timer_next = s->timers;
    s->timers = t;
    atomic_fetch_add_explicit(&s->timer_count, 1, memory_order_release);
    pthread_mutex_unlock(&s->timers_lock);
    // A sleeping worker may be waiting longer than this timer.
    for (int i = 0; i < s->worker_count; i++) worker_wake(&s->workers[i]);
    switch_to_worker(t, SWITCH_PARKED);
    rask_cancel_wait_end();
    return rask_cancel_requested();
}

// ─── Spawn / Join / Detach / Cancel ─────────────────────────

// `result_owned` says the closure hands back a heap box rather than a plain
// value — see `RaskTask::result_owned`. The compiler knows the payload type and
// passes it; the runtime only needs to know whether to free.
RaskTask *rask_green_closure_spawn(void *closure_ptr, int64_t result_owned) {
    GreenScheduler *s = g_sched;
    if (!s) {
        rask_panic("spawn outside `using Multitasking {}` block");
    }
    GreenTask *t = task_new();
    // The closure's return value is the task's result — it's what `h.join()`
    // hands back.
    t->body = *(int64_t (**)(void *))(closure_ptr);
    t->body_arg = (char *)closure_ptr + 8;
    t->task = rask_task_new();
    rask_task_adopt_closure(t->task, closure_ptr, result_owned);
    // Held by the handle; the fiber keeps its own ref until it's done.
    RaskTask *handle = t->task;
    atomic_fetch_add_explicit(&s->active_tasks, 1, memory_order_relaxed);
    sched_enqueue_new(s, t);
    return handle;
}
