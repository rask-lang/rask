// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Sim mode: one task at a time, in an order drawn from a seed (sim/S1–S5).
//
// Every task still runs on its own OS thread, the way thread.c starts it. What
// sim adds is a baton: only the thread whose task is `g.current` executes Rask
// code, and every other task thread sleeps on its own condition variable. At a
// scheduling point the running task draws the next one from the scheduler
// stream, hands the baton over, and waits until it comes back.
//
// All scheduling points are runtime calls (sim.h), so a task only ever gives
// the baton up from inside the runtime, where blocking its thread is safe.
//
// The clock is virtual (sim/C1–C3): it starts at zero, moves 1 µs per
// scheduling step, and jumps to the next timer when nothing is runnable.
//
// Built only with -DRASK_SIM. An ordinary build compiles this file to nothing.

#include "rask_runtime.h"

#ifdef RASK_SIM

#include "sim.h"

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

// ─── Tasks ──────────────────────────────────────────────────

typedef enum {
    SIM_RUNNABLE,
    SIM_PARKED,
    SIM_SLEEPING,
    SIM_DONE,
} SimTaskState;

typedef struct SimTask {
    int64_t          index;       // spawn order; 0 is the test body
    int64_t          task_id;     // the id panics print (ctrl.panic/F1)
    SimTaskState     state;
    const void      *key;         // parked on
    const char      *what;        // what the park is for, for the deadlock report
    const struct SimTask *joining; // set while parked in join
    int64_t          deadline_ns; // sleeping until
    uint64_t         random;      // the task's user-random stream (SD3)
    pthread_cond_t   turn;        // signalled when this task gets the baton
} SimTask;

static struct {
    int              active;
    pthread_mutex_t  lock;
    SimTask        **tasks;
    int64_t          count;
    int64_t          cap;
    SimTask         *current;
    uint64_t         seed;
    uint64_t         sched;
    int64_t          step;
    int64_t          now_ns;
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
    pthread_cond_init(&t->turn, NULL);
    g.tasks[g.count++] = t;
    return t;
}

// ─── Deadlock report (sim/S5) ───────────────────────────────

static void deadlock_locked(void) {
    char msg[4096];
    size_t used = 0;

#define APPEND(...) do { \
        if (used < sizeof(msg)) used += (size_t)snprintf(msg + used, sizeof(msg) - used, __VA_ARGS__); \
    } while (0)

    // Where it happened travels as the failure's step and time, which the
    // runner prints for every sim failure alike.
    APPEND("deadlock: no task can make progress");
    for (int64_t i = 0; i < g.count; i++) {
        SimTask *t = g.tasks[i];
        if (t->state != SIM_PARKED) continue;
        char who[32];
        if (t->index == 0) snprintf(who, sizeof(who), "task %lld (main)", (long long)t->task_id);
        else snprintf(who, sizeof(who), "task %lld", (long long)t->task_id);
        if (t->joining) {
            APPEND("\n  %-18s waiting on join(task %lld)", who, (long long)t->joining->task_id);
        } else {
            APPEND("\n  %-18s waiting on %s", who, t->what ? t->what : "a wakeup");
        }
    }
    APPEND("\n  no timers pending");
#undef APPEND

    rask_test_sim_fail(msg);
}

// ─── The baton ──────────────────────────────────────────────

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
    g.now_ns += 1000;
    wake_expired_locked();

    SimTask *next = pick_locked();
    if (!next && jump_to_next_timer_locked()) next = pick_locked();
    if (!next) deadlock_locked();

    if (next == self) return;
    g.current = next;
    pthread_cond_signal(&next->turn);
    if (self->state == SIM_DONE) return;
    while (g.current != self) {
        pthread_cond_wait(&self->turn, &g.lock);
    }
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
    if (!g.active) return;
    pthread_mutex_lock(&g.lock);
    notify_locked(key);
    pthread_mutex_unlock(&g.lock);
}

void rask_sim_sleep(int64_t ns) {
    pthread_mutex_lock(&g.lock);
    SimTask *self = self_or_die("sleep");
    if (ns > 0) {
        self->state = SIM_SLEEPING;
        self->deadline_ns = g.now_ns + ns;
    }
    schedule_locked(self);
    pthread_mutex_unlock(&g.lock);
}

// A clock read is a scheduling step (sim/C3): observing time costs time.
int64_t rask_sim_now_ns(void) {
    rask_sim_point();
    return g.now_ns;
}

uint64_t rask_sim_random_seed(void) {
    SimTask *self = self_or_die("random");
    return splitmix64(&self->random);
}

// ─── Task lifecycle (thread.c) ──────────────────────────────

// Called by the spawner, which holds the baton, so the new task's place in the
// table (and so its streams) doesn't depend on when its thread starts.
void *rask_sim_task_new(int64_t task_id) {
    pthread_mutex_lock(&g.lock);
    SimTask *t = task_alloc(task_id);
    pthread_mutex_unlock(&g.lock);
    return t;
}

// First thing a task thread does: wait to be picked.
void rask_sim_task_enter(void *task) {
    SimTask *t = (SimTask *)task;
    tl_self = t;
    pthread_mutex_lock(&g.lock);
    while (g.current != t) {
        pthread_cond_wait(&t->turn, &g.lock);
    }
    pthread_mutex_unlock(&g.lock);
}

// Last thing a task thread does. The baton passes on and this thread only
// returns from here, so nothing after it may touch shared state.
void rask_sim_task_exit(void) {
    pthread_mutex_lock(&g.lock);
    SimTask *self = self_or_die("task exit");
    self->state = SIM_DONE;
    notify_locked(self);
    schedule_locked(self);
    pthread_mutex_unlock(&g.lock);
    tl_self = NULL;
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

void rask_sim_begin(uint64_t seed) {
    pthread_mutex_lock(&g.lock);
    g.seed = seed;
    g.sched = stream_seed(seed, STREAM_SCHED);
    g.step = 0;
    g.now_ns = 0;
    g.count = 0;
    SimTask *main_task = task_alloc(0);
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
