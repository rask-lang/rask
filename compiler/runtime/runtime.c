// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Minimal Rask runtime — provides built-in functions for native-compiled programs.
// Linked with the object file produced by rask-codegen.

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>

#include "rask_runtime.h"

// Forward declaration — user's main function, exported from the Rask module as rask_main
extern void rask_main(void);

// ─── Print functions ──────────────────────────────────────────────

void rask_print_i64(int64_t val) {
    printf("%lld", (long long)val);
}

void rask_print_bool(int8_t val) {
    printf("%s", val ? "true" : "false");
}

void rask_print_f64(double val) {
    printf("%g", val);
}

void rask_print_string(const char *ptr) {
    if (ptr) {
        fputs(ptr, stdout);
    }
}

void rask_print_newline(void) {
    putchar('\n');
}

// ─── Runtime support ──────────────────────────────────────────────

void rask_exit(int64_t code) {
    exit((int)code);
}

void rask_panic_unwrap(void) {
    fprintf(stderr, "panic: called unwrap on None/Err value\n");
    abort();
}

void rask_assert_fail(void) {
    fprintf(stderr, "panic: assertion failed\n");
    abort();
}

// ─── I/O primitives ──────────────────────────────────────────────
// Thin wrappers around POSIX syscalls. Return values match POSIX
// conventions: bytes transferred on success, -1 on error.

int64_t rask_io_open(const char *path, int64_t flags, int64_t mode) {
    return (int64_t)open(path, (int)flags, (mode_t)mode);
}

int64_t rask_io_close(int64_t fd) {
    return (int64_t)close((int)fd);
}

int64_t rask_io_read(int64_t fd, void *buf, int64_t len) {
    return (int64_t)read((int)fd, buf, (size_t)len);
}

int64_t rask_io_write(int64_t fd, const void *buf, int64_t len) {
    return (int64_t)write((int)fd, buf, (size_t)len);
}

// ─── Resource tracking ───────────────────────────────────────────
// Runtime enforcement for must-consume (linear) types.
// Simple fixed-size tracker — production would use a growable array.

#define RASK_MAX_RESOURCES 1024

struct rask_resource_entry {
    int64_t id;
    int64_t scope_depth;
    int     active;
};

static struct rask_resource_entry rask_resources[RASK_MAX_RESOURCES];
static int64_t rask_next_resource_id = 1;

int64_t rask_resource_register(int64_t scope_depth) {
    int64_t id = rask_next_resource_id++;
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (!rask_resources[i].active) {
            rask_resources[i].id = id;
            rask_resources[i].scope_depth = scope_depth;
            rask_resources[i].active = 1;
            return id;
        }
    }
    fprintf(stderr, "panic: resource tracker overflow\n");
    abort();
}

void rask_resource_consume(int64_t resource_id) {
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (rask_resources[i].active && rask_resources[i].id == resource_id) {
            rask_resources[i].active = 0;
            return;
        }
    }
    fprintf(stderr, "panic: consuming unknown resource %lld\n", (long long)resource_id);
    abort();
}

void rask_resource_scope_check(int64_t scope_depth) {
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (rask_resources[i].active && rask_resources[i].scope_depth == scope_depth) {
            fprintf(stderr, "panic: unconsumed resource at scope depth %lld\n",
                    (long long)scope_depth);
            abort();
        }
    }
}

// ─── Pool checked access ─────────────────────────────────────────
// Validates a pool handle before returning the element pointer.
// Handle format: lower 32 bits = index, upper 32 bits = generation.
// Pool memory layout: { capacity: i64, gen_array: i64*, data: i64*, occupied: i8* }

int64_t rask_pool_checked_access(int64_t pool_ptr, int64_t handle) {
    if (!pool_ptr) {
        fprintf(stderr, "panic: pool access on null pool\n");
        abort();
    }

    int64_t *pool = (int64_t *)pool_ptr;
    int64_t capacity   = pool[0];
    int64_t *gen_array = (int64_t *)pool[1];
    int64_t *data      = (int64_t *)pool[2];
    int8_t  *occupied  = (int8_t *)pool[3];

    int32_t index      = (int32_t)(handle & 0xFFFFFFFF);
    int32_t generation = (int32_t)((handle >> 32) & 0xFFFFFFFF);

    if (index < 0 || index >= capacity) {
        fprintf(stderr, "panic: pool index %d out of bounds (capacity %lld)\n",
                index, (long long)capacity);
        abort();
    }

    if (occupied && !occupied[index]) {
        fprintf(stderr, "panic: pool access to freed slot (index %d)\n", index);
        abort();
    }

    if (gen_array && gen_array[index] != generation) {
        fprintf(stderr, "panic: stale pool handle (index %d, expected gen %lld, got %d)\n",
                index, (long long)gen_array[index], generation);
        abort();
    }

    return (int64_t)&data[index];
}

// ─── Vec ─────────────────────────────────────────────────────────
// Dynamic array: { capacity: i64, len: i64, data: i64* }

struct rask_vec {
    int64_t  capacity;
    int64_t  len;
    int64_t *data;
};

int64_t rask_vec_new(void) {
    struct rask_vec *v = (struct rask_vec *)malloc(sizeof(struct rask_vec));
    if (!v) { fprintf(stderr, "panic: Vec alloc failed\n"); abort(); }
    v->capacity = 8;
    v->len = 0;
    v->data = (int64_t *)malloc(8 * sizeof(int64_t));
    if (!v->data) { fprintf(stderr, "panic: Vec alloc failed\n"); abort(); }
    return (int64_t)v;
}

void rask_vec_push(int64_t vec_ptr, int64_t value) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    if (v->len >= v->capacity) {
        v->capacity *= 2;
        v->data = (int64_t *)realloc(v->data, (size_t)v->capacity * sizeof(int64_t));
        if (!v->data) { fprintf(stderr, "panic: Vec realloc failed\n"); abort(); }
    }
    v->data[v->len++] = value;
}

int64_t rask_vec_pop(int64_t vec_ptr) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    if (v->len == 0) {
        fprintf(stderr, "panic: pop on empty Vec\n");
        abort();
    }
    return v->data[--v->len];
}

int64_t rask_vec_len(int64_t vec_ptr) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    return v->len;
}

int64_t rask_vec_get(int64_t vec_ptr, int64_t index) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    if (index < 0 || index >= v->len) {
        fprintf(stderr, "panic: Vec index %lld out of bounds (len %lld)\n",
                (long long)index, (long long)v->len);
        abort();
    }
    return v->data[index];
}

void rask_vec_set(int64_t vec_ptr, int64_t index, int64_t value) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    if (index < 0 || index >= v->len) {
        fprintf(stderr, "panic: Vec index %lld out of bounds (len %lld)\n",
                (long long)index, (long long)v->len);
        abort();
    }
    v->data[index] = value;
}

void rask_vec_clear(int64_t vec_ptr) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    v->len = 0;
}

int8_t rask_vec_is_empty(int64_t vec_ptr) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    return v->len == 0 ? 1 : 0;
}

int64_t rask_vec_capacity(int64_t vec_ptr) {
    struct rask_vec *v = (struct rask_vec *)vec_ptr;
    return v->capacity;
}

// ─── String ──────────────────────────────────────────────────────
// Heap-allocated null-terminated C string wrappers.

int64_t rask_string_new(void) {
    char *s = (char *)malloc(1);
    if (!s) { fprintf(stderr, "panic: string alloc failed\n"); abort(); }
    s[0] = '\0';
    return (int64_t)s;
}

int64_t rask_string_len(int64_t str_ptr) {
    if (!str_ptr) return 0;
    return (int64_t)strlen((const char *)str_ptr);
}

int64_t rask_string_concat(int64_t a_ptr, int64_t b_ptr) {
    const char *a = a_ptr ? (const char *)a_ptr : "";
    const char *b = b_ptr ? (const char *)b_ptr : "";
    size_t a_len = strlen(a);
    size_t b_len = strlen(b);
    char *result = (char *)malloc(a_len + b_len + 1);
    if (!result) { fprintf(stderr, "panic: string alloc failed\n"); abort(); }
    memcpy(result, a, a_len);
    memcpy(result + a_len, b, b_len);
    result[a_len + b_len] = '\0';
    return (int64_t)result;
}

// ─── Map ─────────────────────────────────────────────────────────
// Simple linear-scan hash map: { capacity: i64, len: i64, keys: i64*, values: i64* }

struct rask_map {
    int64_t  capacity;
    int64_t  len;
    int64_t *keys;
    int64_t *values;
    int8_t  *occupied;
};

int64_t rask_map_new(void) {
    struct rask_map *m = (struct rask_map *)malloc(sizeof(struct rask_map));
    if (!m) { fprintf(stderr, "panic: Map alloc failed\n"); abort(); }
    m->capacity = 16;
    m->len = 0;
    m->keys = (int64_t *)calloc((size_t)m->capacity, sizeof(int64_t));
    m->values = (int64_t *)calloc((size_t)m->capacity, sizeof(int64_t));
    m->occupied = (int8_t *)calloc((size_t)m->capacity, sizeof(int8_t));
    if (!m->keys || !m->values || !m->occupied) {
        fprintf(stderr, "panic: Map alloc failed\n"); abort();
    }
    return (int64_t)m;
}

void rask_map_insert(int64_t map_ptr, int64_t key, int64_t value) {
    struct rask_map *m = (struct rask_map *)map_ptr;
    // Check for existing key
    for (int64_t i = 0; i < m->capacity; i++) {
        if (m->occupied[i] && m->keys[i] == key) {
            m->values[i] = value;
            return;
        }
    }
    // Find empty slot
    for (int64_t i = 0; i < m->capacity; i++) {
        if (!m->occupied[i]) {
            m->keys[i] = key;
            m->values[i] = value;
            m->occupied[i] = 1;
            m->len++;
            return;
        }
    }
    // Full — grow (simple doubling)
    int64_t old_cap = m->capacity;
    m->capacity *= 2;
    m->keys = (int64_t *)realloc(m->keys, (size_t)m->capacity * sizeof(int64_t));
    m->values = (int64_t *)realloc(m->values, (size_t)m->capacity * sizeof(int64_t));
    m->occupied = (int8_t *)realloc(m->occupied, (size_t)m->capacity * sizeof(int8_t));
    if (!m->keys || !m->values || !m->occupied) {
        fprintf(stderr, "panic: Map realloc failed\n"); abort();
    }
    memset(m->occupied + old_cap, 0, (size_t)(m->capacity - old_cap) * sizeof(int8_t));
    m->keys[old_cap] = key;
    m->values[old_cap] = value;
    m->occupied[old_cap] = 1;
    m->len++;
}

int8_t rask_map_contains_key(int64_t map_ptr, int64_t key) {
    struct rask_map *m = (struct rask_map *)map_ptr;
    for (int64_t i = 0; i < m->capacity; i++) {
        if (m->occupied[i] && m->keys[i] == key) {
            return 1;
        }
    }
    return 0;
}

// ─── Pool ────────────────────────────────────────────────────────
// Object pool: { capacity: i64, gen_array: i64*, data: i64*, occupied: i8* }
// Slots tracked by separate occupied array — gen_array only holds generation
// counters for handle validation.

int64_t rask_pool_new(void) {
    int64_t *pool = (int64_t *)calloc(4, sizeof(int64_t));
    if (!pool) { fprintf(stderr, "panic: pool alloc failed\n"); abort(); }
    int64_t cap = 64;
    pool[0] = cap;
    pool[1] = (int64_t)calloc((size_t)cap, sizeof(int64_t)); // gen_array
    pool[2] = (int64_t)calloc((size_t)cap, sizeof(int64_t)); // data
    pool[3] = (int64_t)calloc((size_t)cap, sizeof(int8_t));  // occupied
    if (!pool[1] || !pool[2] || !pool[3]) {
        fprintf(stderr, "panic: pool alloc failed\n"); abort();
    }
    return (int64_t)pool;
}

int64_t rask_pool_alloc(int64_t pool_ptr) {
    int64_t *pool = (int64_t *)pool_ptr;
    int64_t capacity   = pool[0];
    int64_t *gen_array = (int64_t *)pool[1];
    int8_t  *occupied  = (int8_t *)pool[3];

    for (int64_t i = 0; i < capacity; i++) {
        if (!occupied[i]) {
            occupied[i] = 1;
            gen_array[i]++;
            // Pack index + generation into handle
            return (int64_t)((uint32_t)i | ((uint64_t)gen_array[i] << 32));
        }
    }
    fprintf(stderr, "panic: pool full\n");
    abort();
}

void rask_pool_free(int64_t pool_ptr, int64_t handle) {
    int64_t *pool = (int64_t *)pool_ptr;
    int64_t capacity   = pool[0];
    int64_t *gen_array = (int64_t *)pool[1];
    int8_t  *occupied  = (int8_t *)pool[3];
    int32_t index      = (int32_t)(handle & 0xFFFFFFFF);
    int32_t generation = (int32_t)((handle >> 32) & 0xFFFFFFFF);

    if (index < 0 || index >= capacity) {
        fprintf(stderr, "panic: pool free index %d out of bounds (capacity %lld)\n",
                index, (long long)capacity);
        abort();
    }
    if (!occupied[index]) {
        fprintf(stderr, "panic: pool double-free at index %d\n", index);
        abort();
    }
    if (gen_array[index] != generation) {
        fprintf(stderr, "panic: pool free with stale handle (index %d, expected gen %lld, got %d)\n",
                index, (long long)gen_array[index], generation);
        abort();
    }
    occupied[index] = 0;
}

int64_t rask_pool_get(int64_t pool_ptr, int64_t handle) {
    return rask_pool_checked_access(pool_ptr, handle);
}

// ─── Entry point ──────────────────────────────────────────────────

int main(int argc, char **argv) {
    rask_args_init(argc, argv);
    rask_main();
    return 0;
}
