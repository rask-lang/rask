// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask C runtime — data structures and utilities for native-compiled programs.
// Linked with object files produced by rask-codegen.

#ifndef RASK_RUNTIME_H
#define RASK_RUNTIME_H

#include <stdint.h>
#include <stddef.h>

// ─── Allocator ──────────────────────────────────────────────

void *rask_alloc(int64_t size);
void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size);
void  rask_free(void *ptr);

// ─── Vec ────────────────────────────────────────────────────
// Growable array storing elements as raw bytes.

typedef struct RaskVec RaskVec;

RaskVec *rask_vec_new(int64_t elem_size);
RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap);
void     rask_vec_free(RaskVec *v);
int64_t  rask_vec_len(const RaskVec *v);
int64_t  rask_vec_capacity(const RaskVec *v);
int64_t  rask_vec_push(RaskVec *v, const void *elem);
void    *rask_vec_get(const RaskVec *v, int64_t index);
void     rask_vec_set(RaskVec *v, int64_t index, const void *elem);
int64_t  rask_vec_pop(RaskVec *v, void *out);
int64_t  rask_vec_remove(RaskVec *v, int64_t index);
void     rask_vec_clear(RaskVec *v);
int64_t  rask_vec_reserve(RaskVec *v, int64_t additional);

// ─── String ─────────────────────────────────────────────────
// UTF-8 owned string, always null-terminated.

typedef struct RaskString RaskString;

RaskString *rask_string_new(void);
RaskString *rask_string_from(const char *s);
RaskString *rask_string_from_bytes(const char *data, int64_t len);
void        rask_string_free(RaskString *s);
int64_t     rask_string_len(const RaskString *s);
const char *rask_string_ptr(const RaskString *s);
int64_t     rask_string_push_byte(RaskString *s, uint8_t byte);
int64_t     rask_string_push_char(RaskString *s, int32_t codepoint);
int64_t     rask_string_append(RaskString *s, const RaskString *other);
int64_t     rask_string_append_cstr(RaskString *s, const char *cstr);
RaskString *rask_string_clone(const RaskString *s);
int64_t     rask_string_eq(const RaskString *a, const RaskString *b);
RaskString *rask_string_substr(const RaskString *s, int64_t start, int64_t end);

// ─── Map ────────────────────────────────────────────────────
// Open-addressing hash map with linear probing.
// Keys and values stored as raw bytes. Uses FNV-1a hashing + memcmp by default.
// For string-keyed maps, supply custom hash/eq via rask_map_new_custom.

typedef struct RaskMap RaskMap;

typedef uint64_t (*RaskHashFn)(const void *key, int64_t key_size);
typedef int      (*RaskEqFn)(const void *a, const void *b, int64_t key_size);

RaskMap *rask_map_new(int64_t key_size, int64_t val_size);
RaskMap *rask_map_new_custom(int64_t key_size, int64_t val_size,
                             RaskHashFn hash, RaskEqFn eq);
void     rask_map_free(RaskMap *m);
int64_t  rask_map_len(const RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);
void    *rask_map_get(const RaskMap *m, const void *key);
int64_t  rask_map_remove(RaskMap *m, const void *key);
int64_t  rask_map_contains(const RaskMap *m, const void *key);

// Built-in hash/eq functions
uint64_t rask_hash_bytes(const void *key, int64_t key_size);
int      rask_eq_bytes(const void *a, const void *b, int64_t key_size);

// ─── Pool ───────────────────────────────────────────────────
// Handle-based sparse storage with generation counters.

typedef struct {
    uint32_t pool_id;
    uint32_t index;
    uint32_t generation;
} RaskHandle;

typedef struct RaskPool RaskPool;

RaskPool   *rask_pool_new(int64_t elem_size);
RaskPool   *rask_pool_with_capacity(int64_t elem_size, int64_t cap);
void        rask_pool_free(RaskPool *p);
int64_t     rask_pool_len(const RaskPool *p);
RaskHandle  rask_pool_insert(RaskPool *p, const void *elem);
void       *rask_pool_get(const RaskPool *p, RaskHandle h);
int64_t     rask_pool_remove(RaskPool *p, RaskHandle h, void *out);
int64_t     rask_pool_is_valid(const RaskPool *p, RaskHandle h);

#define RASK_HANDLE_INVALID ((RaskHandle){0, UINT32_MAX, 0})

// ─── CLI args ───────────────────────────────────────────────

void        rask_args_init(int argc, char **argv);
int64_t     rask_args_count(void);
const char *rask_args_get(int64_t index);

#endif // RASK_RUNTIME_H
