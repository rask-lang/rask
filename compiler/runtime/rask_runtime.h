// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask C runtime — data structures and utilities for native-compiled programs.
// Linked with object files produced by rask-codegen.

#ifndef RASK_RUNTIME_H
#define RASK_RUNTIME_H

#include <stdint.h>
#include <stddef.h>

// ─── Allocator ──────────────────────────────────────────────
// Swappable allocator with optional stats tracking.
// Default uses malloc/realloc/free. Call rask_allocator_set() before any
// allocations to swap in a custom allocator.

typedef struct {
    void *(*alloc)(int64_t size, void *ctx);
    void *(*realloc)(void *ptr, int64_t old_size, int64_t new_size, void *ctx);
    void  (*free)(void *ptr, void *ctx);
    void  *ctx;
} RaskAllocator;

typedef struct {
    int64_t alloc_count;
    int64_t free_count;
    int64_t bytes_allocated;
    int64_t bytes_freed;
    int64_t peak_bytes;
} RaskAllocStats;

void  rask_allocator_set(const RaskAllocator *a);
void  rask_alloc_stats(RaskAllocStats *out);

// These use the active allocator (default: malloc/free).
void *rask_alloc(int64_t size);
void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size);
void  rask_free(void *ptr);

// Overflow-checked arithmetic for allocation sizes.
_Noreturn void rask_panic(const char *msg);

static inline int64_t rask_safe_mul(int64_t a, int64_t b) {
    if (a > 0 && b > 0 && a > INT64_MAX / b) rask_panic("allocation size overflow");
    return a * b;
}

static inline int64_t rask_safe_add(int64_t a, int64_t b) {
    if (a > 0 && b > 0 && a > INT64_MAX - b) rask_panic("allocation size overflow");
    return a + b;
}

// ─── Vec ────────────────────────────────────────────────────
// Growable array storing elements as raw bytes.

typedef struct RaskVec RaskVec;

RaskVec *rask_vec_new(int64_t elem_size);
RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap);
RaskVec *rask_vec_from_static(const char *data, int64_t count);
void     rask_vec_free(RaskVec *v);
int64_t  rask_vec_len(const RaskVec *v);
int64_t  rask_vec_capacity(const RaskVec *v);
int64_t  rask_vec_push(RaskVec *v, const void *elem);
void    *rask_vec_get(const RaskVec *v, int64_t index);
void    *rask_vec_get_unchecked(const RaskVec *v, int64_t index);
void     rask_vec_set(RaskVec *v, int64_t index, const void *elem);
int64_t  rask_vec_pop(RaskVec *v, void *out);
int64_t  rask_vec_remove(RaskVec *v, int64_t index);
void     rask_vec_clear(RaskVec *v);
int64_t  rask_vec_reserve(RaskVec *v, int64_t additional);
int64_t  rask_vec_is_empty(const RaskVec *v);
int64_t  rask_vec_insert_at(RaskVec *v, int64_t index, const void *elem);
int64_t  rask_vec_remove_at(RaskVec *v, int64_t index, void *out);
RaskVec *rask_iter_skip(const RaskVec *src, int64_t n);
RaskVec *rask_vec_clone(const RaskVec *v);
void     rask_vec_sort(RaskVec *v);
void     rask_vec_sort_by(RaskVec *v, int64_t comparator);
void     rask_vec_reverse(RaskVec *v);
int64_t  rask_vec_contains(const RaskVec *v, const void *elem);
void     rask_vec_dedup(RaskVec *v);
void    *rask_vec_first(const RaskVec *v);
void    *rask_vec_last(const RaskVec *v);

// ─── String ─────────────────────────────────────────────────
// 16-byte tagged union with small string optimization (SSO).
//
// SSO mode (MSB of byte 15 = 0):
//   [data: u8[15]][remaining: u8]   remaining = 15 - len
//   Unused data bytes zeroed → always null-terminated.
//
// Heap mode (MSB of byte 15 = 1):
//   [header_ptr: *u8 (8B)][tagged_len: u64 (8B)]
//   tagged_len = len | (1<<63). Header: { refcount_u32, cap_u32, data[] }
//
// RC only applies to heap mode. Sentinel refcount (UINT32_MAX) = static literal.

typedef union {
    struct { uint8_t data[15]; uint8_t remaining; } sso;  // remaining = 15 - len
    struct { uint8_t *header; uint64_t tagged_len; } heap; // tagged_len = len | (1<<63)
    uint8_t raw[16];
} RaskStr;

// Constructors (out-param)
void        rask_string_new(RaskStr *out);
void        rask_string_from(RaskStr *out, const char *s);
void        rask_string_from_bytes(RaskStr *out, const char *data, int64_t len);

// RC operations — codegen calls after inline tag check (RC5)
void        rask_string_free(const RaskStr *s);
void        rask_string_clone(const RaskStr *s);

// Read-only accessors
int64_t     rask_string_len(const RaskStr *s);
const char *rask_string_ptr(const RaskStr *s);
int64_t     rask_string_is_empty(const RaskStr *s);
int64_t     rask_string_eq(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_compare(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_lt(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_gt(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_le(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_ge(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_byte_at(const RaskStr *s, int64_t pos);
int64_t     rask_string_char_at(const RaskStr *s, int64_t index);
int64_t     rask_string_contains(const RaskStr *haystack, const RaskStr *needle);
int64_t     rask_string_starts_with(const RaskStr *s, const RaskStr *prefix);
int64_t     rask_string_ends_with(const RaskStr *s, const RaskStr *suffix);
int64_t     rask_string_find(const RaskStr *haystack, const RaskStr *needle);
int64_t     rask_string_rfind(const RaskStr *haystack, const RaskStr *needle);
int64_t     rask_string_parse_int(const RaskStr *s);
double      rask_string_parse_float(const RaskStr *s);

// String-producing operations (out-param: RaskStr *out as first param)
void        rask_string_concat(RaskStr *out, const RaskStr *a, const RaskStr *b);
void        rask_string_substr(RaskStr *out, const RaskStr *s, int64_t start, int64_t end);
void        rask_string_to_lowercase(RaskStr *out, const RaskStr *s);
void        rask_string_to_uppercase(RaskStr *out, const RaskStr *s);
void        rask_string_trim(RaskStr *out, const RaskStr *s);
void        rask_string_trim_start(RaskStr *out, const RaskStr *s);
void        rask_string_trim_end(RaskStr *out, const RaskStr *s);
void        rask_string_repeat(RaskStr *out, const RaskStr *s, int64_t count);
void        rask_string_reverse(RaskStr *out, const RaskStr *s);
void        rask_string_replace(RaskStr *out, const RaskStr *s, const RaskStr *from, const RaskStr *to);

// Builder operations (out-param: mutates string via promote-to-heap)
void        rask_string_push_byte(RaskStr *out, const RaskStr *s, uint8_t byte);
void        rask_string_push_char(RaskStr *out, const RaskStr *s, int32_t codepoint);
void        rask_string_append(RaskStr *out, const RaskStr *s, const RaskStr *other);
void        rask_string_append_cstr(RaskStr *out, const RaskStr *s, const char *cstr);
void        rask_string_push_str(RaskStr *out, const RaskStr *s, const RaskStr *other);

// Vec-returning operations (elements are RaskStr, elem_size=16)
RaskVec    *rask_string_lines(const RaskStr *s);
RaskVec    *rask_string_split(const RaskStr *s, const RaskStr *sep);
RaskVec    *rask_string_split_whitespace(const RaskStr *s);
RaskVec    *rask_string_chars(const RaskStr *s);

// Conversion to string (out-param)
void        rask_i64_to_string(RaskStr *out, int64_t val);
void        rask_bool_to_string(RaskStr *out, int64_t val);
void        rask_f64_to_string(RaskStr *out, double val);
void        rask_char_to_string(RaskStr *out, int32_t codepoint);

// ─── Path ────────────────────────────────────────────────────
// Filesystem path operations. Path is stored as a plain RaskStr.
// Option-returning methods return NULL (None) or pointer to
// thread-local RaskStr (Some). Codegen copies immediately.

// Constructors / conversions (out-param)
void    rask_path_new(RaskStr *out, const RaskStr *s);
void    rask_path_to_string(RaskStr *out, const RaskStr *s);
void    rask_path_join(RaskStr *out, const RaskStr *self, const RaskStr *other);
void    rask_path_with_extension(RaskStr *out, const RaskStr *self, const RaskStr *ext);
void    rask_path_with_file_name(RaskStr *out, const RaskStr *self, const RaskStr *name);

// Option-returning (NULL→None, &buf→Some)
int64_t rask_path_parent(int64_t path_ptr);
int64_t rask_path_file_name(int64_t path_ptr);
int64_t rask_path_extension(int64_t path_ptr);
int64_t rask_path_stem(int64_t path_ptr);

// Bool-returning
int64_t rask_path_is_absolute(int64_t path_ptr);
int64_t rask_path_is_relative(int64_t path_ptr);
int64_t rask_path_has_extension(int64_t path_ptr);

// Vec<string>-returning
int64_t rask_path_components(int64_t path_ptr);

// Char predicates — operate on Unicode codepoints (i32).
int64_t rask_char_is_digit(int32_t c);
int64_t rask_char_is_ascii(int32_t c);
int64_t rask_char_is_alphabetic(int32_t c);
int64_t rask_char_is_numeric(int32_t c);
int64_t rask_char_is_alphanumeric(int32_t c);
int64_t rask_char_is_whitespace(int32_t c);
int64_t rask_char_is_uppercase(int32_t c);
int64_t rask_char_is_lowercase(int32_t c);
int64_t rask_char_to_int(int32_t c);
int64_t rask_char_to_uppercase(int32_t c);
int64_t rask_char_to_lowercase(int32_t c);
int64_t rask_char_len_utf8(int32_t c);
int64_t rask_char_eq(int32_t a, int32_t b);

// ─── Vec (string-dependent) ─────────────────────────────────
void     rask_vec_join(RaskStr *out, const RaskVec *src, const RaskStr *sep);
void     rask_vec_join_i64(RaskStr *out, const RaskVec *src, const RaskStr *sep);

// ─── Map ────────────────────────────────────────────────────
// Open-addressing hash map with linear probing.
// Keys and values stored as raw bytes. Uses FNV-1a hashing + memcmp by default.
// For string-keyed maps, supply custom hash/eq via rask_map_new_custom.

typedef struct RaskMap RaskMap;

typedef uint64_t (*RaskHashFn)(const void *key, int64_t key_size);
typedef int      (*RaskEqFn)(const void *a, const void *b, int64_t key_size);

RaskMap *rask_map_new(int64_t key_size, int64_t val_size);
RaskMap *rask_map_new_string_keys(int64_t key_size, int64_t val_size);
RaskMap *rask_map_new_custom(int64_t key_size, int64_t val_size,
                             RaskHashFn hash, RaskEqFn eq);
void     rask_map_free(RaskMap *m);
int64_t  rask_map_len(const RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);
void    *rask_map_get(const RaskMap *m, const void *key);
void    *rask_map_get_unwrap(const RaskMap *m, const void *key);
int64_t  rask_map_remove(RaskMap *m, const void *key);
int64_t  rask_map_contains(const RaskMap *m, const void *key);
int64_t  rask_map_is_empty(const RaskMap *m);
void     rask_map_clear(RaskMap *m);
RaskVec *rask_map_keys(const RaskMap *m);
RaskVec *rask_map_values(const RaskMap *m);
RaskMap *rask_map_clone(const RaskMap *m);

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
int64_t     rask_pool_is_empty(const RaskPool *p);
RaskHandle  rask_pool_insert(RaskPool *p, const void *elem);
void       *rask_pool_get(const RaskPool *p, RaskHandle h);
int64_t     rask_pool_remove(RaskPool *p, RaskHandle h, void *out);
int64_t     rask_pool_is_valid(const RaskPool *p, RaskHandle h);
RaskHandle  rask_pool_alloc(RaskPool *p);

// Packed i64 handle interface for codegen (index:32 | gen:32, pool_id from pool ptr)
int64_t     rask_pool_alloc_packed(RaskPool *p);
int64_t     rask_pool_insert_packed(RaskPool *p, const void *elem);
int64_t     rask_pool_insert_packed_sized(RaskPool *p, const void *elem, int64_t elem_size);
void       *rask_pool_get_packed(const RaskPool *p, int64_t packed);
void       *rask_pool_get_checked(const RaskPool *p, int64_t packed,
                                  const char *file, int32_t line, int32_t col);
int64_t     rask_pool_remove_packed(RaskPool *p, int64_t packed);
int64_t     rask_pool_is_valid_packed(const RaskPool *p, int64_t packed);
RaskVec    *rask_pool_handles_packed(const RaskPool *p);
RaskVec    *rask_pool_values(const RaskPool *p);
RaskVec    *rask_pool_drain(RaskPool *p);

#define RASK_HANDLE_INVALID ((RaskHandle){0, UINT32_MAX, 0})

// Packed sentinel for Option<Handle<T>> niche optimization.
// All bits set (index=UINT32_MAX, gen=UINT32_MAX) — impossible for a real handle.
// Option<Handle<T>> uses this as None; any other i64 is Some(handle).
#define RASK_HANDLE_PACKED_NONE ((int64_t)-1)

// ─── Rng (random) ───────────────────────────────────────────
// xoshiro256++ PRNG. 32-byte state, heap-allocated.

typedef struct RaskRng RaskRng;

RaskRng *rask_rng_new(void);
RaskRng *rask_rng_from_seed(int64_t seed);
int64_t  rask_rng_u64(RaskRng *rng);
int64_t  rask_rng_i64(RaskRng *rng);
double   rask_rng_f64(RaskRng *rng);
double   rask_rng_f32(RaskRng *rng);
int64_t  rask_rng_bool(RaskRng *rng);
int64_t  rask_rng_range(RaskRng *rng, int64_t lo, int64_t hi);

// Module-level convenience (thread-local PRNG)
double   rask_random_f64(void);
double   rask_random_f32(void);
int64_t  rask_random_i64(void);
int64_t  rask_random_bool(void);
int64_t  rask_random_range(int64_t lo, int64_t hi);

// ─── FS module ──────────────────────────────────────────────
// Higher-level file operations. Return FILE* as i64.

int8_t      rask_fs_exists(const RaskStr *path);

void        rask_fwrite_vec(int64_t fptr, const RaskVec *v);

// Thin wrappers for libc functions whose names clash with Rask methods
// or that access C struct fields
int32_t     rask_libc_rename(const char *from, const char *to);
int32_t     rask_libc_remove(const char *path);
int32_t     rask_libc_mkdir(const char *path, uint32_t mode);
const char *rask_dirent_name(void *entry);
int64_t     rask_stat_size(const char *path);
int64_t     rask_stat_mtime(const char *path);
int64_t     rask_stat_atime(const char *path);
void        rask_fs_read_file(RaskStr *out, const RaskStr *path);
RaskVec    *rask_fs_read_bytes(const RaskStr *path);
void        rask_fs_write_file(const RaskStr *path, const RaskStr *content);
void        rask_fs_write_bytes(const RaskStr *path, RaskVec *data);
RaskVec    *rask_fs_read_lines(const RaskStr *path);
RaskVec    *rask_fs_list_dir(const RaskStr *path);
int64_t     rask_fs_open(const RaskStr *path);
int64_t     rask_fs_create(const RaskStr *path);
void        rask_fs_canonicalize(RaskStr *out, const RaskStr *path);
int64_t     rask_fs_copy(const RaskStr *from, const RaskStr *to);
void        rask_fs_rename(const RaskStr *from, const RaskStr *to);
void        rask_fs_remove(const RaskStr *path);
void        rask_fs_create_dir(const RaskStr *path);
void        rask_fs_create_dir_all(const RaskStr *path);
void        rask_fs_append_file(const RaskStr *path, const RaskStr *content);

// ─── File instance methods ──────────────────────────────────
// Operate on FILE* handles returned by rask_fs_open/rask_fs_create.

void        rask_file_close(int64_t file);
void        rask_file_read_all(RaskStr *out, int64_t file);
void        rask_file_write(int64_t file, const RaskStr *content);
void        rask_file_write_all(int64_t file, const RaskStr *content);
void        rask_file_write_line(int64_t file, const RaskStr *content);
RaskVec    *rask_file_lines(int64_t file);

// ─── IO module ──────────────────────────────────────────────
void        rask_io_read_line(RaskStr *out);
int64_t     rask_io_write_string(int64_t fd, int64_t str_ptr);

// ─── Time module ────────────────────────────────────────────
// Instant = i64 nanoseconds (CLOCK_MONOTONIC), Duration = i64 nanoseconds.

int64_t rask_time_Instant_now(void);
int64_t rask_time_Instant_elapsed(int64_t instant_ns);
int64_t rask_time_Duration_from_nanos(int64_t ns);
int64_t rask_time_Duration_from_millis(int64_t ms);
int64_t rask_time_Duration_as_nanos(int64_t duration_ns);
int64_t rask_time_Duration_as_millis(int64_t duration_ns);
int64_t rask_time_Duration_as_micros(int64_t duration_ns);
int64_t rask_time_Duration_as_secs(int64_t duration_ns);
double  rask_time_Duration_as_secs_f64(int64_t duration_ns);
double  rask_time_Duration_as_secs_f32(int64_t duration_ns);
int64_t rask_time_Duration_seconds(int64_t secs);
int64_t rask_time_Duration_millis(int64_t ms);
int64_t rask_time_Duration_micros(int64_t us);
int64_t rask_time_Duration_nanos(int64_t ns);
int64_t rask_time_Duration_from_secs_f64(double secs);
int64_t rask_time_Instant_duration_since(int64_t self_ns, int64_t other_ns);

// ─── Net module ─────────────────────────────────────────────
// Basic TCP socket operations.

int64_t rask_net_tcp_listen(const RaskStr *addr);
int64_t rask_net_tcp_connect(const RaskStr *addr);
int64_t rask_net_tcp_accept(int64_t listen_fd);
void    rask_net_close(int64_t fd);
int64_t rask_net_clone(int64_t fd);
int64_t rask_net_read_all(int64_t fd, int64_t out_ptr);
int64_t rask_net_write_all(int64_t fd, int64_t str_ptr);
void    rask_net_remote_addr(RaskStr *out, int64_t fd);

// ─── Filesystem metadata ────────────────────────────────────
int64_t rask_fs_metadata(int64_t path_ptr);
int64_t rask_metadata_size(int64_t meta_ptr);
int64_t rask_metadata_accessed(int64_t meta_ptr);
int64_t rask_metadata_modified(int64_t meta_ptr);

// ─── Args parsing ───────────────────────────────────────────
int64_t rask_args_parse(void);
int64_t rask_args_flag(int64_t args_ptr, int64_t long_ptr, int64_t short_ptr);
int64_t rask_args_option(int64_t args_ptr, int64_t long_ptr, int64_t short_ptr);
void    rask_args_option_or(RaskStr *out, int64_t args_ptr, int64_t long_ptr,
                            int64_t short_ptr, int64_t default_ptr);
int64_t rask_args_positional(int64_t args_ptr);
int64_t rask_args_program(int64_t args_ptr);

// Response reading (reads until EOF for Connection: close pattern).
void    rask_io_read_until_close(RaskStr *out, int64_t fd, int64_t max_len);

// ─── JSON module ────────────────────────────────────────────
// Encode helpers — used by codegen-generated struct serialization.

typedef struct RaskJsonBuf RaskJsonBuf;

RaskJsonBuf *rask_json_buf_new(void);
void         rask_json_buf_add_string(RaskJsonBuf *buf, const RaskStr *key, const RaskStr *val);
void         rask_json_buf_add_i64(RaskJsonBuf *buf, const RaskStr *key, int64_t val);
void         rask_json_buf_add_f64(RaskJsonBuf *buf, const RaskStr *key, double val);
void         rask_json_buf_add_bool(RaskJsonBuf *buf, const RaskStr *key, int64_t val);
void         rask_json_buf_add_raw(RaskJsonBuf *buf, const RaskStr *key, const RaskStr *raw_json);
void         rask_json_buf_finish(RaskStr *out, RaskJsonBuf *buf);

void         rask_json_encode(RaskStr *out, int64_t value_ptr);
void         rask_json_encode_string(RaskStr *out, const RaskStr *s);
void         rask_json_encode_i64(RaskStr *out, int64_t val);

// JSON array buffer — keyless element encoding for Vec serialization.
RaskJsonBuf *rask_json_buf_new_array(void);
void         rask_json_buf_array_add_raw(RaskJsonBuf *buf, const RaskStr *raw_json);
void         rask_json_buf_array_add_string(RaskJsonBuf *buf, const RaskStr *val);
void         rask_json_buf_array_add_i64(RaskJsonBuf *buf, int64_t val);
void         rask_json_buf_array_add_f64(RaskJsonBuf *buf, double val);
void         rask_json_buf_array_add_bool(RaskJsonBuf *buf, int64_t val);
void         rask_json_buf_finish_array(RaskStr *out, RaskJsonBuf *buf);

// Decode helpers — minimal JSON object parser.
typedef struct RaskJsonObj RaskJsonObj;

RaskJsonObj *rask_json_parse(const RaskStr *s);
void         rask_json_get_string(RaskStr *out, RaskJsonObj *obj, const char *key);
int64_t      rask_json_get_i64(RaskJsonObj *obj, const char *key);
double       rask_json_get_f64(RaskJsonObj *obj, const char *key);
int8_t       rask_json_get_bool(RaskJsonObj *obj, const char *key);
int64_t      rask_json_decode(const RaskStr *s);

// ─── CLI args ───────────────────────────────────────────────

void        rask_args_init(int argc, char **argv);
int64_t     rask_args_count(void);
const char *rask_args_get(int64_t index);

// ─── Panic ─────────────────────────────────────────────────
// Structured panic: aborts in main thread, catchable in spawned tasks.
// Spawned tasks use setjmp/longjmp to convert panics into JoinError.

#define RASK_PANIC_MSG_MAX 512

_Noreturn void rask_panic(const char *msg);
_Noreturn void rask_panic_at(const char *file, int32_t line, int32_t col,
                             const char *msg);
_Noreturn void rask_panic_fmt(const char *fmt, ...);

// Thread-local panic location — codegen sets before panicking calls
void rask_set_panic_location(const char *file, int32_t line, int32_t col);

// Location-aware panic wrappers for codegen
void rask_panic_unwrap(void);
void rask_panic_unwrap_at(const char *file, int32_t line, int32_t col);
void rask_assert_fail(void);
void rask_assert_fail_at(const char *file, int32_t line, int32_t col);
void rask_assert_fail_msg(const char *msg);
void rask_assert_fail_msg_at(const char *msg, const char *file,
                             int32_t line, int32_t col);
void rask_assert_fail_cmp_i64(int64_t left, int64_t right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);
void rask_assert_fail_cmp_str(const char *left, const char *right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);

// Install/remove panic handler for the current thread.
// Used internally by rask_spawn — not part of the public API.
typedef struct RaskPanicCtx RaskPanicCtx;
RaskPanicCtx *rask_panic_install(void);
void          rask_panic_remove(void);

// ─── Green scheduler (M:N) ──────────────────────────────────
// Work-stealing scheduler with io_uring/epoll I/O engine.
// Tasks are stackless state machines: poll_fn(state, ctx) → 0=READY, 1=PENDING.

void      rask_runtime_init(int64_t worker_count);
void      rask_runtime_shutdown(void);

// Spawn a green task. poll_fn signature: int (*)(void *state, void *task_ctx).
// state is heap-allocated, freed by scheduler on completion.
void     *rask_green_spawn(void *poll_fn, void *state, int64_t state_size);
int64_t   rask_green_join(void *handle);
void      rask_green_detach(void *handle);
int64_t   rask_green_cancel(void *handle);

// Closure-based spawn (bridge for codegen before state machine transform).
void     *rask_green_closure_spawn(void *closure_ptr);

// Yield helpers — called by state machines to pause on I/O.
void      rask_yield_read(int fd, void *buf, size_t len);
void      rask_yield_write(int fd, const void *buf, size_t len);
void      rask_yield_accept(int listen_fd);
void      rask_yield_timeout(uint64_t ns);

// Cooperative yield — re-enqueue current task for later polling.
void      rask_yield(void);

// Check cancel flag for the current green task.
int       rask_green_task_is_cancelled(void);

// ─── Threads ───────────────────────────────────────────────
// Phase A concurrency: one OS thread per spawn (conc.strategy/A1).
// TaskHandle is affine — must be joined, detached, or cancelled.

typedef struct RaskTaskHandle RaskTaskHandle;

// Function signature for spawned tasks: takes environment pointer.
typedef void (*RaskTaskFn)(void *env);

// Spawn a new OS thread running func(env). Caller must join/detach/cancel.
RaskTaskHandle *rask_task_spawn(RaskTaskFn func, void *env);

// Block until task finishes. Returns 0 on success, -1 on panic.
// On panic, if msg_out is non-NULL, receives a heap-allocated panic message
// (caller must free). Consumes the handle.
int64_t rask_task_join(RaskTaskHandle *h, char **msg_out);

// Detach the task (fire-and-forget). Consumes the handle.
void rask_task_detach(RaskTaskHandle *h);

// Request cooperative cancellation, then wait for the task to finish.
// Returns 0 on success, -1 on panic. Consumes the handle.
int64_t rask_task_cancel(RaskTaskHandle *h, char **msg_out);

// Check if the current task has been cancelled. Returns 1 if cancelled.
int8_t rask_task_cancelled(void);

// Sleep the current thread for the given number of nanoseconds.
int64_t rask_sleep_ns(int64_t ns);

// Codegen wrapper: spawn a task from a closure pointer [func_ptr | captures...].
// Extracts func/env, runs the task, and frees the closure allocation on completion.
RaskTaskHandle *rask_closure_spawn(void *closure_ptr);

// Simplified join: no panic message output. Returns 0 on success, -1 on panic.
int64_t rask_task_join_simple(void *h);

// ─── Channels ──────────────────────────────────────────────
// Bounded ring buffer (capacity > 0) or rendezvous (capacity == 0).
// Reference-counted sender/receiver halves. Close-on-drop.

typedef struct RaskChannel RaskChannel;
typedef struct RaskSender  RaskSender;
typedef struct RaskRecver  RaskRecver;

// Status codes for channel operations.
#define RASK_CHAN_OK     0
#define RASK_CHAN_CLOSED -1
#define RASK_CHAN_FULL   -2
#define RASK_CHAN_EMPTY  -3

// Create a channel. capacity=0 for rendezvous (unbuffered).
// Returns sender and receiver through out-params.
void rask_channel_new(int64_t elem_size, int64_t capacity,
                      RaskSender **tx_out, RaskRecver **rx_out);

// Blocking send. Copies elem_size bytes from data into the channel.
// Returns RASK_CHAN_OK or RASK_CHAN_CLOSED.
int64_t rask_channel_send(RaskSender *tx, const void *data);

// Blocking receive. Copies elem_size bytes from channel into data_out.
// Returns RASK_CHAN_OK or RASK_CHAN_CLOSED.
int64_t rask_channel_recv(RaskRecver *rx, void *data_out);

// Non-blocking variants.
int64_t rask_channel_try_send(RaskSender *tx, const void *data);
int64_t rask_channel_try_recv(RaskRecver *rx, void *data_out);

// Clone a sender (increment refcount). Multiple producers supported.
RaskSender *rask_sender_clone(RaskSender *tx);

// Drop sender/receiver. Closes the channel half when refcount hits zero.
void rask_sender_drop(RaskSender *tx);
void rask_recver_drop(RaskRecver *rx);

// i64-based channel wrappers for codegen dispatch table.
int64_t rask_channel_new_i64(int64_t capacity);
int64_t rask_channel_get_tx(int64_t pair);
int64_t rask_channel_get_rx(int64_t pair);
int64_t rask_channel_send_i64(int64_t tx, int64_t value);
int64_t rask_channel_recv_i64(int64_t rx);
void    rask_sender_drop_i64(int64_t tx);
void    rask_recver_drop_i64(int64_t rx);
int64_t rask_sender_clone_i64(int64_t tx);
int64_t rask_channel_try_send_i64(int64_t tx, int64_t value);
int64_t rask_channel_try_recv_i64(int64_t rx);

// ─── Async I/O (dual-path: green task or blocking) ──────────
// Inside a green task, these submit async ops and return PENDING.
// Outside a green task, they fall back to blocking syscalls.

int64_t rask_async_read(int fd, void *buf, int64_t len);
int64_t rask_async_write(int fd, const void *buf, int64_t len);
int64_t rask_async_accept(int listen_fd);

// ─── Async channels (yield-based) ──────────────────────────
// Non-blocking try + yield loop for green tasks.
// Outside green tasks, falls back to blocking channel ops.

int64_t rask_channel_send_async(int64_t tx, int64_t value);
int64_t rask_channel_recv_async(int64_t rx);

// ─── Green-aware sleep ──────────────────────────────────────
// Yields to scheduler in green tasks, blocking nanosleep otherwise.

void rask_green_sleep_ns(int64_t ns);

// ─── Ensure hooks (LIFO cleanup) ───────────────────────────
// Per-task cleanup stack. Hooks run LIFO on cancel or panic.

typedef void (*RaskEnsureFn)(void *ctx);

void rask_ensure_push(RaskEnsureFn fn, void *ctx);
void rask_ensure_pop(void);

// ─── Mutex ─────────────────────────────────────────────────
// Exclusive access wrapper. Closure-based: data accessed only inside lock.
// Wraps pthread_mutex (conc.sync/MX1-MX2).

typedef struct RaskMutex RaskMutex;

// Callback for lock/read/write: receives pointer to the protected data.
typedef void (*RaskAccessFn)(void *data, void *ctx);

RaskMutex *rask_mutex_new(const void *initial_data, int64_t data_size);
void       rask_mutex_free(RaskMutex *m);

// Acquire lock, call f(data, ctx), release lock.
void rask_mutex_lock(RaskMutex *m, RaskAccessFn f, void *ctx);

// Non-blocking. Returns 1 if lock acquired (and f was called), 0 otherwise.
int64_t rask_mutex_try_lock(RaskMutex *m, RaskAccessFn f, void *ctx);

// Pointer-based codegen wrappers for Mutex.
int64_t rask_mutex_new_ptr(int64_t data_ptr, int64_t data_size);
int64_t rask_mutex_lock_ptr(int64_t mutex, int64_t closure);
int64_t rask_mutex_try_lock_ptr(int64_t mutex, int64_t closure);
int64_t rask_mutex_clone(int64_t mutex);
void    rask_mutex_drop(int64_t mutex);

// ─── Shared (RwLock) ───────────────────────────────────────
// Multiple-reader / exclusive-writer wrapper (conc.sync/SY1, R1-R3).
// Wraps pthread_rwlock.

typedef struct RaskShared RaskShared;

RaskShared *rask_shared_new(const void *initial_data, int64_t data_size);
void        rask_shared_free(RaskShared *s);

// Shared read access — multiple concurrent readers allowed.
void rask_shared_read(RaskShared *s, RaskAccessFn f, void *ctx);

// Exclusive write access — blocks until all readers finish.
void rask_shared_write(RaskShared *s, RaskAccessFn f, void *ctx);

// Non-blocking variants. Return 1 if access granted, 0 otherwise.
int64_t rask_shared_try_read(RaskShared *s, RaskAccessFn f, void *ctx);
int64_t rask_shared_try_write(RaskShared *s, RaskAccessFn f, void *ctx);

// i64-based Shared wrappers for codegen dispatch table.
// Closure layout matches closures.rs: [func_ptr(8) | env...].
int64_t rask_shared_new_i64(int64_t value);
int64_t rask_shared_read_i64(int64_t shared, int64_t closure);
int64_t rask_shared_write_i64(int64_t shared, int64_t closure);
int64_t rask_shared_clone_i64(int64_t shared);
void    rask_shared_drop_i64(int64_t shared);

// Pointer-based wrappers for aggregate types (struct data).
int64_t rask_shared_new_ptr(int64_t data_ptr, int64_t data_size);
int64_t rask_shared_read_ptr(int64_t shared, int64_t closure);
int64_t rask_shared_write_ptr(int64_t shared, int64_t closure);

// Pointer-based channel wrappers for aggregate element types.
int64_t rask_channel_new_ptr(int64_t elem_size, int64_t capacity);
int64_t rask_channel_send_ptr(int64_t tx, int64_t data_ptr);
int64_t rask_channel_recv_ptr(int64_t rx, int64_t out_ptr);
int64_t rask_channel_send_async_ptr(int64_t tx, int64_t data_ptr);
int64_t rask_channel_recv_async_ptr(int64_t rx, int64_t out_ptr);

#endif // RASK_RUNTIME_H
