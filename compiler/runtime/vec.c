// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Vec — growable array storing elements as raw bytes.
// Growth factor: 2x. Initial allocation deferred until first push.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

struct RaskVec {
    char   *data;
    int64_t len;
    int64_t cap;
    int64_t elem_size;
};

RaskVec *rask_vec_new(int64_t elem_size) {
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->data = NULL;
    v->len = 0;
    v->cap = 0;
    v->elem_size = elem_size;
    return v;
}

RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap) {
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->len = 0;
    v->elem_size = elem_size;
    if (cap > 0) {
        v->data = (char *)rask_alloc(rask_safe_mul(elem_size, cap));
        v->cap = cap;
    } else {
        v->data = NULL;
        v->cap = 0;
    }
    return v;
}

// elem_size comes from the caller: a static array of fat pointers (trait
// objects, slices) has 16-byte elements, not 8.
RaskVec *rask_vec_from_static(const char *data, int64_t count, int64_t elem_size) {
    if (elem_size <= 0) elem_size = 8;
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->len = count;
    v->cap = count;
    v->elem_size = elem_size;
    int64_t total = rask_safe_mul(elem_size, count);
    v->data = (char *)rask_alloc(total);
    memcpy(v->data, data, total);
    return v;
}

void rask_vec_free(RaskVec *v) {
    if (!v) return;
    if (v->data) rask_realloc(v->data, rask_safe_mul(v->cap, v->elem_size), 0);
    rask_realloc(v, (int64_t)sizeof(RaskVec), 0);
}

int64_t rask_vec_len(const RaskVec *v) {
    return v ? v->len : 0;
}

int64_t rask_vec_capacity(const RaskVec *v) {
    return v ? v->cap : 0;
}

static int vec_grow(RaskVec *v, int64_t needed) {
    if (needed <= v->cap) return 0;
    int64_t new_cap = v->cap ? v->cap : 4;
    while (new_cap < needed) {
        if (new_cap > INT64_MAX / 2) rask_panic("Vec capacity overflow");
        new_cap *= 2;
    }
    char *new_data = (char *)rask_realloc(v->data, rask_safe_mul(v->cap, v->elem_size),
                                          rask_safe_mul(new_cap, v->elem_size));
    v->data = new_data;
    v->cap = new_cap;
    return 0;
}

int64_t rask_vec_push(RaskVec *v, const void *elem) {
    if (!v) return -1;
    if (vec_grow(v, v->len + 1) != 0) return -1;
    memcpy(v->data + v->len * v->elem_size, elem, (size_t)v->elem_size);
    v->len++;
    return 0;
}

void *rask_vec_get(const RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    return v->data + index * v->elem_size;
}

// Unchecked variant — bounds already proven by the compiler.
void *rask_vec_get_unchecked(const RaskVec *v, int64_t index) {
    return v->data + index * v->elem_size;
}

// Safe get (V3): NULL on OOB (Option None), else pointer to element. No panic.
// Indexing (`v[i]`) uses rask_vec_get instead, which panics on OOB.
void *rask_vec_get_opt(const RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) {
        return NULL;
    }
    return v->data + index * v->elem_size;
}

void rask_vec_set(RaskVec *v, int64_t index, const void *elem) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    memcpy(v->data + index * v->elem_size, elem, (size_t)v->elem_size);
}

// Pop returns NULL when empty (Option encoding via DerefOption codegen
// adapter — same convention as rask_map_get). On success, returns a
// pointer into the buffer for the just-vacated slot; the codegen reads
// out the bytes into the destination Option payload before any
// subsequent vec mutation could clobber it.
void *rask_vec_pop(RaskVec *v) {
    if (!v || v->len == 0) {
        return NULL;
    }
    v->len--;
    return v->data + v->len * v->elem_size;
}

int64_t rask_vec_remove(RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    // Shift elements left
    int64_t remaining = v->len - index - 1;
    if (remaining > 0) {
        memmove(v->data + index * v->elem_size,
                v->data + (index + 1) * v->elem_size,
                (size_t)(remaining * v->elem_size));
    }
    v->len--;
    return 0;
}

void rask_vec_clear(RaskVec *v) {
    if (v) v->len = 0;
}

int64_t rask_vec_reserve(RaskVec *v, int64_t additional) {
    if (!v) return -1;
    return vec_grow(v, v->len + additional);
}

int64_t rask_vec_is_empty(const RaskVec *v) {
    return (!v || v->len == 0) ? 1 : 0;
}

int64_t rask_vec_insert_at(RaskVec *v, int64_t index, const void *elem) {
    if (!v || index < 0 || index > v->len) {
        rask_panic_fmt("insert index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    if (vec_grow(v, v->len + 1) != 0) return -1;
    // Shift elements right to make room
    int64_t to_move = v->len - index;
    if (to_move > 0) {
        memmove(v->data + (index + 1) * v->elem_size,
                v->data + index * v->elem_size,
                (size_t)(to_move * v->elem_size));
    }
    memcpy(v->data + index * v->elem_size, elem, (size_t)v->elem_size);
    v->len++;
    return 0;
}

int64_t rask_vec_remove_at(RaskVec *v, int64_t index, void *out) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    if (out) {
        memcpy(out, v->data + index * v->elem_size, (size_t)v->elem_size);
    }
    // Shift elements left
    int64_t remaining = v->len - index - 1;
    if (remaining > 0) {
        memmove(v->data + index * v->elem_size,
                v->data + (index + 1) * v->elem_size,
                (size_t)(remaining * v->elem_size));
    }
    v->len--;
    return 0;
}

// clone — deep copy of the Vec (copies element bytes, not deep-cloning elements).
RaskVec *rask_vec_clone(const RaskVec *src) {
    if (!src) return rask_vec_new(8);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, src->len);
    if (src->len > 0) {
        memcpy(dst->data, src->data, (size_t)(src->len * src->elem_size));
    }
    dst->len = src->len;
    return dst;
}

// `v.take_all()` hands the elements over and leaves `v` empty (I3). The copy
// is what makes the source safe to keep using — iteration reads the returned
// vec, and nothing points into the original's buffer any more.
RaskVec *rask_vec_take_all(RaskVec *v) {
    RaskVec *out = rask_vec_clone(v);
    if (v) rask_vec_clear(v);
    return out;
}

// rask_string_append is the builder primitive: when the accumulator is the sole
// owner of its buffer it appends in place and hands the SAME buffer back, so the
// result aliases the accumulator. Freeing the accumulator after appending
// therefore frees the buffer the result now owns — the join wrote into freed
// memory and produced garbage or tripped the allocator (#414/#461). The other
// paths (SSO promote, shared detach) either allocate fresh or drop the old
// reference themselves, so there is nothing for the caller to free either way.

// join(vec_of_strings, separator) — concatenate strings with separator.
void rask_vec_join(RaskStr *out, const RaskVec *src, const RaskStr *sep) {
    rask_string_new(out);
    if (!src || src->len == 0) return;
    for (int64_t i = 0; i < src->len; i++) {
        if (i > 0 && sep) {
            RaskStr tmp;
            rask_string_append(&tmp, out, sep);
            *out = tmp;
        }
        const RaskStr *elem = (const RaskStr *)(src->data + i * src->elem_size);
        RaskStr tmp;
        rask_string_append(&tmp, out, elem);
        *out = tmp;
    }
}

// join(vec_of_ints, separator) — convert integers to strings and concatenate.
void rask_vec_join_i64(RaskStr *out, const RaskVec *src, const RaskStr *sep) {
    rask_string_new(out);
    if (!src || src->len == 0) return;
    for (int64_t i = 0; i < src->len; i++) {
        if (i > 0 && sep) {
            RaskStr tmp;
            rask_string_append(&tmp, out, sep);
            *out = tmp;
        }
        int64_t val = *(int64_t *)(src->data + i * src->elem_size);
        RaskStr s;
        rask_i64_to_string(&s, val);
        RaskStr tmp;
        rask_string_append(&tmp, out, &s);
        rask_string_free(&s);
        *out = tmp;
    }
}

// slice(vec, start, end) — returns a new Vec with elements [start..end).
RaskVec *rask_vec_slice(const RaskVec *src, int64_t start, int64_t end) {
    if (!src) return rask_vec_new(8);
    if (start < 0) start = 0;
    if (end > src->len) end = src->len;
    int64_t new_len = end - start;
    if (new_len <= 0) return rask_vec_new(src->elem_size);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, new_len);
    memcpy(dst->data, src->data + start * src->elem_size,
           (size_t)(new_len * src->elem_size));
    dst->len = new_len;
    return dst;
}

// chunks(vec, chunk_size) — returns a Vec of Vec* pointers, each a sub-range view.
// Each chunk is a freshly allocated Vec with copied elements.
RaskVec *rask_vec_chunks(const RaskVec *src, int64_t chunk_size) {
    RaskVec *result = rask_vec_new(8); // Vec of pointers (8 bytes each)
    if (!src || chunk_size <= 0) return result;
    for (int64_t i = 0; i < src->len; i += chunk_size) {
        int64_t remaining = src->len - i;
        int64_t this_chunk = remaining < chunk_size ? remaining : chunk_size;
        RaskVec *chunk = rask_vec_with_capacity(src->elem_size, this_chunk);
        memcpy(chunk->data, src->data + i * src->elem_size,
               (size_t)(this_chunk * src->elem_size));
        chunk->len = this_chunk;
        int64_t chunk_ptr = (int64_t)(uintptr_t)chunk;
        rask_vec_push(result, &chunk_ptr);
    }
    return result;
}

// map(vec, fn_ptr) — apply fn to each element, returning new Vec.
// Stub: calls fn(elem) for each element, stores result.
RaskVec *rask_vec_map(const RaskVec *src, int64_t fn_ptr) {
    typedef int64_t (*MapFn)(int64_t);
    MapFn func = (MapFn)(uintptr_t)fn_ptr;
    if (!src) return rask_vec_new(8);
    RaskVec *dst = rask_vec_with_capacity(8, src->len);
    for (int64_t i = 0; i < src->len; i++) {
        int64_t elem = *(int64_t *)(src->data + i * src->elem_size);
        int64_t result = func(elem);
        rask_vec_push(dst, &result);
    }
    return dst;
}

// collect — identity (Vec is already materialized).
RaskVec *rask_vec_collect(const RaskVec *src) {
    return rask_vec_clone(src);
}

// wide_sum(plan) — reduce int64 lanes with +. A Wide<T> is a RaskVec* at
// runtime (conc.data-parallel). Integer lanes only; float lanes need the
// element-type-aware path that native map/zip still lack.
int64_t rask_wide_sum(const RaskVec *v) {
    if (!v) return 0;
    int64_t acc = 0;
    for (int64_t i = 0; i < v->len; i++) {
        acc += *(int64_t *)(v->data + i * v->elem_size);
    }
    return acc;
}

// filter(vec, fn_ptr) — keep elements where fn returns non-zero.
RaskVec *rask_vec_filter(const RaskVec *src, int64_t fn_ptr) {
    typedef int64_t (*FilterFn)(int64_t);
    FilterFn func = (FilterFn)(uintptr_t)fn_ptr;
    if (!src) return rask_vec_new(8);
    RaskVec *dst = rask_vec_new(src->elem_size);
    for (int64_t i = 0; i < src->len; i++) {
        int64_t elem = *(int64_t *)(src->data + i * src->elem_size);
        if (func(elem)) {
            rask_vec_push(dst, &elem);
        }
    }
    return dst;
}

// as_ptr(vec) — raw pointer to underlying buffer (unsafe).
int64_t rask_vec_as_ptr(const RaskVec *v) {
    return v ? (int64_t)(uintptr_t)v->data : 0;
}

// sort(vec) — in-place sort using default i64 comparison.
static int rask_i64_compare(const void *a, const void *b) {
    int64_t va = *(const int64_t *)a;
    int64_t vb = *(const int64_t *)b;
    if (va < vb) return -1;
    if (va > vb) return 1;
    return 0;
}

void rask_vec_sort(RaskVec *v) {
    if (!v || v->len <= 1) return;
    qsort(v->data, (size_t)v->len, (size_t)v->elem_size, rask_i64_compare);
}

// sort_by(vec, comparator) — in-place sort with a closure comparator.
//
// `comparator` is a Rask closure block, not a bare function pointer: the code
// address sits at offset 0 and the environment follows, and the call takes the
// env as its first argument (see closures.rs). Calling the block address
// directly jumped into the closure's own data.
//
// How the two elements are handed over follows codegen's own rule for
// aggregates: anything wider than a word is a pointer to its storage, a word or
// less is the value itself. That matches what the closure body compiles to —
// `|a, b| a.rank.compare(b.rank)` reads fields through a pointer, while
// `Vec<i64>` compares plain integers. Returns <0 / 0 / >0.
typedef int64_t (*RaskCmpFn)(int64_t env, int64_t a, int64_t b);

static __thread int64_t rask_sort_comparator;
static __thread int rask_sort_by_ptr;

static int rask_sort_by_adapter(const void *a, const void *b) {
    RaskCmpFn fn = (RaskCmpFn)(uintptr_t)CLOSURE_FUNC(rask_sort_comparator);
    int64_t env = CLOSURE_ENV(rask_sort_comparator);
    int64_t va, vb;
    if (rask_sort_by_ptr) {
        va = (int64_t)(uintptr_t)a;
        vb = (int64_t)(uintptr_t)b;
    } else {
        va = *(const int64_t *)a;
        vb = *(const int64_t *)b;
    }
    int64_t result = fn(env, va, vb);
    if (result < 0) return -1;
    if (result > 0) return 1;
    return 0;
}

void rask_vec_sort_by(RaskVec *v, int64_t comparator) {
    if (!v || v->len <= 1 || !comparator) return;
    rask_sort_comparator = comparator;
    rask_sort_by_ptr = v->elem_size > 8;
    qsort(v->data, (size_t)v->len, (size_t)v->elem_size, rask_sort_by_adapter);
}

// reverse(vec) — in-place reversal.
void rask_vec_reverse(RaskVec *v) {
    if (!v || v->len <= 1) return;
    char tmp[16]; // max elem_size we support for stack swap
    int64_t es = v->elem_size;
    char *lo = v->data;
    char *hi = v->data + (v->len - 1) * es;
    while (lo < hi) {
        memcpy(tmp, lo, (size_t)es);
        memcpy(lo, hi, (size_t)es);
        memcpy(hi, tmp, (size_t)es);
        lo += es;
        hi -= es;
    }
}

// swap(i, j) — exchange two elements in place. Out-of-range indices panic,
// same as indexing does; a silent no-op would hide the mistake.
void rask_vec_swap(RaskVec *v, int64_t i, int64_t j) {
    if (!v) return;
    if (i < 0 || i >= v->len || j < 0 || j >= v->len) {
        rask_panic("Vec.swap: index out of bounds");
        return;
    }
    if (i == j) return;
    int64_t es = v->elem_size;
    char *a = v->data + i * es;
    char *b = v->data + j * es;
    for (int64_t k = 0; k < es; k++) {
        char t = a[k];
        a[k] = b[k];
        b[k] = t;
    }
}

// contains(vec, value) — returns 1 if any element equals value.
int64_t rask_vec_contains(const RaskVec *v, const void *elem) {
    if (!v) return 0;
    for (int64_t i = 0; i < v->len; i++) {
        if (memcmp(v->data + i * v->elem_size, elem, (size_t)v->elem_size) == 0) {
            return 1;
        }
    }
    return 0;
}

// contains(vec, needle) — string elements. A heap RaskStr holds a pointer, so
// two equal strings differ byte-for-byte; memcmp can't be used here.
int64_t rask_vec_contains_str(const RaskVec *v, const RaskStr *needle) {
    if (!v || !needle) return 0;
    for (int64_t i = 0; i < v->len; i++) {
        const RaskStr *elem = (const RaskStr *)(v->data + i * v->elem_size);
        if (rask_string_eq(elem, needle)) return 1;
    }
    return 0;
}

// dedup(vec) — remove consecutive duplicates in-place.
void rask_vec_dedup(RaskVec *v) {
    if (!v || v->len <= 1) return;
    int64_t write = 1;
    for (int64_t read = 1; read < v->len; read++) {
        if (memcmp(v->data + read * v->elem_size,
                   v->data + (write - 1) * v->elem_size,
                   (size_t)v->elem_size) != 0) {
            if (write != read) {
                memcpy(v->data + write * v->elem_size,
                       v->data + read * v->elem_size,
                       (size_t)v->elem_size);
            }
            write++;
        }
    }
    v->len = write;
}

// first(vec) — pointer to the first element, or NULL when empty.
// `first()` is declared `-> Option<T>`, so empty is `none`, not a panic.
void *rask_vec_first(const RaskVec *v) {
    if (!v || v->len == 0) return NULL;
    return v->data;
}

// last(vec) — pointer to the last element, or NULL when empty.
void *rask_vec_last(const RaskVec *v) {
    if (!v || v->len == 0) return NULL;
    return v->data + (v->len - 1) * v->elem_size;
}

// skip(vec, n) — returns a new Vec with the first n elements removed.
RaskVec *rask_iter_skip(const RaskVec *src, int64_t n) {
    if (!src) return rask_vec_new(8);
    if (n < 0) n = 0;
    int64_t new_len = src->len - n;
    if (new_len <= 0) return rask_vec_new(src->elem_size);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, new_len);
    memcpy(dst->data, src->data + n * src->elem_size, (size_t)(new_len * src->elem_size));
    dst->len = new_len;
    return dst;
}

// Write Vec data to a FILE*. Used by self-hosted fs.write_bytes.
void rask_fwrite_vec(int64_t fptr, const RaskVec *v) {
    FILE *f = (FILE *)(uintptr_t)fptr;
    if (!f || !v || !v->data) return;
    fwrite(v->data, 1, (size_t)v->len, f);
}
