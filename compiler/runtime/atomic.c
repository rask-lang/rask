// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask atomic runtime — C11 stdatomic wrappers for native-compiled programs.
// All integer atomic types share one implementation since codegen represents
// all values as i64. AtomicBool is separate (0/1 semantics).

#include "rask_runtime.h"

#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>

// ── Ordering conversion ────────────────────────────────────
// Maps Rask Ordering enum tag to C11 memory_order.
// Tag values match resolver registration order (after Less/Equal/Greater):
//   Relaxed=3, Acquire=4, Release=5, AcqRel=6, SeqCst=7

static memory_order to_order(int64_t o) {
    switch (o) {
        case 3: return memory_order_relaxed;
        case 4: return memory_order_acquire;
        case 5: return memory_order_release;
        case 6: return memory_order_acq_rel;
        case 7: return memory_order_seq_cst;
        default: return memory_order_seq_cst;
    }
}

// ═══════════════════════════════════════════════════════════
// Integer atomics (AtomicI8..AtomicU64, AtomicUsize, AtomicIsize)
// All use _Atomic(int64_t) since codegen represents values as i64.
// ═══════════════════════════════════════════════════════════

typedef struct { _Atomic(int64_t) value; } RaskAtomicInt;

// ── Construction ────────────────────────────────────────────

int64_t rask_atomic_int_new(int64_t val) {
    RaskAtomicInt *a = rask_alloc(sizeof(RaskAtomicInt));
    atomic_init(&a->value, val);
    return (int64_t)(uintptr_t)a;
}

int64_t rask_atomic_int_default(void) {
    return rask_atomic_int_new(0);
}

// ── Load / Store / Swap ─────────────────────────────────────

int64_t rask_atomic_int_load(int64_t ptr, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_load_explicit(&a->value, to_order(ordering));
}

void rask_atomic_int_store(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    atomic_store_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_swap(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_exchange_explicit(&a->value, val, to_order(ordering));
}

// ── Compare-and-Exchange ────────────────────────────────────
// Returns the old value. Writes 1 to *out_ok on success, 0 on failure.
// Caller encodes this into Result<T, T>.

int64_t rask_atomic_int_compare_exchange(int64_t ptr, int64_t expected,
                                          int64_t desired, int64_t success_ord,
                                          int64_t fail_ord, int64_t *out_ok) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t current = expected;
    _Bool ok = atomic_compare_exchange_strong_explicit(
        &a->value, &current, desired,
        to_order(success_ord), to_order(fail_ord));
    *out_ok = ok ? 1 : 0;
    return current;
}

int64_t rask_atomic_int_compare_exchange_weak(int64_t ptr, int64_t expected,
                                               int64_t desired, int64_t success_ord,
                                               int64_t fail_ord, int64_t *out_ok) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t current = expected;
    _Bool ok = atomic_compare_exchange_weak_explicit(
        &a->value, &current, desired,
        to_order(success_ord), to_order(fail_ord));
    *out_ok = ok ? 1 : 0;
    return current;
}

// ── Fetch operations ────────────────────────────────────────

int64_t rask_atomic_int_fetch_add(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_fetch_add_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_fetch_sub(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_fetch_sub_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_fetch_and(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_fetch_and_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_fetch_or(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_fetch_or_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_fetch_xor(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    return atomic_fetch_xor_explicit(&a->value, val, to_order(ordering));
}

int64_t rask_atomic_int_fetch_nand(int64_t ptr, int64_t val, int64_t ordering) {
    // C11 doesn't have atomic_fetch_nand, implement with CAS loop
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t old = atomic_load_explicit(&a->value, memory_order_relaxed);
    while (!atomic_compare_exchange_weak_explicit(
        &a->value, &old, ~(old & val),
        to_order(ordering), memory_order_relaxed)) {
        // old updated by CAS failure
    }
    return old;
}

int64_t rask_atomic_int_fetch_max(int64_t ptr, int64_t val, int64_t ordering) {
    // CAS loop — C11 doesn't have atomic_fetch_max for signed
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t old = atomic_load_explicit(&a->value, memory_order_relaxed);
    while (old < val) {
        if (atomic_compare_exchange_weak_explicit(
                &a->value, &old, val,
                to_order(ordering), memory_order_relaxed)) {
            return old;
        }
    }
    return old;
}

int64_t rask_atomic_int_fetch_min(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t old = atomic_load_explicit(&a->value, memory_order_relaxed);
    while (old > val) {
        if (atomic_compare_exchange_weak_explicit(
                &a->value, &old, val,
                to_order(ordering), memory_order_relaxed)) {
            return old;
        }
    }
    return old;
}

// ── Non-atomic access ───────────────────────────────────────

int64_t rask_atomic_int_into_inner(int64_t ptr) {
    RaskAtomicInt *a = (RaskAtomicInt *)(uintptr_t)ptr;
    int64_t val = atomic_load_explicit(&a->value, memory_order_relaxed);
    rask_free(a);
    return val;
}

// ═══════════════════════════════════════════════════════════
// Bool atomics
// Uses _Atomic(int) for C11 compatibility (bool atomics can be tricky).
// Values: 0 = false, 1 = true.
// ═══════════════════════════════════════════════════════════

typedef struct { _Atomic(int) value; } RaskAtomicBool;

// ── Construction ────────────────────────────────────────────

int64_t rask_atomic_bool_new(int64_t val) {
    RaskAtomicBool *a = rask_alloc(sizeof(RaskAtomicBool));
    atomic_init(&a->value, val ? 1 : 0);
    return (int64_t)(uintptr_t)a;
}

int64_t rask_atomic_bool_default(void) {
    return rask_atomic_bool_new(0);
}

// ── Load / Store / Swap ─────────────────────────────────────

int64_t rask_atomic_bool_load(int64_t ptr, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    return atomic_load_explicit(&a->value, to_order(ordering)) ? 1 : 0;
}

void rask_atomic_bool_store(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    atomic_store_explicit(&a->value, val ? 1 : 0, to_order(ordering));
}

int64_t rask_atomic_bool_swap(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    return atomic_exchange_explicit(&a->value, val ? 1 : 0, to_order(ordering)) ? 1 : 0;
}

// ── Compare-and-Exchange ────────────────────────────────────

int64_t rask_atomic_bool_compare_exchange(int64_t ptr, int64_t expected,
                                           int64_t desired, int64_t success_ord,
                                           int64_t fail_ord, int64_t *out_ok) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    int current = expected ? 1 : 0;
    _Bool ok = atomic_compare_exchange_strong_explicit(
        &a->value, &current, desired ? 1 : 0,
        to_order(success_ord), to_order(fail_ord));
    *out_ok = ok ? 1 : 0;
    return current ? 1 : 0;
}

int64_t rask_atomic_bool_compare_exchange_weak(int64_t ptr, int64_t expected,
                                                int64_t desired, int64_t success_ord,
                                                int64_t fail_ord, int64_t *out_ok) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    int current = expected ? 1 : 0;
    _Bool ok = atomic_compare_exchange_weak_explicit(
        &a->value, &current, desired ? 1 : 0,
        to_order(success_ord), to_order(fail_ord));
    *out_ok = ok ? 1 : 0;
    return current ? 1 : 0;
}

// ── Bool fetch (bitwise on 0/1) ─────────────────────────────

int64_t rask_atomic_bool_fetch_and(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    return atomic_fetch_and_explicit(&a->value, val ? 1 : 0, to_order(ordering)) ? 1 : 0;
}

int64_t rask_atomic_bool_fetch_or(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    return atomic_fetch_or_explicit(&a->value, val ? 1 : 0, to_order(ordering)) ? 1 : 0;
}

int64_t rask_atomic_bool_fetch_xor(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    return atomic_fetch_xor_explicit(&a->value, val ? 1 : 0, to_order(ordering)) ? 1 : 0;
}

int64_t rask_atomic_bool_fetch_nand(int64_t ptr, int64_t val, int64_t ordering) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    int old = atomic_load_explicit(&a->value, memory_order_relaxed);
    int nand = !((old ? 1 : 0) & (val ? 1 : 0)) ? 1 : 0;
    while (!atomic_compare_exchange_weak_explicit(
        &a->value, &old, nand,
        to_order(ordering), memory_order_relaxed)) {
        nand = !((old ? 1 : 0) & (val ? 1 : 0)) ? 1 : 0;
    }
    return old ? 1 : 0;
}

// ── Non-atomic access ───────────────────────────────────────

int64_t rask_atomic_bool_into_inner(int64_t ptr) {
    RaskAtomicBool *a = (RaskAtomicBool *)(uintptr_t)ptr;
    int val = atomic_load_explicit(&a->value, memory_order_relaxed);
    rask_free(a);
    return val ? 1 : 0;
}

// ═══════════════════════════════════════════════════════════
// Memory fences
// ═══════════════════════════════════════════════════════════

void rask_fence(int64_t ordering) {
    atomic_thread_fence(to_order(ordering));
}

void rask_compiler_fence(int64_t ordering) {
    atomic_signal_fence(to_order(ordering));
}
