// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Vec — growable array storing elements as raw bytes.
// Growth factor: 2x. Initial allocation deferred until first push.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>

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

RaskVec *rask_vec_from_static(const char *data, int64_t count) {
    int64_t elem_size = 8; // all comptime values are i64
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

void rask_vec_set(RaskVec *v, int64_t index, const void *elem) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index %lld, len %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    memcpy(v->data + index * v->elem_size, elem, (size_t)v->elem_size);
}

int64_t rask_vec_pop(RaskVec *v, void *out) {
    if (!v || v->len == 0) {
        rask_panic("pop from empty Vec");
    }
    v->len--;
    if (out) {
        memcpy(out, v->data + v->len * v->elem_size, (size_t)v->elem_size);
    }
    return 0;
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

// join(vec_of_strings, separator) — concatenate strings with separator.
RaskString *rask_vec_join(const RaskVec *src, const RaskString *sep) {
    RaskString *result = rask_string_new();
    if (!src || src->len == 0) return result;
    for (int64_t i = 0; i < src->len; i++) {
        if (i > 0 && sep) {
            rask_string_append(result, sep);
        }
        RaskString *elem = *(RaskString **)(src->data + i * src->elem_size);
        if (elem) rask_string_append(result, elem);
    }
    return result;
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
