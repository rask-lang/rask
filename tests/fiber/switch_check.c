// SPDX-License-Identifier: (MIT OR Apache-2.0)

// The fiber context switch (compiler/runtime/fiber.c), checked on its own.
//
// Every task switch in the green scheduler goes through rask_fiber_swap, so a
// register it forgets to save shows up as a wrong value in some unrelated
// function much later. This checks the contract directly, per architecture:
//
//   - callee-saved integer registers survive a switch away and back
//   - callee-saved float registers survive (aarch64 d8-d15)
//   - the float control state is per fiber: a rounding mode set on one fiber
//     doesn't leak into another, and is still there when it comes back
//   - a fiber can use its stack deeply, and thousands of switches and
//     fibers go through without drift
//
// tests/fiber_gate.sh builds this for the host and, when a cross compiler
// and qemu are around, for aarch64, which no CI machine runs natively.

#include "../../compiler/runtime/fiber.h"

#include <fenv.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static RaskFiber main_fiber;
static RaskFiber fiber_a;
static RaskFiber fiber_b;
static int failures = 0;

#define CHECK(cond, ...) do { \
        if (!(cond)) { \
            fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); \
            fprintf(stderr, __VA_ARGS__); \
            fputc('\n', stderr); \
            failures++; \
        } \
    } while (0)

// ─── Callee-saved registers, by name ────────────────────────
//
// Each fiber pins its own values into every callee-saved register right
// across its switch, and checks them when it comes back. The other fiber has
// meanwhile run with *its* values in the same registers, so a register the
// switch doesn't save comes back holding the other fiber's number.
//
// (Clobbering them from a separate function doesn't work: declaring a
// callee-saved register clobbered makes the compiler restore it before that
// function returns, so the switch never sees the clobber.)

#define IVAL(seed, k) ((uint64_t)(seed) * 0x0101010101010101ULL + (k))
#define FVAL(seed, k) ((double)(seed) * 100.0 + (k) + 0.5)

#if defined(__aarch64__)
static void registers_survive(RaskFiber *self, RaskFiber *other, uint64_t seed) {
    register uint64_t r19 __asm__("x19") = IVAL(seed, 19);
    register uint64_t r20 __asm__("x20") = IVAL(seed, 20);
    register uint64_t r21 __asm__("x21") = IVAL(seed, 21);
    register uint64_t r22 __asm__("x22") = IVAL(seed, 22);
    register uint64_t r23 __asm__("x23") = IVAL(seed, 23);
    register uint64_t r24 __asm__("x24") = IVAL(seed, 24);
    register uint64_t r25 __asm__("x25") = IVAL(seed, 25);
    register uint64_t r26 __asm__("x26") = IVAL(seed, 26);
    register uint64_t r27 __asm__("x27") = IVAL(seed, 27);
    register uint64_t r28 __asm__("x28") = IVAL(seed, 28);
    register double d8 __asm__("d8") = FVAL(seed, 8);
    register double d9 __asm__("d9") = FVAL(seed, 9);
    register double d10 __asm__("d10") = FVAL(seed, 10);
    register double d11 __asm__("d11") = FVAL(seed, 11);
    register double d12 __asm__("d12") = FVAL(seed, 12);
    register double d13 __asm__("d13") = FVAL(seed, 13);
    register double d14 __asm__("d14") = FVAL(seed, 14);
    register double d15 __asm__("d15") = FVAL(seed, 15);
    // The empty asm statements keep each value in its register across the
    // call instead of letting the compiler spill and reload it.
    __asm__ volatile("" : "+r"(r19), "+r"(r20), "+r"(r21), "+r"(r22), "+r"(r23),
                          "+r"(r24), "+r"(r25), "+r"(r26), "+r"(r27), "+r"(r28));
    __asm__ volatile("" : "+w"(d8), "+w"(d9), "+w"(d10), "+w"(d11),
                          "+w"(d12), "+w"(d13), "+w"(d14), "+w"(d15));
    rask_fiber_switch(self, other);
    __asm__ volatile("" : "+r"(r19), "+r"(r20), "+r"(r21), "+r"(r22), "+r"(r23),
                          "+r"(r24), "+r"(r25), "+r"(r26), "+r"(r27), "+r"(r28));
    __asm__ volatile("" : "+w"(d8), "+w"(d9), "+w"(d10), "+w"(d11),
                          "+w"(d12), "+w"(d13), "+w"(d14), "+w"(d15));
    uint64_t x[] = { r19, r20, r21, r22, r23, r24, r25, r26, r27, r28 };
    for (int i = 0; i < 10; i++) {
        CHECK(x[i] == IVAL(seed, 19 + i), "x%d = %llx, want %llx", 19 + i,
              (unsigned long long)x[i], (unsigned long long)IVAL(seed, 19 + i));
    }
    double d[] = { d8, d9, d10, d11, d12, d13, d14, d15 };
    for (int i = 0; i < 8; i++) {
        CHECK(d[i] == FVAL(seed, 8 + i), "d%d = %g, want %g", 8 + i, d[i], FVAL(seed, 8 + i));
    }
}
#elif defined(__x86_64__)
static void registers_survive(RaskFiber *self, RaskFiber *other, uint64_t seed) {
    register uint64_t rbx __asm__("rbx") = IVAL(seed, 3);
    register uint64_t r12 __asm__("r12") = IVAL(seed, 12);
    register uint64_t r13 __asm__("r13") = IVAL(seed, 13);
    register uint64_t r14 __asm__("r14") = IVAL(seed, 14);
    register uint64_t r15 __asm__("r15") = IVAL(seed, 15);
    __asm__ volatile("" : "+r"(rbx), "+r"(r12), "+r"(r13), "+r"(r14), "+r"(r15));
    rask_fiber_switch(self, other);
    __asm__ volatile("" : "+r"(rbx), "+r"(r12), "+r"(r13), "+r"(r14), "+r"(r15));
    CHECK(rbx == IVAL(seed, 3), "rbx = %llx", (unsigned long long)rbx);
    CHECK(r12 == IVAL(seed, 12), "r12 = %llx", (unsigned long long)r12);
    CHECK(r13 == IVAL(seed, 13), "r13 = %llx", (unsigned long long)r13);
    CHECK(r14 == IVAL(seed, 14), "r14 = %llx", (unsigned long long)r14);
    CHECK(r15 == IVAL(seed, 15), "r15 = %llx", (unsigned long long)r15);
}
#else
#error "switch_check: no register check for this architecture"
#endif

// ─── The fibers ─────────────────────────────────────────────

static volatile int b_saw_default_rounding = 0;

// Deep enough to use a good part of the stack, shallow enough for 1 MiB.
static int recurse(int n) {
    volatile char pad[256];
    pad[0] = (char)n;
    return n == 0 ? pad[0] : recurse(n - 1) + 1;
}

static void fiber_a_main(void *arg) {
    (void)arg;
    rask_fiber_started();
    registers_survive(&fiber_a, &fiber_b, 0xA);

    // A rounding mode set here belongs to this fiber.
    fesetround(FE_UPWARD);
    rask_fiber_switch(&fiber_a, &fiber_b);
    CHECK(fegetround() == FE_UPWARD, "fiber a lost its rounding mode across a switch");
    fesetround(FE_TONEAREST);

    CHECK(recurse(2000) == 2000, "deep recursion on a fiber stack");

    // Ping-pong.
    for (int i = 0; i < 100000; i++) rask_fiber_switch(&fiber_a, &fiber_b);

    rask_fiber_switch_final(&fiber_a, &main_fiber);
}

static void fiber_b_main(void *arg) {
    (void)arg;
    rask_fiber_started();
    registers_survive(&fiber_b, &fiber_a, 0xB);

    b_saw_default_rounding = fegetround() == FE_TONEAREST;
    rask_fiber_switch(&fiber_b, &fiber_a);

    for (;;) rask_fiber_switch(&fiber_b, &fiber_a);
}

static RaskFiber fiber_c;

static void fiber_c_main(void *arg) {
    (void)arg;
    rask_fiber_started();
    // Never checks its own: main destroys it while it's switched out.
    registers_survive(&fiber_c, &main_fiber, 0xC);
}

// ─── Many short fibers ──────────────────────────────────────

static RaskFiber short_fiber;
static int short_runs = 0;

static void short_main(void *arg) {
    rask_fiber_started();
    short_runs += (int)(intptr_t)arg;
    rask_fiber_switch_final(&short_fiber, &main_fiber);
}

int main(void) {
    rask_fiber_init_thread(&main_fiber);

    rask_fiber_init(&fiber_a, fiber_a_main, NULL);
    rask_fiber_init(&fiber_b, fiber_b_main, NULL);
    rask_fiber_switch(&main_fiber, &fiber_a);
    CHECK(b_saw_default_rounding, "fiber b saw fiber a's rounding mode");
    rask_fiber_destroy(&fiber_a);
    rask_fiber_destroy(&fiber_b);

    // The thread's own stack is saved and restored the same way.
    rask_fiber_init(&fiber_c, fiber_c_main, NULL);
    registers_survive(&main_fiber, &fiber_c, 0xD);
    rask_fiber_destroy(&fiber_c);

    for (int i = 0; i < 5000; i++) {
        rask_fiber_init(&short_fiber, short_main, (void *)(intptr_t)1);
        rask_fiber_switch(&main_fiber, &short_fiber);
        rask_fiber_destroy(&short_fiber);
    }
    CHECK(short_runs == 5000, "short fibers ran %d times", short_runs);

    if (failures) {
        fprintf(stderr, "switch_check: %d failure(s)\n", failures);
        return 1;
    }
    printf("switch_check: ok\n");
    return 0;
}
