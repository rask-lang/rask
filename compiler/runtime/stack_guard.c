// SPDX-License-Identifier: (MIT OR Apache-2.0)

// S5: running into a stack's guard is a panic, on every thread that runs Rask
// code. A task's guard is the page below its fiber stack (fiber.c); a
// thread's is whatever stops its stack growing, the glibc guard page or the
// kernel's gap below the main stack. Codegen probes every page of a large
// frame from the top down, so an overflow faults in the guard rather than
// stepping over it into another mapping: an 80 MB local array wrote below the
// main stack and died with a bare SIGSEGV (#2066).
//
// The panic doesn't run in the signal handler. A panic allocates its message
// and runs the dying frames' ensures, none of which is safe inside a handler:
// one that interrupted malloc would deadlock on malloc's lock. So the handler
// only decides whether this is an overflow, then edits the interrupted
// context so that returning from the handler lands in `overflow_panic` with
// its stack pointer at the top of a per-thread unwind stack. The panic then
// runs as ordinary code, and its unwind longjmps back down to whatever caught
// it on the thread's own stack. Go and the JVM leave their fault handlers the
// same way.
//
// Each thread maps two regions: the small stack the handler itself runs on
// (sigaltstack, since the faulting stack has no room), and the unwind stack
// with a guard page at its low end. An ensure that runs the unwind stack out
// faults in that guard, and an overflow during an unwind has nowhere left to
// go, so either aborts (A1). Any fault that isn't an overflow goes to the
// handler that was installed before.
//
// Linux and macOS on x86-64 and arm64. Elsewhere a thread gets no guard, and
// an overflow is the platform's own crash.

#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif

#include "rask_runtime.h"

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
#if defined(__APPLE__)
#include <sys/ucontext.h>
#else
#include <ucontext.h>
#endif
#include <unistd.h>

#if defined(__has_include)
#if __has_include(<valgrind/valgrind.h>)
#include <valgrind/valgrind.h>
#define RASK_VALGRIND 1
#endif
#endif

#define SIGNAL_STACK (64 * 1024)
#define UNWIND_STACK (256 * 1024)
// Below a thread's lowest stack byte, how far a fault still counts as running
// into its guard: the kernel's gap below the main stack is 256 pages.
#define THREAD_GUARD_REACH (1024 * 1024)

static struct sigaction prev_segv;
static struct sigaction prev_bus;
static __thread const char *tl_stack_lo;
static __thread const char *tl_stack_hi;
static __thread char *tl_signal_map;
// Its lowest page is the guard.
static __thread char *tl_unwind_map;
// What `overflow_panic` reports, set by the handler before it returns there.
static __thread long long tl_overflow_kib;
static __thread int tl_overflow_in_task;
#ifdef RASK_VALGRIND
static __thread unsigned tl_unwind_valgrind_id;
#endif

static long page_size(void) {
    return sysconf(_SC_PAGESIZE);
}

// The bounds of the calling thread's stack, or 0 when the platform won't say.
static int thread_stack_bounds(const char **lo, const char **hi) {
#if defined(__APPLE__)
    pthread_t self = pthread_self();
    *hi = (const char *)pthread_get_stackaddr_np(self);
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
    *lo = (const char *)addr;
    *hi = *lo + size;
    return 1;
#endif
}

static int in_thread_guard(const char *p) {
    if (!tl_stack_lo) return 0;
    return p < tl_stack_lo + page_size() && p >= tl_stack_lo - THREAD_GUARD_REACH;
}

// Where the interrupted code resumes once the handler returns: ordinary
// code, on the unwind stack.
static _Noreturn void overflow_panic(void) {
    rask_panic_fmt("stack overflow: all %lld KiB of the %s's stack is in use "
                   "(unbounded recursion, or a local too large for the stack?)",
                   tl_overflow_kib, tl_overflow_in_task ? "task" : "thread");
}

// Make returning from the handler a call to `overflow_panic` with `sp` as
// its stack: the stack pointer as it would be just after a call instruction,
// with a return address of zero, since the function never returns.
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
    const char *addr = info ? (const char *)info->si_addr : NULL;
    long page = page_size();
    int task = addr && rask_task_in_guard(addr);
    int thread = !task && addr && in_thread_guard(addr);
    int unwind = addr && tl_unwind_map && addr >= tl_unwind_map && addr < tl_unwind_map + page;
    if (task || thread || unwind) {
        if (unwind || rask_panic_unwinding() || !tl_unwind_map) {
            static const char again[] = "stack overflow while a panic unwinds\n";
            static const char none[] = "stack overflow, and no stack left to report it on\n";
            const char *msg = tl_unwind_map ? again : none;
            ssize_t ignored = write(2, msg, strlen(msg));
            (void)ignored;
            signal(SIGABRT, SIG_DFL);
            abort();
        }
        tl_overflow_in_task = task;
        tl_overflow_kib = task ? 1024 : (long long)((tl_stack_hi - tl_stack_lo) / 1024);
        resume_in_panic(uctx, tl_unwind_map + page + UNWIND_STACK);
        return;
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

// Every thread that runs Rask code calls this first, and the exit on its way
// out.
void rask_stack_guard_thread_init(void) {
    pthread_once(&overflow_once, install_overflow_handler);
    if (!thread_stack_bounds(&tl_stack_lo, &tl_stack_hi)) {
        tl_stack_lo = tl_stack_hi = NULL;
    }
    long page = page_size();
    char *sig_map = mmap(NULL, SIGNAL_STACK, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (sig_map == MAP_FAILED) return;
    char *unwind_map = mmap(NULL, UNWIND_STACK + page, PROT_READ | PROT_WRITE,
                            MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (unwind_map == MAP_FAILED) {
        munmap(sig_map, SIGNAL_STACK);
        return;
    }
    stack_t alt = { .ss_sp = sig_map, .ss_size = SIGNAL_STACK, .ss_flags = 0 };
    if (mprotect(unwind_map, page, PROT_NONE) != 0 || sigaltstack(&alt, NULL) != 0) {
        munmap(sig_map, SIGNAL_STACK);
        munmap(unwind_map, UNWIND_STACK + page);
        return;
    }
    tl_signal_map = sig_map;
    tl_unwind_map = unwind_map;
#ifdef RASK_VALGRIND
    // The unwind longjmps from here back to the thread's own stack.
    // Unregistered, valgrind reads that jump as the stack shrinking across
    // everything between the two and marks it all uninitialised.
    tl_unwind_valgrind_id = VALGRIND_STACK_REGISTER(unwind_map + page, unwind_map + page + UNWIND_STACK);
#endif
}

void rask_stack_guard_thread_exit(void) {
    if (tl_signal_map) {
        stack_t off = { .ss_flags = SS_DISABLE };
        sigaltstack(&off, NULL);
        munmap(tl_signal_map, SIGNAL_STACK);
        tl_signal_map = NULL;
    }
    if (tl_unwind_map) {
#ifdef RASK_VALGRIND
        VALGRIND_STACK_DEREGISTER(tl_unwind_valgrind_id);
#endif
        munmap(tl_unwind_map, UNWIND_STACK + page_size());
        tl_unwind_map = NULL;
    }
    tl_stack_lo = tl_stack_hi = NULL;
}

#else

void rask_stack_guard_thread_init(void) {}
void rask_stack_guard_thread_exit(void) {}

#endif
