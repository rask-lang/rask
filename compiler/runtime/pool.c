// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Pool — handle-based sparse storage with generation counters.
// Each slot tracks a generation to detect stale handles at O(1).
// Free slots form a singly-linked list through an index field.
//
// Layout: interleaved [gen:u32][next_free:i32][data:elem_size] per slot.
// Single allocation, one cache line per access.
// next_free == -2 means "occupied" (sentinel).

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>

// Slot offsets within the interleaved array
#define SLOT_GEN_OFFSET    0
#define SLOT_NEXT_OFFSET   4
#define SLOT_DATA_OFFSET   8

// Occupied sentinel — distinct from valid free-list values (>= 0 or -1)
#define SLOT_OCCUPIED      (-2)

struct RaskPool {
    uint32_t  pool_id;       // offset 0
    uint32_t  _pad;          // offset 4 (alignment)
    int64_t   elem_size;     // offset 8
    int64_t   slot_stride;   // offset 16
    int64_t   cap;           // offset 24
    int64_t   len;           // offset 32
    char     *slots;         // offset 40
    int32_t   free_head;     // offset 48
};

// Compile-time layout verification — codegen hardcodes these offsets
_Static_assert(offsetof(struct RaskPool, pool_id) == 0, "pool_id offset");
_Static_assert(offsetof(struct RaskPool, elem_size) == 8, "elem_size offset");
_Static_assert(offsetof(struct RaskPool, slot_stride) == 16, "slot_stride offset");
_Static_assert(offsetof(struct RaskPool, cap) == 24, "cap offset");
_Static_assert(offsetof(struct RaskPool, len) == 32, "len offset");
_Static_assert(offsetof(struct RaskPool, slots) == 40, "slots offset");
_Static_assert(offsetof(struct RaskPool, free_head) == 48, "free_head offset");

static uint32_t g_next_pool_id = 1;

// Compute stride: header (8 bytes) + elem_size, rounded up to 8-byte alignment
static inline int64_t compute_stride(int64_t elem_size) {
    return ((8 + elem_size + 7) / 8) * 8;
}

// Slot accessors — slot base address is p->slots + idx * p->slot_stride
static inline char *slot_at(const RaskPool *p, int64_t idx) {
    return p->slots + idx * p->slot_stride;
}

static inline uint32_t slot_gen(const char *slot) {
    uint32_t g;
    memcpy(&g, slot + SLOT_GEN_OFFSET, sizeof(g));
    return g;
}

static inline void slot_set_gen(char *slot, uint32_t g) {
    memcpy(slot + SLOT_GEN_OFFSET, &g, sizeof(g));
}

static inline int32_t slot_next(const char *slot) {
    int32_t n;
    memcpy(&n, slot + SLOT_NEXT_OFFSET, sizeof(n));
    return n;
}

static inline void slot_set_next(char *slot, int32_t n) {
    memcpy(slot + SLOT_NEXT_OFFSET, &n, sizeof(n));
}

static inline void *slot_data(const char *slot) {
    return (void *)(slot + SLOT_DATA_OFFSET);
}

static void pool_grow(RaskPool *p, int64_t new_cap) {
    int64_t old_bytes = rask_safe_mul(p->cap, p->slot_stride);
    int64_t new_bytes = rask_safe_mul(new_cap, p->slot_stride);
    p->slots = (char *)rask_realloc(p->slots, old_bytes, new_bytes);

    // Initialize new slots as free, chained together
    for (int64_t i = p->cap; i < new_cap; i++) {
        char *slot = slot_at(p, i);
        slot_set_gen(slot, 0);
        int32_t next = (i + 1 < new_cap) ? (int32_t)(i + 1) : p->free_head;
        slot_set_next(slot, next);
    }
    // New free list: old_cap -> old_cap+1 -> ... -> new_cap-1 -> old free_head
    p->free_head = (int32_t)p->cap;
    p->cap = new_cap;
}

RaskPool *rask_pool_new(int64_t elem_size) {
    RaskPool *p = (RaskPool *)rask_alloc(sizeof(RaskPool));
    p->pool_id = g_next_pool_id++;
    p->_pad = 0;
    p->elem_size = elem_size;
    p->slot_stride = compute_stride(elem_size);
    p->cap = 0;
    p->len = 0;
    p->slots = NULL;
    p->free_head = -1;
    return p;
}

RaskPool *rask_pool_with_capacity(int64_t elem_size, int64_t cap) {
    RaskPool *p = rask_pool_new(elem_size);
    if (cap > 0) {
        pool_grow(p, cap);
    }
    return p;
}

void rask_pool_free(RaskPool *p) {
    if (!p) return;
    if (p->slots) rask_realloc(p->slots, rask_safe_mul(p->cap, p->slot_stride), 0);
    rask_realloc(p, (int64_t)sizeof(RaskPool), 0);
}

int64_t rask_pool_len(const RaskPool *p) {
    return p ? p->len : 0;
}

RaskHandle rask_pool_insert(RaskPool *p, const void *elem) {
    RaskHandle h = RASK_HANDLE_INVALID;
#ifdef RASK_DEBUG
    if (!p) return h;
#endif

    // Grow if no free slots
    if (p->free_head < 0) {
        int64_t new_cap = p->cap ? p->cap * 2 : 4;
        pool_grow(p, new_cap);
    }

    // Pop from free list
    int32_t idx = p->free_head;
    char *slot = slot_at(p, idx);
    p->free_head = slot_next(slot);
    slot_set_next(slot, SLOT_OCCUPIED);

    // Write element data
    memcpy(slot_data(slot), elem, (size_t)p->elem_size);
    p->len++;

    h.pool_id = p->pool_id;
    h.index = (uint32_t)idx;
    h.generation = slot_gen(slot);
    return h;
}

static int pool_validate(const RaskPool *p, RaskHandle h) {
#ifdef RASK_DEBUG
    if (!p) return 0;
    if (h.pool_id != p->pool_id) return 0;
#endif
    if (h.index >= (uint32_t)p->cap) return 0;
    char *slot = slot_at(p, h.index);
    if (slot_gen(slot) != h.generation) return 0;
    return 1;
}

void *rask_pool_get(const RaskPool *p, RaskHandle h) {
    if (!pool_validate(p, h)) return NULL;
    return slot_data(slot_at(p, h.index));
}

int64_t rask_pool_remove(RaskPool *p, RaskHandle h, void *out) {
    if (!pool_validate(p, h)) return -1;

    char *slot = slot_at(p, h.index);

    if (out) {
        memcpy(out, slot_data(slot), (size_t)p->elem_size);
    }

    // Bump generation (saturate at UINT32_MAX to permanently invalidate)
    uint32_t gen = slot_gen(slot);
    if (gen < UINT32_MAX) {
        slot_set_gen(slot, gen + 1);
    }

    // Push onto free list
    slot_set_next(slot, p->free_head);
    p->free_head = (int32_t)h.index;
    p->len--;
    return 0;
}

int64_t rask_pool_is_valid(const RaskPool *p, RaskHandle h) {
    return pool_validate(p, h);
}

// ─── Packed i64 handle interface (codegen) ─────────────────
// Codegen currently represents handles as i64 (index:32 | gen:32).
// These functions bridge to the typed pool API by reconstructing
// the pool_id from the pool pointer.

static int64_t handle_pack(RaskHandle h) {
    return (int64_t)((uint64_t)h.index | ((uint64_t)h.generation << 32));
}

static RaskHandle handle_unpack(const RaskPool *p, int64_t packed) {
    RaskHandle h;
    h.pool_id = p->pool_id;
    h.index = (uint32_t)(packed & 0xFFFFFFFF);
    h.generation = (uint32_t)((packed >> 32) & 0xFFFFFFFF);
    return h;
}

int64_t rask_pool_alloc_packed(RaskPool *p) {
    RaskHandle h = rask_pool_alloc(p);
    return handle_pack(h);
}

int64_t rask_pool_insert_packed(RaskPool *p, const void *elem) {
    RaskHandle h = rask_pool_insert(p, elem);
    return handle_pack(h);
}

int64_t rask_pool_insert_packed_sized(RaskPool *p, const void *elem, int64_t elem_size) {
#ifdef RASK_DEBUG
    // Verify caller's elem_size matches pool's
    if (p->len > 0 || p->cap > 0) {
        if (elem_size != p->elem_size) {
            rask_panic("pool insert: elem_size mismatch");
        }
    }
#endif
    // Update elem_size on first insert (pool was created with elem_size=8 placeholder)
    if (p->len == 0 && p->cap == 0 && elem_size > p->elem_size) {
        p->elem_size = elem_size;
        p->slot_stride = compute_stride(elem_size);
    }
    return rask_pool_insert_packed(p, elem);
}

void *rask_pool_get_packed(const RaskPool *p, int64_t packed) {
    return rask_pool_get(p, handle_unpack(p, packed));
}

void *rask_pool_get_checked(const RaskPool *p, int64_t packed,
                            const char *file, int32_t line, int32_t col) {
    void *result = rask_pool_get(p, handle_unpack(p, packed));
    if (!result) {
        rask_panic_at(file, line, col, "pool access with invalid handle");
    }
    return result;
}

int64_t rask_pool_remove_packed(RaskPool *p, int64_t packed) {
    return rask_pool_remove(p, handle_unpack(p, packed), NULL);
}

int64_t rask_pool_is_valid_packed(const RaskPool *p, int64_t packed) {
    return rask_pool_is_valid(p, handle_unpack(p, packed));
}

RaskVec *rask_pool_handles_packed(const RaskPool *p) {
    RaskVec *v = rask_vec_new(8);
    if (!p) return v;
    for (int64_t i = 0; i < p->cap; i++) {
        char *slot = slot_at(p, i);
        if (slot_next(slot) != SLOT_OCCUPIED) continue;
        RaskHandle h;
        h.pool_id = p->pool_id;
        h.index = (uint32_t)i;
        h.generation = slot_gen(slot);
        int64_t packed = handle_pack(h);
        rask_vec_push(v, &packed);
    }
    return v;
}

RaskVec *rask_pool_values(const RaskPool *p) {
    RaskVec *v = rask_vec_new(p ? p->elem_size : 8);
    if (!p) return v;
    for (int64_t i = 0; i < p->cap; i++) {
        char *slot = slot_at(p, i);
        if (slot_next(slot) != SLOT_OCCUPIED) continue;
        rask_vec_push(v, slot_data(slot));
    }
    return v;
}

RaskVec *rask_pool_drain(RaskPool *p) {
    RaskVec *v = rask_vec_new(p ? p->elem_size : 8);
    if (!p) return v;
    for (int64_t i = 0; i < p->cap; i++) {
        char *slot = slot_at(p, i);
        if (slot_next(slot) != SLOT_OCCUPIED) continue;
        rask_vec_push(v, slot_data(slot));
        // Free the slot
        uint32_t gen = slot_gen(slot);
        if (gen < UINT32_MAX) {
            slot_set_gen(slot, gen + 1);
        }
        slot_set_next(slot, p->free_head);
        p->free_head = (int32_t)i;
        p->len--;
    }
    return v;
}

// Allocate a zero-initialized slot and return a handle to it.
RaskHandle rask_pool_alloc(RaskPool *p) {
    RaskHandle h = RASK_HANDLE_INVALID;
#ifdef RASK_DEBUG
    if (!p) return h;
#endif

    // Grow if no free slots
    if (p->free_head < 0) {
        int64_t new_cap = p->cap ? p->cap * 2 : 4;
        pool_grow(p, new_cap);
    }

    // Pop from free list
    int32_t idx = p->free_head;
    char *slot = slot_at(p, idx);
    p->free_head = slot_next(slot);
    slot_set_next(slot, SLOT_OCCUPIED);

    // Zero-initialize element data
    memset(slot_data(slot), 0, (size_t)p->elem_size);
    p->len++;

    h.pool_id = p->pool_id;
    h.index = (uint32_t)idx;
    h.generation = slot_gen(slot);
    return h;
}
