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
#include <stdint.h>
#if defined(__linux__) || defined(__APPLE__)
#include <dlfcn.h>
#endif

// ─── Default allocator (malloc) ────────────────────────────

// What `malloc` promises on every platform the runtime builds for. A request at
// or under it takes the ordinary path, so only an `@align(N)` type past 16
// pays for the aligned one.
#define MALLOC_ALIGN 16

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

// `free` takes a `posix_memalign` block on glibc, musl and macOS, so an
// over-aligned block needs no free of its own.
static void *default_alloc_aligned(int64_t size, int64_t align, void *ctx) {
    (void)ctx;
    void *ptr = NULL;
    if (posix_memalign(&ptr, (size_t)align, (size_t)size) != 0) return NULL;
    return ptr;
}

// ─── Active allocator ──────────────────────────────────────

static RaskAllocator active_allocator = {
    .alloc   = default_alloc,
    .realloc = default_realloc,
    .free    = default_free,
    .alloc_aligned = default_alloc_aligned,
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


// ─── Leak tracing ──────────────────────────────────────────
//
// `RASK_LEAK_CHECK=1` says *how many* allocations a program still holds;
// `RASK_LEAK_TRACE=1` says where they came from. 151 suite files leak and the
// count alone gives a bisect and nothing else, so this records the caller of
// every live allocation and groups the survivors by it at exit.
//
// The site is `__builtin_return_address(0)` inside the allocator, which is the
// runtime function that asked — `rask_vec_new`, `rask_string_from_parts`,
// `rask_closure_alloc`. Symbolized with `dladdr` when the binary exports it,
// and printed as an address otherwise (`nm -C <binary> | grep <addr>` finishes
// the job, or `addr2line -fe <binary> <addr>`).
//
// Off by default and by construction: without the environment variable nothing
// is recorded and the table is never touched.

#define LEAK_TRACE_SLOTS (1 << 17)

// The symbol an address falls in, or NULL when the binary doesn't say. A static
// link keeps its symbol table but exports nothing, so `dladdr` usually answers
// NULL there and the address gets printed instead.
static const char *rask_symbol_name(void *addr) {
#if defined(__linux__) || defined(__APPLE__)
    Dl_info info;
    if (dladdr(addr, &info) && info.dli_sname) return info.dli_sname;
#endif
    (void)addr;
    return NULL;
}

typedef struct {
    void   *ptr;   // NULL for a free slot
    int64_t size;
    void   *site;
} LeakSlot;

static LeakSlot *leak_slots;
static int rask_leak_trace_enabled;
static int leak_trace_overflowed;

void rask_leak_trace_init(void) {
    const char *env = getenv("RASK_LEAK_TRACE");
    if (!env || env[0] == '0' || env[0] == '\0') return;
    leak_slots = (LeakSlot *)calloc(LEAK_TRACE_SLOTS, sizeof(LeakSlot));
    rask_leak_trace_enabled = leak_slots != NULL;
}

static size_t leak_hash(void *ptr) {
    uintptr_t x = (uintptr_t)ptr >> 4;
    x *= 0x9E3779B97F4A7C15ull;
    return (size_t)(x >> 40) & (LEAK_TRACE_SLOTS - 1);
}

static void leak_trace_record(void *ptr, int64_t size, void *site) {
    if (!rask_leak_trace_enabled || !ptr) return;
    size_t i = leak_hash(ptr);
    for (size_t probe = 0; probe < LEAK_TRACE_SLOTS; probe++) {
        size_t at = (i + probe) & (LEAK_TRACE_SLOTS - 1);
        if (leak_slots[at].ptr == NULL || leak_slots[at].ptr == ptr) {
            leak_slots[at].ptr = ptr;
            leak_slots[at].size = size;
            leak_slots[at].site = site;
            return;
        }
    }
    leak_trace_overflowed = 1;
}

static void leak_trace_forget(void *ptr) {
    if (!rask_leak_trace_enabled || !ptr) return;
    size_t i = leak_hash(ptr);
    for (size_t probe = 0; probe < LEAK_TRACE_SLOTS; probe++) {
        size_t at = (i + probe) & (LEAK_TRACE_SLOTS - 1);
        if (leak_slots[at].ptr == ptr) {
            // Tombstone-free deletion isn't safe with linear probing, so the
            // slot keeps its place and only loses its pointer. A run long
            // enough to fill the table says so instead of lying.
            leak_slots[at].ptr = NULL;
            return;
        }
        if (leak_slots[at].ptr == NULL) return;
    }
}

// Group the survivors by allocation site, most allocations first.
void rask_leak_trace_report(void) {
    if (!rask_leak_trace_enabled) return;
    typedef struct { void *site; int64_t count; int64_t bytes; } LeakGroup;
    LeakGroup groups[256];
    int n_groups = 0;
    for (size_t i = 0; i < LEAK_TRACE_SLOTS; i++) {
        if (!leak_slots[i].ptr) continue;
        int found = -1;
        for (int g = 0; g < n_groups; g++) {
            if (groups[g].site == leak_slots[i].site) { found = g; break; }
        }
        if (found < 0) {
            if (n_groups == 256) continue;
            found = n_groups++;
            groups[found].site = leak_slots[i].site;
            groups[found].count = 0;
            groups[found].bytes = 0;
        }
        groups[found].count++;
        groups[found].bytes += leak_slots[i].size;
    }
    if (n_groups == 0) return;
    for (int a = 0; a < n_groups; a++) {
        for (int b = a + 1; b < n_groups; b++) {
            if (groups[b].count > groups[a].count) {
                LeakGroup t = groups[a];
                groups[a] = groups[b];
                groups[b] = t;
            }
        }
    }
    fprintf(stderr, "  still held, by the runtime function that allocated it:\n");
    for (int g = 0; g < n_groups; g++) {
        const char *name = rask_symbol_name(groups[g].site);
        if (name) {
            fprintf(stderr, "    %lld allocation%s, %lld bytes — %s\n",
                    (long long)groups[g].count, groups[g].count == 1 ? "" : "s",
                    (long long)groups[g].bytes, name);
        } else {
            fprintf(stderr, "    %lld allocation%s, %lld bytes — %p (addr2line -fe <binary> %p)\n",
                    (long long)groups[g].count, groups[g].count == 1 ? "" : "s",
                    (long long)groups[g].bytes, groups[g].site, groups[g].site);
        }
    }
    if (leak_trace_overflowed) {
        fprintf(stderr, "    (the trace table filled up — counts are a lower bound)\n");
    }
    fflush(stderr);
}

// The four entries share these, so each records its *own* caller as the
// allocation site. One calling another would name the inner entry instead,
// and every Vec's buffer would trace to `rask_alloc_aligned`.
static void *alloc_at(int64_t size, int64_t align, void *site) {
    if (size <= 0) {
        return NULL;
    }
    void *ptr;
    if (align <= MALLOC_ALIGN) {
        ptr = active_allocator.alloc(size, active_allocator.ctx);
    } else {
        if (!active_allocator.alloc_aligned) {
            rask_panic_fmt("rask: the installed allocator can't align to %lld bytes "
                           "(it has no `alloc_aligned`)", (long long)align);
        }
        ptr = active_allocator.alloc_aligned(size, align, active_allocator.ctx);
    }
    if (!ptr) {
        fprintf(stderr, "rask: allocation failed (%lld bytes)\n", (long long)size);
        abort();
    }
    stats_track_alloc(size);
    leak_trace_record(ptr, size, site);
    return ptr;
}

static void free_sized(void *ptr, int64_t size) {
    if (!ptr) return;
    active_allocator.free(ptr, active_allocator.ctx);
    if (size > 0) stats_track_free(size);
    leak_trace_forget(ptr);
}

static void *realloc_at(void *ptr, int64_t old_size, int64_t new_size, int64_t align,
                        void *site) {
    if (new_size <= 0) {
        free_sized(ptr, old_size);
        return NULL;
    }
    // There is no aligned `realloc`, so a block past 16 moves by hand: a
    // fresh aligned block, the bytes both sizes share, and the old one freed.
    if (align > MALLOC_ALIGN) {
        void *fresh = alloc_at(new_size, align, site);
        if (ptr) {
            int64_t keep = old_size < new_size ? old_size : new_size;
            if (keep > 0) memcpy(fresh, ptr, (size_t)keep);
            free_sized(ptr, old_size);
        }
        return fresh;
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
    leak_trace_forget(ptr);
    leak_trace_record(new_ptr, new_size, site);
    return new_ptr;
}

void *rask_alloc(int64_t size) {
    return alloc_at(size, 0, __builtin_return_address(0));
}

void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size) {
    return realloc_at(ptr, old_size, new_size, 0, __builtin_return_address(0));
}

void *rask_alloc_aligned(int64_t size, int64_t align) {
    return alloc_at(size, align, __builtin_return_address(0));
}

void *rask_realloc_aligned(void *ptr, int64_t old_size, int64_t new_size, int64_t align) {
    return realloc_at(ptr, old_size, new_size, align, __builtin_return_address(0));
}

// ─── Closure blocks ────────────────────────────────────────
//
// A closure block is `[block_size | env_drop | refs | flags | func_ptr |
// captures...]` and the closure value points at `func_ptr`, so the environment is still
// `closure + 8`. Every header word is there for the same reason: whoever frees
// the block usually didn't build it. `let tick = counter()` hands the caller a
// block whose capture layout only `counter` knew, so the caller can neither
// account for the bytes nor release what the captures own.
//
// `env_drop` is the second — a generated `<closure>__env_drop(env)` that frees
// each container the environment owns, or NULL when it owns none. Without it a
// closure holding a `Vec` gave the block back and left the vector inside it,
// which is every adapter chain capturing its source (#1045, #943).
//
// `refs` is the third, and it's what lets a container hold a closure. A
// `Vec<func>` owns its elements' blocks and has to free them, but a vector
// *derived* from it — clone, slice, chunk, concat — copies element bytes, so
// two vectors then name one block and whichever is freed second frees it again.
// A deep copy isn't available: the block's env layout is known only to a
// generated glue, so copying one means retaining whatever its captures own, and
// there is no `env_retain` to ask. A count is: one owner logically, `env_drop`
// runs once, and nobody needs to know what's inside. Eight bytes per closure,
// against the alternative of refusing `.clone()` on a `Vec<func>`. The same
// count lets a frame hand a closure it only borrows — `spawn(fs[0])` — to
// something that will free it: the keeper gets a reference of its own
// (`ClosureRetain` in MIR, #1386).
//
// `flags` is the fourth. `RASK_CLOSURE_TASK_BOUND` marks a closure that
// captured a link or a `Local` box, which may not reach another task. The
// checker rejects a `spawn` it can see the closure in; one that got there by a
// return, a field or a container is invisible at the spawn, so the block says
// so itself and `rask_task_adopt_closure` refuses it (#1356).
//
// A *stack*-allocated closure has no header — a non-escaping closure is just
// `[func_ptr | captures]` in a frame — so neither of these may be called on
// one. Nothing does: a closure that can't escape can't reach a container.

// An `@align(64)` capture needs the environment on a multiple of 64, so the
// header slides up the block until it is, leaving `lead` bytes of padding in
// front. The free has to find the block's start again, and the code freeing a
// closure doesn't know what it captured, so the block says: `lead` rides in the
// top half of the flags word. A closure with no over-aligned capture has no
// lead and pays nothing for it.

#define CLOSURE_HEADER_WORDS 4
#define CLOSURE_LEAD_SHIFT 32

static int64_t *closure_block_start(int64_t *base) {
    return (int64_t *)((char *)base - (int64_t)((uint64_t)base[3] >> CLOSURE_LEAD_SHIFT));
}

void *rask_closure_alloc(int64_t block_size, void (*env_drop)(void *), int64_t flags,
                         int64_t env_align) {
    // The environment starts one word past the closure value.
    int64_t env_at = (CLOSURE_HEADER_WORDS + 1) * 8;
    int64_t lead = 0;
    if (env_align > 8) {
        lead = ((env_at + env_align - 1) & ~(env_align - 1)) - env_at;
    }
    int64_t total = lead + block_size + CLOSURE_HEADER_WORDS * 8;
    char *start = (char *)rask_alloc_aligned(total, env_align);
    int64_t *base = (int64_t *)(start + lead);
    base[0] = total;
    base[1] = (int64_t)(intptr_t)env_drop;
    base[2] = 1;
    base[3] = (int64_t)(((uint64_t)lead << CLOSURE_LEAD_SHIFT) | (uint32_t)flags);
    return (void *)(base + CLOSURE_HEADER_WORDS);
}

void rask_closure_retain(void *ptr) {
    if (!ptr) return;
    int64_t *base = ((int64_t *)ptr) - CLOSURE_HEADER_WORDS;
    // Atomic: a spawned task can hold a reference to a block the spawning
    // frame still holds too, and the task's is released on its own thread.
    __atomic_add_fetch(&base[2], 1, __ATOMIC_RELAXED);
}

int rask_closure_task_bound(const void *ptr) {
    if (!ptr) return 0;
    const int64_t *base = ((const int64_t *)ptr) - CLOSURE_HEADER_WORDS;
    return (base[3] & RASK_CLOSURE_TASK_BOUND) != 0;
}

void rask_closure_free(void *ptr) {
    if (!ptr) return;
    int64_t *base = ((int64_t *)ptr) - CLOSURE_HEADER_WORDS;
    if (__atomic_sub_fetch(&base[2], 1, __ATOMIC_ACQ_REL) > 0) return;
    void (*env_drop)(void *) = (void (*)(void *))(intptr_t)base[1];
    // The environment starts one word past the closure value, which is where
    // the captures the glue names live.
    if (env_drop) env_drop((char *)ptr + 8);
    rask_realloc((void *)closure_block_start(base), base[0], 0);
}

// ─── Trait object blocks ───────────────────────────────────
//
// A box's block is `[refs | value...]` and the fat pointer's data half points
// at the value, so `self` reaches a method unchanged and nothing but these
// three functions knows the count is there.
//
// The count is for a *derived* container. `boxes.clone()`, `.skip(n)`,
// `.take(n)` and `.chunks(n)` all copy element bytes, and a box's element is a
// pointer — so without one, two vectors would name one block and the second
// release would free it again (a segfault, reproducibly). Copying the value
// instead would mean copying whatever it holds, and Rask doesn't deep clone
// implicitly: the cost stays visible, so the copy shares.
//
// Sharing rather than cloning is also what a closure block does, and for the
// same reason — the environment's layout is known only to its generated glue,
// as a boxed value's contents are known only to its `owned_release`.
//
// The vtable says how big the value is and how it aligns, and both the alloc
// and the release read it there, so they can't disagree about where the block
// starts. The count sits in the word before the value, which is 8 in for most
// values and `align` in for anything aligned past 8.
static int64_t box_lead(const int64_t *vtable) {
    int64_t align = vtable[RASK_VTABLE_ALIGN_WORD];
    return align > 8 ? align : 8;
}

void *rask_box_alloc(const int64_t *vtable) {
    int64_t value_size = vtable[RASK_VTABLE_SIZE_WORD];
    int64_t lead = box_lead(vtable);
    int64_t total = (value_size < 8 ? 8 : value_size) + lead;
    char *start = (char *)rask_alloc_aligned(total, vtable[RASK_VTABLE_ALIGN_WORD]);
    int64_t *value = (int64_t *)(start + lead);
    value[-1] = 1;
    return (void *)value;
}

void rask_box_retain(void *value) {
    if (!value) return;
    int64_t *refs = ((int64_t *)value) - 1;
    refs[0] += 1;
}

// `owns` is false for a borrowed box, whose contents belong to the frame
// that boxed it (#1144); the block goes and the value's insides stay. A box
// that owns its value releases them through the vtable's `owned_release`, when
// the last reference goes.
void rask_box_release(void *value, const int64_t *vtable, int64_t owns) {
    if (!value) return;
    int64_t *refs = ((int64_t *)value) - 1;
    if ((refs[0] -= 1) > 0) return;
    void (*owned_release)(void *) =
        (void (*)(void *))(intptr_t)vtable[RASK_VTABLE_OWNED_RELEASE_WORD];
    if (owns && owned_release) owned_release(value);
    rask_free((char *)value - box_lead(vtable));
}

void rask_free(void *ptr) {
    if (ptr) {
        leak_trace_forget(ptr);
        active_allocator.free(ptr, active_allocator.ctx);
        // Note: we don't know the size here, so free_count increments
        // but bytes_freed doesn't. Use rask_realloc(ptr, old_size, 0)
        // for accurate byte tracking when size is known.
        atomic_fetch_add_explicit(&stat_free_count, 1, memory_order_relaxed);
    }
}
