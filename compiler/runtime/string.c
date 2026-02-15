// SPDX-License-Identifier: (MIT OR Apache-2.0)

// String — UTF-8 owned string, always null-terminated.
// Internal buffer has room for the null byte beyond the reported length.

#include "rask_runtime.h"
#include <stdlib.h>
#include <string.h>

struct RaskString {
    char   *data;
    int64_t len;
    int64_t cap; // capacity excluding null terminator
};

static void string_grow(RaskString *s, int64_t needed) {
    if (needed <= s->cap) return;
    int64_t new_cap = s->cap ? s->cap : 8;
    while (new_cap < needed) {
        new_cap *= 2;
    }
    // +1 for null terminator
    s->data = (char *)rask_realloc(s->data, s->cap + 1, new_cap + 1);
    s->cap = new_cap;
}

RaskString *rask_string_new(void) {
    RaskString *s = (RaskString *)rask_alloc(sizeof(RaskString));
    s->data = (char *)rask_alloc(1);
    s->data[0] = '\0';
    s->len = 0;
    s->cap = 0;
    return s;
}

RaskString *rask_string_from(const char *cstr) {
    if (!cstr) return rask_string_new();
    int64_t len = (int64_t)strlen(cstr);
    RaskString *s = (RaskString *)rask_alloc(sizeof(RaskString));
    s->data = (char *)rask_alloc(len + 1);
    memcpy(s->data, cstr, (size_t)len + 1);
    s->len = len;
    s->cap = len;
    return s;
}

RaskString *rask_string_from_bytes(const char *data, int64_t len) {
    if (!data || len <= 0) return rask_string_new();
    RaskString *s = (RaskString *)rask_alloc(sizeof(RaskString));
    s->data = (char *)rask_alloc(len + 1);
    memcpy(s->data, data, (size_t)len);
    s->data[len] = '\0';
    s->len = len;
    s->cap = len;
    return s;
}

void rask_string_free(RaskString *s) {
    if (!s) return;
    rask_free(s->data);
    rask_free(s);
}

int64_t rask_string_len(const RaskString *s) {
    return s ? s->len : 0;
}

const char *rask_string_ptr(const RaskString *s) {
    if (!s) return "";
    return s->data;
}

int64_t rask_string_push_byte(RaskString *s, uint8_t byte) {
    if (!s) return -1;
    string_grow(s, s->len + 1);
    s->data[s->len] = (char)byte;
    s->len++;
    s->data[s->len] = '\0';
    return 0;
}

// Encode a Unicode codepoint as UTF-8 and append it.
int64_t rask_string_push_char(RaskString *s, int32_t cp) {
    if (!s) return -1;
    uint8_t buf[4];
    int n;
    if (cp < 0) {
        return -1;
    } else if (cp <= 0x7F) {
        buf[0] = (uint8_t)cp;
        n = 1;
    } else if (cp <= 0x7FF) {
        buf[0] = 0xC0 | (uint8_t)(cp >> 6);
        buf[1] = 0x80 | (uint8_t)(cp & 0x3F);
        n = 2;
    } else if (cp <= 0xFFFF) {
        // Reject surrogates — not valid Unicode scalar values
        if (cp >= 0xD800 && cp <= 0xDFFF) return -1;
        buf[0] = 0xE0 | (uint8_t)(cp >> 12);
        buf[1] = 0x80 | (uint8_t)((cp >> 6) & 0x3F);
        buf[2] = 0x80 | (uint8_t)(cp & 0x3F);
        n = 3;
    } else if (cp <= 0x10FFFF) {
        buf[0] = 0xF0 | (uint8_t)(cp >> 18);
        buf[1] = 0x80 | (uint8_t)((cp >> 12) & 0x3F);
        buf[2] = 0x80 | (uint8_t)((cp >> 6) & 0x3F);
        buf[3] = 0x80 | (uint8_t)(cp & 0x3F);
        n = 4;
    } else {
        return -1; // invalid codepoint
    }
    string_grow(s, s->len + n);
    memcpy(s->data + s->len, buf, (size_t)n);
    s->len += n;
    s->data[s->len] = '\0';
    return 0;
}

int64_t rask_string_append(RaskString *s, const RaskString *other) {
    if (!s || !other || other->len == 0) return 0;
    string_grow(s, s->len + other->len);
    memcpy(s->data + s->len, other->data, (size_t)other->len);
    s->len += other->len;
    s->data[s->len] = '\0';
    return 0;
}

int64_t rask_string_append_cstr(RaskString *s, const char *cstr) {
    if (!s || !cstr) return 0;
    int64_t clen = (int64_t)strlen(cstr);
    if (clen == 0) return 0;
    string_grow(s, s->len + clen);
    memcpy(s->data + s->len, cstr, (size_t)clen);
    s->len += clen;
    s->data[s->len] = '\0';
    return 0;
}

RaskString *rask_string_clone(const RaskString *s) {
    if (!s) return rask_string_new();
    return rask_string_from_bytes(s->data, s->len);
}

int64_t rask_string_eq(const RaskString *a, const RaskString *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->len != b->len) return 0;
    return memcmp(a->data, b->data, (size_t)a->len) == 0;
}

RaskString *rask_string_substr(const RaskString *s, int64_t start, int64_t end) {
    if (!s) return rask_string_new();
    if (start < 0) start = 0;
    if (end > s->len) end = s->len;
    if (start >= end) return rask_string_new();
    return rask_string_from_bytes(s->data + start, end - start);
}
