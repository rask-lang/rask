// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Pool — handle-based sparse storage with generation counters.
// Each slot tracks a generation to detect stale handles at O(1).
// Free slots form a singly-linked list through an index field.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>

// Per-slot metadata, stored separately from element data.
typedef struct {
    uint32_t generation;
    int32_t  next_free; // next free slot index, only meaningful when !occupied
    uint8_t  occupied;
} PoolSlot;

struct RaskPool {
    uint32_t  pool_id;
    int64_t   elem_size;
    int64_t   cap;
    int64_t   len;
    PoolSlot *slots;
    char     *data;
    int32_t   free_head; // -1 = no free slots
};

static uint32_t g_next_pool_id = 1;

static void pool_grow(RaskPool *p, int64_t new_cap) {
    p->slots = (PoolSlot *)rask_realloc(p->slots,
        (int64_t)(p->cap * sizeof(PoolSlot)),
        (int64_t)(new_cap * sizeof(PoolSlot)));
    p->data = (char *)rask_realloc(p->data,
        p->cap * p->elem_size,
        new_cap * p->elem_size);

    // Initialize new slots as free, chained together
    for (int64_t i = p->cap; i < new_cap; i++) {
        p->slots[i].generation = 0;
        p->slots[i].next_free = (i + 1 < new_cap) ? (int32_t)(i + 1) : p->free_head;
        p->slots[i].occupied = 0;
    }
    // New free list: old_cap -> old_cap+1 -> ... -> new_cap-1 -> old free_head
    p->free_head = (int32_t)p->cap;
    p->cap = new_cap;
}

RaskPool *rask_pool_new(int64_t elem_size) {
    RaskPool *p = (RaskPool *)rask_alloc(sizeof(RaskPool));
    p->pool_id = g_next_pool_id++;
    p->elem_size = elem_size;
    p->cap = 0;
    p->len = 0;
    p->slots = NULL;
    p->data = NULL;
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
    rask_free(p->slots);
    rask_free(p->data);
    rask_free(p);
}

int64_t rask_pool_len(const RaskPool *p) {
    return p ? p->len : 0;
}

RaskHandle rask_pool_insert(RaskPool *p, const void *elem) {
    RaskHandle h = RASK_HANDLE_INVALID;
    if (!p) return h;

    // Grow if no free slots
    if (p->free_head < 0) {
        int64_t new_cap = p->cap ? p->cap * 2 : 4;
        pool_grow(p, new_cap);
    }

    // Pop from free list
    int32_t idx = p->free_head;
    p->free_head = p->slots[idx].next_free;
    p->slots[idx].occupied = 1;

    // Write element data
    memcpy(p->data + idx * p->elem_size, elem, (size_t)p->elem_size);
    p->len++;

    h.pool_id = p->pool_id;
    h.index = (uint32_t)idx;
    h.generation = p->slots[idx].generation;
    return h;
}

static int pool_validate(const RaskPool *p, RaskHandle h) {
    if (!p) return 0;
    if (h.pool_id != p->pool_id) return 0;
    if (h.index >= (uint32_t)p->cap) return 0;
    if (!p->slots[h.index].occupied) return 0;
    if (p->slots[h.index].generation != h.generation) return 0;
    return 1;
}

void *rask_pool_get(const RaskPool *p, RaskHandle h) {
    if (!pool_validate(p, h)) return NULL;
    return (void *)(p->data + h.index * p->elem_size);
}

int64_t rask_pool_remove(RaskPool *p, RaskHandle h, void *out) {
    if (!pool_validate(p, h)) return -1;

    if (out) {
        memcpy(out, p->data + h.index * p->elem_size, (size_t)p->elem_size);
    }

    // Bump generation (saturate at UINT32_MAX to permanently invalidate)
    if (p->slots[h.index].generation < UINT32_MAX) {
        p->slots[h.index].generation++;
    }

    // Push onto free list
    p->slots[h.index].occupied = 0;
    p->slots[h.index].next_free = p->free_head;
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
        if (!p->slots[i].occupied) continue;
        RaskHandle h;
        h.pool_id = p->pool_id;
        h.index = (uint32_t)i;
        h.generation = p->slots[i].generation;
        int64_t packed = handle_pack(h);
        rask_vec_push(v, &packed);
    }
    return v;
}

RaskVec *rask_pool_values(const RaskPool *p) {
    RaskVec *v = rask_vec_new(p ? p->elem_size : 8);
    if (!p) return v;
    for (int64_t i = 0; i < p->cap; i++) {
        if (!p->slots[i].occupied) continue;
        rask_vec_push(v, p->data + i * p->elem_size);
    }
    return v;
}

RaskVec *rask_pool_drain(RaskPool *p) {
    RaskVec *v = rask_vec_new(p ? p->elem_size : 8);
    if (!p) return v;
    for (int64_t i = 0; i < p->cap; i++) {
        if (!p->slots[i].occupied) continue;
        rask_vec_push(v, p->data + i * p->elem_size);
        // Free the slot
        if (p->slots[i].generation < UINT32_MAX) {
            p->slots[i].generation++;
        }
        p->slots[i].occupied = 0;
        p->slots[i].next_free = p->free_head;
        p->free_head = (int32_t)i;
        p->len--;
    }
    return v;
}

// Allocate a zero-initialized slot and return a handle to it.
RaskHandle rask_pool_alloc(RaskPool *p) {
    RaskHandle h = RASK_HANDLE_INVALID;
    if (!p) return h;

    // Grow if no free slots
    if (p->free_head < 0) {
        int64_t new_cap = p->cap ? p->cap * 2 : 4;
        pool_grow(p, new_cap);
    }

    // Pop from free list
    int32_t idx = p->free_head;
    p->free_head = p->slots[idx].next_free;
    p->slots[idx].occupied = 1;

    // Zero-initialize element data
    memset(p->data + idx * p->elem_size, 0, (size_t)p->elem_size);
    p->len++;

    h.pool_id = p->pool_id;
    h.index = (uint32_t)idx;
    h.generation = p->slots[idx].generation;
    return h;
}
