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
    // How many element pointers are currently lent out. A callee holding a
    // pointer into `data` is only safe while `data` stays put, so growing with
    // a borrow outstanding is refused instead of left to corrupt memory.
    int64_t borrows;
    // std.collections/CP1-CP3: the ceiling this vector may not grow past, or
    // -1 for unbounded. `cap` is the allocation, which grows on demand; this is
    // a promise about how large the vector is allowed to get. `Vec.fixed(0)` is
    // a legitimate bound of zero, so unbounded can't be spelled 0.
    int64_t bound;
    // Where the strings sit inside one element, handed over once at
    // construction. See `RaskElemStrs` in rask_runtime.h for why the container
    // carries this rather than every `free` site working it out.
    RaskElemStrs strs;
};

static void vec_check_no_borrows(const RaskVec *v, const char *op);

const int32_t rask_elem_strs_one[1] = {0};
const int32_t rask_elem_strs_pair[2] = {0, 16};

// Take a reference to every string in `count` elements starting at `from`.
//
// A vector derived from another — clone, slice, chunk, skip — copies element
// bytes. Two vectors then point at one string buffer, and whichever is freed
// second reads memory that is already gone. Copying the map is what makes the
// copy an owner, so it has to take the reference that goes with it.
static void vec_retain_elems(const RaskVec *v, int64_t from, int64_t count) {
    if (!v || !v->strs.offsets || v->strs.count <= 0 || !v->data) return;
    for (int64_t i = from; i < from + count; i++) {
        const char *elem = v->data + i * v->elem_size;
        for (int64_t k = 0; k < v->strs.count; k++) {
            rask_string_clone((const RaskStr *)(elem + v->strs.offsets[k]));
        }
    }
}

RaskVec *rask_vec_new(int64_t elem_size, const int32_t *str_offs, int64_t n_str_offs) {
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->data = NULL;
    v->len = 0;
    v->cap = 0;
    v->elem_size = elem_size;
    v->borrows = 0;
    v->bound = -1;
    v->strs.offsets = str_offs;
    v->strs.count = n_str_offs;
    return v;
}

RaskVec *rask_vec_with_capacity(int64_t elem_size, int64_t cap,
                                const int32_t *str_offs, int64_t n_str_offs) {
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->len = 0;
    v->elem_size = elem_size;
    v->borrows = 0;
    // CP1: a capacity hint pre-allocates but sets no ceiling.
    v->bound = -1;
    v->strs.offsets = str_offs;
    v->strs.count = n_str_offs;
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
RaskVec *rask_vec_from_static(const char *data, int64_t count, int64_t elem_size,
                              const int32_t *str_offs, int64_t n_str_offs) {
    if (elem_size <= 0) elem_size = 8;
    RaskVec *v = (RaskVec *)rask_alloc(sizeof(RaskVec));
    v->len = count;
    v->cap = count;
    v->elem_size = elem_size;
    v->borrows = 0;
    v->bound = -1;
    v->strs.offsets = str_offs;
    v->strs.count = n_str_offs;
    int64_t total = rask_safe_mul(elem_size, count);
    v->data = (char *)rask_alloc(total);
    memcpy(v->data, data, total);
    // The elements are copied in, so this vector is a second owner of whatever
    // they hold. A literal's sentinel refcount makes that free; a `["{a}",
    // "{b}"]` built at runtime is the case that needs it, since the locals that
    // made those strings release their own reference on the way out.
    vec_retain_elems(v, 0, v->len);
    return v;
}

// Releases every string the elements hold, then the vector itself.
//
// The map came from the constructor: `Vec<string>` is the one-entry case at
// offset zero, a `Vec<Holder>` lists each string field, and elements that own
// nothing have no map at all.
void rask_vec_free(RaskVec *v) {
    if (!v) return;
    vec_check_no_borrows(v, "free");
    if (v->strs.offsets && v->strs.count > 0 && v->data) {
        for (int64_t i = 0; i < v->len; i++) {
            const char *elem = v->data + i * v->elem_size;
            for (int64_t k = 0; k < v->strs.count; k++) {
                rask_string_free((const RaskStr *)(elem + v->strs.offsets[k]));
            }
        }
    }
    if (v->data) rask_realloc(v->data, rask_safe_mul(v->cap, v->elem_size), 0);
    rask_realloc(v, (int64_t)sizeof(RaskVec), 0);
}

int64_t rask_vec_len(const RaskVec *v) {
    return v ? v->len : 0;
}

int64_t rask_vec_capacity(const RaskVec *v) {
    return v ? v->cap : 0;
}

// CP3: bounded and pre-allocated at creation. A bound of 0 is legal — the
// vector is permanently full — so unbounded is -1, never 0.
RaskVec *rask_vec_fixed(int64_t elem_size, int64_t n,
                        const int32_t *str_offs, int64_t n_str_offs) {
    if (n < 0) rask_panic("Vec.fixed needs a non-negative bound");
    RaskVec *v = rask_vec_with_capacity(elem_size, n, str_offs, n_str_offs);
    v->bound = n;
    return v;
}

// CP1/CP2: the ceiling, or -1 when there isn't one. The Rask-facing
// `capacity()` and `remaining()` answer `none` on -1.
int64_t rask_vec_bound(const RaskVec *v) {
    return v ? v->bound : -1;
}

// Room left before the bound, or -1 when unbounded.
int64_t rask_vec_remaining(const RaskVec *v) {
    if (!v || v->bound < 0) return -1;
    return v->bound - v->len;
}

int64_t rask_vec_is_bounded(const RaskVec *v) {
    return (v && v->bound >= 0) ? 1 : 0;
}

int64_t rask_vec_is_full(const RaskVec *v) {
    return (v && v->bound >= 0 && v->len >= v->bound) ? 1 : 0;
}

// C2: growth past the bound panics. `try_push` is the variant that hands the
// value back instead.
static int vec_at_bound(const RaskVec *v) {
    return v->bound >= 0 && v->len >= v->bound;
}

static int vec_grow(RaskVec *v, int64_t needed) {
    if (needed <= v->cap) return 0;
    // Reallocating moves `data`, which would leave every lent-out element
    // pointer dangling. mem.ownership: a stale reference is caught at the
    // access, never silent.
    if (v->borrows > 0) {
        rask_panic("Vec grew while one of its elements was being modified — "
                   "the element reference would dangle");
    }
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

// Every mutator that moves elements or frees the buffer goes through this.
// Writing *into* an element (rask_vec_set) is fine — nothing moves.
static void vec_check_no_borrows(const RaskVec *v, const char *op) {
    if (v && v->borrows > 0) {
        rask_panic_fmt("Vec.%s while one of its elements was being modified — "
                       "the element reference would dangle", op);
    }
}

// Lend out a pointer straight into the buffer, so a callee writing through a
// `mutate` parameter writes the real element instead of a copy. Paired with
// rask_vec_release_elem; in between, anything that would move the buffer panics.
void *rask_vec_borrow_elem(RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index is %lld but length is %lld",
                       (long long)index, (long long)(v ? v->len : 0));
    }
    v->borrows++;
    return v->data + index * v->elem_size;
}

void rask_vec_release_elem(RaskVec *v) {
    if (v && v->borrows > 0) v->borrows--;
}

int64_t rask_vec_push(RaskVec *v, const void *elem) {
    if (!v) return -1;
    if (vec_at_bound(v)) {
        rask_panic_fmt("push failed - collection at capacity (bound %lld)",
                       (long long)v->bound);
    }
    if (vec_grow(v, v->len + 1) != 0) return -1;
    memcpy(v->data + v->len * v->elem_size, elem, (size_t)v->elem_size);
    v->len++;
    return 0;
}

// std.collections/C2: 0 on success, 1 when the vector is at its bound. The
// allocator panics on OOM rather than reporting it, so `GrowError.NoMemory`
// has no path to come from here yet.
int64_t rask_vec_try_push(RaskVec *v, const void *elem) {
    if (!v) return 1;
    if (vec_at_bound(v)) return 1;
    return rask_vec_push(v, elem) == 0 ? 0 : 1;
}

void *rask_vec_get(const RaskVec *v, int64_t index) {
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index is %lld but length is %lld",
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
        rask_panic_fmt("index out of bounds: index is %lld but length is %lld",
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
    vec_check_no_borrows(v, "pop");
    if (!v || v->len == 0) {
        return NULL;
    }
    v->len--;
    return v->data + v->len * v->elem_size;
}

int64_t rask_vec_remove(RaskVec *v, int64_t index) {
    vec_check_no_borrows(v, "remove");
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index is %lld but length is %lld",
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
    vec_check_no_borrows(v, "clear");
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
    vec_check_no_borrows(v, "insert");
    if (!v || index < 0 || index > v->len) {
        rask_panic_fmt("insert index out of bounds: index is %lld but length is %lld",
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
    vec_check_no_borrows(v, "remove_at");
    if (!v || index < 0 || index >= v->len) {
        rask_panic_fmt("index out of bounds: index is %lld but length is %lld",
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
    if (!src) return rask_vec_new(8, NULL, 0);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, src->len,
                                          src->strs.offsets, src->strs.count);
    if (src->len > 0) {
        memcpy(dst->data, src->data, (size_t)(src->len * src->elem_size));
    }
    dst->len = src->len;
    vec_retain_elems(dst, 0, dst->len);
    return dst;
}

// `v.take_all()` hands the elements over and leaves `v` empty (I3). The copy
// is what makes the source safe to keep using — iteration reads the returned
// vec, and nothing points into the original's buffer any more.
RaskVec *rask_vec_take_all(RaskVec *v) {
    vec_check_no_borrows(v, "take_all");
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
// join(vec_of_strings, separator) — a Vec stores its elements inline.
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
//
// The element's own width, not a machine word. Reading eight bytes at a
// four-byte stride returned each pair of elements packed into one number: a
// `Vec<i32>` of 1, 2, 3 joined as `8589934593,12884901890,3`, where that first
// value is `(2<<32)|1`.
void rask_vec_join_i64(RaskStr *out, const RaskVec *src, const RaskStr *sep) {
    rask_string_new(out);
    if (!src || src->len == 0) return;
    for (int64_t i = 0; i < src->len; i++) {
        if (i > 0 && sep) {
            RaskStr tmp;
            rask_string_append(&tmp, out, sep);
            *out = tmp;
        }
        const uint8_t *p = src->data + i * src->elem_size;
        int64_t val;
        switch (src->elem_size) {
            case 1:  val = *(const int8_t *)p; break;
            case 2:  val = *(const int16_t *)p; break;
            case 4:  val = *(const int32_t *)p; break;
            default: val = *(const int64_t *)p; break;
        }
        RaskStr s;
        rask_i64_to_string(&s, val);
        RaskStr tmp;
        rask_string_append(&tmp, out, &s);
        rask_string_free(&s);
        *out = tmp;
    }
}

// Take a reference to every string in every element. A container built by
// copying another's elements is an owner only once it has done this.
void rask_vec_retain_all(RaskVec *v) {
    if (v) vec_retain_elems(v, 0, v->len);
}

// slice(vec, start, end) — returns a new Vec with elements [start..end).
RaskVec *rask_vec_slice(const RaskVec *src, int64_t start, int64_t end) {
    if (!src) return rask_vec_new(8, NULL, 0);
    if (start < 0) start = 0;
    if (end > src->len) end = src->len;
    int64_t new_len = end - start;
    if (new_len <= 0) return rask_vec_new(src->elem_size, src->strs.offsets, src->strs.count);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, new_len,
                                          src->strs.offsets, src->strs.count);
    memcpy(dst->data, src->data + start * src->elem_size,
           (size_t)(new_len * src->elem_size));
    dst->len = new_len;
    vec_retain_elems(dst, 0, dst->len);
    return dst;
}

// chunks(vec, chunk_size) — returns a Vec of Vec* pointers, each a sub-range view.
// Each chunk is a freshly allocated Vec with copied elements.
RaskVec *rask_vec_chunks(const RaskVec *src, int64_t chunk_size) {
    RaskVec *result = rask_vec_new(8, NULL, 0); // Vec of pointers (8 bytes each)
    if (!src || chunk_size <= 0) return result;
    for (int64_t i = 0; i < src->len; i += chunk_size) {
        int64_t remaining = src->len - i;
        int64_t this_chunk = remaining < chunk_size ? remaining : chunk_size;
        RaskVec *chunk = rask_vec_with_capacity(src->elem_size, this_chunk,
                                                src->strs.offsets, src->strs.count);
        memcpy(chunk->data, src->data + i * src->elem_size,
               (size_t)(this_chunk * src->elem_size));
        chunk->len = this_chunk;
        vec_retain_elems(chunk, 0, chunk->len);
        int64_t chunk_ptr = (int64_t)(uintptr_t)chunk;
        rask_vec_push(result, &chunk_ptr);
    }
    return result;
}

// A closure argument is a pointer to `[ func_ptr | captures... ]`, and the
// compiled body takes the environment — the captures, at closure+8 — as an
// implicit first parameter. See crates/rask-codegen/src/closures.rs.
//
// Casting that block straight to a function pointer jumps into data. Most
// adapters never noticed because MIR inlines them when the receiver is a plain
// local; a receiver reached through a struct field takes the call instead, and
// segfaulted (rask-lang/rask#866).
// The return width has to match what the body actually returns, or the upper
// bits of the return register are read as data. A predicate returns `bool`,
// which codegen lowers to I8 — reading that as an i64 kept elements a `false`
// should have dropped.
typedef int8_t  (*RaskClosurePred)(void *env, int64_t arg);
typedef int64_t (*RaskClosureMap)(void *env, int64_t arg);

static inline int closure_call_pred(int64_t closure, int64_t arg) {
    RaskClosurePred fn = *(RaskClosurePred *)(uintptr_t)closure;
    return fn((char *)(uintptr_t)closure + 8, arg) != 0;
}

// A mapping closure returns through a full word. Narrower element types come
// back correctly on x86-64 because a 32-bit result is written through `eax`,
// which clears the rest of the register; `i32` maps round-trip, negatives
// included. Worth revisiting for a target where that does not hold.
static inline int64_t closure_call_map(int64_t closure, int64_t arg) {
    RaskClosureMap fn = *(RaskClosureMap *)(uintptr_t)closure;
    return fn((char *)(uintptr_t)closure + 8, arg);
}

// map(vec, closure) — apply fn to each element, returning new Vec.
RaskVec *rask_vec_map(const RaskVec *src, int64_t closure) {
    if (!src || !closure) return rask_vec_new(8, NULL, 0);
    RaskVec *dst = rask_vec_with_capacity(8, src->len, NULL, 0);
    for (int64_t i = 0; i < src->len; i++) {
        int64_t elem = *(int64_t *)(src->data + i * src->elem_size);
        int64_t result = closure_call_map(closure, elem);
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

// filter(vec, closure) — keep elements where fn returns non-zero.
RaskVec *rask_vec_filter(const RaskVec *src, int64_t closure) {
    if (!src || !closure) return rask_vec_new(8, NULL, 0);
    RaskVec *dst = rask_vec_new(src->elem_size, src->strs.offsets, src->strs.count);
    for (int64_t i = 0; i < src->len; i++) {
        int64_t elem = *(int64_t *)(src->data + i * src->elem_size);
        if (closure_call_pred(closure, elem)) {
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
// Bottom-up merge sort — std.collections/SO1, "stable by default".
//
// qsort makes no stability promise, and glibc's introsort actively reorders
// equal elements. That is observable the moment a comparator looks at less than
// the whole element: `users.sort_by(|a, b| a.score.compare(b.score))` shuffled
// same-score users on native while the interpreter (Rust's stable sort_by) kept
// them in place.
//
// One scratch buffer, ping-ponged between passes, so each pass is a linear
// merge and nothing is copied twice. The `<= 0` on the merge is the whole of
// stability: on a tie the left run's element goes first, and the left run is
// always the earlier one.
//
// Used by sort_by only. Stability is observable exactly when the comparator
// looks at less than the whole element, and that is what sort_by is for — the
// plain sorts compare whole scalars and whole strings, where two equal elements
// are indistinguishable and which one lands first cannot be detected. Measured
// on a million i64: 304ms here against 224ms for qsort, plus n*elem_size of
// scratch. That is a lot to pay for a property nobody can observe, so they keep
// qsort. Don't "unify" these without re-measuring.
static void rask_stable_sort(void *base, int64_t n, int64_t size,
                             int (*cmp)(const void *, const void *)) {
    if (!base || n < 2 || size <= 0) return;
    char *src = (char *)base;
    char *buf = (char *)rask_alloc(rask_safe_mul(n, size));
    char *dst = buf;

    for (int64_t width = 1; width < n; width *= 2) {
        for (int64_t lo = 0; lo < n; lo += 2 * width) {
            int64_t mid = lo + width;
            int64_t hi = lo + 2 * width;
            if (mid > n) mid = n;
            if (hi > n) hi = n;
            int64_t i = lo, j = mid, k = lo;
            while (i < mid && j < hi) {
                if (cmp(src + i * size, src + j * size) <= 0)
                    memcpy(dst + k++ * size, src + i++ * size, (size_t)size);
                else
                    memcpy(dst + k++ * size, src + j++ * size, (size_t)size);
            }
            while (i < mid) memcpy(dst + k++ * size, src + i++ * size, (size_t)size);
            while (j < hi)  memcpy(dst + k++ * size, src + j++ * size, (size_t)size);
        }
        char *swap = src; src = dst; dst = swap;
    }

    // An odd number of passes leaves the result in the scratch buffer.
    if (src != (char *)base) memcpy(base, src, (size_t)rask_safe_mul(n, size));
    rask_realloc(buf, rask_safe_mul(n, size), 0);
}

static int rask_i64_compare(const void *a, const void *b) {
    int64_t va = *(const int64_t *)a;
    int64_t vb = *(const int64_t *)b;
    if (va < vb) return -1;
    if (va > vb) return 1;
    return 0;
}

void rask_vec_sort(RaskVec *v) {
    vec_check_no_borrows(v, "sort");
    if (!v || v->len <= 1) return;
    qsort(v->data, (size_t)v->len, (size_t)v->elem_size, rask_i64_compare);
}

// sort(vec) for Vec<string> — lexicographic, the same order `<` gives.
//
// The default sort reads each element's first 8 bytes as an int64_t. A string
// is a 16-byte RaskStr, so that compared inline character bytes as a
// little-endian number for a short string and a heap pointer for a long one:
// ["pear", "apple"] came back unsorted, and which order you got depended on the
// allocator. Compare the contents instead.
static int rask_str_compare_elem(const void *a, const void *b) {
    return (int)rask_string_compare((const RaskStr *)a, (const RaskStr *)b);
}

void rask_vec_sort_str(RaskVec *v) {
    if (!v || v->len <= 1) return;
    qsort(v->data, (size_t)v->len, (size_t)v->elem_size, rask_str_compare_elem);
}

// sort(vec) for Vec<f64> — the total order from type.operators/ORD3.
//
// The default sort compares elements as int64_t whatever they hold. For floats
// that is wrong twice over: a negative float's bit pattern orders backwards
// against another negative (-1.5 sorted before -2.5), and a NaN lands wherever
// its sign bit puts it. Both were silent — positive floats happen to order
// correctly as integers, so a Vec of positives sorted fine and hid it.
//
// The transform is the standard IEEE totalOrder key: for a negative value flip
// every bit, for a non-negative one set only the sign bit. Ascending unsigned
// order over the result is ascending float order, with -NaN first and +NaN last.
static uint64_t rask_f64_order_key(double d) {
    uint64_t bits;
    memcpy(&bits, &d, sizeof bits);
    return (bits & 0x8000000000000000ULL) ? ~bits : (bits | 0x8000000000000000ULL);
}

static int rask_f64_compare(const void *a, const void *b) {
    uint64_t ka = rask_f64_order_key(*(const double *)a);
    uint64_t kb = rask_f64_order_key(*(const double *)b);
    if (ka < kb) return -1;
    if (ka > kb) return 1;
    return 0;
}

void rask_vec_sort_f64(RaskVec *v) {
    vec_check_no_borrows(v, "sort");
    if (!v || v->len <= 1) return;
    qsort(v->data, (size_t)v->len, (size_t)v->elem_size, rask_f64_compare);
}

// compare(a, b) on f64 — the same total order, returning an Ordering *tag*.
//
// Less=0, Equal=1, Greater=2, from rask_stdlib::ORDERING_VARIANTS. Not -1/0/1:
// the value feeds a match on Ordering variants, and a C-style sign would match
// no arm at all.
int64_t rask_f64_compare_total(double a, double b) {
    uint64_t ka = rask_f64_order_key(a);
    uint64_t kb = rask_f64_order_key(b);
    if (ka < kb) return 0;  /* Less */
    if (ka > kb) return 2;  /* Greater */
    return 1;               /* Equal */
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
/* Ordering's tags, from rask-stdlib's ORDERING_VARIANTS: Less, Equal, Greater. */
#define RASK_ORDERING_EQUAL 1

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
    /* The comparator is declared `-> Ordering`, and an Ordering crosses this
       boundary as its tag: Less 0, Equal 1, Greater 2. The sort wants a sign, so
       the mapping is tag - 1.

       Reading the tag as a sign instead — as this used to — makes Less look
       like "equal" and Equal look like "greater", so a comparator that answers
       Equal for every pair reversed the whole vector. Ascending sorts still
       came out ascending, because (0, +, +) is monotone in the true ordering,
       which is why it went unnoticed. */
    return (int)(fn(env, va, vb) - RASK_ORDERING_EQUAL);
}

void rask_vec_sort_by(RaskVec *v, int64_t comparator) {
    vec_check_no_borrows(v, "sort_by");
    if (!v || v->len <= 1 || !comparator) return;
    rask_sort_comparator = comparator;
    rask_sort_by_ptr = v->elem_size > 8;
    rask_stable_sort(v->data, v->len, v->elem_size, rask_sort_by_adapter);
}

// reverse(vec) — in-place reversal.
void rask_vec_reverse(RaskVec *v) {
    vec_check_no_borrows(v, "reverse");
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
    vec_check_no_borrows(v, "swap");
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
    vec_check_no_borrows(v, "dedup");
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
    if (!src) return rask_vec_new(8, NULL, 0);
    if (n < 0) n = 0;
    int64_t new_len = src->len - n;
    if (new_len <= 0) return rask_vec_new(src->elem_size, src->strs.offsets, src->strs.count);
    RaskVec *dst = rask_vec_with_capacity(src->elem_size, new_len,
                                          src->strs.offsets, src->strs.count);
    memcpy(dst->data, src->data + n * src->elem_size, (size_t)(new_len * src->elem_size));
    dst->len = new_len;
    vec_retain_elems(dst, 0, dst->len);
    return dst;
}

// Write Vec data to a FILE*. Used by self-hosted fs.write_bytes.
void rask_fwrite_vec(int64_t fptr, const RaskVec *v) {
    FILE *f = (FILE *)(uintptr_t)fptr;
    if (!f || !v || !v->data) return;
    fwrite(v->data, 1, (size_t)v->len, f);
}
