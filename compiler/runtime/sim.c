// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Sim mode: one thread at a time, in an order drawn from a seed (sim/S1–S5).
//
// This file plays the operating system. What it schedules are the program's
// threads: the test body, each ThreadPool worker, and each of the green
// scheduler's workers. Every one is a fiber on the one OS thread that runs the
// test (fiber.c). At a scheduling point the running thread draws the next one
// from the scheduler stream and switches straight to it. Nothing else runs,
// so the program is single-threaded and the order comes from the seed alone.
//
// Tasks are the green scheduler's (green.c), as in production: its workers
// take them from the same run queues, steal from each other, preempt and park
// them. What green.c would leave to the machine — which worker runs next,
// which one a steal picks, when a running task is cut off — it draws from the
// seed instead (#1381). A task's wait parks its fiber and leaves the worker to
// green.c; only a thread with no task on it parks here.
//
// It used to be a baton over OS threads: each task on its own thread, all but
// one asleep on a condition variable. Fibers make the deterministic tests run
// the switch, the stacks and the per-task state swap that ship (ROADMAP v0.5).
//
// All scheduling points are runtime calls (sim.h), so a task only ever
// switches from inside the runtime, and never while it holds a lock another
// task could want.
//
// The clock is virtual (sim/C1–C3): it starts at zero, moves 1 µs per
// scheduling step, and jumps to the next timer when nothing is runnable.
//
// Built only with -DRASK_SIM. An ordinary build compiles this file to nothing.

#include "rask_runtime.h"

#ifdef RASK_SIM

#include "sim.h"
#include "fiber.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

// ─── Seed streams (sim/SD1–SD3) ─────────────────────────────

static uint64_t splitmix64(uint64_t *state) {
    uint64_t z = (*state += 0x9e3779b97f4a7c15ULL);
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}

// Derive an independent stream from the test seed. Each consumer gets its own
// tag, so draws in one never shift another.
static uint64_t stream_seed(uint64_t seed, uint64_t tag) {
    uint64_t s = seed ^ (tag * 0xd1b54a32d192ed03ULL);
    return splitmix64(&s);
}

#define STREAM_SCHED 1
#define STREAM_HASH  2
#define STREAM_TASK  3
#define STREAM_FAULT 4

// ─── Tasks ──────────────────────────────────────────────────

typedef enum {
    SIM_RUNNABLE,
    SIM_PARKED,
    SIM_SLEEPING,
    SIM_DONE,
} SimTaskState;

#define SIM_POOL_WORKER  (-1)  // task_id of a ThreadPool worker
#define SIM_GREEN_WORKER (-2)  // task_id of a green scheduler worker

typedef struct SimTask {
    int64_t          index;       // spawn order; 0 is the test body
    int64_t          task_id;     // the id panics print (ctrl.panic/F1)
    SimTaskState     state;
    const void      *key;         // parked on
    const char      *what;        // what the park is for, for the deadlock report
    const struct SimTask *joining; // set while parked in join
    int64_t          deadline_ns; // sleeping until
    uint64_t         random;      // the task's user-random stream (SD3)
    RaskFiber        fiber;       // the test's own stack for task 0
    void            *tls;         // its thread-local state while switched out
    void            *green_tls;   // green.c's share of it (which worker, which task)
    int64_t          worker;      // green worker number, for reports
    void           (*entry)(void *);
    void            *arg;
} SimTask;

static struct {
    int              active;
    pthread_mutex_t  lock;
    SimTask        **tasks;
    int64_t          count;
    int64_t          cap;
    SimTask         *current;
    uint64_t         seed;
    int64_t          max_steps;   // sim/S5a
    uint64_t         sched;
    uint64_t         fault;
    int64_t          faults;      // SIM_FAULT_* bits the test asked for (F2)
    int64_t          wall_jump_ns; // accumulated ClockJump, SystemTime only
    char             sick_log[2048];
    char             fault_log[4096];
    int64_t          step;
    int64_t          now_ns;
    SimTask         *reap;        // finished, its stack still to give back
} g = { .lock = PTHREAD_MUTEX_INITIALIZER };

static __thread SimTask *tl_self;

static SimTask *task_alloc(int64_t task_id) {
    SimTask *t = (SimTask *)calloc(1, sizeof(SimTask));
    if (!t) {
        fprintf(stderr, "sim: out of memory creating a task\n");
        _exit(1);
    }
    if (g.count == g.cap) {
        g.cap = g.cap ? g.cap * 2 : 16;
        g.tasks = (SimTask **)realloc(g.tasks, (size_t)g.cap * sizeof(SimTask *));
        if (!g.tasks) {
            fprintf(stderr, "sim: out of memory growing the task table\n");
            _exit(1);
        }
    }
    t->index = g.count;
    t->task_id = task_id;
    t->state = SIM_RUNNABLE;
    t->random = stream_seed(g.seed, STREAM_TASK + ((uint64_t)t->index << 8));
    // Zeroed is a task that hasn't started (rask_task_tls_swap).
    t->tls = calloc(1, rask_task_tls_size());
    t->green_tls = calloc(1, rask_green_thread_tls_size() + 1);
    if (!t->tls || !t->green_tls) {
        fprintf(stderr, "sim: out of memory creating a task\n");
        _exit(1);
    }
    g.tasks[g.count++] = t;
    return t;
}

// ─── Stuck reports (sim/S5, S5a) ────────────────────────────

static const char *task_name(const SimTask *t, char *buf, size_t cap) {
    if (t->index == 0) snprintf(buf, cap, "task %lld (main)", (long long)t->task_id);
    else if (t->task_id == SIM_POOL_WORKER) snprintf(buf, cap, "pool worker");
    else if (t->task_id == SIM_GREEN_WORKER) snprintf(buf, cap, "worker %lld", (long long)t->worker);
    else snprintf(buf, cap, "task %lld", (long long)t->task_id);
    return buf;
}

// `headline`, then one line per task that hasn't finished: what it waits on,
// or that it could run. Where it happened travels as the failure's step and
// time, which the runner prints for every sim failure alike.
_Noreturn static void stuck_locked(const char *headline, const char *tail) {
    char msg[4096];
    size_t used = 0;

#define APPEND(...) do { \
        if (used < sizeof(msg)) used += (size_t)snprintf(msg + used, sizeof(msg) - used, __VA_ARGS__); \
    } while (0)

    APPEND("%s", headline);
    for (int64_t i = 0; i < g.count; i++) {
        SimTask *t = g.tasks[i];
        char who[32];
        task_name(t, who, sizeof(who));
        if (t->state == SIM_PARKED && t->joining) {
            APPEND("\n  %-18s waiting on join(task %lld)", who, (long long)t->joining->task_id);
        } else if (t->state == SIM_PARKED && t->task_id == SIM_GREEN_WORKER) {
            // An idle worker; the tasks it would run are listed below.
            continue;
        } else if (t->state == SIM_PARKED) {
            APPEND("\n  %-18s waiting on %s", who, t->what ? t->what : "a wakeup");
        } else if (t->state == SIM_SLEEPING) {
            APPEND("\n  %-18s sleeping", who);
        } else if (t->state == SIM_RUNNABLE) {
            APPEND("\n  %-18s running", who);
        }
    }
    // The tasks, which park on green.c's side and so aren't in the table.
    if (used < sizeof(msg)) used += rask_green_describe_waits(msg + used, sizeof(msg) - used);
    if (used > sizeof(msg)) used = sizeof(msg);
    if (tail) APPEND("\n  %s", tail);
#undef APPEND

    rask_test_sim_fail(msg);
}

static void deadlock_locked(void) {
    stuck_locked("deadlock: no task can make progress", "no timers pending");
}

// A test that spins — polling an atomic, `try_receive` or `try_lock` in a
// loop — never parks, so it can never be proven stuck. The budget turns that
// into a failure at a step the seed decides, instead of a hang.
static void over_budget_locked(void) {
    char headline[160];
    snprintf(headline, sizeof(headline),
             "the test used its %lld scheduling steps without finishing — "
             "a task is probably spinning on something nobody will change",
             (long long)g.max_steps);
    stuck_locked(headline, "raise the budget with `--max-steps` if the test is just long");
}

// ─── The switch ─────────────────────────────────────────────

// A finished task's stack can't be given back while it is still the one
// running, so the task switched to does it.
static void reap_locked(void) {
    SimTask *t = g.reap;
    if (!t) return;
    g.reap = NULL;
    rask_fiber_destroy(&t->fiber);
    free(t->tls);
    t->tls = NULL;
    free(t->green_tls);
    t->green_tls = NULL;
}

// Hand the thread to `next`. Returns when something switches back to `self`,
// which never happens once `self` is done.
static void switch_to_locked(SimTask *self, SimTask *next) {
    // Every task shares this thread, so its thread-local state goes with it:
    // out of the thread into `self`'s blob, and `next`'s back in.
    rask_task_tls_swap(self->tls);
    rask_task_tls_swap(next->tls);
    rask_green_thread_tls_swap(self->green_tls);
    rask_green_thread_tls_swap(next->green_tls);
    tl_self = next;
    // Held across the switch it would be held by whoever runs next, on the
    // same thread, and the first thing they do is take it.
    pthread_mutex_unlock(&g.lock);
    if (self->state == SIM_DONE) {
        g.reap = self;
        rask_fiber_switch_final(&self->fiber, &next->fiber);
    }
    rask_fiber_switch(&self->fiber, &next->fiber);
    pthread_mutex_lock(&g.lock);
    reap_locked();
}

// ─── Picking who runs ───────────────────────────────────────

static void wake_expired_locked(void) {
    for (int64_t i = 0; i < g.count; i++) {
        SimTask *t = g.tasks[i];
        if (t->state == SIM_SLEEPING && t->deadline_ns <= g.now_ns) {
            t->state = SIM_RUNNABLE;
        }
    }
}

// Nothing is runnable: move the clock to the earliest deadline (sim/C2).
static int jump_to_next_timer_locked(void) {
    int found = 0;
    int64_t earliest = 0;
    for (int64_t i = 0; i < g.count; i++) {
        SimTask *t = g.tasks[i];
        if (t->state == SIM_SLEEPING && (!found || t->deadline_ns < earliest)) {
            earliest = t->deadline_ns;
            found = 1;
        }
    }
    if (!found) return 0;
    if (earliest > g.now_ns) g.now_ns = earliest;
    wake_expired_locked();
    return 1;
}

// Uniform among runnable, from the scheduler stream (sim/S2).
static SimTask *pick_locked(void) {
    int64_t runnable = 0;
    for (int64_t i = 0; i < g.count; i++) {
        if (g.tasks[i]->state == SIM_RUNNABLE) runnable++;
    }
    if (runnable == 0) return NULL;
    int64_t n = (int64_t)(splitmix64(&g.sched) % (uint64_t)runnable);
    for (int64_t i = 0; i < g.count; i++) {
        if (g.tasks[i]->state != SIM_RUNNABLE) continue;
        if (n-- == 0) return g.tasks[i];
    }
    return NULL;
}

// One scheduling step. `self` has already set its own state: RUNNABLE at a
// plain point, PARKED or SLEEPING when it waits, DONE when it exits.
static void schedule_locked(SimTask *self) {
    g.step++;
    if (g.step > g.max_steps) over_budget_locked();
    g.now_ns += 1000;
    wake_expired_locked();

    SimTask *next = pick_locked();
    if (!next && jump_to_next_timer_locked()) next = pick_locked();
    if (!next) deadlock_locked();

    if (next == self) return;
    g.current = next;
    switch_to_locked(self, next);
}

static SimTask *self_or_die(const char *where) {
    SimTask *self = tl_self;
    if (!self || g.current != self) {
        fprintf(stderr, "sim: %s called from a thread sim doesn't schedule\n", where);
        _exit(1);
    }
    return self;
}

// ─── Scheduling points (sim.h) ──────────────────────────────

int rask_sim_active(void) {
    return g.active;
}

void rask_sim_point(void) {
    if (!g.active) return;
    pthread_mutex_lock(&g.lock);
    schedule_locked(self_or_die("a scheduling point"));
    pthread_mutex_unlock(&g.lock);
}

static void park_locked(SimTask *self, const void *key, const char *what) {
    self->state = SIM_PARKED;
    self->key = key;
    self->what = what;
    schedule_locked(self);
    self->key = NULL;
    self->what = NULL;
}

void rask_sim_park(const void *key, const char *what) {
    // A task parks its fiber and its worker moves on. No scheduling point
    // first: the caller checked its condition, and a wake landing between
    // that check and the park would be lost.
    if (rask_fiber_active()) {
        rask_fiber_park(key, what);
        return;
    }
    pthread_mutex_lock(&g.lock);
    park_locked(self_or_die("a wait"), key, what);
    pthread_mutex_unlock(&g.lock);
}

static void notify_locked(const void *key) {
    for (int64_t i = 0; i < g.count; i++) {
        SimTask *t = g.tasks[i];
        if (t->state == SIM_PARKED && t->key == key) t->state = SIM_RUNNABLE;
    }
}

void rask_sim_notify(const void *key) {
    rask_fiber_notify(key, 1);
    if (!g.active) return;
    pthread_mutex_lock(&g.lock);
    notify_locked(key);
    pthread_mutex_unlock(&g.lock);
}

// `pthread_cond_signal` wakes one waiter, and which one is up to the system.
// Under sim the seed picks, so code that signals where it should broadcast —
// two conditions sharing one variable, and the wrong waiter woken — fails on
// some seed instead of passing every time.
// A green task parked on the key may be woken as well: a wake can be spurious,
// and every waiter loops on its own condition.
void rask_sim_notify_one(const void *key) {
    rask_fiber_notify(key, 0);
    if (!g.active) return;
    pthread_mutex_lock(&g.lock);
    int64_t waiting = 0;
    for (int64_t i = 0; i < g.count; i++) {
        if (g.tasks[i]->state == SIM_PARKED && g.tasks[i]->key == key) waiting++;
    }
    if (waiting > 0) {
        int64_t n = (int64_t)(splitmix64(&g.sched) % (uint64_t)waiting);
        for (int64_t i = 0; i < g.count; i++) {
            SimTask *t = g.tasks[i];
            if (t->state != SIM_PARKED || t->key != key) continue;
            if (n-- == 0) {
                t->state = SIM_RUNNABLE;
                break;
            }
        }
    }
    pthread_mutex_unlock(&g.lock);
}

void rask_sim_sleep(int64_t ns) {
    if (rask_fiber_active()) {
        rask_sim_point();
        rask_fiber_sleep_ns(ns);
        return;
    }
    pthread_mutex_lock(&g.lock);
    SimTask *self = self_or_die("sleep");
    if (ns > 0) {
        self->state = SIM_SLEEPING;
        // A sleep past the end of time is forever, not a wrap into the past.
        self->deadline_ns = ns > INT64_MAX - g.now_ns ? INT64_MAX : g.now_ns + ns;
    }
    schedule_locked(self);
    pthread_mutex_unlock(&g.lock);
}

void *rask_sim_self(void) {
    return self_or_die("a cancellable wait");
}

void rask_sim_wake(void *task) {
    pthread_mutex_lock(&g.lock);
    SimTask *t = (SimTask *)task;
    if (t->state == SIM_SLEEPING) t->state = SIM_RUNNABLE;
    pthread_mutex_unlock(&g.lock);
}

// A clock read is a scheduling step (sim/C3): observing time costs time.
int64_t rask_sim_now_ns(void) {
    rask_sim_point();
    return g.now_ns;
}

// `sim.require(faults: [...])` (sim/F2).
int64_t rask_sim_enable_faults(int64_t mask) {
    if (!g.active) return 0;
    g.faults |= mask;
    return 1;
}

int rask_sim_fault_enabled(int64_t bit) {
    return g.active && (g.faults & bit) != 0;
}

// ─── Sickness (sim/F4) ──────────────────────────────────────
//
// Faults land on resources, not on everything: at open, the seed decides
// whether that file or connection is sick for this run, and only a sick one
// ever fails. The two rates are the spec's open question; these are the v1
// answers, and the report names the resource rather than a percentage.

#define SICK_ONE_IN      4   // resources opened while a fault is enabled
#define FAIL_ONE_IN      3   // operations on a sick resource
#define CLOCK_JUMP_ONE_IN 8  // SystemTime reads with ClockJump enabled

static void log_append(char *log, size_t cap, const char *text) {
    size_t used = strlen(log);
    if (used + 3 >= cap) return;
    snprintf(log + used, cap - used, "%s%s", used ? "; " : "", text);
}

// The faults that led up to a failure are the ones worth reading, so the
// log keeps the newest and counts what it let go. (It used to keep the
// oldest and drop the rest, which cut off exactly the part that mattered.)
#define FAULTS_KEPT 16
static char   fault_ring[FAULTS_KEPT][192];
static int64_t faults_seen;

static void fault_record(const char *line) {
    snprintf(fault_ring[faults_seen % FAULTS_KEPT], sizeof(fault_ring[0]), "%s", line);
    faults_seen++;
}

int rask_sim_draw_sick(int64_t fault_bits, const char *what) {
    if (!g.active || !(g.faults & fault_bits)) return 0;
    if (splitmix64(&g.fault) % SICK_ONE_IN != 0) return 0;
    log_append(g.sick_log, sizeof(g.sick_log), what);
    return 1;
}

int rask_sim_draw_failure(const char *what) {
    if (splitmix64(&g.fault) % FAIL_ONE_IN != 0) return 0;
    char line[512];
    snprintf(line, sizeof(line), "%s (step %lld)", what, (long long)g.step);
    fault_record(line);
    return 1;
}

// SystemTime under ClockJump: sometimes the wall clock leaps forward, the way
// an NTP correction or a suspended VM makes it. `Instant` never does.
int64_t rask_sim_wall_jump_ns(void) {
    if (g.active && (g.faults & SIM_FAULT_CLOCK_JUMP) &&
        splitmix64(&g.fault) % CLOCK_JUMP_ONE_IN == 0) {
        int64_t jump = (int64_t)(1 + splitmix64(&g.fault) % 3600) * 1000000000LL;
        g.wall_jump_ns += jump;
        char line[128];
        snprintf(line, sizeof(line), "SystemTime jumped %llds (step %lld)",
                 (long long)(jump / 1000000000LL), (long long)g.step);
        fault_record(line);
    }
    return g.wall_jump_ns;
}

const char *rask_sim_sick_log(void) { return g.sick_log; }
const char *rask_sim_fault_log(void) {
    g.fault_log[0] = '\0';
    int64_t first = faults_seen > FAULTS_KEPT ? faults_seen - FAULTS_KEPT : 0;
    if (first > 0) {
        char line[64];
        snprintf(line, sizeof(line), "%lld earlier", (long long)first);
        log_append(g.fault_log, sizeof(g.fault_log), line);
    }
    for (int64_t i = first; i < faults_seen; i++) {
        log_append(g.fault_log, sizeof(g.fault_log), fault_ring[i % FAULTS_KEPT]);
    }
    return g.fault_log;
}

// Short reads, latencies and injected errors all draw here, so none of them
// can shift the schedule (sim/SD2).
// How many safe points a task gets before it steps aside for a waiting one
// (thread.c). From the schedule's stream, so different seeds preempt at
// different places and a replay preempts at the same ones. Anywhere from one
// to a couple of thousand: short enough to cut into a small critical section,
// long enough that a spinning test still makes progress.
int64_t rask_sim_preempt_budget(void) {
    return 1 + (int64_t)(splitmix64(&g.sched) % 2048);
}

uint64_t rask_sim_fault_draw(void) {
    return splitmix64(&g.fault);
}

// A choice the green scheduler would leave to timing — a steal's victim, how
// many workers a default scope gets — from the schedule's stream.
uint64_t rask_sim_draw(uint64_t n) {
    return n ? splitmix64(&g.sched) % n : 0;
}

uint64_t rask_sim_random_seed(void) {
    SimTask *self = self_or_die("random");
    return splitmix64(&self->random);
}

// ─── Task lifecycle (thread.c, threadpool.c) ────────────────

// Where every task's fiber starts. Whoever switched here left the lock
// released and may have finished, so its stack comes back first.
static void sim_fiber_main(void *arg) {
    rask_fiber_started();
    // A fresh fiber stack reads as zeros, which hides a slot codegen forgot to
    // write the way a fresh thread stack does.
    rask_poison_stack();
    SimTask *t = (SimTask *)arg;
    pthread_mutex_lock(&g.lock);
    reap_locked();
    pthread_mutex_unlock(&g.lock);

    t->entry(t->arg);

    pthread_mutex_lock(&g.lock);
    t->state = SIM_DONE;
    notify_locked(t);
    schedule_locked(t);
    // Not reached: nothing is runnable but a finished task is not either, so
    // the switch above is final or sim reported a deadlock and exited.
    abort();
}

// Called by the spawner, which is the running task, so the new task's place in
// the table (and so its streams) is fixed by the spawn and nothing else. It is
// runnable from here and first runs when the seed picks it.
void *rask_sim_task_spawn(int64_t task_id, void (*entry)(void *), void *arg) {
    pthread_mutex_lock(&g.lock);
    SimTask *t = task_alloc(task_id);
    t->entry = entry;
    t->arg = arg;
    rask_fiber_init(&t->fiber, sim_fiber_main, t);
    pthread_mutex_unlock(&g.lock);
    return t;
}

// A pool worker runs many tasks' bodies, so it has no task id of its own.
void *rask_sim_worker_spawn(void (*entry)(void *), void *arg) {
    return rask_sim_task_spawn(SIM_POOL_WORKER, entry, arg);
}

// One of green.c's workers. Neither has a task id: the tasks they run do.
void *rask_sim_green_worker_spawn(int64_t worker, void (*entry)(void *), void *arg) {
    SimTask *t = (SimTask *)rask_sim_task_spawn(SIM_GREEN_WORKER, entry, arg);
    t->worker = worker;
    return t;
}

void rask_sim_task_join(void *task) {
    SimTask *t = (SimTask *)task;
    pthread_mutex_lock(&g.lock);
    SimTask *self = self_or_die("join");
    // Join is a scheduling point whether or not the task is already done.
    if (t->state == SIM_DONE) {
        schedule_locked(self);
    }
    while (t->state != SIM_DONE) {
        self->joining = t;
        park_locked(self, t, "join");
        self->joining = NULL;
    }
    pthread_mutex_unlock(&g.lock);
}

// ─── The edge of the model (sim/B3) ─────────────────────────

// Falling through to the real call would make the run depend on the machine
// without saying so, so the test fails at the call instead.
void rask_sim_unsimulated(const char *fmt, ...) {
    if (!g.active) return;
    char what[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(what, sizeof(what), fmt, ap);
    va_end(ap);
    rask_panic_fmt("no simulated implementation for %s: sim fails here instead of "
                   "reaching the real machine, which the seed can't replay (sim/B3)",
                   what);
}

// ─── Test lifecycle (test.c) ────────────────────────────────

static char *sim_argv[] = { "<test>", NULL };

void rask_sim_begin(uint64_t seed, int64_t max_steps) {
    pthread_mutex_lock(&g.lock);
    g.seed = seed;
    g.max_steps = max_steps;
    g.sched = stream_seed(seed, STREAM_SCHED);
    g.fault = stream_seed(seed, STREAM_FAULT);
    g.step = 0;
    g.now_ns = 0;
    g.count = 0;
    SimTask *main_task = task_alloc(0);
    rask_fiber_init_thread(&main_task->fiber);
    g.current = main_task;
    tl_self = main_task;
    pthread_mutex_unlock(&g.lock);

    // The world starts sealed (sim/B4): no inherited environment, fixed args.
    clearenv();
    rask_args_init(1, sim_argv);
    rask_map_set_seed(stream_seed(seed, STREAM_HASH));

    g.active = 1;
}

int64_t rask_sim_step(void) {
    return g.step;
}

int64_t rask_sim_time_ns(void) {
    return g.now_ns;
}

#endif // RASK_SIM

#ifndef RASK_SIM
// `sim.require` outside sim: no faults to turn on, so the test is skipped as
// sim-only (sim/F3).
int64_t rask_sim_enable_faults(int64_t mask) {
    (void)mask;
    return 0;
}
#endif
