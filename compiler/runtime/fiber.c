// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Stackful fibers: a stack per task and a switch between stacks
// (conc.runtime/T1-T3).
//
// A fiber's stack is a 1 MiB reservation of address space, of which only the
// pages a task actually touches become memory. The lowest page is PROT_NONE,
// so running off the end faults instead of writing into a neighbour.
//
// The switch saves the callee-saved registers on the current stack, stores the
// stack pointer, loads the other one and pops its registers. Everything else is
// already dead across a call by the ABI, which is what makes a switch a few
// dozen instructions rather than a signal mask and a syscall (swapcontext).
//
// Sanitizers are told about every switch. Without it TSan reads one thread
// running two stacks as a race on everything they share, ASan loses track of
// which stack is live, and valgrind reports every access to a fresh fiber
// stack as outside any stack at all.

#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE   // pthread_getattr_np
#endif

#include "fiber.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sys/mman.h>
#include <unistd.h>

#if defined(__SANITIZE_THREAD__)
#define RASK_TSAN 1
#elif defined(__has_feature)
#if __has_feature(thread_sanitizer)
#define RASK_TSAN 1
#endif
#endif

#if defined(__SANITIZE_ADDRESS__)
#define RASK_ASAN 1
#elif defined(__has_feature)
#if __has_feature(address_sanitizer)
#define RASK_ASAN 1
#endif
#endif

#ifdef RASK_TSAN
void *__tsan_get_current_fiber(void);
void *__tsan_create_fiber(unsigned flags);
void  __tsan_destroy_fiber(void *fiber);
void  __tsan_switch_to_fiber(void *fiber, unsigned flags);
#endif

#ifdef RASK_ASAN
void __sanitizer_start_switch_fiber(void **fake_stack_save, const void *bottom, size_t size);
void __sanitizer_finish_switch_fiber(void *fake_stack_save, const void **bottom_old, size_t *size_old);
#endif

#if defined(__has_include)
#if __has_include(<valgrind/valgrind.h>)
#include <valgrind/valgrind.h>
#define RASK_VALGRIND 1
#endif
#endif

#define FIBER_STACK_SIZE (1024 * 1024)

// ─── The switch ─────────────────────────────────────────────
//
//   void rask_fiber_swap(void **save_sp, void *load_sp)
//
// Pushes the callee-saved state, stores the stack pointer through `save_sp`,
// switches to `load_sp` and pops what is there. A fresh fiber's stack is built
// by `rask_fiber_init` to look like one that was switched away from inside
// `fiber_trampoline`, so the first switch into it "returns" there.

void rask_fiber_swap(void **save_sp, void *load_sp);
void rask_fiber_trampoline(void);

#if defined(__APPLE__)
#define FIBER_SYM(name) "_" #name
#define FIBER_FUNC_BEGIN(name) ".globl _" #name "\n.p2align 4\n_" #name ":\n"
#define FIBER_FUNC_END(name) ""
#else
#define FIBER_SYM(name) #name
#define FIBER_FUNC_BEGIN(name) ".globl " #name "\n.type " #name ", @function\n.p2align 4\n" #name ":\n"
#define FIBER_FUNC_END(name) ".size " #name ", .-" #name "\n"
#endif

#if defined(__x86_64__)

// Saved frame, from the stack pointer up: mxcsr+x87 control word (8 bytes),
// r15, r14, r13, r12, rbx, rbp, return address.
__asm__(
    ".text\n"
    FIBER_FUNC_BEGIN(rask_fiber_swap)
    "    pushq %rbp\n"
    "    pushq %rbx\n"
    "    pushq %r12\n"
    "    pushq %r13\n"
    "    pushq %r14\n"
    "    pushq %r15\n"
    "    subq  $8, %rsp\n"
    "    stmxcsr (%rsp)\n"
    "    fnstcw  4(%rsp)\n"
    "    movq  %rsp, (%rdi)\n"
    "    movq  %rsi, %rsp\n"
    "    ldmxcsr (%rsp)\n"
    "    fldcw   4(%rsp)\n"
    "    addq  $8, %rsp\n"
    "    popq  %r15\n"
    "    popq  %r14\n"
    "    popq  %r13\n"
    "    popq  %r12\n"
    "    popq  %rbx\n"
    "    popq  %rbp\n"
    "    ret\n"
    FIBER_FUNC_END(rask_fiber_swap)
    // Entered by `ret` with r12 = entry, r13 = arg, and the stack 16-byte
    // aligned — so the `call` below leaves the callee the alignment it expects.
    FIBER_FUNC_BEGIN(rask_fiber_trampoline)
    "    movq  %r13, %rdi\n"
    "    callq *%r12\n"
    "    ud2\n"
    FIBER_FUNC_END(rask_fiber_trampoline)
);

#define FRAME_WORDS 8

static void *initial_frame(char *top, void (*entry)(void *), void *arg) {
    // The trampoline starts with rsp == top - 0 after popping its return
    // address, so `top` must be 16-aligned.
    uint64_t *sp = (uint64_t *)((uintptr_t)top & ~(uintptr_t)15);
    sp -= FRAME_WORDS;
    uint32_t mxcsr = 0x1F80;   // round-to-nearest, all exceptions masked
    uint16_t fpucw = 0x037F;   // the x87 default
    memcpy((char *)&sp[0], &mxcsr, 4);
    memcpy((char *)&sp[0] + 4, &fpucw, 2);
    sp[1] = 0;                         // r15
    sp[2] = 0;                         // r14
    sp[3] = (uint64_t)(uintptr_t)arg;  // r13
    sp[4] = (uint64_t)(uintptr_t)entry;// r12
    sp[5] = 0;                         // rbx
    sp[6] = 0;                         // rbp
    sp[7] = (uint64_t)(uintptr_t)rask_fiber_trampoline;
    return sp;
}

#elif defined(__aarch64__)

// Saved frame, from the stack pointer up: x19..x28, x29 (fp), x30 (lr),
// d8..d15, fpcr — 22 words, padded to 24 to keep sp 16-aligned.
__asm__(
    ".text\n"
    FIBER_FUNC_BEGIN(rask_fiber_swap)
    "    sub  sp, sp, #192\n"
    "    stp  x19, x20, [sp, #0]\n"
    "    stp  x21, x22, [sp, #16]\n"
    "    stp  x23, x24, [sp, #32]\n"
    "    stp  x25, x26, [sp, #48]\n"
    "    stp  x27, x28, [sp, #64]\n"
    "    stp  x29, x30, [sp, #80]\n"
    "    stp  d8,  d9,  [sp, #96]\n"
    "    stp  d10, d11, [sp, #112]\n"
    "    stp  d12, d13, [sp, #128]\n"
    "    stp  d14, d15, [sp, #144]\n"
    "    mrs  x9, fpcr\n"
    "    str  x9, [sp, #160]\n"
    "    mov  x9, sp\n"
    "    str  x9, [x0]\n"
    "    mov  sp, x1\n"
    "    ldr  x9, [sp, #160]\n"
    "    msr  fpcr, x9\n"
    "    ldp  x19, x20, [sp, #0]\n"
    "    ldp  x21, x22, [sp, #16]\n"
    "    ldp  x23, x24, [sp, #32]\n"
    "    ldp  x25, x26, [sp, #48]\n"
    "    ldp  x27, x28, [sp, #64]\n"
    "    ldp  x29, x30, [sp, #80]\n"
    "    ldp  d8,  d9,  [sp, #96]\n"
    "    ldp  d10, d11, [sp, #112]\n"
    "    ldp  d12, d13, [sp, #128]\n"
    "    ldp  d14, d15, [sp, #144]\n"
    "    add  sp, sp, #192\n"
    "    ret\n"
    FIBER_FUNC_END(rask_fiber_swap)
    // Entered by `ret` with x19 = entry, x20 = arg.
    FIBER_FUNC_BEGIN(rask_fiber_trampoline)
    "    mov  x0, x20\n"
    "    blr  x19\n"
    "    brk  #0\n"
    FIBER_FUNC_END(rask_fiber_trampoline)
);

#define FRAME_WORDS 24

static void *initial_frame(char *top, void (*entry)(void *), void *arg) {
    uint64_t *sp = (uint64_t *)((uintptr_t)top & ~(uintptr_t)15);
    sp -= FRAME_WORDS;
    memset(sp, 0, FRAME_WORDS * sizeof(uint64_t));
    sp[0]  = (uint64_t)(uintptr_t)entry;                  // x19
    sp[1]  = (uint64_t)(uintptr_t)arg;                    // x20
    sp[11] = (uint64_t)(uintptr_t)rask_fiber_trampoline;  // x30
    return sp;                                            // fpcr 0: the default
}

#else
#error "fibers: no context switch for this architecture"
#endif

// ─── Stacks ─────────────────────────────────────────────────
//
// Freed stacks are kept for the next spawn rather than unmapped: mapping is a
// syscall and a fresh set of page faults, and a program that spawns once
// usually spawns again. The pool is capped so a burst doesn't pin its peak
// address space forever.

#define STACK_POOL_CAP 256

static pthread_mutex_t pool_lock = PTHREAD_MUTEX_INITIALIZER;
static void *pool[STACK_POOL_CAP];
static int pool_len = 0;

static size_t page_size(void) {
    // Every worker asks; whichever answers first, the answer is the same.
    static _Atomic size_t cached = 0;
    size_t v = atomic_load_explicit(&cached, memory_order_relaxed);
    if (!v) {
        long p = sysconf(_SC_PAGESIZE);
        v = p > 0 ? (size_t)p : 4096;
        atomic_store_explicit(&cached, v, memory_order_relaxed);
    }
    return v;
}

static void *stack_get(void) {
    pthread_mutex_lock(&pool_lock);
    void *base = pool_len > 0 ? pool[--pool_len] : NULL;
    pthread_mutex_unlock(&pool_lock);
    if (base) return base;

    int flags = MAP_PRIVATE | MAP_ANONYMOUS;
#ifdef MAP_NORESERVE
    flags |= MAP_NORESERVE;
#endif
#ifdef MAP_STACK
    flags |= MAP_STACK;
#endif
    base = mmap(NULL, FIBER_STACK_SIZE, PROT_READ | PROT_WRITE, flags, -1, 0);
    if (base == MAP_FAILED) {
        fprintf(stderr,
                "rask: out of address space for a task stack (%d KiB each)\n",
                FIBER_STACK_SIZE / 1024);
        abort();
    }
    // The guard: the stack grows down into it.
    if (mprotect(base, page_size(), PROT_NONE) != 0) {
        fprintf(stderr, "rask: could not protect a task stack's guard page\n");
        abort();
    }
    return base;
}

static void stack_put(void *base) {
    pthread_mutex_lock(&pool_lock);
    if (pool_len < STACK_POOL_CAP) {
        pool[pool_len++] = base;
        base = NULL;
    }
    pthread_mutex_unlock(&pool_lock);
    if (base) munmap(base, FIBER_STACK_SIZE);
}

int rask_fiber_in_guard(const RaskFiber *f, const void *addr) {
    if (!f || !f->stack) return 0;
    const char *lo = (const char *)f->stack;
    const char *p = (const char *)addr;
    return p >= lo && p < lo + page_size();
}

// ─── Fibers ─────────────────────────────────────────────────

void rask_fiber_init_thread(RaskFiber *f) {
    memset(f, 0, sizeof(*f));
#ifdef RASK_TSAN
    f->tsan = __tsan_get_current_fiber();
#endif
#if defined(RASK_ASAN) && defined(__linux__)
    pthread_attr_t attr;
    if (pthread_getattr_np(pthread_self(), &attr) == 0) {
        void *lo;
        size_t size;
        if (pthread_attr_getstack(&attr, &lo, &size) == 0) {
            f->asan_bottom = lo;
            f->asan_size = size;
        }
        pthread_attr_destroy(&attr);
    }
#endif
}

void rask_fiber_init(RaskFiber *f, void (*entry)(void *), void *arg) {
    memset(f, 0, sizeof(*f));
    f->stack = stack_get();
    char *top = (char *)f->stack + FIBER_STACK_SIZE;
    f->sp = initial_frame(top, entry, arg);
#ifdef RASK_TSAN
    f->tsan = __tsan_create_fiber(0);
#endif
#ifdef RASK_ASAN
    f->asan_bottom = (char *)f->stack + page_size();
    f->asan_size = FIBER_STACK_SIZE - page_size();
#endif
#ifdef RASK_VALGRIND
    f->valgrind_id = VALGRIND_STACK_REGISTER((char *)f->stack + page_size(), top);
#endif
}

void rask_fiber_destroy(RaskFiber *f) {
    if (!f->stack) return;
#ifdef RASK_TSAN
    if (f->tsan) __tsan_destroy_fiber(f->tsan);
#endif
#ifdef RASK_VALGRIND
    VALGRIND_STACK_DEREGISTER(f->valgrind_id);
#endif
    stack_put(f->stack);
    memset(f, 0, sizeof(*f));
}

void rask_fiber_switch(RaskFiber *from, RaskFiber *to) {
#ifdef RASK_TSAN
    __tsan_switch_to_fiber(to->tsan, 0);
#endif
#ifdef RASK_ASAN
    void *fake = NULL;
    __sanitizer_start_switch_fiber(&fake, to->asan_bottom, to->asan_size);
#endif
    rask_fiber_swap(&from->sp, to->sp);
#ifdef RASK_ASAN
    __sanitizer_finish_switch_fiber(fake, NULL, NULL);
#endif
}

// Called on a fiber that is about to finish for good: ASan must be told the
// stack it is leaving won't come back, or it keeps the fake frames for it.
_Noreturn void rask_fiber_switch_final(RaskFiber *from, RaskFiber *to) {
#ifdef RASK_TSAN
    __tsan_switch_to_fiber(to->tsan, 0);
#endif
#ifdef RASK_ASAN
    __sanitizer_start_switch_fiber(NULL, to->asan_bottom, to->asan_size);
#endif
    rask_fiber_swap(&from->sp, to->sp);
    __builtin_unreachable();
}

// First thing a fresh fiber runs: the other half of the switch that started it.
void rask_fiber_started(void) {
#ifdef RASK_ASAN
    __sanitizer_finish_switch_fiber(NULL, NULL, NULL);
#endif
}
