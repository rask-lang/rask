// SPDX-License-Identifier: (MIT OR Apache-2.0)

// S5: running out of stack is a panic, on every thread that runs Rask code.
//
// Each stack ends in two zones. At the very bottom is the hard guard: the
// page below a fiber's stack (fiber.c), glibc's guard page under a thread's,
// the unmapped space under the main stack. Just above it is the reserve,
// RASK_STACK_RESERVE bytes (a quarter of a smaller stack) kept PROT_NONE
// while the stack is in ordinary use.
// Codegen probes every page of a large frame from the top down, so an
// overflow always faults at the top of the reserve rather than stepping over
// it into another mapping: an 80 MB local array wrote below the main stack and
// died with a bare SIGSEGV (#2066).
//
// The fault opens the reserve and the panic runs in it, on the same stack.
// The handler itself does nothing a signal handler can't: it unprotects the
// reserve, then edits the interrupted context so that returning from the
// handler lands in `overflow_panic` with its stack pointer at the top of the
// reserve. The panic then runs as ordinary code (it allocates its message and
// runs the dying frames' ensures, which a handler that interrupted malloc
// couldn't), and longjmps up the same stack to whatever catches it. The
// catcher closes the reserve again (`rask_stack_guard_rearm`). The JVM's
// yellow zone works the same way.
//
// Staying on the faulting stack is what makes this hold up. A task whose
// ensure parks keeps its frames on its own fiber while the worker runs
// others, and a longjmp never crosses from one stack to another, which TSan
// can't follow. The cost is a bound: while an overflow unwinds, the ensures
// have the reserve to run in and no more. One that runs past it reaches the
// hard guard, and that aborts (A1). So does any other fault in the hard
// guard. A fault anywhere else goes to the handler installed before.
//
// Linux and macOS on x86-64 and arm64. Elsewhere a thread gets no guard, and
// an overflow is the platform's own crash.

#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif

#include "rask_runtime.h"
#include "fiber.h"

#if (defined(__linux__) || defined(__APPLE__)) && (defined(__x86_64__) || defined(__aarch64__))
#define RASK_STACK_GUARD 1
#endif

#ifdef RASK_STACK_GUARD

#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/resource.h>
#if defined(__APPLE__)
#include <sys/ucontext.h>
#else
#include <ucontext.h>
#endif
#include <unistd.h>

#define SIGNAL_STACK (64 * 1024)

static struct sigaction prev_segv;
static struct sigaction prev_bus;
// This thread's own stack: its reserve starts at the bottom, `tl_stack_lo`,
// and is `tl_reserve` bytes. NULL when the thread has no reserve.
static __thread char *tl_stack_lo;
static __thread size_t tl_reserve;
static __thread char *tl_stack_hi;
static __thread char *tl_signal_map;
// What `overflow_panic` reports, set by the handler before it returns there.
static __thread long long tl_overflow_kib;
static __thread int tl_overflow_in_task;

static long page_size(void) {
    return sysconf(_SC_PAGESIZE);
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

// The stack running now: its reserve, the reserve's size and the stack's
// top. A task's when one is running, else the thread's.
static int running_stack(char **reserve, size_t *size, char **top) {
    if (rask_task_stack(reserve, top)) {
        *size = RASK_STACK_RESERVE;
        return 1;
    }
    if (!tl_stack_lo) return 0;
    *reserve = tl_stack_lo;
    *size = tl_reserve;
    *top = tl_stack_hi;
    return 1;
}

// Where the interrupted code resumes once the handler returns: ordinary
// code, at the top of the reserve.
static _Noreturn void overflow_panic(void) {
    rask_panic_fmt("stack overflow: all %lld KiB of the %s's stack is in use "
                   "(unbounded recursion, or a local too large for the stack?)",
                   tl_overflow_kib, tl_overflow_in_task ? "task" : "thread");
}

// Make returning from the handler a call to `overflow_panic` with `sp` as
// its stack: the stack pointer as it would be just after a call instruction,
// with a return address of zero, since the function never returns.
//
// The top of the reserve is always free to use. Everything the interrupted
// frames wrote is above it, since the reserve couldn't be written, and that
// is where the ensures' state lives.
static void resume_in_panic(void *uctx, char *sp_top) {
    ucontext_t *uc = (ucontext_t *)uctx;
    uintptr_t sp = (uintptr_t)sp_top & ~(uintptr_t)15;
    uintptr_t pc = (uintptr_t)overflow_panic;
#if defined(__x86_64__)
    sp -= 8;
    *(uintptr_t *)sp = 0;
#if defined(__APPLE__)
    uc->uc_mcontext->__ss.__rsp = sp;
    uc->uc_mcontext->__ss.__rip = pc;
#else
    uc->uc_mcontext.gregs[REG_RSP] = (greg_t)sp;
    uc->uc_mcontext.gregs[REG_RIP] = (greg_t)pc;
#endif
#elif defined(__aarch64__)
#if defined(__APPLE__)
    __darwin_arm_thread_state64_set_sp(uc->uc_mcontext->__ss, sp);
    __darwin_arm_thread_state64_set_lr_fptr(uc->uc_mcontext->__ss, (void *)0);
    __darwin_arm_thread_state64_set_pc_fptr(uc->uc_mcontext->__ss, (void *)pc);
#else
    uc->uc_mcontext.sp = sp;
    uc->uc_mcontext.regs[30] = 0;
    uc->uc_mcontext.pc = pc;
#endif
#endif
}

static void overflow_handler(int sig, siginfo_t *info, void *uctx) {
    char *addr = info ? (char *)info->si_addr : NULL;
    char *reserve, *top;
    size_t size;
    if (addr && running_stack(&reserve, &size, &top)) {
        char *usable = reserve + size;
        if (addr >= reserve && addr < usable) {
            if (mprotect(reserve, size, PROT_READ | PROT_WRITE) == 0) {
                tl_overflow_in_task = rask_task_stack(&reserve, &top);
                tl_overflow_kib = (long long)((top - usable) / 1024);
                resume_in_panic(uctx, usable);
                return;
            }
        } else if (addr < reserve && addr >= reserve - page_size()) {
            static const char msg[] = "stack overflow while a stack overflow unwinds: "
                                      "an ensure needs more stack than is left\n";
            ssize_t ignored = write(2, msg, sizeof msg - 1);
            (void)ignored;
            signal(SIGABRT, SIG_DFL);
            abort();
        }
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

// The handler runs on a stack of its own, since the faulting one is full.
// Tasks need it too: they run on the thread, so its signal stack serves them.
static void guard_init(int main_thread) {
    pthread_once(&overflow_once, install_overflow_handler);
    char *sig_map = mmap(NULL, SIGNAL_STACK, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (sig_map == MAP_FAILED) return;
    stack_t alt = { .ss_sp = sig_map, .ss_size = SIGNAL_STACK, .ss_flags = 0 };
    if (sigaltstack(&alt, NULL) != 0) {
        munmap(sig_map, SIGNAL_STACK);
        return;
    }
    tl_signal_map = sig_map;

    char *lo, *hi;
    if (!thread_stack_bounds(&lo, &hi)) return;
    // A small stack keeps three quarters of itself.
    long page = page_size();
    size_t reserve = (size_t)(hi - lo) / 4;
    if (reserve > RASK_STACK_RESERVE) reserve = RASK_STACK_RESERVE;
    reserve &= ~(size_t)(page - 1);
    if (reserve == 0) return;
    if (main_thread) {
#if defined(__linux__)
        // An unlimited main stack has no bottom to guard: it grows until it
        // meets a mapping.
        struct rlimit rl;
        if (getrlimit(RLIMIT_STACK, &rl) != 0 || rl.rlim_cur == RLIM_INFINITY) return;
        // The main stack is mapped as it grows, so the pages the reserve
        // goes on don't exist yet, and mprotect has nothing to protect.
        // Touching the reserve's lowest page grows the stack down to there at
        // once, without filling in the pages between. It starts a page above
        // the limit, which leaves the unmapped space below as the hard guard
        // and room for however the C library rounded `lo`.
        lo += page;
        (void)*(volatile char *)lo;
#endif
    }
    if (mprotect(lo, reserve, PROT_NONE) != 0) return;
    tl_stack_lo = lo;
    tl_reserve = reserve;
    tl_stack_hi = hi;
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
    // The C library hands this stack to the next thread it starts, which may
    // not be one of ours.
    if (tl_stack_lo) {
        mprotect(tl_stack_lo, tl_reserve, PROT_READ | PROT_WRITE);
        tl_stack_lo = tl_stack_hi = NULL;
    }
}

// A panic caught inside an ensure that is itself running in the reserve
// leaves it open: closing it there would pull the stack out from under that
// ensure. The catch that ends the overflow closes it.
void rask_stack_guard_rearm(void) {
    char *reserve, *top;
    size_t size;
    char here;
    if (running_stack(&reserve, &size, &top) && &here >= reserve + size) {
        mprotect(reserve, size, PROT_NONE);
    }
}

#else

void rask_stack_guard_main_init(void) {}
void rask_stack_guard_thread_init(void) {}
void rask_stack_guard_thread_exit(void) {}
void rask_stack_guard_rearm(void) {}

#endif
