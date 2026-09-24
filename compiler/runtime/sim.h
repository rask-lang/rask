// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Where tasks meet — the scheduling points (sim/S3, conc.runtime).
//
// Every lock and condition variable that one task can wait on for another goes
// through these wrappers. In an ordinary build a wait on a green fiber parks it
// and anything else is the pthread call (determinism/D2). Built with -DRASK_SIM, the same call sites
// become the places the seeded scheduler decides who runs next: a wait parks
// the task on a key, a signal marks one task parked on that key runnable (a
// broadcast marks all of them), and the thread itself sleeps until the baton
// comes back (sim.c).
//
// A lock that is only ever held for a few instructions and never across a wait
// (a channel's own mutex, the print lock) doesn't need to be here. Under sim
// nothing else can be running while it is held.

#ifndef RASK_SIM_H
#define RASK_SIM_H

#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <sys/stat.h>

#ifdef RASK_SIM

int  rask_sim_active(void);
void rask_sim_point(void);
void rask_sim_park(const void *key, const char *what);
void rask_sim_notify(const void *key);
void rask_sim_notify_one(const void *key);
void rask_sim_sleep(int64_t ns);
int64_t rask_sim_now_ns(void);
uint64_t rask_sim_random_seed(void);
uint64_t rask_sim_fault_draw(void);

// Opt-in faults (sim/F2). The bits match `fault_bit` in stdlib/sim.rk.
#define SIM_FAULT_IO_ERROR   1
#define SIM_FAULT_DISCONNECT 2
#define SIM_FAULT_CLOCK_JUMP 4
int rask_sim_fault_enabled(int64_t bit);
// At open: is this resource sick for the run? Logs it when it is.
int rask_sim_draw_sick(int64_t fault_bits, const char *what);
// On a sick resource: does this operation fail? Logs it with the step.
int rask_sim_draw_failure(const char *what);
int64_t rask_sim_wall_jump_ns(void);
const char *rask_sim_sick_log(void);
const char *rask_sim_fault_log(void);

// Task lifecycle, called from thread.c.
void *rask_sim_task_new(int64_t task_id);
void rask_sim_task_enter(void *task);
void rask_sim_task_exit(void);
void rask_sim_task_join(void *task);
void *rask_sim_worker_new(void);
void rask_sim_task_abandon(void *task);

// Test lifecycle, called from test.c. A sim test runs alone in its process,
// so the failure paths report and exit rather than unwind.
void rask_sim_begin(uint64_t seed, int64_t max_steps);
int64_t rask_sim_step(void);
int64_t rask_sim_time_ns(void);
_Noreturn void rask_test_sim_fail(const char *msg);

// A call sim has no model for (sim/B3). Panics under sim with "no simulated
// implementation for <what>", where the format names the call and what it
// would have reached.
void rask_sim_unsimulated(const char *fmt, ...) __attribute__((format(printf, 1, 2)));

// Filesystem overlay (sim_fs.c, sim/B5). Under sim every file operation goes
// here; the overlay asks the real tree itself for what the test hasn't
// touched.
FILE *rask_sim_fs_fopen(const char *path, const char *mode);
int   rask_sim_fs_rename(const char *from, const char *to);
int   rask_sim_fs_remove(const char *path);
int   rask_sim_fs_mkdir(const char *path);
int   rask_sim_fs_stat(const char *path, struct stat *st);
char **rask_sim_fs_list(const char *path, size_t *count);

// Loopback network (sim_net.c). Sim sockets are fd numbers the real OS never
// hands out; `owns` says whether an fd is one.
int     rask_sim_net_owns(int64_t fd);
int64_t rask_sim_net_listen(const char *host, const char *port);
int64_t rask_sim_net_connect(const char *host, const char *port);
int64_t rask_sim_net_accept(int64_t fd);
int64_t rask_sim_net_read(int64_t fd, void *buf, size_t n);
int64_t rask_sim_net_write(int64_t fd, const void *buf, size_t n);
int     rask_sim_net_close(int64_t fd);
int64_t rask_sim_net_dup(int64_t fd);
void    rask_sim_net_addr(int64_t fd, int remote, char *out, size_t cap);

#define RASK_SIM_POINT() rask_sim_point()
#define RASK_SIM_UNSIMULATED(...) rask_sim_unsimulated(__VA_ARGS__)

static inline void rask_task_cond_wait(pthread_cond_t *c, pthread_mutex_t *m,
                                       const char *what) {
    if (!rask_sim_active()) {
        pthread_cond_wait(c, m);
        return;
    }
    pthread_mutex_unlock(m);
    rask_sim_park(c, what);
    pthread_mutex_lock(m);
}

static inline void rask_task_cond_signal(pthread_cond_t *c) {
    pthread_cond_signal(c);
    rask_sim_notify_one(c);
}

static inline void rask_task_cond_broadcast(pthread_cond_t *c) {
    pthread_cond_broadcast(c);
    rask_sim_notify(c);
}

// A lock held across a scheduling point would block the thread that holds the
// baton, so under sim a contended acquire parks instead of blocking.
static inline void rask_task_mutex_lock(pthread_mutex_t *m, const char *what) {
    if (!rask_sim_active()) {
        pthread_mutex_lock(m);
        return;
    }
    rask_sim_point();
    while (pthread_mutex_trylock(m) != 0) rask_sim_park(m, what);
}

static inline int rask_task_mutex_trylock(pthread_mutex_t *m) {
    RASK_SIM_POINT();
    return pthread_mutex_trylock(m);
}

static inline void rask_task_mutex_unlock(pthread_mutex_t *m) {
    pthread_mutex_unlock(m);
    rask_sim_notify(m);
}

static inline void rask_task_rwlock_rdlock(pthread_rwlock_t *l, const char *what) {
    if (!rask_sim_active()) {
        pthread_rwlock_rdlock(l);
        return;
    }
    rask_sim_point();
    while (pthread_rwlock_tryrdlock(l) != 0) rask_sim_park(l, what);
}

static inline void rask_task_rwlock_wrlock(pthread_rwlock_t *l, const char *what) {
    if (!rask_sim_active()) {
        pthread_rwlock_wrlock(l);
        return;
    }
    rask_sim_point();
    while (pthread_rwlock_trywrlock(l) != 0) rask_sim_park(l, what);
}

static inline int rask_task_rwlock_tryrdlock(pthread_rwlock_t *l) {
    RASK_SIM_POINT();
    return pthread_rwlock_tryrdlock(l);
}

static inline int rask_task_rwlock_trywrlock(pthread_rwlock_t *l) {
    RASK_SIM_POINT();
    return pthread_rwlock_trywrlock(l);
}

static inline void rask_task_rwlock_unlock(pthread_rwlock_t *l) {
    pthread_rwlock_unlock(l);
    rask_sim_notify(l);
}

#else // !RASK_SIM

#define RASK_SIM_POINT() ((void)0)
#define RASK_SIM_UNSIMULATED(...) ((void)0)

// Outside sim, a wait on a green fiber parks the fiber and gives its worker
// back (green.c); anywhere else — the scope's own thread, `Thread.spawn`, a
// pool worker — it is the pthread call. A signal reaches both kinds of waiter,
// so a fiber and a thread can wait on the same condvar or lock. Off Linux,
// green_threads.c answers "not a fiber" and every wait is the pthread one.
//
// A thread blocking here says so (thread.c), so the scheduler can tell a
// deadlock from a task waiting on a thread that is still working.
int  rask_fiber_active(void);
void rask_fiber_notify(const void *key, int all);
void rask_fiber_cond_wait(pthread_cond_t *c, pthread_mutex_t *m, const char *what);
void rask_fiber_mutex_lock(pthread_mutex_t *m, const char *what);
void rask_fiber_rwlock_rdlock(pthread_rwlock_t *l, const char *what);
void rask_fiber_rwlock_wrlock(pthread_rwlock_t *l, const char *what);
void rask_thread_wait_begin(const char *what);
void rask_thread_wait_end(void);

static inline void rask_task_cond_wait(pthread_cond_t *c, pthread_mutex_t *m,
                                       const char *what) {
    if (rask_fiber_active()) {
        rask_fiber_cond_wait(c, m, what);
        return;
    }
    rask_thread_wait_begin(what);
    pthread_cond_wait(c, m);
    rask_thread_wait_end();
}
static inline void rask_task_cond_signal(pthread_cond_t *c) {
    pthread_cond_signal(c);
    rask_fiber_notify(c, 0);
}
static inline void rask_task_cond_broadcast(pthread_cond_t *c) {
    pthread_cond_broadcast(c);
    rask_fiber_notify(c, 1);
}

static inline void rask_task_mutex_lock(pthread_mutex_t *m, const char *what) {
    if (rask_fiber_active()) {
        rask_fiber_mutex_lock(m, what);
        return;
    }
    if (pthread_mutex_trylock(m) == 0) return;
    rask_thread_wait_begin(what);
    pthread_mutex_lock(m);
    rask_thread_wait_end();
}
static inline int rask_task_mutex_trylock(pthread_mutex_t *m) { return pthread_mutex_trylock(m); }
static inline void rask_task_mutex_unlock(pthread_mutex_t *m) {
    pthread_mutex_unlock(m);
    rask_fiber_notify(m, 0);
}

static inline void rask_task_rwlock_rdlock(pthread_rwlock_t *l, const char *what) {
    if (rask_fiber_active()) {
        rask_fiber_rwlock_rdlock(l, what);
        return;
    }
    if (pthread_rwlock_tryrdlock(l) == 0) return;
    rask_thread_wait_begin(what);
    pthread_rwlock_rdlock(l);
    rask_thread_wait_end();
}
static inline void rask_task_rwlock_wrlock(pthread_rwlock_t *l, const char *what) {
    if (rask_fiber_active()) {
        rask_fiber_rwlock_wrlock(l, what);
        return;
    }
    if (pthread_rwlock_trywrlock(l) == 0) return;
    rask_thread_wait_begin(what);
    pthread_rwlock_wrlock(l);
    rask_thread_wait_end();
}
static inline int rask_task_rwlock_tryrdlock(pthread_rwlock_t *l) { return pthread_rwlock_tryrdlock(l); }
static inline int rask_task_rwlock_trywrlock(pthread_rwlock_t *l) { return pthread_rwlock_trywrlock(l); }
static inline void rask_task_rwlock_unlock(pthread_rwlock_t *l) {
    pthread_rwlock_unlock(l);
    // Readers and a writer may all be waiting; each retries.
    rask_fiber_notify(l, 1);
}

#endif // RASK_SIM

#endif // RASK_SIM_H
