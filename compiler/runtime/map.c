// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Map — open-addressing hash map with linear probing.
// Separate arrays for slot states, keys, and values.
// Default hash: FNV-1a. Default equality: memcmp.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>

#define MAP_EMPTY     0
#define MAP_OCCUPIED  1
#define MAP_TOMBSTONE 2

#define MAP_INITIAL_CAP 16
#define MAP_LOAD_MAX_NUM 3  // load factor = 3/4 = 0.75
#define MAP_LOAD_MAX_DEN 4

struct RaskMap {
    int64_t    key_size;
    int64_t    val_size;
    int64_t    cap;
    int64_t    len;
    int64_t    tombstones;
    uint8_t   *states;
    char      *keys;
    char      *vals;
    RaskHashFn hash_fn;
    RaskEqFn   eq_fn;
};

// ─── Built-in hash/eq ───────────────────────────────────────

uint64_t rask_hash_bytes(const void *key, int64_t key_size) {
    const uint8_t *p = (const uint8_t *)key;
    uint64_t h = 0xcbf29ce484222325ULL;
    for (int64_t i = 0; i < key_size; i++) {
        h ^= p[i];
        h *= 0x100000001b3ULL;
    }
    return h;
}

int rask_eq_bytes(const void *a, const void *b, int64_t key_size) {
    return memcmp(a, b, (size_t)key_size) == 0;
}

// ─── Internal ───────────────────────────────────────────────

static void map_alloc_tables(RaskMap *m, int64_t cap) {
    m->cap = cap;
    m->states = (uint8_t *)rask_alloc(cap);
    memset(m->states, MAP_EMPTY, (size_t)cap);
    m->keys = (char *)rask_alloc(cap * m->key_size);
    m->vals = (char *)rask_alloc(cap * m->val_size);
}

static int64_t map_find_slot(const RaskMap *m, const void *key) {
    uint64_t h = m->hash_fn(key, m->key_size);
    int64_t idx = (int64_t)(h % (uint64_t)m->cap);
    int64_t first_tombstone = -1;

    for (int64_t i = 0; i < m->cap; i++) {
        int64_t slot = (idx + i) % m->cap;
        uint8_t state = m->states[slot];

        if (state == MAP_EMPTY) {
            return (first_tombstone >= 0) ? first_tombstone : slot;
        }
        if (state == MAP_TOMBSTONE) {
            if (first_tombstone < 0) first_tombstone = slot;
            continue;
        }
        // MAP_OCCUPIED — compare key
        if (m->eq_fn(m->keys + slot * m->key_size, key, m->key_size)) {
            return slot;
        }
    }
    // Table is full (shouldn't happen with load factor < 1)
    return (first_tombstone >= 0) ? first_tombstone : -1;
}

static void map_rehash(RaskMap *m) {
    int64_t old_cap = m->cap;
    uint8_t *old_states = m->states;
    char *old_keys = m->keys;
    char *old_vals = m->vals;

    map_alloc_tables(m, old_cap * 2);
    m->len = 0;
    m->tombstones = 0;

    for (int64_t i = 0; i < old_cap; i++) {
        if (old_states[i] == MAP_OCCUPIED) {
            rask_map_insert(m, old_keys + i * m->key_size,
                            old_vals + i * m->val_size);
        }
    }

    rask_free(old_states);
    rask_free(old_keys);
    rask_free(old_vals);
}

// ─── Public API ─────────────────────────────────────────────

RaskMap *rask_map_new(int64_t key_size, int64_t val_size) {
    return rask_map_new_custom(key_size, val_size, rask_hash_bytes, rask_eq_bytes);
}

RaskMap *rask_map_new_custom(int64_t key_size, int64_t val_size,
                             RaskHashFn hash, RaskEqFn eq) {
    RaskMap *m = (RaskMap *)rask_alloc(sizeof(RaskMap));
    m->key_size = key_size;
    m->val_size = val_size;
    m->len = 0;
    m->tombstones = 0;
    m->hash_fn = hash;
    m->eq_fn = eq;
    map_alloc_tables(m, MAP_INITIAL_CAP);
    return m;
}

void rask_map_free(RaskMap *m) {
    if (!m) return;
    rask_free(m->states);
    rask_free(m->keys);
    rask_free(m->vals);
    rask_free(m);
}

int64_t rask_map_len(const RaskMap *m) {
    return m ? m->len : 0;
}

// Returns 0 if inserted new, 1 if updated existing.
int64_t rask_map_insert(RaskMap *m, const void *key, const void *val) {
    if (!m) return -1;

    // Rehash if occupied + tombstones exceed load threshold.
    // Tombstones degrade probe chains just like occupied slots.
    if ((m->len + m->tombstones + 1) * MAP_LOAD_MAX_DEN > m->cap * MAP_LOAD_MAX_NUM) {
        map_rehash(m);
    }

    int64_t slot = map_find_slot(m, key);
    if (slot < 0) {
        // Shouldn't happen after rehash
        map_rehash(m);
        slot = map_find_slot(m, key);
    }

    uint8_t prev_state = m->states[slot];
    memcpy(m->keys + slot * m->key_size, key, (size_t)m->key_size);
    memcpy(m->vals + slot * m->val_size, val, (size_t)m->val_size);
    m->states[slot] = MAP_OCCUPIED;
    if (prev_state == MAP_TOMBSTONE) m->tombstones--;
    if (prev_state != MAP_OCCUPIED) m->len++;
    return (prev_state == MAP_OCCUPIED) ? 1 : 0;
}

void *rask_map_get(const RaskMap *m, const void *key) {
    if (!m || m->len == 0) return NULL;

    uint64_t h = m->hash_fn(key, m->key_size);
    int64_t idx = (int64_t)(h % (uint64_t)m->cap);

    for (int64_t i = 0; i < m->cap; i++) {
        int64_t slot = (idx + i) % m->cap;
        uint8_t state = m->states[slot];

        if (state == MAP_EMPTY) return NULL;
        if (state == MAP_TOMBSTONE) continue;
        if (m->eq_fn(m->keys + slot * m->key_size, key, m->key_size)) {
            return m->vals + slot * m->val_size;
        }
    }
    return NULL;
}

int64_t rask_map_remove(RaskMap *m, const void *key) {
    if (!m || m->len == 0) return -1;

    uint64_t h = m->hash_fn(key, m->key_size);
    int64_t idx = (int64_t)(h % (uint64_t)m->cap);

    for (int64_t i = 0; i < m->cap; i++) {
        int64_t slot = (idx + i) % m->cap;
        uint8_t state = m->states[slot];

        if (state == MAP_EMPTY) return -1;
        if (state == MAP_TOMBSTONE) continue;
        if (m->eq_fn(m->keys + slot * m->key_size, key, m->key_size)) {
            m->states[slot] = MAP_TOMBSTONE;
            m->len--;
            m->tombstones++;
            return 0;
        }
    }
    return -1;
}

int64_t rask_map_contains(const RaskMap *m, const void *key) {
    return rask_map_get(m, key) != NULL;
}

int64_t rask_map_is_empty(const RaskMap *m) {
    return (!m || m->len == 0) ? 1 : 0;
}

void rask_map_clear(RaskMap *m) {
    if (!m) return;
    memset(m->states, MAP_EMPTY, (size_t)m->cap);
    m->len = 0;
    m->tombstones = 0;
}

RaskVec *rask_map_keys(const RaskMap *m) {
    RaskVec *v = rask_vec_new(m ? m->key_size : 8);
    if (!m) return v;
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] == MAP_OCCUPIED) {
            rask_vec_push(v, m->keys + i * m->key_size);
        }
    }
    return v;
}

RaskVec *rask_map_values(const RaskMap *m) {
    RaskVec *v = rask_vec_new(m ? m->val_size : 8);
    if (!m) return v;
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] == MAP_OCCUPIED) {
            rask_vec_push(v, m->vals + i * m->val_size);
        }
    }
    return v;
}

RaskMap *rask_map_clone(const RaskMap *m) {
    if (!m) return rask_map_new(8, 8);
    RaskMap *dst = rask_map_new_custom(m->key_size, m->val_size, m->hash_fn, m->eq_fn);
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] == MAP_OCCUPIED) {
            rask_map_insert(dst, m->keys + i * m->key_size, m->vals + i * m->val_size);
        }
    }
    return dst;
}
