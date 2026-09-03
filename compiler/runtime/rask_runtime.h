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
void *rask_closure_alloc(int64_t block_size);
void  rask_closure_free(void *ptr);

// Overflow-checked arithmetic for allocation sizes.
_Noreturn void rask_panic(const char *msg);
_Noreturn void rask_panic_fmt(const char *fmt, ...);

// Debug null/validity checks. Active in debug builds (RASK_DEBUG defined)
// or when the RASK_RUNTIME_CHECKS=1 environment variable is set at startup.
// In release builds without the env var, these compile to nothing.
#ifdef RASK_DEBUG
#define RASK_CHECK_NONNULL(ptr, msg) \
    do { if (!(ptr)) rask_panic(msg); } while(0)
#else
#define RASK_CHECK_NONNULL(ptr, msg) \
    do { if (__builtin_expect(rask_runtime_checks_enabled, 0) && !(ptr)) rask_panic(msg); } while(0)
#endif
extern int rask_runtime_checks_enabled;

// Fill the stack below this frame with a nonzero pattern when
// RASK_POISON_STACK is set, so reads of never-written stack slots are
// deterministic instead of accidentally reading zeros.
void rask_poison_stack(void);

static inline int64_t rask_safe_mul(int64_t a, int64_t b) {
    if (a > 0 && b > 0 && a > INT64_MAX / b) rask_panic("allocation size overflow");
    return a * b;
}

static inline int64_t rask_safe_add(int64_t a, int64_t b) {
    if (a > 0 && b > 0 && a > INT64_MAX - b) rask_panic("allocation size overflow");
    return a + b;
}

// ─── Resource tracking ─────────────────────────────────────
// Consumed-flag tracker for ensure consumption cancellation (C1/C2).
int64_t rask_resource_register(int64_t scope_depth);
void    rask_resource_consume(int64_t id);
int64_t rask_resource_is_consumed(int64_t id);
void    rask_resource_scope_check(int64_t scope_depth);

// ─── Vec ────────────────────────────────────────────────────
// Growable array storing elements as raw bytes.

typedef struct RaskVec RaskVec;

// Where the strings sit inside one element.
//
// A container is a byte store: it knows how big an element is and nothing
// else, so it can't tell a sixteen-byte string from a sixteen-byte struct. It
// is told once, at construction, by the only place that knows the element type
// — lowering, reading the checker. From then on the map travels with the
// value, through a return, into another function, across an inlining, so
// `free` needs no argument and no caller has to work the answer out again.
//
// `offsets` is NULL and `count` 0 when the elements own nothing. Built by
// codegen's `string_offsets_of` from the element tag lowering emitted.
typedef struct {
    const int32_t *offsets;
    int64_t        count;
} RaskElemStrs;

// Two maps the runtime needs constantly: a container of bare strings (one
// string, at offset zero) and one of (string, string) pairs — `split`,
// `lines`, `os.args`, `env_vars`, HTTP headers. The runtime builds those
// itself and hands them to the program, which is what frees them.
extern const int32_t rask_elem_strs_one[1];
extern const int32_t rask_elem_strs_pair[2];

RaskVec *rask_vec_new(int64_t elem_size, const int32_t *str_offs, int64_t n_str_offs);
RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap,
                                const int32_t *str_offs, int64_t n_str_offs);
RaskVec *rask_vec_from_static(const char *data, int64_t count, int64_t elem_size,
                              const int32_t *str_offs, int64_t n_str_offs);
// Releases every string the elements hold, then the vector itself.
void     rask_vec_free(RaskVec *v);
// Takes a reference to every string the elements hold. A container built by
// copying another's elements owns them only after this.
void     rask_vec_retain_all(RaskVec *v);
int64_t  rask_vec_len(const RaskVec *v);
int64_t  rask_vec_capacity(const RaskVec *v);
RaskVec *rask_vec_fixed(int64_t elem_size, int64_t n,
                        const int32_t *str_offs, int64_t n_str_offs);
int64_t  rask_vec_bound(const RaskVec *v);
int64_t  rask_vec_remaining(const RaskVec *v);
int64_t  rask_vec_is_bounded(const RaskVec *v);
int64_t  rask_vec_is_full(const RaskVec *v);
int64_t  rask_vec_try_push(RaskVec *v, const void *elem);
int64_t  rask_vec_push(RaskVec *v, const void *elem);
void    *rask_vec_get(const RaskVec *v, int64_t index);
// Element pointer lent straight out of the buffer, so a `mutate` callee writes
// the real element instead of a copy. Between borrow and release, anything that
// would move the buffer panics rather than leave the pointer dangling.
void    *rask_vec_borrow_elem(RaskVec *v, int64_t index);
void     rask_vec_release_elem(RaskVec *v);
void    *rask_vec_get_unchecked(const RaskVec *v, int64_t index);
void    *rask_vec_get_opt(const RaskVec *v, int64_t index);
void     rask_vec_set(RaskVec *v, int64_t index, const void *elem);
void    *rask_vec_pop(RaskVec *v);
int64_t  rask_vec_remove(RaskVec *v, int64_t index);
void     rask_vec_clear(RaskVec *v);
int64_t  rask_vec_reserve(RaskVec *v, int64_t additional);
int64_t  rask_vec_is_empty(const RaskVec *v);
int64_t  rask_vec_insert_at(RaskVec *v, int64_t index, const void *elem);
int64_t  rask_vec_remove_at(RaskVec *v, int64_t index, void *out);
RaskVec *rask_iter_skip(const RaskVec *src, int64_t n);
RaskVec *rask_vec_clone(const RaskVec *v);
RaskVec *rask_vec_take_all(RaskVec *v);
int64_t  rask_wide_sum(const RaskVec *v);
void     rask_vec_sort(RaskVec *v);
void     rask_vec_sort_f64(RaskVec *v);
int64_t  rask_f64_compare_total(double a, double b);
void     rask_vec_sort_by(RaskVec *v, int64_t comparator);
void     rask_vec_reverse(RaskVec *v);
void     rask_vec_swap(RaskVec *v, int64_t i, int64_t j);
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

// `RASK_STRING_DEBUG=1`: a released string buffer is poisoned and kept rather
// than returned to the allocator, so the next retain or release of it says so
// and aborts instead of corrupting whatever moved into those bytes. Leaks by
// design — a debugging mode, not a hardening one.
extern int  rask_string_debug_enabled;

// `RASK_LEAK_CHECK=1`: at the end of `main`, anything this program allocated
// and never gave back is reported and the process exits 97. Every
// `rask_alloc`, not just strings — a clean program ends at exactly zero.
extern int  rask_leak_check_enabled;
void        rask_leak_check(void);

// Read-only accessors
int64_t     rask_string_len(const RaskStr *s);
const char *rask_string_ptr(const RaskStr *s);
int64_t     rask_string_is_empty(const RaskStr *s);
int64_t     rask_string_eq(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_hash(const RaskStr *s);

// struct.targets/EX4: main returned its error branch — print and exit 1.
_Noreturn void rask_main_error_exit(const RaskStr *msg);

// Shortest round-tripping decimal for a double, never in exponent form.
// Matches the interpreter's float formatting. Buffers must be this big:
// a large magnitude spelled out needs every digit before the point.
#define RASK_F64_BUF_SIZE 350
void rask_fmt_double(char *buf, size_t n, double val);
void rask_fmt_float(char *buf, size_t n, float val);
void rask_f32_to_string(RaskStr *out, float val);
int64_t     rask_string_compare(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_lt(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_gt(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_le(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_ge(const RaskStr *a, const RaskStr *b);
int64_t     rask_string_byte_at(const RaskStr *s, int64_t pos);
int64_t     rask_string_char_at(const RaskStr *s, int64_t byte_offset);
int64_t     rask_string_index(const RaskStr *s, int64_t index);
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
void        rask_string_replace_limit(RaskStr *out, const RaskStr *s, const RaskStr *from, const RaskStr *to, int64_t limit);
int64_t     rask_string_str_is_ascii(const RaskStr *s);

// Text units (std.strings/U1-U5): bytes index, graphemes display.
int64_t     rask_string_width(const RaskStr *s);
RaskVec    *rask_string_graphemes(const RaskStr *s);
void        rask_string_truncate(RaskStr *out, const RaskStr *s, int64_t cols);
void        rask_string_normalized(RaskStr *out, const RaskStr *s);
void        rask_string_from_char(RaskStr *out, int64_t cp);

// Builder operations (out-param: mutates string via promote-to-heap)
void        rask_string_push_byte(RaskStr *out, const RaskStr *s, uint8_t byte);
void        rask_string_push_char(RaskStr *out, const RaskStr *s, int32_t codepoint);
void        rask_string_append(RaskStr *out, const RaskStr *s, const RaskStr *other);
void        rask_string_append_cstr(RaskStr *out, const RaskStr *s, const char *cstr);
void        rask_string_push_str(RaskStr *out, const RaskStr *s, const RaskStr *other);

// ─── StringBuilder ─────────────────────────────────────────
int64_t     rask_string_builder_new(void);
int64_t     rask_string_builder_with_capacity(int64_t cap);
void        rask_string_builder_append(int64_t handle, int64_t str_ptr);
void        rask_string_builder_append_char(int64_t handle, int64_t codepoint);
void        rask_string_builder_build(RaskStr *out, int64_t handle);
int64_t     rask_string_builder_len(int64_t handle);
int64_t     rask_string_builder_is_empty(int64_t handle);

// Vec-returning operations (elements are RaskStr, elem_size=16)
RaskVec    *rask_string_lines(const RaskStr *s);
RaskVec    *rask_string_split(const RaskStr *s, const RaskStr *sep);
RaskVec    *rask_string_split_whitespace(const RaskStr *s);
RaskVec    *rask_string_chars(const RaskStr *s);

// Conversion to string (out-param)
void        rask_i64_to_string(RaskStr *out, int64_t val);
void        rask_u64_to_string(RaskStr *out, uint64_t val);
void        rask_bool_to_string(RaskStr *out, int64_t val);
void        rask_f64_to_string(RaskStr *out, double val);
void        rask_char_to_string(RaskStr *out, int32_t codepoint);

// 128-bit operations Cranelift can't lower — see int128.c. Each returns
// 0 (ok), 1 (divide by zero) or 2 (overflow); the caller panics with the span.
// C11 has no 128-bit integer, so the width is a compiler extension.
__extension__ typedef __int128 RaskI128;
__extension__ typedef unsigned __int128 RaskU128;

int32_t     rask_i128_mul(RaskI128 a, RaskI128 b, RaskI128 *out);
int32_t     rask_u128_mul(RaskU128 a, RaskU128 b, RaskU128 *out);
int32_t     rask_i128_div(RaskI128 a, RaskI128 b, RaskI128 *out);
int32_t     rask_i128_rem(RaskI128 a, RaskI128 b, RaskI128 *out);
int32_t     rask_u128_div(RaskU128 a, RaskU128 b, RaskU128 *out);
int32_t     rask_u128_rem(RaskU128 a, RaskU128 b, RaskU128 *out);
void        rask_i128_to_string(RaskStr *out, RaskI128 val);
void        rask_u128_to_string(RaskStr *out, RaskU128 val);
void        rask_print_i128(RaskI128 val);
void        rask_print_u128(RaskU128 val);
void        rask_eprint_i128(RaskI128 val);
void        rask_eprint_u128(RaskU128 val);
RaskI128    rask_i128_abs(RaskI128 v);
void        rask_assert_fail_cmp_i128(RaskI128 left, RaskI128 right,
                                      const char *op, const char *file,
                                      int32_t line, int32_t col);
void        rask_assert_fail_cmp_u128(RaskU128 left, RaskU128 right,
                                      const char *op, const char *file,
                                      int32_t line, int32_t col);

// Format specs (std.fmt/S1). The spec is parsed at compile time; each piece
// arrives here separately — a base conversion, then padding.
void        rask_i64_to_base(RaskStr *out, int64_t val, int64_t base, int64_t upper);
void        rask_u64_to_base(RaskStr *out, uint64_t val, int64_t base, int64_t upper);
void        rask_f64_to_precision(RaskStr *out, double val, int64_t precision);
void        rask_f64_to_exp(RaskStr *out, double val);
void        rask_string_truncate_chars(RaskStr *out, const RaskStr *s, int64_t count);
void        rask_string_pad(RaskStr *out, const RaskStr *s, int64_t width, int64_t align, int32_t fill);
void        rask_string_debug(RaskStr *out, const RaskStr *s);
void        rask_char_debug(RaskStr *out, int32_t codepoint);

// ─── Path ────────────────────────────────────────────────────
// Filesystem path operations. Path is stored as a plain RaskStr.
// Option-returning methods return NULL (None) or pointer to
// thread-local RaskStr (Some). Codegen copies immediately.

// Constructors / conversions (out-param)

// Option-returning (NULL→None, &buf→Some)

// Bool-returning

// Vec<string>-returning

// Char predicates — operate on Unicode codepoints (i32).
int64_t rask_char_is_digit(int32_t c);
int64_t rask_char_is_ascii(int32_t c);
int64_t rask_char_is_alphabetic(int32_t c);
int64_t rask_char_is_numeric(int32_t c);
int64_t rask_char_is_alphanumeric(int32_t c);
int64_t rask_char_is_whitespace(int32_t c);
int64_t rask_char_is_control(int32_t c);
int64_t rask_char_is_ascii_alphabetic(int32_t c);
int64_t rask_char_is_ascii_digit(int32_t c);
int64_t rask_char_is_ascii_hexdigit(int32_t c);
int64_t rask_char_is_ascii_punctuation(int32_t c);
int64_t rask_char_to_ascii_lowercase(int32_t c);
int64_t rask_char_to_ascii_uppercase(int32_t c);
int64_t rask_char_is_uppercase(int32_t c);
int64_t rask_char_is_lowercase(int32_t c);
int64_t rask_char_to_int(int32_t c);
int64_t rask_char_to_uppercase(int32_t c);
int64_t rask_char_to_lowercase(int32_t c);
int64_t rask_char_len_utf8(int32_t c);
int64_t rask_char_eq(int32_t a, int32_t b);

// ─── Unicode case mapping (generated: unicode_case.c) ───────
//
// Case conversion used to be ASCII-only here, so `"aöb".to_uppercase()` came
// back `AöB` and Greek was left untouched entirely, while the interpreter — which
// uses Rust's std — answered `AÖB` and `αβγ` (#779). The tables are generated
// from that same source, so the two can't drift.

/// One scalar in, one scalar out.
typedef struct {
    uint32_t from;
    uint32_t to;
} RaskCaseSimple;

/// One scalar in, up to three out — `ß` uppercases to `SS`, `İ` lowercases to
/// `i` followed by a combining dot.
typedef struct {
    uint32_t from;
    uint8_t n;
    uint32_t to[3];
} RaskCaseMulti;

/// The most a single scalar can grow in bytes under either mapping.
extern const int RASK_CASE_MAX_GROWTH;

/// Map `cp`, writing up to three scalars into `out`. Returns the count, always at
/// least 1 — an unmapped scalar maps to itself.
int rask_case_map(uint32_t cp, int to_upper, uint32_t out[3]);

/// The single-scalar answer, for `char.to_uppercase()`/`to_lowercase()`.
uint32_t rask_case_map_one(uint32_t cp, int to_upper);

/// An inclusive scalar range in a character-class table.
typedef struct {
    uint32_t lo;
    uint32_t hi;
} RaskCharRange;

/// What `char.is_alphabetic()` and friends answer from. Generated from Rust's
/// own predicates, same as the case tables — `is_alphabetic` used to be "any
/// scalar above 127", so `'\u{20AC}'` and a combining accent were letters.
#define RASK_CLASS_ALPHABETIC 0
#define RASK_CLASS_NUMERIC    1
#define RASK_CLASS_LOWERCASE  2
#define RASK_CLASS_UPPERCASE  3
#define RASK_CLASS_CONTROL    4
int rask_char_class(uint32_t cp, int which);

/// Text units (std.strings/U1-U5). Generated into unicode_text.c from the same
/// crates the interpreter uses, so the backends cannot drift.
int      rask_scalar_width(uint32_t cp);
int      rask_grapheme_joins_left(uint32_t cp);
int      rask_grapheme_is_prepend(uint32_t cp);
uint8_t  rask_ccc(uint32_t cp);
int      rask_canonical_decompose(uint32_t cp, uint32_t *out, int cap);
uint32_t rask_canonical_compose(uint32_t a, uint32_t b);

// ─── Vec (string-dependent) ─────────────────────────────────
void     rask_vec_join(RaskStr *out, const RaskVec *src, const RaskStr *sep);
void     rask_vec_join_i64(RaskStr *out, const RaskVec *src, const RaskStr *sep);
int64_t  rask_vec_contains_str(const RaskVec *v, const RaskStr *needle);

// ─── Map ────────────────────────────────────────────────────
// Open-addressing hash map with linear probing.
// Keys and values stored as raw bytes. Uses FNV-1a hashing + memcmp by default.
// For string-keyed maps, supply custom hash/eq via rask_map_new_custom.

typedef struct RaskMap RaskMap;

typedef uint64_t (*RaskHashFn)(const void *key, int64_t key_size);
typedef int      (*RaskEqFn)(const void *a, const void *b, int64_t key_size);

// Value pointer lent straight out of the table, so a `mutate` callee writes the
// real value. Between borrow and release, anything that would move or free the
// value array panics rather than leave the pointer dangling.
void    *rask_map_borrow_elem(RaskMap *m, const void *key);
void     rask_map_release_elem(RaskMap *m);

// The two element maps are the keys' and the values'. See `RaskElemStrs`.
RaskMap *rask_map_new(int64_t key_size, int64_t val_size,
                      const int32_t *key_offs, int64_t n_key_offs,
                      const int32_t *val_offs, int64_t n_val_offs);
RaskMap *rask_map_new_string_keys(int64_t key_size, int64_t val_size,
                                  const int32_t *key_offs, int64_t n_key_offs,
                                  const int32_t *val_offs, int64_t n_val_offs);
RaskMap *rask_map_new_custom(int64_t key_size, int64_t val_size,
                             RaskHashFn hash, RaskEqFn eq);
// Releases every string the keys and values hold, then the map itself.
void     rask_map_free(RaskMap *m);
int64_t  rask_map_len(const RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);
// `Map.insert` answers `V?`: a pointer to the value this call displaced, or
// NULL if the key was fresh. Good until the next insert on this map.
void    *rask_map_insert_displaced(RaskMap *m, const void *key, const void *val);
void    *rask_map_get(const RaskMap *m, const void *key);
void    *rask_map_get_unwrap(const RaskMap *m, const void *key);
int64_t  rask_map_remove(RaskMap *m, const void *key);
void    *rask_map_take(RaskMap *m, const void *key);
int64_t  rask_map_contains(const RaskMap *m, const void *key);
int64_t  rask_map_is_empty(const RaskMap *m);
void     rask_map_clear(RaskMap *m);
RaskVec *rask_map_keys(const RaskMap *m);
RaskVec *rask_map_values(const RaskMap *m);
RaskMap *rask_map_clone(const RaskMap *m);
// mem.racks/RK3: drop every entry whose value is this link.
int64_t  rask_map_drop_value_ptr(RaskMap *m, const void *target);
// Rewrite every value in place through `f` (Rack.snapshot re-points links).
void     rask_map_map_values_ptr(RaskMap *m, void *(*f)(void *value, void *ctx), void *ctx);

// Built-in hash/eq functions
uint64_t rask_hash_bytes(const void *key, int64_t key_size);
uint64_t rask_int_hash(uint64_t lo, uint64_t hi, int64_t width);
int      rask_eq_bytes(const void *a, const void *b, int64_t key_size);
// Hashes a RaskStr by content — what string-keyed maps and string.hash() use.
uint64_t rask_hash_string_key(const void *key, int64_t key_size);
// Pins the per-process seed mixed into the above (see map.c) — a hook for a
// future sim runtime, unused today.
void     rask_map_set_seed(uint64_t seed);

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
int64_t     rask_pool_try_insert_packed_sized(RaskPool *p, const void *elem, int64_t elem_size);
void       *rask_pool_get_packed(const RaskPool *p, int64_t packed);
void       *rask_pool_get_checked(const RaskPool *p, int64_t packed,
                                  const char *file, int32_t line, int32_t col);
int64_t     rask_pool_remove_packed(RaskPool *p, int64_t packed);
int64_t     rask_pool_remove_out(RaskPool *p, int64_t packed, void *out);
int64_t     rask_pool_is_valid_packed(const RaskPool *p, int64_t packed);
RaskVec    *rask_pool_handles_packed(const RaskPool *p);
RaskVec    *rask_pool_values(const RaskPool *p);
RaskVec    *rask_pool_drain(RaskPool *p);

#define RASK_HANDLE_INVALID ((RaskHandle){0, UINT32_MAX, 0})

// Packed sentinel for Option<Handle<T>> niche optimization.
// All bits set (index=UINT32_MAX, gen=UINT32_MAX) — impossible for a real handle.
// Option<Handle<T>> uses this as None; any other i64 is Some(handle).
#define RASK_HANDLE_PACKED_NONE ((int64_t)-1)

// ─── Rack + Link (mem.racks) ────────────────────────────────
//
// A `Link<T>` is the node's address — no ticket, no generation check. `none` is
// a null pointer, which is what makes `Link<T>?` eight bytes and lets `delete`
// null an edge with one store.

typedef struct RaskRack RaskRack;

// `none` for a link is the null address — the one address that can never name
// a node. A pool handle is index+generation and uses all-ones instead; the two
// niches don't share a sentinel, they each pick what their own domain can't
// produce. Null buys two things here: a rack chunk arrives zeroed, so a node's
// links start out absent with nothing written, and the check is `if (!link)`.
#define RASK_LINK_NONE ((void *)0)

static inline int rask_link_is_none(const void *p) {
    return p == NULL;
}

RaskRack *rask_rack_new(void);
void      rask_rack_free(RaskRack *r);
int64_t   rask_rack_len(const RaskRack *r);
int64_t   rask_rack_is_empty(const RaskRack *r);
int64_t   rask_rack_contains(const RaskRack *r, const void *link);
// What a link-bearing field of the node type holds. Packed into the descriptor
// alongside the byte offset, two int32s per field.
#define RASK_RACK_FIELD_LINK 0
#define RASK_RACK_FIELD_VEC  1
#define RASK_RACK_FIELD_MAP  2

// The node type's shape arrives here rather than at `new`: `Rack.new()` has no
// argument to read `T` off. `fields` is `field_count` pairs of
// (kind, byte offset), which is what lets the fixup find a node's own edges —
// and what lets `snapshot` re-point them.
void     *rask_rack_insert(RaskRack *r, const void *value, int64_t elem_size,
                           int64_t field_count, const int32_t *fields);
void      rask_rack_delete(RaskRack *r, void *link);
void      rask_rack_clear(RaskRack *r);
RaskVec  *rask_rack_nodes(const RaskRack *r);
RaskRack *rask_rack_snapshot(const RaskRack *r);
void     *rask_rack_corresponding(const RaskRack *r, const void *link);
void      rask_rack_print_stats(void);

// Edge maintenance. `set` writes the slot and keeps the target's incoming list
// in step; `forget` drops the record without writing, for a holder that is
// going away while its target stays alive.
void      rask_link_set(void **slot, void *target);
// `payload.<field at offset> = target` for a node of some rack. The node's own
// link fields keep their edge record inline in the header, so this unlinks and
// re-splices in O(1) — no scan of the old target's incoming list.
void      rask_link_set_node(void *payload, int64_t offset, void *target);
void      rask_link_forget(void **slot);
// A link stored in a container. The record names the container, not a position:
// pushes, removals and rehashing all move entries around.
void      rask_link_register_element(RaskVec *v, void *target);
void      rask_link_register_entry(RaskMap *m, void *target);
// A container that arrived whole rather than entry by entry — `filter` builds a
// fresh vector whose entries no push ever recorded.
void      rask_link_register_vec(RaskVec *v);
void      rask_link_register_map(RaskMap *m);
// The edges a struct's own fields carry, against the storage it sits in. Same
// (kind, byte offset) pairs `rask_rack_insert` takes.
void      rask_link_register_struct(void *base, int64_t field_count, const int32_t *fields);

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
void     rask_random_shuffle(RaskRng *rng, RaskVec *v);
void    *rask_random_choice(RaskRng *rng, RaskVec *v);

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

int64_t     rask_file_is_null(int64_t file);
void        rask_file_close(int64_t file);
// ─── String-out-param calls ────────────────────────────────
// A call that hands a string back through an out-param says how it ended, and
// carries the reason when it failed. It used to return a bare 0/1, and codegen
// turned every 1 into `IoError.UnexpectedEof` — right for `read_line`, wrong
// for a file read, which reported "unexpected end of file" for a descriptor
// that was write-only (#682).
// "Bad file descriptor (os error 9)" — the exact shape Rust's std::io::Error
// prints, which is what the interpreter reports, so both backends say the same
// thing. Defined further down runtime.c; declared here because the string-out
// calls above it need it.
const char *rask_io_error_text(int32_t err);

#define RASK_STROUT_OK    0
#define RASK_STROUT_ERROR 1   // *err_out holds the message → IoError.Other(msg)
#define RASK_STROUT_EOF   2   // input ran out → IoError.UnexpectedEof

int64_t     rask_file_read_all(RaskStr *out, int64_t file, RaskStr *err_out);
int64_t     rask_file_read_bytes(int64_t file);
void        rask_file_write(int64_t file, const RaskStr *content);
void        rask_file_write_all(int64_t file, const RaskStr *content);
int64_t     rask_file_write_bytes(int64_t file, int64_t vec_ptr);
void        rask_file_write_line(int64_t file, const RaskStr *content);
RaskVec    *rask_file_lines(int64_t file);

// ─── IO module ──────────────────────────────────────────────
int64_t     rask_io_read_line(RaskStr *out, RaskStr *err_out);
int64_t     rask_io_write_string(int64_t fd, int64_t str_ptr);
int64_t     rask_io_std_write_text(int64_t which, int64_t str_ptr);
int64_t     rask_io_std_write_bytes(int64_t which, int64_t vec_ptr);
int64_t     rask_io_std_flush(int64_t which);
int64_t     rask_io_std_read_bytes(int64_t max);

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
void    rask_http_server_close(int64_t server_ptr);
int64_t rask_net_clone(int64_t fd);
int64_t rask_net_read_all(int64_t fd, int64_t out_ptr);
int64_t rask_net_write_all(int64_t fd, int64_t str_ptr);
int64_t rask_net_read_bytes(int64_t fd);
int64_t rask_net_write_bytes(int64_t fd, int64_t vec_ptr);
void    rask_net_remote_addr(RaskStr *out, int64_t fd);
void    rask_net_local_addr(RaskStr *out, int64_t fd);
int8_t  rask_net_is_invalid(int64_t handle);
int8_t  rask_net_is_unresolved(int64_t handle);

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

// ─── JSON value tree + typed decode (json.c) ────────────────
//
// `json.decode<T>(s)` lowers to: build a shape describing T, hand it and the
// input to rask_json_decode_into, read back the error kind. The shape is what
// stands in for reflection — codegen has no type info at runtime.

// Parsed value kinds.
#define RASK_JSON_NULL 0
#define RASK_JSON_BOOL 1
#define RASK_JSON_NUM  2
#define RASK_JSON_STR  3
#define RASK_JSON_ARR  4
#define RASK_JSON_OBJ  5

// Decode outcomes. Nonzero maps onto a JsonError variant at the call site.
#define RASK_JSON_OK          0
#define RASK_JSON_ERR_PARSE   1
#define RASK_JSON_ERR_TYPE    2
#define RASK_JSON_ERR_MISSING 3

// Shape kinds. The primitives come first and their order is the index into the
// singleton table, so keep them contiguous and keep PRIM_COUNT last of them.
#define RASK_JSHAPE_BOOL   0
#define RASK_JSHAPE_I8     1
#define RASK_JSHAPE_I16    2
#define RASK_JSHAPE_I32    3
#define RASK_JSHAPE_I64    4
#define RASK_JSHAPE_U8     5
#define RASK_JSHAPE_U16    6
#define RASK_JSHAPE_U32    7
#define RASK_JSHAPE_U64    8
#define RASK_JSHAPE_F32    9
#define RASK_JSHAPE_F64    10
#define RASK_JSHAPE_STRING 11
#define RASK_JSHAPE_PRIM_COUNT 12
#define RASK_JSHAPE_VEC    12
#define RASK_JSHAPE_MAP    13
#define RASK_JSHAPE_OPT    14
#define RASK_JSHAPE_STRUCT 15

// Shape field flags.
// The key may be absent; whatever the caller already wrote stands (@default).
#define RASK_JFIELD_OPTIONAL 1

// Mirrors rask_mono::abi — an Option is [tag:8][payload], tag 0 = Some.
#define RASK_OPTION_PAYLOAD_OFFSET 8

typedef struct RaskJsonVal RaskJsonVal;
typedef struct RaskJsonShape RaskJsonShape;

RaskJsonVal *rask_json_tree_parse(const RaskStr *s);
void         rask_json_tree_free(RaskJsonVal *v);

RaskJsonShape *rask_json_shape_prim(int64_t kind);
RaskJsonShape *rask_json_shape_struct(int64_t size);
RaskJsonShape *rask_json_shape_vec(RaskJsonShape *elem, int64_t elem_slot);
RaskJsonShape *rask_json_shape_map(RaskJsonShape *val, int64_t val_slot);
RaskJsonShape *rask_json_shape_opt(RaskJsonShape *inner, int64_t total_size);
void           rask_json_shape_field(RaskJsonShape *s, const RaskStr *key, int64_t offset,
                                     RaskJsonShape *fs, int64_t flags);
void           rask_json_shape_free(RaskJsonShape *s);

int64_t rask_json_decode_into(void *dst, RaskJsonShape *shape, const RaskStr *input);
void    rask_json_encode_shaped(RaskStr *out, const void *src, RaskJsonShape *shape);
void    rask_json_decode_zero(void *dst, int64_t size);
int64_t rask_json_error_kind(void);
void    rask_json_error_message(RaskStr *out);

// ─── CLI args ───────────────────────────────────────────────

void        rask_args_init(int argc, char **argv);
int64_t     rask_args_count(void);
const char *rask_args_get(int64_t index);

// Environment variables
const RaskStr *rask_os_env(const RaskStr *name);
void           rask_os_env_or(RaskStr *out, const RaskStr *name, const RaskStr *def);

// ─── Print locking ─────────────────────────────────────────
// Codegen brackets the writes for one print/println call with these, so the
// whole line lands before another thread's does. Recursive: nesting is fine.
void rask_print_lock(void);
void rask_print_unlock(void);
void rask_eprint_lock(void);
void rask_eprint_unlock(void);

// Release everything this thread still holds — the panic path calls it before
// longjmping past an unlock that will never run.
void rask_print_unlock_all(void);

// ─── Panic ─────────────────────────────────────────────────
// Structured panic: aborts in main thread, catchable in spawned tasks.
// Spawned tasks use setjmp/longjmp to convert panics into JoinError.

// Big enough to hold an assertion message whose operands are floats spelled out
// in full. A double near the top of its range is ~309 digits with no exponent
// form (RASK_F64_BUF_SIZE), and the comparison messages print each operand
// twice — so 512 truncated mid-number for large magnitudes. These are stack
// buffers on a path that is about to unwind, so the headroom is free.
#define RASK_PANIC_MSG_MAX 2048

_Noreturn void rask_panic(const char *msg);
_Noreturn void rask_panic_at(const char *file, int32_t line, int32_t col,
                             const char *msg);
_Noreturn void rask_panic_fmt(const char *fmt, ...);

// ctrl.panic/A1: an `extern "C"` function's body is bracketed with these, so a
// panic inside it aborts at the boundary instead of unwinding into the C
// caller's frames. Nesting is counted; a normal return unwinds one level.
void rask_ffi_boundary_enter(void);
void rask_ffi_boundary_exit(void);
int  rask_in_ffi_boundary(void);

// Thread-local panic location — codegen sets before panicking calls
void rask_set_panic_location(const char *file, int32_t line, int32_t col);

// Location-aware panic wrappers for codegen
void rask_panic_unwrap(int32_t was_error);

// Checked-arithmetic panics that name their operands (ctrl.panic/F3). `tail` is
// the static "<type> range [min, max]" / "<type> bit width (n)" half.
_Noreturn void rask_panic_overflow_binary(const char *file, int32_t line, int32_t col,
                                          const char *op, const char *tail,
                                          int64_t lhs, int64_t rhs, int32_t is_unsigned);
_Noreturn void rask_panic_overflow_neg(const char *file, int32_t line, int32_t col,
                                       const char *tail, int64_t operand);
_Noreturn void rask_panic_shift_amount(const char *file, int32_t line, int32_t col,
                                       const char *tail, int64_t amount);
// The 128-bit forms. Separate because printing a 128-bit value needs the digit
// walk in int128.c — snprintf has no conversion for one.
_Noreturn void rask_panic_overflow_binary_i128(const char *file, int32_t line, int32_t col,
                                               const char *op, const char *tail,
                                               RaskI128 lhs, RaskI128 rhs, int32_t is_unsigned);
_Noreturn void rask_panic_overflow_neg_i128(const char *file, int32_t line, int32_t col,
                                            const char *tail, RaskI128 operand);
void rask_panic_unwrap_at(const char *file, int32_t line, int32_t col, int32_t was_error);
void rask_assert_fail(void);
void rask_assert_fail_at(const char *file, int32_t line, int32_t col);
void rask_assert_fail_msg(const char *msg);
void rask_assert_fail_msg_at(const char *msg, const char *file,
                             int32_t line, int32_t col);
void rask_assert_fail_cmp_i64(int64_t left, int64_t right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);
void rask_assert_fail_cmp_char(int64_t left, int64_t right,
                               const char *op, const char *file,
                               int32_t line, int32_t col);
void rask_assert_fail_cmp_str(const RaskStr *left, const RaskStr *right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);
void rask_assert_fail_cmp_f64(double left, double right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);
void rask_assert_fail_cmp_f32(float left, float right,
                              const char *op, const char *file,
                              int32_t line, int32_t col);

// assert_eq failure reporting — got/expected wording (testing A4).
// Generated code does the comparison and calls the variant matching the
// operand type; the last one covers aggregates, which have no value diff.
void rask_assert_eq_fail_i64(int64_t got, int64_t expected,
                             const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail_bool(int64_t got, int64_t expected,
                              const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail_char(int64_t got, int64_t expected,
                              const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail_f64(double got, double expected,
                             const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail_f32(float got, float expected,
                             const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail_str(const RaskStr *got, const RaskStr *expected,
                             const char *file, int32_t line, int32_t col);
void rask_assert_eq_fail(const char *file, int32_t line, int32_t col);

// Install/remove panic handler for the current thread.
// Used internally by rask_spawn — not part of the public API.
typedef struct RaskPanicCtx RaskPanicCtx;
RaskPanicCtx *rask_panic_install(void);
void          rask_panic_remove(void);

// Task ids (ctrl.panic/F1) — used internally by thread.c/green.c to prefix
// panic output with "task N" while a runtime task is executing.
int64_t rask_next_task_id(void);
void    rask_panic_set_task_id(int64_t id);

// ─── Green scheduler (M:N) ──────────────────────────────────
// Work-stealing scheduler with io_uring/epoll I/O engine.
// Tasks are stackless state machines: poll_fn(state, ctx) → 0=READY, 1=PENDING.

void      rask_runtime_init(int64_t worker_count);
void      rask_runtime_shutdown(void);

// Spawn a green task. poll_fn signature: int (*)(void *state, void *task_ctx).
// state is heap-allocated, freed by scheduler on completion.
void     *rask_green_spawn(void *poll_fn, void *state, int64_t state_size);

// Block until the task finishes. Returns 0 on success, -1 on panic.
// On panic, if msg_out is non-NULL, receives a heap-allocated panic message
// (caller must free). Consumes the handle. Never re-panics in the joining
// context — the caller decides what to do with the error (ctrl.panic/O1).
int64_t   rask_green_join(void *handle, char **msg_out);
void      rask_green_detach(void *handle);

// Request cooperative cancellation, then wait for the task to finish.
// Returns 0 on success, -1 on panic. Consumes the handle.
int64_t   rask_green_cancel(void *handle, char **msg_out);

// Simplified join/cancel: no panic message output. Returns 0 on success, -1 on panic.
int64_t   rask_green_join_simple(void *handle);
int64_t   rask_green_cancel_simple(void *handle);

// Closure-based spawn (bridge for codegen before state machine transform).
void     *rask_green_closure_spawn(void *closure_ptr, int64_t result_owned);

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

// Function signature for spawned tasks: takes environment pointer, hands back
// the task's return value. A task body that returns nothing still matches this
// on every ABI Rask targets — the unused return register is simply garbage,
// and join() on a `ThreadHandle<void>` never looks at it.
typedef int64_t (*RaskTaskFn)(void *env);

// Spawn a new OS thread running func(env). Caller must join/detach/cancel.
RaskTaskHandle *rask_task_spawn(RaskTaskFn func, void *env);

// Block until task finishes. Returns 0 on success, -1 on panic.
// On panic, if msg_out is non-NULL, receives a heap-allocated panic message
// (caller must free). Consumes the handle.
int64_t rask_task_join(RaskTaskHandle *h, char **msg_out);

// Detach the task (fire-and-forget). Consumes the handle.
void rask_task_detach(RaskTaskHandle *h);

// ctrl.panic/O4: block until every detached task has finished reporting, so a
// detached panic can't be lost to process exit. Called from `main`.
void rask_await_detached_tasks(void);

// Request cooperative cancellation, then wait for the task to finish.
// Returns 0 on success, -1 on panic. Consumes the handle.
int64_t rask_task_cancel(RaskTaskHandle *h, char **msg_out);

// Check if the current task has been cancelled. Returns 1 if cancelled.
int8_t rask_task_cancelled(void);

// Sleep the current thread for the given number of nanoseconds.
int64_t rask_sleep_ns(int64_t ns);

// Codegen wrapper: spawn a task from a closure pointer [func_ptr | captures...].
// Extracts func/env, runs the task, and frees the closure allocation on completion.
RaskTaskHandle *rask_closure_spawn(void *closure_ptr, int64_t result_owned);

// ─── Worker pool (threadpool.c) ────────────────────────────
// `using ThreadPool(workers: n)` brackets its block with these. Workers are
// OS threads that run a job to completion (conc.io-context/IO2) — a plain
// pool, independent of the green scheduler that `using Multitasking` starts.

// Start n workers (n <= 0 means one per core). Idempotent.
void rask_threadpool_init(int64_t worker_count);

// Drain the queue, stop the workers, join them. Idempotent.
void rask_threadpool_shutdown(void);

// ThreadPool.spawn — enqueues a job and hands back the same handle shape
// Thread.spawn gives, so join/detach/cancel are unchanged. Outside a
// `using ThreadPool` block there is no pool, so it falls back to one thread.
RaskTaskHandle *rask_threadpool_spawn(void *closure_ptr, int64_t result_owned);
struct RaskTaskState;
void rask_task_state_set_result_owned(struct RaskTaskState *state, int64_t owned);

// Simplified join: no panic message output. Returns 0 on success, -1 on panic.
int64_t rask_task_join_simple(void *h);

// ─── Join outcome (T or JoinError) ─────────────────────────
// How a joined task ended. Codegen turns this into the Result tag and, for the
// two failure cases, the JoinError variant tag — so the numbering here is the
// only thing the two sides have to agree on besides the offsets.
#define RASK_JOIN_OK        0
#define RASK_JOIN_PANICKED  1
#define RASK_JOIN_CANCELLED 2

// Join and report the outcome separately from the value, so a task that
// legitimately returns -1 isn't mistaken for a panic. `*value_out` gets the
// task's return value (0 when it failed); `*msg_out` is always left a valid
// string — the panic message, or empty. Consumes the handle.
int64_t rask_task_join_outcome(void *h, int64_t *value_out, RaskStr *msg_out);

// Same for the green scheduler's task handles.
int64_t rask_green_join_outcome(void *h, int64_t *value_out, RaskStr *msg_out);

// Cancel-then-join. Reports CANCELLED unless the task panicked on its way out.
int64_t rask_green_cancel_outcome(void *h, int64_t *value_out, RaskStr *msg_out);

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
int64_t rask_channel_try_recv_into(int64_t rx, int64_t out_ptr);
int64_t rask_sender_close_i64(int64_t tx);
int64_t rask_recver_close_i64(int64_t rx);

// Round-robin starting offset for a native `select` with num_arms arms
// (conc.select/P1) — see rask-mir's lower_select.
int64_t rask_select_rotate(int64_t num_arms);

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

// Drain the stack LIFO during panic unwind (ctrl.panic/U1, E2, E3).
void rask_ensure_run_all(void);

// Park/resume the current thread's stack head (opaque; for fiber workers).
void *rask_ensure_stack_take(void);
void  rask_ensure_stack_set(void *head);

// ─── Held access (ctrl.panic/U3, U4) ───────────────────────
// A `with` block over a sync box, and the inline `m.lock().f` form, emit an
// acquire and a release around the access. Only the release is inline, so a
// panic in between jumped straight past it and left the lock held for the rest
// of the process — the next acquirer blocked forever, including an ensure body
// running during that very unwind. Each acquire registers its release here, the
// matching release deregisters it, and the panic path drains what's left before
// running any ensure.

typedef void (*RaskReleaseFn)(int64_t handle);

void rask_access_push(RaskReleaseFn fn, int64_t handle);
void rask_access_pop(int64_t handle);
void rask_access_release_all(void);

// Park/resume the current thread's held-access stack (opaque; fiber workers).
void *rask_access_stack_take(void);
void  rask_access_stack_set(void *head);

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
int64_t rask_mutex_acquire(int64_t mutex);
void    rask_mutex_release(int64_t mutex);
int64_t rask_mutex_data(int64_t mutex);
int64_t rask_mutex_try_lock_ptr(int64_t mutex, int64_t closure);
int64_t rask_shared_read_acquire(int64_t shared);
int64_t rask_shared_write_acquire(int64_t shared);
int64_t rask_shared_data(int64_t shared);
void    rask_shared_release(int64_t shared);

// Staged access (conc.sync/ST1–ST4). `acquire` locks and hands back a working
// copy; `commit` puts it back as one move and unlocks; `discard` drops it and
// unlocks. Codegen schedules the commit as the block's inline cleanup, and the
// acquire registers the discard on the unwind stack — so a panic runs one and an
// ordinary exit the other, without either path knowing about the other.
int64_t rask_mutex_staged_acquire(int64_t mutex);
int64_t rask_mutex_staged_data(int64_t mutex);
void    rask_mutex_staged_commit(int64_t mutex);
void    rask_mutex_staged_discard(int64_t mutex);
int64_t rask_shared_staged_acquire(int64_t shared);
int64_t rask_shared_staged_data(int64_t shared);
void    rask_shared_staged_commit(int64_t shared);
void    rask_shared_staged_discard(int64_t shared);
int64_t rask_shared_staged_ptr(int64_t shared, int64_t closure);
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

// Rask closure layout (see closures.rs): [func_ptr(8) | env...].
// The call takes the env pointer as its first argument.
#define CLOSURE_FUNC(cl)  (*(int64_t *)(intptr_t)(cl))
#define CLOSURE_ENV(cl)   ((cl) + 8)

// i64-based Shared wrappers for codegen dispatch table.
int64_t rask_shared_new_i64(int64_t value);
int64_t rask_shared_read_i64(int64_t shared, int64_t closure);
int64_t rask_shared_write_i64(int64_t shared, int64_t closure);
int64_t rask_shared_clone_i64(int64_t shared);
void    rask_shared_drop_i64(int64_t shared);

// Pointer-based wrappers for aggregate types (struct data).
int64_t rask_shared_new_ptr(int64_t data_ptr, int64_t data_size);

// Cell — single-owner interior mutability (mem.cell). No lock.
int64_t rask_os_pid(void);
_Noreturn void rask_os_exit(int64_t code);
void    rask_os_set_env(const RaskStr *name, const RaskStr *value);
void    rask_os_remove_env(const RaskStr *name);
RaskVec *rask_os_args(void);
void    rask_os_platform(RaskStr *out);
void    rask_os_arch(RaskStr *out);
RaskVec *rask_os_env_vars(void);

int64_t rask_cell_new(int64_t data_ptr, int64_t data_size);
int64_t rask_cell_get(int64_t cell);
void    rask_cell_set(int64_t cell, int64_t data_ptr);
int64_t rask_cell_replace(int64_t cell, int64_t data_ptr);
void    rask_cell_free(int64_t cell);
int64_t rask_shared_read_ptr(int64_t shared, int64_t closure);
int64_t rask_shared_write_ptr(int64_t shared, int64_t closure);

// `get`/`set`/`replace` under each lock — the single-expression shorthand
// (CE6) that `Local` gets for free. See sync.c for why they exist per strategy.
int64_t rask_shared_get(int64_t shared);
void    rask_shared_set(int64_t shared, int64_t data_ptr);
int64_t rask_shared_replace(int64_t shared, int64_t data_ptr);
int64_t rask_mutex_get(int64_t mutex);
void    rask_mutex_set(int64_t mutex, int64_t data_ptr);
int64_t rask_mutex_replace(int64_t mutex, int64_t data_ptr);
int64_t rask_shared_try_read_ptr(int64_t shared, int64_t closure);
int64_t rask_shared_try_write_ptr(int64_t shared, int64_t closure);

// Pointer-based channel wrappers for aggregate element types.
int64_t rask_channel_new_ptr(int64_t elem_size, int64_t capacity);
int64_t rask_channel_send_ptr(int64_t tx, int64_t data_ptr);
int64_t rask_channel_recv_ptr(int64_t rx, int64_t out_ptr);
int64_t rask_channel_send_async_ptr(int64_t tx, int64_t data_ptr);
int64_t rask_channel_recv_async_ptr(int64_t rx, int64_t out_ptr);

// ── Error origin (ER15/ER16) ────────────────────────────────────
// Set the source file name for error origin formatting.
void rask_set_origin_file(const char *file);
// Read origin from a Result and format as "file.rk:42" string.
void rask_result_origin(RaskStr *out, const void *result_ptr);

#endif // RASK_RUNTIME_H
