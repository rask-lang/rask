// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Path — filesystem path operations on RaskStr values.
//
// Path is represented as a plain RaskStr (16-byte SSO string).
// Option-returning methods use a thread-local buffer and return
// NULL (None) or &buffer (Some). Codegen copies immediately via
// DerefOption, so the buffer is safe to reuse.

#include "rask_runtime.h"
#include <string.h>

static _Thread_local RaskStr path_buf;

static int64_t rfind_sep(const char *data, int64_t len) {
    for (int64_t i = len - 1; i >= 0; i--)
        if (data[i] == '/') return i;
    return -1;
}

static int64_t rfind_dot(const char *data, int64_t start, int64_t len) {
    for (int64_t i = len - 1; i >= start; i--)
        if (data[i] == '.') return i;
    return -1;
}

// ── String-returning (out-param pattern) ────────────────────

void rask_path_new(RaskStr *out, const RaskStr *s) {
    rask_string_from_bytes(out, rask_string_ptr(s), rask_string_len(s));
}

void rask_path_to_string(RaskStr *out, const RaskStr *s) {
    rask_string_from_bytes(out, rask_string_ptr(s), rask_string_len(s));
}

void rask_path_join(RaskStr *out, const RaskStr *self, const RaskStr *other) {
    const char *a = rask_string_ptr(self);
    int64_t a_len = rask_string_len(self);
    const char *b = rask_string_ptr(other);
    int64_t b_len = rask_string_len(other);

    if (b_len > 0 && b[0] == '/') {
        rask_string_from_bytes(out, b, b_len);
        return;
    }

    int need_sep = (a_len > 0 && a[a_len - 1] != '/') ? 1 : 0;
    int64_t total = a_len + need_sep + b_len;
    char *buf = (char *)rask_alloc(total + 1);
    memcpy(buf, a, (size_t)a_len);
    if (need_sep) buf[a_len] = '/';
    memcpy(buf + a_len + need_sep, b, (size_t)b_len);
    buf[total] = '\0';
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
}

void rask_path_with_extension(RaskStr *out, const RaskStr *self, const RaskStr *ext) {
    const char *data = rask_string_ptr(self);
    int64_t len = rask_string_len(self);
    const char *ext_d = rask_string_ptr(ext);
    int64_t ext_len = rask_string_len(ext);

    int64_t sep = rfind_sep(data, len);
    int64_t dot = rfind_dot(data, sep + 1, len);
    int64_t base_len = (dot > sep + 1) ? dot : len;

    int64_t total = base_len + 1 + ext_len;
    char *buf = (char *)rask_alloc(total + 1);
    memcpy(buf, data, (size_t)base_len);
    buf[base_len] = '.';
    memcpy(buf + base_len + 1, ext_d, (size_t)ext_len);
    buf[total] = '\0';
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
}

void rask_path_with_file_name(RaskStr *out, const RaskStr *self, const RaskStr *name) {
    const char *data = rask_string_ptr(self);
    int64_t len = rask_string_len(self);
    const char *nd = rask_string_ptr(name);
    int64_t nlen = rask_string_len(name);

    int64_t sep = rfind_sep(data, len);
    int64_t dir_len = sep >= 0 ? sep + 1 : 0;
    int64_t total = dir_len + nlen;
    char *buf = (char *)rask_alloc(total + 1);
    if (dir_len > 0) memcpy(buf, data, (size_t)dir_len);
    memcpy(buf + dir_len, nd, (size_t)nlen);
    buf[total] = '\0';
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
}

// ── Option-returning (DerefOption: NULL→None, &buf→Some) ────

int64_t rask_path_parent(int64_t ptr) {
    const char *data = rask_string_ptr((const RaskStr *)(intptr_t)ptr);
    int64_t len = rask_string_len((const RaskStr *)(intptr_t)ptr);

    while (len > 1 && data[len - 1] == '/') len--;
    int64_t sep = rfind_sep(data, len);
    if (sep < 0) return 0;

    int64_t plen = sep == 0 ? 1 : sep;
    rask_string_from_bytes(&path_buf, data, plen);
    return (int64_t)(intptr_t)&path_buf;
}

int64_t rask_path_file_name(int64_t ptr) {
    const char *data = rask_string_ptr((const RaskStr *)(intptr_t)ptr);
    int64_t len = rask_string_len((const RaskStr *)(intptr_t)ptr);

    while (len > 1 && data[len - 1] == '/') len--;
    int64_t sep = rfind_sep(data, len);
    const char *name = data + sep + 1;
    int64_t nlen = len - sep - 1;
    if (nlen <= 0) return 0;

    rask_string_from_bytes(&path_buf, name, nlen);
    return (int64_t)(intptr_t)&path_buf;
}

int64_t rask_path_extension(int64_t ptr) {
    const char *data = rask_string_ptr((const RaskStr *)(intptr_t)ptr);
    int64_t len = rask_string_len((const RaskStr *)(intptr_t)ptr);

    int64_t sep = rfind_sep(data, len);
    int64_t dot = rfind_dot(data, sep + 1, len);
    if (dot < 0 || dot == sep + 1) return 0;

    int64_t elen = len - dot - 1;
    if (elen <= 0) return 0;
    rask_string_from_bytes(&path_buf, data + dot + 1, elen);
    return (int64_t)(intptr_t)&path_buf;
}

int64_t rask_path_stem(int64_t ptr) {
    const char *data = rask_string_ptr((const RaskStr *)(intptr_t)ptr);
    int64_t len = rask_string_len((const RaskStr *)(intptr_t)ptr);

    int64_t sep = rfind_sep(data, len);
    int64_t name_start = sep + 1;
    if (name_start >= len) return 0;

    int64_t dot = rfind_dot(data, name_start, len);
    int64_t stem_end = (dot > name_start) ? dot : len;
    rask_string_from_bytes(&path_buf, data + name_start, stem_end - name_start);
    return (int64_t)(intptr_t)&path_buf;
}

// ── Bool-returning ──────────────────────────────────────────

int64_t rask_path_is_absolute(int64_t ptr) {
    const RaskStr *s = (const RaskStr *)(intptr_t)ptr;
    int64_t len = rask_string_len(s);
    return (len > 0 && rask_string_ptr(s)[0] == '/') ? 1 : 0;
}

int64_t rask_path_is_relative(int64_t ptr) {
    return rask_path_is_absolute(ptr) ? 0 : 1;
}

int64_t rask_path_has_extension(int64_t ptr) {
    return rask_path_extension(ptr) != 0 ? 1 : 0;
}

// ── Vec-returning ───────────────────────────────────────────

int64_t rask_path_components(int64_t ptr) {
    const char *data = rask_string_ptr((const RaskStr *)(intptr_t)ptr);
    int64_t len = rask_string_len((const RaskStr *)(intptr_t)ptr);
    RaskVec *v = rask_vec_new(16);

    int64_t start = 0;
    for (int64_t i = 0; i <= len; i++) {
        if (i == len || data[i] == '/') {
            if (i > start) {
                RaskStr comp;
                rask_string_from_bytes(&comp, data + start, i - start);
                rask_vec_push(v, &comp);
            } else if (i == 0 && len > 0 && data[0] == '/') {
                RaskStr comp;
                rask_string_from_bytes(&comp, "/", 1);
                rask_vec_push(v, &comp);
            }
            start = i + 1;
        }
    }
    return (int64_t)(intptr_t)v;
}
