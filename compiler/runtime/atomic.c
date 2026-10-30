// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask atomic runtime — C11 stdatomic wrappers for native-compiled programs.
//
// `Atomic<T>` is the only atomic type (mem.atomics/GA1). The cell holds the
// payload at its own width — one, two, four or eight bytes — and knows what
// kind of value it is, so a `u8` wraps at 255 in the cell itself, an unsigned
// `fetch_max` compares unsigned, and a float adds as a float. It used to be
// one `_Atomic(int64_t)` for every payload: a `u8` counter held 260 after
// 250 + 10, and a later compare-exchange against the 4 the program saw
// failed.
//
// Values cross the call as the payload's bits in an int64_t: an integer
// sign- or zero-extended by its kind, a float or a struct as its raw bytes.
// Codegen converts on both sides.

#include "rask_runtime.h"
#include "rask_abi.h"
#include "sim.h"

#include <stdatomic.h>
#include <stdint.h>
#include <string.h>

// ── Ordering conversion ────────────────────────────────────
// `MemoryOrder`'s tag to a C11 memory_order. The tags are generated into
// rask_abi.h from the compiler's one list.

static memory_order to_order(int64_t o) {
    switch (o) {
        case RASK_MEMORY_ORDER_RELAXED: return memory_order_relaxed;
        case RASK_MEMORY_ORDER_ACQUIRE: return memory_order_acquire;
        case RASK_MEMORY_ORDER_RELEASE: return memory_order_release;
        case RASK_MEMORY_ORDER_ACQ_REL: return memory_order_acq_rel;
        default: return memory_order_seq_cst;
    }
}

// A failed compare-exchange only reads, so it keeps the acquire half of the
// one order the call gave.
static memory_order failure_order(memory_order mo) {
    switch (mo) {
        case memory_order_release: return memory_order_relaxed;
        case memory_order_acq_rel: return memory_order_acquire;
        default: return mo;
    }
}

// A load has no release half and a store no acquire half (mem.atomics/AT6).
// The checker rejects one written at the call; this is the order that came
// in a variable. C11 leaves both undefined.
static memory_order load_order(int64_t o) {
    memory_order mo = to_order(o);
    if (mo == memory_order_release || mo == memory_order_acq_rel) {
        rask_panic("a load can't be Release or AcqRel: it only reads");
    }
    return mo;
}

static memory_order store_order(int64_t o) {
    memory_order mo = to_order(o);
    if (mo == memory_order_acquire || mo == memory_order_acq_rel) {
        rask_panic("a store can't be Acquire or AcqRel: it only writes");
    }
    return mo;
}

// A failure ordering may not be a release or stronger than the success one;
// the relaxed load that starts every read-modify-write loop is enough.
#define LOOP_LOAD memory_order_relaxed

typedef struct {
    union {
        _Atomic(uint8_t)  w1;
        _Atomic(uint16_t) w2;
        _Atomic(uint32_t) w4;
        _Atomic(uint64_t) w8;
    } v;
    uint8_t size;   // 1, 2, 4 or 8
    uint8_t kind;   // RASK_ELEM_KIND_*
} RaskAtomic;

static RaskAtomic *cell(int64_t ptr) {
    return (RaskAtomic *)(uintptr_t)ptr;
}

// ── Width dispatch ──────────────────────────────────────────

static uint64_t raw_load(RaskAtomic *a, memory_order mo) {
    switch (a->size) {
        case 1: return atomic_load_explicit(&a->v.w1, mo);
        case 2: return atomic_load_explicit(&a->v.w2, mo);
        case 4: return atomic_load_explicit(&a->v.w4, mo);
        default: return atomic_load_explicit(&a->v.w8, mo);
    }
}

static void raw_store(RaskAtomic *a, uint64_t x, memory_order mo) {
    switch (a->size) {
        case 1: atomic_store_explicit(&a->v.w1, (uint8_t)x, mo); break;
        case 2: atomic_store_explicit(&a->v.w2, (uint16_t)x, mo); break;
        case 4: atomic_store_explicit(&a->v.w4, (uint32_t)x, mo); break;
        default: atomic_store_explicit(&a->v.w8, x, mo); break;
    }
}

static uint64_t raw_swap(RaskAtomic *a, uint64_t x, memory_order mo) {
    switch (a->size) {
        case 1: return atomic_exchange_explicit(&a->v.w1, (uint8_t)x, mo);
        case 2: return atomic_exchange_explicit(&a->v.w2, (uint16_t)x, mo);
        case 4: return atomic_exchange_explicit(&a->v.w4, (uint32_t)x, mo);
        default: return atomic_exchange_explicit(&a->v.w8, x, mo);
    }
}

// Compare-exchange at the cell's width. `*expected` is updated with what the
// cell held when it fails.
static _Bool raw_cas(RaskAtomic *a, uint64_t *expected, uint64_t desired, _Bool weak,
                     memory_order ok, memory_order fail) {
#define CAS_AT(field, T) do {                                                         \
        T e = (T)*expected;                                                           \
        _Bool r = weak                                                                \
            ? atomic_compare_exchange_weak_explicit(&a->v.field, &e, (T)desired, ok, fail)  \
            : atomic_compare_exchange_strong_explicit(&a->v.field, &e, (T)desired, ok, fail); \
        *expected = e;                                                                \
        return r;                                                                     \
    } while (0)
    switch (a->size) {
        case 1: CAS_AT(w1, uint8_t);
        case 2: CAS_AT(w2, uint16_t);
        case 4: CAS_AT(w4, uint32_t);
        default: CAS_AT(w8, uint64_t);
    }
#undef CAS_AT
}

// ── Reading the bits as a value ─────────────────────────────

// The cell's bits as the int64_t that crosses the call: a signed kind
// sign-extended, everything else zero-extended.
static int64_t widen(const RaskAtomic *a, uint64_t bits) {
    if (a->kind == RASK_ELEM_KIND_SIGNED && a->size < 8) {
        unsigned shift = 64 - 8u * a->size;
        return (int64_t)(bits << shift) >> shift;
    }
    return (int64_t)bits;
}

static double as_float(const RaskAtomic *a, uint64_t bits) {
    if (a->size == 4) {
        uint32_t b = (uint32_t)bits;
        float f;
        memcpy(&f, &b, sizeof f);
        return f;
    }
    double d;
    memcpy(&d, &bits, sizeof d);
    return d;
}

static uint64_t float_bits(const RaskAtomic *a, double d) {
    if (a->size == 4) {
        float f = (float)d;
        uint32_t b;
        memcpy(&b, &f, sizeof b);
        return b;
    }
    uint64_t b;
    memcpy(&b, &d, sizeof b);
    return b;
}

// -1, 0 or 1 comparing two payloads by the cell's kind.
static int compare(const RaskAtomic *a, uint64_t x, uint64_t y) {
    switch (a->kind) {
        case RASK_ELEM_KIND_SIGNED: {
            int64_t sx = widen(a, x), sy = widen(a, y);
            return (sx > sy) - (sx < sy);
        }
        case RASK_ELEM_KIND_FLOAT: {
            double fx = as_float(a, x), fy = as_float(a, y);
            return (fx > fy) - (fx < fy);
        }
        default:
            return (x > y) - (x < y);
    }
}

// ── Read-modify-write ───────────────────────────────────────

enum { RMW_ADD, RMW_SUB, RMW_AND, RMW_OR, RMW_XOR, RMW_NAND, RMW_MAX, RMW_MIN };

// The new bits for `old <op> x`. Integer arithmetic wraps at the cell's
// width when the store truncates it (AT5).
static uint64_t apply(const RaskAtomic *a, int op, uint64_t old, uint64_t x) {
    if (a->kind == RASK_ELEM_KIND_FLOAT && (op == RMW_ADD || op == RMW_SUB)) {
        double l = as_float(a, old), r = as_float(a, x);
        return float_bits(a, op == RMW_ADD ? l + r : l - r);
    }
    switch (op) {
        case RMW_ADD:  return old + x;
        case RMW_SUB:  return old - x;
        case RMW_AND:  return old & x;
        case RMW_OR:   return old | x;
        case RMW_XOR:  return old ^ x;
        case RMW_NAND: return ~(old & x);
        case RMW_MAX:  return compare(a, old, x) >= 0 ? old : x;
        default:       return compare(a, old, x) <= 0 ? old : x;
    }
}

// One lock-free loop for every fetch operation: read, compute, compare-
// exchange until nothing raced it. Returns the old value.
static int64_t rmw(int64_t ptr, int op, int64_t x, int64_t ordering) {
    RASK_SIM_POINT();
    RaskAtomic *a = cell(ptr);
    uint64_t old = raw_load(a, LOOP_LOAD);
    for (;;) {
        uint64_t next = apply(a, op, old, (uint64_t)x);
        if (raw_cas(a, &old, next, 1, to_order(ordering), LOOP_LOAD)) {
            return widen(a, old);
        }
    }
}

// ── Construction ────────────────────────────────────────────

int64_t rask_atomic_new(int64_t bits, int64_t size, int64_t kind) {
    if (size != 1 && size != 2 && size != 4 && size != 8) {
        rask_panic("an atomic payload is 1, 2, 4 or 8 bytes");
    }
    RaskAtomic *a = rask_alloc(sizeof(RaskAtomic));
    memset(a, 0, sizeof *a);
    a->size = (uint8_t)size;
    a->kind = (uint8_t)kind;
    raw_store(a, (uint64_t)bits, memory_order_relaxed);
    return (int64_t)(uintptr_t)a;
}

int64_t rask_atomic_default(int64_t size, int64_t kind) {
    return rask_atomic_new(0, size, kind);
}

// ── Load / Store / Swap ─────────────────────────────────────
//
// Every operation below is a scheduling point under sim (sim/S3). Atomics
// never park, so without that a task spinning on a flag would never let the
// task that sets it run.

int64_t rask_atomic_load(int64_t ptr, int64_t ordering) {
    RASK_SIM_POINT();
    RaskAtomic *a = cell(ptr);
    return widen(a, raw_load(a, load_order(ordering)));
}

void rask_atomic_store(int64_t ptr, int64_t bits, int64_t ordering) {
    RASK_SIM_POINT();
    raw_store(cell(ptr), (uint64_t)bits, store_order(ordering));
}

int64_t rask_atomic_swap(int64_t ptr, int64_t bits, int64_t ordering) {
    RASK_SIM_POINT();
    RaskAtomic *a = cell(ptr);
    return widen(a, raw_swap(a, (uint64_t)bits, to_order(ordering)));
}

// ── Compare-and-Exchange ────────────────────────────────────
// Returns the value the cell held. Writes 1 to *out_ok on success, 0 on
// failure; codegen builds the `T or CasFailed<T>` from the two.

static int64_t cas(int64_t ptr, int64_t expected, int64_t desired, int64_t ordering,
                   int64_t *out_ok, _Bool weak) {
    RASK_SIM_POINT();
    RaskAtomic *a = cell(ptr);
    // Compared at the cell's width: the bits above it in `expected` are the
    // extension, not part of the value.
    uint64_t current = (uint64_t)expected;
    memory_order ok_order = to_order(ordering);
    _Bool ok = raw_cas(a, &current, (uint64_t)desired, weak, ok_order, failure_order(ok_order));
    *out_ok = ok ? 1 : 0;
    return ok ? expected : widen(a, current);
}

int64_t rask_atomic_compare_exchange(int64_t ptr, int64_t expected, int64_t desired,
                                     int64_t ordering, int64_t *out_ok) {
    return cas(ptr, expected, desired, ordering, out_ok, 0);
}

int64_t rask_atomic_compare_exchange_weak(int64_t ptr, int64_t expected, int64_t desired,
                                          int64_t ordering, int64_t *out_ok) {
    return cas(ptr, expected, desired, ordering, out_ok, 1);
}

// ── Fetch operations ────────────────────────────────────────

int64_t rask_atomic_fetch_add(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_ADD, x, o); }
int64_t rask_atomic_fetch_sub(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_SUB, x, o); }
int64_t rask_atomic_fetch_and(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_AND, x, o); }
int64_t rask_atomic_fetch_or(int64_t p, int64_t x, int64_t o)   { return rmw(p, RMW_OR, x, o); }
int64_t rask_atomic_fetch_xor(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_XOR, x, o); }
int64_t rask_atomic_fetch_nand(int64_t p, int64_t x, int64_t o) { return rmw(p, RMW_NAND, x, o); }
int64_t rask_atomic_fetch_max(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_MAX, x, o); }
int64_t rask_atomic_fetch_min(int64_t p, int64_t x, int64_t o)  { return rmw(p, RMW_MIN, x, o); }

// ── Non-atomic access ───────────────────────────────────────

// Give the block back. `into_inner` already does this and hands the value out
// with it; this is the ordinary drop, for an atomic that just goes out of
// scope.
void rask_atomic_free(int64_t ptr) {
    RaskAtomic *a = cell(ptr);
    if (!a) return;
    rask_free(a);
}

int64_t rask_atomic_into_inner(int64_t ptr) {
    RaskAtomic *a = cell(ptr);
    int64_t val = widen(a, raw_load(a, memory_order_relaxed));
    rask_free(a);
    return val;
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
