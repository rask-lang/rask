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
        v->data = (char *)rask_alloc(elem_size * cap);
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
    int64_t total = elem_size * count;
    v->data = (char *)rask_alloc(total);
    memcpy(v->data, data, total);
    return v;
}

void rask_vec_free(RaskVec *v) {
    if (!v) return;
    rask_free(v->data);
    rask_free(v);
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
        new_cap *= 2;
    }
    char *new_data = (char *)rask_realloc(v->data, v->cap * v->elem_size,
                                          new_cap * v->elem_size);
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
    if (!v || index < 0 || index >= v->len) return NULL;
    return v->data + index * v->elem_size;
}

void rask_vec_set(RaskVec *v, int64_t index, const void *elem) {
    if (!v || index < 0 || index >= v->len) return;
    memcpy(v->data + index * v->elem_size, elem, (size_t)v->elem_size);
}

int64_t rask_vec_pop(RaskVec *v, void *out) {
    if (!v || v->len == 0) return -1;
    v->len--;
    if (out) {
        memcpy(out, v->data + v->len * v->elem_size, (size_t)v->elem_size);
    }
    return 0;
}

int64_t rask_vec_remove(RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) return -1;
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
    if (!v || index < 0 || index > v->len) return -1;
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
    if (!v || index < 0 || index >= v->len) return -1;
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
