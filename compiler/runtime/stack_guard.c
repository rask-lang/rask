// SPDX-License-Identifier: (MIT OR Apache-2.0)

// S5: running out of stack is a panic, on every thread that runs Rask code.
//
// Every Rask function starts by comparing its stack pointer against
// `rask_stack_limit`, this thread's limit, and calls `rask_stack_overflow_at`
// when it is below. So the overflow is noticed by Rask code at a function
// entry, never by a fault in the middle of whatever C code was running, and
// the panic is an ordinary call: it allocates its message, runs the dying
// frames' ensures and longjmps up to whatever catches it.
//
// The limit sits `reserve` bytes above the bottom of the stack (a quarter of
// a small stack, RASK_STACK_RESERVE at most). That room is for what runs
// below a Rask frame that passed its check: the frame itself, and the C
// runtime it calls (malloc, printf, the panic machinery), which has no check
// of its own. A frame too big to fit there gets its check before the frame is
// pushed (codegen splits the function in two), so its own probes never reach
// the guard.
//
// While an overflow unwinds, the limit drops to a quarter of the reserve, so
// the ensures have the rest of it to run in. The catch that ends the overflow
// puts the limit back (`rask_stack_limit_restore`). An ensure that runs past
// the lowered limit aborts (A1): there's no room left to unwind it.
//
// Below the stack is the hard guard: the page under a fiber's stack (fiber.c),
// glibc's guard page under a thread's, the unmapped gap under the main stack.
// Only C code that ran past the whole reserve reaches it, and a fault there
// aborts with a message instead of a bare SIGSEGV.
//
// A task carries its stack's bounds with it: they are part of the state
// `rask_task_tls_swap` moves when a fiber switches on or off a thread, and a
// fiber sets them when it starts (`rask_stack_guard_fiber_start`).

#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif

#include "rask_runtime.h"

#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>

// Read by every Rask function's entry. Zero, which no stack pointer is below,
// on a thread that has no bounds.
__thread char *rask_stack_limit;

typedef struct StackBounds {
    char *limit;
    char *lo;       // the bottom of the usable stack; the hard guard is below
    char *hi;
    char *normal;   // the limit while no overflow is unwinding
    int   in_task;
} StackBounds;

static __thread StackBounds tl_bounds;

static void set_bounds(char *lo, char *hi, int in_task) {
    size_t reserve = (size_t)(hi - lo) / 4;
    if (reserve > RASK_STACK_RESERVE) reserve = RASK_STACK_RESERVE;
    tl_bounds.lo = lo;
    tl_bounds.hi = hi;
    tl_bounds.normal = lo + reserve;
    tl_bounds.in_task = in_task;
    rask_stack_limit = tl_bounds.normal;
}

static void clear_bounds(void) {
    memset(&tl_bounds, 0, sizeof tl_bounds);
    rask_stack_limit = NULL;
}

size_t rask_stack_tls_size(void) {
    return sizeof(StackBounds);
}

void rask_stack_tls_swap(void *blob) {
    StackBounds *b = (StackBounds *)blob;
    tl_bounds.limit = rask_stack_limit;
    StackBounds tmp = tl_bounds;
    tl_bounds = *b;
    *b = tmp;
    rask_stack_limit = tl_bounds.limit;
}

void rask_stack_guard_fiber_start(char *lo, char *hi) {
    set_bounds(lo, hi, 1);
}

_Noreturn void rask_stack_overflow_at(const char *file, int32_t line, int32_t col) {
    if (rask_stack_limit != tl_bounds.normal) {
        static const char msg[] = "stack overflow while a stack overflow unwinds: "
                                  "an ensure needs more stack than is left\n";
        ssize_t ignored = write(2, msg, sizeof msg - 1);
        (void)ignored;
        signal(SIGABRT, SIG_DFL);
        abort();
    }
    rask_stack_limit = tl_bounds.lo + (size_t)(tl_bounds.normal - tl_bounds.lo) / 4;
    long long kib = (long long)((tl_bounds.hi - tl_bounds.normal) / 1024);
    rask_panic_at_owned(file, line, col,
        rask_msg_format("stack overflow: all %lld KiB of the %s's stack is in use "
                        "(unbounded recursion, or a local too large for the stack?)",
                        kib, tl_bounds.in_task ? "task" : "thread"));
}

// Called by every catch once it has landed. A catch inside an ensure that is
// still unwinding an overflow is itself below the normal limit, and leaves
// it lowered; the catch above the limit ends the overflow.
void rask_stack_limit_restore(void) {
    if (rask_stack_limit != tl_bounds.normal
        && (char *)__builtin_frame_address(0) >= tl_bounds.normal) {
        rask_stack_limit = tl_bounds.normal;
    }
}

#if (defined(__linux__) || defined(__APPLE__)) && (defined(__x86_64__) || defined(__aarch64__))
#define RASK_STACK_GUARD 1
#endif

#ifdef RASK_STACK_GUARD

#include <sys/mman.h>
#include <sys/resource.h>

#define SIGNAL_STACK (64 * 1024)
// How far below the stack a fault still counts as running off its end.
#define GUARD_WINDOW (64 * 1024)

static struct sigaction prev_segv;
static struct sigaction prev_bus;
static __thread char *tl_signal_map;

static void guard_handler(int sig, siginfo_t *info, void *uctx) {
    char *addr = info ? (char *)info->si_addr : NULL;
    char *lo = tl_bounds.lo;
    if (addr && lo && addr < lo && addr >= lo - GUARD_WINDOW) {
        static const char msg[] = "stack overflow in C code: the runtime or a C "
                                  "library used more stack than was left below "
                                  "the Rask frame that called it\n";
        ssize_t ignored = write(2, msg, sizeof msg - 1);
        (void)ignored;
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

static pthread_once_t handler_once = PTHREAD_ONCE_INIT;

static void install_guard_handler(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = guard_handler;
    sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGSEGV, &sa, &prev_segv);
    sigaction(SIGBUS, &sa, &prev_bus);
}

// The bounds of the calling thread's stack, or 0 when the platform won't say.
static int thread_stack_bounds(char **lo, char **hi) {
#if defined(__APPLE__)
    pthread_t self = pthread_self();
    *hi = (char *)pthread_get_stackaddr_np(self);
    *lo = *hi - pthread_get_stacksize_np(self);
    return 1;
#else
    pthread_attr_t attr;
    void *addr;
    size_t size;
    if (pthread_getattr_np(pthread_self(), &attr) != 0) return 0;
    int ok = pthread_attr_getstack(&attr, &addr, &size) == 0;
    pthread_attr_destroy(&attr);
    if (!ok) return 0;
    *lo = (char *)addr;
    *hi = *lo + size;
    return 1;
#endif
}

// The handler runs on a stack of its own, since the faulting one is full.
// Tasks run on the thread, so its signal stack serves them too.
static void guard_init(int main_thread) {
    pthread_once(&handler_once, install_guard_handler);
    char *sig_map = mmap(NULL, SIGNAL_STACK, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (sig_map != MAP_FAILED) {
        stack_t alt = { .ss_sp = sig_map, .ss_size = SIGNAL_STACK, .ss_flags = 0 };
        if (sigaltstack(&alt, NULL) == 0) {
            tl_signal_map = sig_map;
        } else {
            munmap(sig_map, SIGNAL_STACK);
        }
    }

    char *lo, *hi;
    if (!thread_stack_bounds(&lo, &hi)) return;
    if (main_thread) {
#if defined(__linux__)
        // An unlimited main stack has no bottom: it grows until it meets a
        // mapping, so there is nothing to measure a limit from.
        struct rlimit rl;
        if (getrlimit(RLIMIT_STACK, &rl) != 0 || rl.rlim_cur == RLIM_INFINITY) return;
        // The C library rounds the main stack's bottom; a page of slack keeps
        // the limit inside what the kernel will actually grow to.
        lo += sysconf(_SC_PAGESIZE);
#endif
    }
    set_bounds(lo, hi, 0);
}

void rask_stack_guard_main_init(void) {
    guard_init(1);
}

// Every other thread that runs Rask code calls this first, and the exit on
// its way out.
void rask_stack_guard_thread_init(void) {
    guard_init(0);
}

void rask_stack_guard_thread_exit(void) {
    if (tl_signal_map) {
        stack_t off = { .ss_flags = SS_DISABLE };
        sigaltstack(&off, NULL);
        munmap(tl_signal_map, SIGNAL_STACK);
        tl_signal_map = NULL;
    }
    clear_bounds();
}

#else

void rask_stack_guard_main_init(void) {}
void rask_stack_guard_thread_init(void) {}
void rask_stack_guard_thread_exit(void) {
    clear_bounds();
}

#endif
