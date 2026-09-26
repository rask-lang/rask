// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// The green scheduler's entry points, backed by OS threads.
//
// `green.c` needs an I/O engine and the only two are epoll and io_uring, so
// there is no green scheduler off Linux. Codegen emitted calls into it anyway,
// so `spawn(|| { … })` on macOS failed at link with a wall of runtime
// internals — "Undefined symbols: _rask_green_spawn, referenced from
// _rask_main" — for a program that wrote no C (#1180).
//
// One OS thread per task is what Phase A concurrency is (conc.strategy/A1), and
// `thread.c` already builds everywhere, so the green names are given bodies in
// terms of it. A task runs on its own thread, and every wait blocks it. The
// observable difference from Linux is that tasks don't multiplex onto a
// worker pool: one spawn is one thread.
//
// This whole file is empty on a build that has the real scheduler.

#include "rask_runtime.h"
#include "sim.h"

#if !RASK_HAS_GREEN

#include <sched.h>
#include <errno.h>
#include <poll.h>
#include <sys/socket.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

// ─── Scheduler lifecycle ───────────────────────────────────
//
// `using Multitasking(n)` brackets its block with these. There is no scheduler
// to start and no workers to size — a task is a thread that lives as long as it
// runs — so init has nothing to do.
//
// Shutdown does: the block waits for its tasks before it exits
// (conc.async/C4), which is what makes a task that hangs or panics fail the
// test that spawned it. A no-op here left `task x done` printing after the test
// had finished, which is the failure the rule exists to prevent.
// `rask_await_detached_tasks` is the same wait `main` does at process exit
// (ctrl.panic/O4) and is safe to run again.

void rask_runtime_init(int64_t worker_count) {
    // There is no scheduler to start, but the worker count still means
    // something: how many tasks may run at once (#1111). `thread.c` holds the
    // count, because every task body funnels through it.
    rask_task_slots_install(worker_count);
}

void rask_runtime_shutdown(void) {
    rask_await_detached_tasks();
    rask_task_slots_clear();
}

// ─── Spawning ──────────────────────────────────────────────

// A closure runs once, and that is exactly what `rask_closure_spawn` does — including freeing the
// closure allocation and carrying `result_owned`, which is how a task's boxed
// return value survives the join.
RaskTask *rask_green_closure_spawn(void *closure_ptr, int64_t result_owned) {
    return rask_closure_spawn(closure_ptr, result_owned);
}

// ─── Fiber waits ────────────────────────────────────────────
//
// sim.h asks these whether a wait is on a green fiber. Here no task is: each
// one owns a thread, and every wait is the pthread one.

int rask_fiber_active(void) { return 0; }
void rask_fiber_notify(const void *key, int all) { (void)key; (void)all; }
void rask_fiber_cond_wait(pthread_cond_t *c, pthread_mutex_t *m, const char *what) {
    (void)what;
    pthread_cond_wait(c, m);
}
void rask_fiber_mutex_lock(pthread_mutex_t *m, const char *what) {
    (void)what;
    pthread_mutex_lock(m);
}
void rask_fiber_rwlock_rdlock(pthread_rwlock_t *l, const char *what) {
    (void)what;
    pthread_rwlock_rdlock(l);
}
void rask_fiber_rwlock_wrlock(pthread_rwlock_t *l, const char *what) {
    (void)what;
    pthread_rwlock_wrlock(l);
}
int rask_fiber_sleep_ns(int64_t ns) { return rask_sleep_ns(ns) != 0; }

// No fibers to park, so waiting on a socket blocks the thread.
int rask_io_wait(int64_t fd, int64_t want_write) {
    return rask_thread_io_wait(fd, want_write);
}

#endif // !RASK_HAS_GREEN
