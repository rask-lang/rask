// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Stackful fibers — see fiber.c.

#ifndef RASK_FIBER_H
#define RASK_FIBER_H

#include <stddef.h>
#include <stdint.h>

typedef struct RaskFiber {
    void    *sp;      // saved stack pointer while switched out
    void    *stack;   // mapping base (the guard page), NULL for a thread's own
    void    *tsan;
    void    *asan_bottom;
    size_t   asan_size;
    unsigned valgrind_id;
} RaskFiber;

// Describe the running thread's own stack, to switch away from and back to.
void rask_fiber_init_thread(RaskFiber *f);

// A fresh fiber whose first switch-in runs `entry(arg)` on its own stack.
// `entry` must never return: it ends by switching away for good.
void rask_fiber_init(RaskFiber *f, void (*entry)(void *), void *arg);

// Return the stack. Never on the fiber being destroyed.
void rask_fiber_destroy(RaskFiber *f);

// Save the running context into `from`, resume `to`. Returns when something
// switches back to `from`.
void rask_fiber_switch(RaskFiber *from, RaskFiber *to);

// Call first thing in a fiber's entry function.
void rask_fiber_started(void);

// The last switch off a fiber that will never be resumed.
_Noreturn void rask_fiber_switch_final(RaskFiber *from, RaskFiber *to);

// Whether `addr` is in `f`'s guard page — a stack overflow.
int rask_fiber_in_guard(const RaskFiber *f, const void *addr);

#endif // RASK_FIBER_H
