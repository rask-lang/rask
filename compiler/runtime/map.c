// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Map — open-addressing hash map with linear probing.
// Separate arrays for slot states, keys, and values.
// Default hash: FNV-1a. Default equality: memcmp.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <time.h>
#include <unistd.h>

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
    // Value pointers currently lent out. Rehashing moves `vals`, so it is
    // refused while one is outstanding rather than left to dangle.
    int64_t    borrows;
    // Holds the value an overwrite displaced, so `insert` can hand it back
    // after the slot has been written over. Allocated on the first overwrite
    // and reused; good until the next one, which is the same window
    // `rask_map_take`'s pointer already gets.
    char      *displaced;
};

// ─── Hash seed ───────────────────────────────────────────────
// Mixed into bucket placement (see `map_bucket_hash`) so map layout — and
// thus iteration order — differs run to run: an attacker can no longer
// precompute FNV-1a collisions (HashDoS), and no program can come to depend
// on the exact order, matching determinism/D7 for production. (sim's
// replay-exact seeding is future work, once sim mode itself exists;
// rask_map_set_seed is the hook for it.)
//
// Not mixed into the hash *functions*: those are the public `.hash()`, which
// has to answer the same number for the same content every run (#744).
static uint64_t g_map_seed;
static pthread_once_t g_map_seed_once = PTHREAD_ONCE_INIT;

static uint64_t map_seed_splitmix64(uint64_t z) {
    z += 0x9e3779b97f4a7c15ULL;
    z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
    z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
    return z ^ (z >> 31);
}

static void map_seed_init(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t raw = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    raw ^= (uint64_t)getpid() << 32;
    raw ^= (uint64_t)(uintptr_t)&g_map_seed; // ASLR salt
    g_map_seed = map_seed_splitmix64(raw);
}

static uint64_t map_seed(void) {
    pthread_once(&g_map_seed_once, map_seed_init);
    return g_map_seed;
}

// Lets a future sim runtime pin map hashing to a value derived from the sim
// seed (for replay-exact order) instead of process entropy. Meant to be
// called once, single-threaded, during startup before any Map exists.
void rask_map_set_seed(uint64_t seed) {
    pthread_once(&g_map_seed_once, map_seed_init);
    g_map_seed = seed;
}

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

// `x.hash()` on an integer, a bool or a char (type.generics/HA1).
//
// Same FNV-1a over the value's little-endian bytes that an int-keyed Map buckets
// with, so a value and the same value used as a Map key agree. Unseeded, for the
// reason spelled out below `map_bucket_hash`: the seed belongs to bucket
// placement, not to the hash of a value — `.hash()` has to be as stable as `==`.
//
// `width` bytes are read from `lo` and then, if 16 are asked for, from `hi`. That
// covers every width from a 1-byte bool to a 16-byte u128 without the caller
// having to spell an address, which for a 128-bit value in a register it can't.
uint64_t rask_int_hash(uint64_t lo, uint64_t hi, int64_t width) {
    uint8_t bytes[16];
    for (int i = 0; i < 8; i++) {
        bytes[i] = (uint8_t)(lo >> (i * 8));
        bytes[8 + i] = (uint8_t)(hi >> (i * 8));
    }
    if (width < 1) width = 1;
    if (width > 16) width = 16;
    return rask_hash_bytes(bytes, width);
}

// String-keyed maps: key slot holds a 16-byte RaskStr value, hash/eq use string content
uint64_t rask_hash_string_key(const void *key, int64_t key_size) {
    (void)key_size;
    const RaskStr *s = (const RaskStr *)key;
    int64_t len = rask_string_len(s);
    const char *data = rask_string_ptr(s);
    uint64_t h = 0xcbf29ce484222325ULL;
    for (int64_t i = 0; i < len; i++) {
        h ^= (uint8_t)data[i];
        h *= 0x100000001b3ULL;
    }
    return h;
}

int rask_eq_string_key(const void *a, const void *b, int64_t key_size) {
    (void)key_size;
    const RaskStr *sa = (const RaskStr *)a;
    const RaskStr *sb = (const RaskStr *)b;
    // Fast path: bitwise identical (same SSO or same heap pointer+len)
    if (memcmp(sa, sb, 16) == 0) return 1;
    int64_t la = rask_string_len(sa);
    int64_t lb = rask_string_len(sb);
    if (la != lb) return 0;
    return memcmp(rask_string_ptr(sa), rask_string_ptr(sb), (size_t)la) == 0;
}

// ─── Internal ───────────────────────────────────────────────

static void map_alloc_tables(RaskMap *m, int64_t cap) {
    m->cap = cap;
    m->states = (uint8_t *)rask_alloc(cap);
    memset(m->states, MAP_EMPTY, (size_t)cap);
    m->keys = (char *)rask_alloc(rask_safe_mul(cap, m->key_size));
    m->vals = (char *)rask_alloc(rask_safe_mul(cap, m->val_size));
}

// Where the per-process seed belongs: bucket placement, not the hash value.
//
// It used to be mixed into the FNV accumulator inside `rask_hash_bytes` and
// `rask_hash_string_key`. Those are also what `string.hash()` is built on, so
// the public method inherited the randomization and answered a different number
// every run — while the interpreter, which seeds only its iteration order,
// answered the same number every time (#744). `.hash()` on a value should be as
// stable as `==` on it.
//
// Splitmix64 rather than a bare XOR because the bucket is `h % cap`: the seed
// has to reach the low bits, and this is the same mixer the seed itself is
// built with.
static uint64_t map_bucket_hash(const RaskMap *m, const void *key) {
    return map_seed_splitmix64(m->hash_fn(key, m->key_size) ^ map_seed());
}

static int64_t map_find_slot(const RaskMap *m, const void *key) {
    uint64_t h = map_bucket_hash(m, key);
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

// Every mutator that moves the value array or frees it goes through this.
static void map_check_no_borrows(const RaskMap *m, const char *op) {
    if (m && m->borrows > 0) {
        rask_panic_fmt("Map.%s while one of its values was being modified — "
                       "the value reference would dangle", op);
    }
}

// Value pointer lent straight out of the table, so a `mutate` callee writes the
// real value instead of a copy. Paired with rask_map_release_elem.
void *rask_map_borrow_elem(RaskMap *m, const void *key) {
    void *slot = rask_map_get(m, key);
    if (!slot) {
        rask_panic("key not found");
    }
    m->borrows++;
    return slot;
}

void rask_map_release_elem(RaskMap *m) {
    if (m && m->borrows > 0) m->borrows--;
}

static void map_rehash(RaskMap *m) {
    map_check_no_borrows(m, "insert");
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

    rask_realloc(old_states, old_cap, 0);
    rask_realloc(old_keys, rask_safe_mul(old_cap, m->key_size), 0);
    rask_realloc(old_vals, rask_safe_mul(old_cap, m->val_size), 0);
}

// ─── Public API ─────────────────────────────────────────────

RaskMap *rask_map_new(int64_t key_size, int64_t val_size) {
    return rask_map_new_custom(key_size, val_size, rask_hash_bytes, rask_eq_bytes);
}

RaskMap *rask_map_new_string_keys(int64_t key_size, int64_t val_size) {
    return rask_map_new_custom(key_size, val_size, rask_hash_string_key, rask_eq_string_key);
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
    m->borrows = 0;
    m->displaced = NULL;
    map_alloc_tables(m, MAP_INITIAL_CAP);
    return m;
}

void rask_map_free(RaskMap *m) {
    map_check_no_borrows(m, "free");
    if (!m) return;
    if (m->states) rask_realloc(m->states, m->cap, 0);
    if (m->keys) rask_realloc(m->keys, rask_safe_mul(m->cap, m->key_size), 0);
    if (m->vals) rask_realloc(m->vals, rask_safe_mul(m->cap, m->val_size), 0);
    if (m->displaced) rask_realloc(m->displaced, m->val_size, 0);
    rask_realloc(m, (int64_t)sizeof(RaskMap), 0);
}

int64_t rask_map_len(const RaskMap *m) {
    return m ? m->len : 0;
}

// Writes the entry and reports what it displaced.
//
// `displaced_out`, when given, is set to a pointer to the old value on an
// overwrite and to NULL on a fresh key. The slot is about to be written over,
// so the old bytes are copied into the map's scratch buffer first — returning
// the slot pointer the way `rask_map_take` does would hand back the *new*
// value.
static int64_t map_insert_impl(RaskMap *m, const void *key, const void *val,
                               void **displaced_out) {
    if (displaced_out) *displaced_out = NULL;
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
    if (displaced_out && prev_state == MAP_OCCUPIED) {
        if (!m->displaced) {
            m->displaced = (char *)rask_alloc(m->val_size);
        }
        memcpy(m->displaced, m->vals + slot * m->val_size, (size_t)m->val_size);
        *displaced_out = m->displaced;
    }
    memcpy(m->keys + slot * m->key_size, key, (size_t)m->key_size);
    memcpy(m->vals + slot * m->val_size, val, (size_t)m->val_size);
    m->states[slot] = MAP_OCCUPIED;
    if (prev_state == MAP_TOMBSTONE) m->tombstones--;
    if (prev_state != MAP_OCCUPIED) m->len++;
    return (prev_state == MAP_OCCUPIED) ? 1 : 0;
}

// Returns 0 if inserted new, 1 if updated existing. Used where the caller
// discards the answer — `Map.set`, rehashing, cloning, the runtime's own maps.
int64_t rask_map_insert(RaskMap *m, const void *key, const void *val) {
    return map_insert_impl(m, key, val, NULL);
}

// `Map.insert` is declared `-> V?`, so this is what it calls: a pointer to the
// displaced value, or NULL if the key was fresh. Same NULL-is-none shape as
// `rask_map_take`, so codegen adapts it with `RetAdapt::DerefOption`.
void *rask_map_insert_displaced(RaskMap *m, const void *key, const void *val) {
    void *old = NULL;
    map_insert_impl(m, key, val, &old);
    return old;
}

void *rask_map_get(const RaskMap *m, const void *key) {
    if (!m || m->len == 0) return NULL;

    uint64_t h = map_bucket_hash(m, key);
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

void *rask_map_get_unwrap(const RaskMap *m, const void *key) {
    void *result = rask_map_get(m, key);
    if (!result) {
        rask_panic("Map.get().unwrap(): key not found");
    }
    return result;
}

// Remove and hand back the value. `Map.remove` is declared `-> Option<V>`, so
// the caller needs the value, not just whether something was there. The slot is
// tombstoned but its bytes are left alone, so the returned pointer stays good
// until the map is next written — the same window `rask_map_get`'s callers
// already copy within.
void *rask_map_take(RaskMap *m, const void *key) {
    map_check_no_borrows(m, "remove");
    if (!m || m->len == 0) return NULL;

    uint64_t h = map_bucket_hash(m, key);
    int64_t idx = (int64_t)(h % (uint64_t)m->cap);

    for (int64_t i = 0; i < m->cap; i++) {
        int64_t slot = (idx + i) % m->cap;
        uint8_t state = m->states[slot];

        if (state == MAP_EMPTY) return NULL;
        if (state == MAP_TOMBSTONE) continue;
        if (m->eq_fn(m->keys + slot * m->key_size, key, m->key_size)) {
            m->states[slot] = MAP_TOMBSTONE;
            m->len--;
            m->tombstones++;
            return m->vals + slot * m->val_size;
        }
    }
    return NULL;
}

int64_t rask_map_remove(RaskMap *m, const void *key) {
    return rask_map_take(m, key) != NULL ? 0 : -1;
}

int64_t rask_map_contains(const RaskMap *m, const void *key) {
    return rask_map_get(m, key) != NULL;
}

int64_t rask_map_is_empty(const RaskMap *m) {
    return (!m || m->len == 0) ? 1 : 0;
}

void rask_map_clear(RaskMap *m) {
    map_check_no_borrows(m, "clear");
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

// entries: return Vec of (key, value) pairs for iteration.
// Each entry is a 16-byte struct { key: i64, value: i64 }.
// Entries as a Vec of (key, value) pairs laid out the way codegen lays out a
// tuple: key at 0, value at the next 8-byte boundary past it. The old version
// copied a fixed 8 bytes of each, which truncated a string key to its first
// word and handed iteration a corrupt RaskStr.
RaskVec *rask_map_entries(const RaskMap *m) {
    if (!m) return rask_vec_new(16);
    int64_t voff = (m->key_size + 7) & ~(int64_t)7;
    int64_t stride = (voff + m->val_size + 7) & ~(int64_t)7;
    RaskVec *v = rask_vec_with_capacity(stride, m->len);
    char *pair = (char *)rask_alloc(stride);
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] == MAP_OCCUPIED) {
            memset(pair, 0, (size_t)stride);
            memcpy(pair, m->keys + i * m->key_size, (size_t)m->key_size);
            memcpy(pair + voff, m->vals + i * m->val_size, (size_t)m->val_size);
            rask_vec_push(v, pair);
        }
    }
    rask_free(pair);
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

// mem.racks/RK3: drop every entry whose value is this link.
//
// The index-maintenance move a database makes when a row goes away — the entry
// leaves, it does not become a `none` under a live key. Values are compared as
// whole pointers because a link is one; nothing else in the map can match.
int64_t rask_map_drop_value_ptr(RaskMap *m, const void *target) {
    map_check_no_borrows(m, "drop_value_ptr");
    if (!m || m->val_size != (int64_t)sizeof(void *)) return 0;
    int64_t dropped = 0;
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] != MAP_OCCUPIED) continue;
        void *v;
        memcpy(&v, m->vals + i * m->val_size, sizeof(v));
        if (v != target) continue;
        m->states[i] = MAP_TOMBSTONE;
        m->len--;
        m->tombstones++;
        dropped++;
    }
    return dropped;
}

// Rewrite every value in place through `f`. For `Rack.snapshot()`, where a
// `Map<K, Link<T>>` field's values have to be re-pointed at the copied nodes:
// the values live in the map's own storage, which the caller can't reach.
void rask_map_map_values_ptr(RaskMap *m, void *(*f)(void *value, void *ctx), void *ctx) {
    map_check_no_borrows(m, "map_values_ptr");
    if (!m || m->val_size != (int64_t)sizeof(void *)) return;
    for (int64_t i = 0; i < m->cap; i++) {
        if (m->states[i] != MAP_OCCUPIED) continue;
        void *v;
        memcpy(&v, m->vals + i * m->val_size, sizeof(v));
        void *next = f(v, ctx);
        memcpy(m->vals + i * m->val_size, &next, sizeof(next));
    }
}
