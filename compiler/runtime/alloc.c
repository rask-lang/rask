// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Heap allocator with swappable backend and stats tracking.
//
// Default backend: malloc/realloc/free.
// Call rask_allocator_set() before any allocations to swap in a custom
// allocator (arena, pool, debug, etc.). Not thread-safe to swap — do it
// once at startup.
//
// Stats are tracked with atomics so concurrent allocations don't lose counts.
// Peak tracking uses a compare-and-swap loop.

#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <stdatomic.h>

// ─── Default allocator (malloc) ────────────────────────────

static void *default_alloc(int64_t size, void *ctx) {
    (void)ctx;
    return malloc((size_t)size);
}

static void *default_realloc(void *ptr, int64_t old_size, int64_t new_size, void *ctx) {
    (void)ctx;
    (void)old_size;
    return realloc(ptr, (size_t)new_size);
}

static void default_free(void *ptr, void *ctx) {
    (void)ctx;
    free(ptr);
}

// ─── Active allocator ──────────────────────────────────────

static RaskAllocator active_allocator = {
    .alloc   = default_alloc,
    .realloc = default_realloc,
    .free    = default_free,
    .ctx     = NULL,
};

// ─── Stats (atomic for thread safety) ──────────────────────

static atomic_int_least64_t stat_alloc_count;
static atomic_int_least64_t stat_free_count;
static atomic_int_least64_t stat_bytes_allocated;
static atomic_int_least64_t stat_bytes_freed;
static atomic_int_least64_t stat_current_bytes;
static atomic_int_least64_t stat_peak_bytes;

static void stats_track_alloc(int64_t size) {
    atomic_fetch_add_explicit(&stat_alloc_count, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&stat_bytes_allocated, size, memory_order_relaxed);
    int64_t current = atomic_fetch_add_explicit(&stat_current_bytes, size,
                                                 memory_order_relaxed) + size;
    // CAS loop to update peak
    int64_t peak = atomic_load_explicit(&stat_peak_bytes, memory_order_relaxed);
    while (current > peak) {
        if (atomic_compare_exchange_weak_explicit(&stat_peak_bytes, &peak, current,
                                                   memory_order_relaxed,
                                                   memory_order_relaxed)) {
            break;
        }
    }
}

static void stats_track_free(int64_t size) {
    atomic_fetch_add_explicit(&stat_free_count, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&stat_bytes_freed, size, memory_order_relaxed);
    atomic_fetch_sub_explicit(&stat_current_bytes, size, memory_order_relaxed);
}

// ─── Public API ────────────────────────────────────────────

void rask_allocator_set(const RaskAllocator *a) {
    active_allocator = *a;
}

void rask_alloc_stats(RaskAllocStats *out) {
    out->alloc_count    = atomic_load_explicit(&stat_alloc_count, memory_order_relaxed);
    out->free_count     = atomic_load_explicit(&stat_free_count, memory_order_relaxed);
    out->bytes_allocated = atomic_load_explicit(&stat_bytes_allocated, memory_order_relaxed);
    out->bytes_freed    = atomic_load_explicit(&stat_bytes_freed, memory_order_relaxed);
    out->peak_bytes     = atomic_load_explicit(&stat_peak_bytes, memory_order_relaxed);
}

void *rask_alloc(int64_t size) {
    if (size <= 0) {
        return NULL;
    }
    void *ptr = active_allocator.alloc(size, active_allocator.ctx);
    if (!ptr) {
        fprintf(stderr, "rask: allocation failed (%lld bytes)\n", (long long)size);
        abort();
    }
    stats_track_alloc(size);
    return ptr;
}

void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size) {
    if (new_size <= 0) {
        if (ptr) {
            active_allocator.free(ptr, active_allocator.ctx);
            if (old_size > 0) stats_track_free(old_size);
        }
        return NULL;
    }
    void *new_ptr = active_allocator.realloc(ptr, old_size, new_size,
                                              active_allocator.ctx);
    if (!new_ptr) {
        fprintf(stderr, "rask: reallocation failed (%lld bytes)\n", (long long)new_size);
        abort();
    }
    // Track the delta
    if (old_size > 0) stats_track_free(old_size);
    stats_track_alloc(new_size);
    return new_ptr;
}

void rask_free(void *ptr) {
    if (ptr) {
        active_allocator.free(ptr, active_allocator.ctx);
        // Note: we don't know the size here, so free_count increments
        // but bytes_freed doesn't. Use rask_realloc(ptr, old_size, 0)
        // for accurate byte tracking when size is known.
        atomic_fetch_add_explicit(&stat_free_count, 1, memory_order_relaxed);
    }
}
