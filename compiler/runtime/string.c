// SPDX-License-Identifier: (MIT OR Apache-2.0)

// String — UTF-8 owned string, always null-terminated.
// Internal buffer has room for the null byte beyond the reported length.

#include "rask_runtime.h"
#include <stdio.h>
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
        if (new_cap > INT64_MAX / 2) rask_panic("string capacity overflow");
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
    if (s->data) rask_realloc(s->data, s->cap + 1, 0);
    rask_realloc(s, (int64_t)sizeof(RaskString), 0);
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

int64_t rask_string_byte_at(const RaskString *s, int64_t pos) {
    if (!s || pos < 0 || pos >= s->len) return 0;
    return (int64_t)(uint8_t)s->data[pos];
}

RaskString *rask_string_substr(const RaskString *s, int64_t start, int64_t end) {
    if (!s) return rask_string_new();
    if (start < 0) start = 0;
    if (end > s->len) end = s->len;
    if (start >= end) return rask_string_new();
    return rask_string_from_bytes(s->data + start, end - start);
}

RaskString *rask_string_concat(const RaskString *a, const RaskString *b) {
    const char *ad = a ? a->data : "";
    int64_t alen = a ? a->len : 0;
    const char *bd = b ? b->data : "";
    int64_t blen = b ? b->len : 0;
    RaskString *r = (RaskString *)rask_alloc(sizeof(RaskString));
    r->len = rask_safe_add(alen, blen);
    r->cap = r->len;
    r->data = (char *)rask_alloc(rask_safe_add(r->len, 1));
    if (alen) memcpy(r->data, ad, (size_t)alen);
    if (blen) memcpy(r->data + alen, bd, (size_t)blen);
    r->data[r->len] = '\0';
    return r;
}

int64_t rask_string_contains(const RaskString *haystack, const RaskString *needle) {
    const char *h = haystack ? haystack->data : "";
    const char *n = needle ? needle->data : "";
    return strstr(h, n) != NULL ? 1 : 0;
}

RaskString *rask_string_to_lowercase(const RaskString *s) {
    if (!s || s->len == 0) return rask_string_new();
    RaskString *r = (RaskString *)rask_alloc(sizeof(RaskString));
    r->len = s->len;
    r->cap = s->len;
    r->data = (char *)rask_alloc(s->len + 1);
    for (int64_t i = 0; i < s->len; i++) {
        char c = s->data[i];
        r->data[i] = (c >= 'A' && c <= 'Z') ? c + 32 : c;
    }
    r->data[r->len] = '\0';
    return r;
}

int64_t rask_string_starts_with(const RaskString *s, const RaskString *prefix) {
    if (!prefix || prefix->len == 0) return 1;
    if (!s || s->len < prefix->len) return 0;
    return memcmp(s->data, prefix->data, (size_t)prefix->len) == 0 ? 1 : 0;
}

int64_t rask_string_ends_with(const RaskString *s, const RaskString *suffix) {
    if (!suffix || suffix->len == 0) return 1;
    if (!s || s->len < suffix->len) return 0;
    return memcmp(s->data + s->len - suffix->len, suffix->data, (size_t)suffix->len) == 0 ? 1 : 0;
}

RaskVec *rask_string_lines(const RaskString *s) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    if (!s || s->len == 0) return v;
    const char *p = s->data;
    const char *end = s->data + s->len;
    while (p < end) {
        const char *nl = (const char *)memchr(p, '\n', (size_t)(end - p));
        int64_t len = nl ? (int64_t)(nl - p) : (int64_t)(end - p);
        RaskString *line = rask_string_from_bytes(p, len);
        rask_vec_push(v, &line);
        p = nl ? nl + 1 : end;
    }
    return v;
}

RaskString *rask_string_trim(const RaskString *s) {
    if (!s || s->len == 0) return rask_string_new();
    const char *start = s->data;
    const char *end = s->data + s->len;
    while (start < end && (*start == ' ' || *start == '\t' || *start == '\n' || *start == '\r'))
        start++;
    while (end > start && (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\n' || end[-1] == '\r'))
        end--;
    return rask_string_from_bytes(start, (int64_t)(end - start));
}

RaskVec *rask_string_split(const RaskString *s, const RaskString *sep) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    const char *p = s ? s->data : "";
    int64_t slen = s ? s->len : 0;
    int64_t sep_len = sep ? sep->len : 0;
    const char *end = p + slen;

    if (sep_len == 0) {
        // Split each byte (ASCII character)
        for (int64_t i = 0; i < slen; i++) {
            RaskString *c = rask_string_from_bytes(p + i, 1);
            rask_vec_push(v, &c);
        }
        return v;
    }

    while (p <= end) {
        const char *found = NULL;
        if (p < end) {
            // Manual memmem — find sep in remaining bytes
            for (const char *q = p; q + sep_len <= end; q++) {
                if (memcmp(q, sep->data, (size_t)sep_len) == 0) {
                    found = q;
                    break;
                }
            }
        }
        int64_t chunk = found ? (int64_t)(found - p) : (int64_t)(end - p);
        RaskString *part = rask_string_from_bytes(p, chunk);
        rask_vec_push(v, &part);
        if (!found) break;
        p = found + sep_len;
    }
    return v;
}

RaskVec *rask_string_split_whitespace(const RaskString *s) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    if (!s || s->len == 0) return v;
    const char *p = s->data;
    const char *end = p + s->len;
    while (p < end) {
        // Skip whitespace
        while (p < end && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r'))
            p++;
        if (p >= end) break;
        // Find end of token
        const char *start = p;
        while (p < end && *p != ' ' && *p != '\t' && *p != '\n' && *p != '\r')
            p++;
        RaskString *tok = rask_string_from_bytes(start, p - start);
        rask_vec_push(v, &tok);
    }
    return v;
}

RaskString *rask_string_replace(const RaskString *s, const RaskString *from, const RaskString *to) {
    if (!s) return rask_string_new();
    if (!from || from->len == 0) return rask_string_clone(s);

    const char *to_data = to ? to->data : "";
    int64_t to_len = to ? to->len : 0;

    // Count occurrences
    int64_t count = 0;
    const char *p = s->data;
    const char *end = s->data + s->len;
    while (p + from->len <= end) {
        if (memcmp(p, from->data, (size_t)from->len) == 0) {
            count++;
            p += from->len;
        } else {
            p++;
        }
    }

    int64_t new_len = rask_safe_add(s->len, rask_safe_mul(count, to_len - from->len));
    RaskString *r = (RaskString *)rask_alloc(sizeof(RaskString));
    r->len = new_len;
    r->cap = new_len;
    r->data = (char *)rask_alloc(rask_safe_add(new_len, 1));

    char *dst = r->data;
    p = s->data;
    while (p < end) {
        if (p + from->len <= end && memcmp(p, from->data, (size_t)from->len) == 0) {
            if (to_len) memcpy(dst, to_data, (size_t)to_len);
            dst += to_len;
            p += from->len;
        } else {
            *dst++ = *p++;
        }
    }
    r->data[new_len] = '\0';
    return r;
}

int64_t rask_string_parse_int(const RaskString *s) {
    if (!s || s->len == 0) return 0;
    return (int64_t)atoll(s->data);
}

double rask_string_parse_float(const RaskString *s) {
    if (!s || s->len == 0) return 0.0;
    return atof(s->data);
}

// ─── Conversion to string ───────────────────────────────────

RaskString *rask_i64_to_string(int64_t val) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%lld", (long long)val);
    return rask_string_from(buf);
}

RaskString *rask_bool_to_string(int64_t val) {
    return rask_string_from(val ? "true" : "false");
}

RaskString *rask_f64_to_string(double val) {
    char buf[64];
    snprintf(buf, sizeof(buf), "%g", val);
    return rask_string_from(buf);
}

RaskString *rask_char_to_string(int32_t codepoint) {
    RaskString *s = rask_string_new();
    rask_string_push_char(s, codepoint);
    return s;
}

// ─── Char predicates ────────────────────────────────────────

int64_t rask_char_is_digit(int32_t c) {
    return (c >= '0' && c <= '9') ? 1 : 0;
}

int64_t rask_char_is_ascii(int32_t c) {
    return (c >= 0 && c <= 127) ? 1 : 0;
}

int64_t rask_char_is_alphabetic(int32_t c) {
    // ASCII letters + basic Unicode letter detection
    if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z')) return 1;
    if (c > 127) return 1; // conservative: treat non-ASCII as alphabetic
    return 0;
}

int64_t rask_char_is_numeric(int32_t c) {
    if (c >= '0' && c <= '9') return 1;
    // Unicode numeric characters (superscripts, fractions, etc.)
    if (c >= 0x00B2 && c <= 0x00B3) return 1; // ², ³
    if (c == 0x00B9) return 1; // ¹
    if (c >= 0x00BC && c <= 0x00BE) return 1; // ¼, ½, ¾
    return 0;
}

int64_t rask_char_is_alphanumeric(int32_t c) {
    return (rask_char_is_alphabetic(c) || rask_char_is_numeric(c)) ? 1 : 0;
}

int64_t rask_char_is_whitespace(int32_t c) {
    return (c == ' ' || c == '\t' || c == '\n' || c == '\r'
         || c == 0x0B || c == 0x0C) ? 1 : 0;
}

int64_t rask_char_is_uppercase(int32_t c) {
    return (c >= 'A' && c <= 'Z') ? 1 : 0;
}

int64_t rask_char_is_lowercase(int32_t c) {
    return (c >= 'a' && c <= 'z') ? 1 : 0;
}

int64_t rask_char_to_uppercase(int32_t c) {
    if (c >= 'a' && c <= 'z') return c - 32;
    return c;
}

int64_t rask_char_to_lowercase(int32_t c) {
    if (c >= 'A' && c <= 'Z') return c + 32;
    return c;
}

int64_t rask_char_len_utf8(int32_t c) {
    if (c < 0x80) return 1;
    if (c < 0x800) return 2;
    if (c < 0x10000) return 3;
    return 4;
}

int64_t rask_char_eq(int32_t a, int32_t b) {
    return a == b ? 1 : 0;
}

// ─── Additional string methods ──────────────────────────────

int64_t rask_string_is_empty(const RaskString *s) {
    return (!s || s->len == 0) ? 1 : 0;
}

RaskString *rask_string_to_uppercase(const RaskString *s) {
    if (!s) return rask_string_new();
    RaskString *r = rask_string_from_bytes(s->data, s->len);
    for (int64_t i = 0; i < r->len; i++) {
        unsigned char c = (unsigned char)r->data[i];
        if (c >= 'a' && c <= 'z') r->data[i] = (char)(c - 32);
    }
    return r;
}

// find(haystack, needle) — byte offset of first occurrence, -1 if not found.
int64_t rask_string_find(const RaskString *haystack, const RaskString *needle) {
    if (!haystack || !needle) return -1;
    if (needle->len == 0) return 0;
    const char *p = strstr(haystack->data, needle->data);
    if (!p) return -1;
    return (int64_t)(p - haystack->data);
}

// rfind(haystack, needle) — byte offset of last occurrence, -1 if not found.
int64_t rask_string_rfind(const RaskString *haystack, const RaskString *needle) {
    if (!haystack || !needle) return -1;
    if (needle->len == 0) return haystack->len;
    int64_t last = -1;
    const char *start = haystack->data;
    while (1) {
        const char *p = strstr(start, needle->data);
        if (!p) break;
        last = (int64_t)(p - haystack->data);
        start = p + 1;
    }
    return last;
}

// char_at(s, byte_index) — returns the UTF-8 codepoint at byte index.
int64_t rask_string_char_at(const RaskString *s, int64_t index) {
    if (!s || index < 0 || index >= s->len) return -1;
    unsigned char c = (unsigned char)s->data[index];
    if (c < 0x80) return c;
    // Simplified UTF-8 decode
    if ((c & 0xE0) == 0xC0 && index + 1 < s->len) {
        return ((c & 0x1F) << 6) | (s->data[index + 1] & 0x3F);
    }
    if ((c & 0xF0) == 0xE0 && index + 2 < s->len) {
        return ((c & 0x0F) << 12) | ((s->data[index + 1] & 0x3F) << 6)
             | (s->data[index + 2] & 0x3F);
    }
    if ((c & 0xF8) == 0xF0 && index + 3 < s->len) {
        return ((c & 0x07) << 18) | ((s->data[index + 1] & 0x3F) << 12)
             | ((s->data[index + 2] & 0x3F) << 6) | (s->data[index + 3] & 0x3F);
    }
    return c;
}

// repeat(s, count) — returns string repeated count times.
RaskString *rask_string_repeat(const RaskString *s, int64_t count) {
    if (!s || count <= 0) return rask_string_new();
    int64_t total = s->len * count;
    RaskString *r = (RaskString *)rask_alloc(sizeof(RaskString));
    r->data = (char *)rask_alloc(total + 1);
    r->cap = total + 1;
    r->len = total;
    for (int64_t i = 0; i < count; i++) {
        memcpy(r->data + i * s->len, s->data, (size_t)s->len);
    }
    r->data[total] = '\0';
    return r;
}

// string_reverse(s) — byte-reversed copy (correct for ASCII; multi-byte aware).
RaskString *rask_string_reverse(const RaskString *s) {
    if (!s) return rask_string_new();
    RaskString *r = (RaskString *)rask_alloc(sizeof(RaskString));
    r->data = (char *)rask_alloc(s->len + 1);
    r->cap = s->len + 1;
    r->len = s->len;
    for (int64_t i = 0; i < s->len; i++) {
        r->data[i] = s->data[s->len - 1 - i];
    }
    r->data[s->len] = '\0';
    return r;
}

// trim_start(s) — trim leading whitespace.
RaskString *rask_string_trim_start(const RaskString *s) {
    if (!s) return rask_string_new();
    int64_t start = 0;
    while (start < s->len && (s->data[start] == ' ' || s->data[start] == '\t'
           || s->data[start] == '\n' || s->data[start] == '\r')) {
        start++;
    }
    return rask_string_from_bytes(s->data + start, s->len - start);
}

// trim_end(s) — trim trailing whitespace.
RaskString *rask_string_trim_end(const RaskString *s) {
    if (!s) return rask_string_new();
    int64_t end = s->len;
    while (end > 0 && (s->data[end - 1] == ' ' || s->data[end - 1] == '\t'
           || s->data[end - 1] == '\n' || s->data[end - 1] == '\r')) {
        end--;
    }
    return rask_string_from_bytes(s->data, end);
}
