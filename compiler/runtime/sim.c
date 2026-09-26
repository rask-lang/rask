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
#define STREAM_FAULT 4

// ─── Tasks ──────────────────────────────────────────────────

typedef enum {
    SIM_RUNNABLE,
    SIM_PARKED,
    SIM_SLEEPING,
    SIM_DONE,
} SimTaskState;

#define SIM_POOL_WORKER (-1)   // task_id of a ThreadPool worker

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
    int64_t          max_steps;   // sim/S5a
    uint64_t         sched;
    uint64_t         fault;
    int64_t          faults;      // SIM_FAULT_* bits the test asked for (F2)
    int64_t          wall_jump_ns; // accumulated ClockJump, SystemTime only
    char             sick_log[2048];
    char             fault_log[4096];
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

// ─── Stuck reports (sim/S5, S5a) ────────────────────────────

static const char *task_name(const SimTask *t, char *buf, size_t cap) {
    if (t->index == 0) snprintf(buf, cap, "task %lld (main)", (long long)t->task_id);
    else if (t->task_id == SIM_POOL_WORKER) snprintf(buf, cap, "pool worker");
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
        } else if (t->state == SIM_PARKED) {
            APPEND("\n  %-18s waiting on %s", who, t->what ? t->what : "a wakeup");
        } else if (t->state == SIM_SLEEPING) {
            APPEND("\n  %-18s sleeping", who);
        } else if (t->state == SIM_RUNNABLE) {
            APPEND("\n  %-18s running", who);
        }
    }
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
    if (g.step > g.max_steps) over_budget_locked();
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

// `pthread_cond_signal` wakes one waiter, and which one is up to the system.
// Under sim the seed picks, so code that signals where it should broadcast —
// two conditions sharing one variable, and the wrong waiter woken — fails on
// some seed instead of passing every time.
void rask_sim_notify_one(const void *key) {
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
uint64_t rask_sim_fault_draw(void) {
    return splitmix64(&g.fault);
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

// A pool worker runs many tasks' bodies, so it has no task id of its own.
void *rask_sim_worker_new(void) {
    return rask_sim_task_new(SIM_POOL_WORKER);
}

// A task whose thread couldn't be started. It is done before it began, so
// nothing picks it and nothing waits for it.
void rask_sim_task_abandon(void *task) {
    SimTask *t = (SimTask *)task;
    pthread_mutex_lock(&g.lock);
    t->state = SIM_DONE;
    notify_locked(t);
    pthread_mutex_unlock(&g.lock);
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
