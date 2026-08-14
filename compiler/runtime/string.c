// SPDX-License-Identifier: (MIT OR Apache-2.0)

// String — 16-byte tagged union with small string optimization (SSO).
//
// SSO mode (MSB of byte 15 = 0):
//   [data: u8[15]][remaining: u8]   remaining = 15 - len
//   Unused data bytes zeroed → always null-terminated.
//
// Heap mode (MSB of byte 15 = 1):
//   [header_ptr: *u8 (8B)][tagged_len: u64 (8B)]
//   tagged_len = len | RASK_HEAP_FLAG
//   Header: { atomic_u32 refcount, u32 capacity, u8 data[] }
//   Single contiguous allocation. Data null-terminated.
//
// Refcounting only applies to heap mode. SSO strings have no refcount.
// Sentinel refcount (UINT32_MAX) marks static literals — never freed.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <dirent.h>
#include <errno.h>

#define RASK_HEAP_FLAG   ((uint64_t)1 << 63)
#define RASK_RC_SENTINEL UINT32_MAX
#define RASK_SSO_MAX     15

// ─── Inline helpers ─────────────────────────────────────────

static inline int str_is_heap(const RaskStr *s) {
    return (s->raw[15] & 0x80) != 0;
}

static inline int64_t str_len(const RaskStr *s) {
    if (str_is_heap(s))
        return (int64_t)(s->heap.tagged_len & ~RASK_HEAP_FLAG);
    return 15 - (int64_t)s->sso.remaining;
}

static inline const char *str_data(const RaskStr *s) {
    if (str_is_heap(s))
        return (const char *)(s->heap.header + 8);
    return (const char *)s->sso.data;
}

// Heap header accessors
static inline uint32_t *heap_rc(const RaskStr *s) {
    return (uint32_t *)s->heap.header;
}

static inline uint32_t heap_cap(const RaskStr *s) {
    return *(uint32_t *)(s->heap.header + 4);
}

// ─── Constructors ───────────────────────────────────────────

static void str_make_sso(RaskStr *out, const char *data, int64_t len) {
    memset(out->raw, 0, 16);
    if (len > 0) memcpy(out->sso.data, data, (size_t)len);
    out->sso.remaining = (uint8_t)(15 - len);
}

static void str_make_heap(RaskStr *out, const char *data, int64_t len) {
    int64_t cap = len;
    // Header: [refcount: u32][capacity: u32][data: u8[cap+1]]
    uint8_t *header = (uint8_t *)rask_alloc(8 + cap + 1);
    *(uint32_t *)header = 1;              // refcount = 1
    *(uint32_t *)(header + 4) = (uint32_t)cap; // capacity
    if (len > 0) memcpy(header + 8, data, (size_t)len);
    header[8 + len] = '\0';
    out->heap.header = header;
    out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
}

static void str_make(RaskStr *out, const char *data, int64_t len) {
    if (len <= RASK_SSO_MAX)
        str_make_sso(out, data, len);
    else
        str_make_heap(out, data, len);
}

void rask_string_new(RaskStr *out) {
    str_make_sso(out, NULL, 0);
}

void rask_string_from(RaskStr *out, const char *cstr) {
    if (!cstr) { rask_string_new(out); return; }
    int64_t len = (int64_t)strlen(cstr);
    str_make(out, cstr, len);
}

void rask_string_from_bytes(RaskStr *out, const char *data, int64_t len) {
    if (!data || len <= 0) { rask_string_new(out); return; }
    str_make(out, data, len);
}

// ─── RC operations ──────────────────────────────────────────

void rask_string_free(const RaskStr *s) {
    if (!str_is_heap(s)) return;
    uint32_t *rc = heap_rc(s);
    if (*rc == RASK_RC_SENTINEL) return;
    if (__atomic_sub_fetch(rc, 1, __ATOMIC_ACQ_REL) == 0) {
        uint32_t cap = heap_cap(s);
        rask_realloc(s->heap.header, 8 + cap + 1, 0);
    }
}

void rask_string_clone(const RaskStr *s) {
    if (!str_is_heap(s)) return;
    uint32_t *rc = heap_rc(s);
    if (*rc == RASK_RC_SENTINEL) return;
    __atomic_add_fetch(rc, 1, __ATOMIC_RELAXED);
}

// ─── Accessors ──────────────────────────────────────────────

int64_t rask_string_len(const RaskStr *s) {
    return str_len(s);
}

const char *rask_string_ptr(const RaskStr *s) {
    return str_data(s);
}

int64_t rask_string_is_empty(const RaskStr *s) {
    return str_len(s) == 0 ? 1 : 0;
}

// FNV-1a over the contents, matching what string-keyed maps hash with — a
// string and the same string used as a Map key must agree.
int64_t rask_string_hash(const RaskStr *s) {
    return (int64_t)rask_hash_string_key(s, 0);
}

// ─── Equality and comparison ────────────────────────────────

int64_t rask_string_eq(const RaskStr *a, const RaskStr *b) {
    int64_t alen = str_len(a);
    int64_t blen = str_len(b);
    if (alen != blen) return 0;
    if (alen == 0) return 1;
    return memcmp(str_data(a), str_data(b), (size_t)alen) == 0;
}

int64_t rask_string_compare(const RaskStr *a, const RaskStr *b) {
    const char *ad = str_data(a);
    int64_t alen = str_len(a);
    const char *bd = str_data(b);
    int64_t blen = str_len(b);
    int64_t min_len = alen < blen ? alen : blen;
    int cmp = memcmp(ad, bd, (size_t)min_len);
    if (cmp != 0) return cmp < 0 ? -1 : 1;
    if (alen < blen) return -1;
    if (alen > blen) return 1;
    return 0;
}

int64_t rask_string_lt(const RaskStr *a, const RaskStr *b) {
    return rask_string_compare(a, b) < 0;
}
int64_t rask_string_gt(const RaskStr *a, const RaskStr *b) {
    return rask_string_compare(a, b) > 0;
}
int64_t rask_string_le(const RaskStr *a, const RaskStr *b) {
    return rask_string_compare(a, b) <= 0;
}
int64_t rask_string_ge(const RaskStr *a, const RaskStr *b) {
    return rask_string_compare(a, b) >= 0;
}

// ─── Read-only operations ───────────────────────────────────

int64_t rask_string_byte_at(const RaskStr *s, int64_t pos) {
    int64_t len = str_len(s);
    if (pos < 0 || pos >= len) return 0;
    return (int64_t)(uint8_t)str_data(s)[pos];
}

// The Unicode scalar at *character* index `index`, or -1 when out of range.
// Character index, not byte index — `s.char_at(i)` and `s[i]` both count
// scalars, and `s.len()` counting bytes doesn't change that.
static int64_t str_scalar_at(const RaskStr *s, int64_t index) {
    if (index < 0) return -1;
    int64_t len = str_len(s);
    const char *d = str_data(s);
    int64_t byte = 0;
    int64_t seen = 0;
    while (byte < len) {
        unsigned char c = (unsigned char)d[byte];
        int64_t width = c < 0x80 ? 1
                      : (c & 0xE0) == 0xC0 ? 2
                      : (c & 0xF0) == 0xE0 ? 3
                      : (c & 0xF8) == 0xF0 ? 4
                      : 1;
        if (byte + width > len) return -1;
        if (seen == index) {
            switch (width) {
                case 2: return ((c & 0x1F) << 6) | (d[byte + 1] & 0x3F);
                case 3: return ((c & 0x0F) << 12) | ((d[byte + 1] & 0x3F) << 6)
                             | (d[byte + 2] & 0x3F);
                case 4: return ((c & 0x07) << 18) | ((d[byte + 1] & 0x3F) << 12)
                             | ((d[byte + 2] & 0x3F) << 6) | (d[byte + 3] & 0x3F);
                default: return c;
            }
        }
        byte += width;
        seen++;
    }
    return -1;
}

// How many Unicode scalars the string holds (for the index-out-of-bounds message).
static int64_t str_char_count(const RaskStr *s) {
    int64_t len = str_len(s);
    const char *d = str_data(s);
    int64_t byte = 0;
    int64_t seen = 0;
    while (byte < len) {
        unsigned char c = (unsigned char)d[byte];
        int64_t width = c < 0x80 ? 1
                      : (c & 0xE0) == 0xC0 ? 2
                      : (c & 0xF0) == 0xE0 ? 3
                      : (c & 0xF8) == 0xF0 ? 4
                      : 1;
        byte += width;
        seen++;
    }
    return seen;
}

int64_t rask_string_char_at(const RaskStr *s, int64_t index) {
    return str_scalar_at(s, index);
}

// s[i] — indexing, so an out-of-range index panics rather than answering none.
int64_t rask_string_index(const RaskStr *s, int64_t index) {
    int64_t scalar = str_scalar_at(s, index);
    if (scalar < 0) {
        char buf[128];
        snprintf(buf, sizeof(buf),
                 "string index out of bounds: index is %lld but length is %lld",
                 (long long)index, (long long)str_char_count(s));
        rask_panic(buf);
    }
    return scalar;
}

int64_t rask_string_contains(const RaskStr *haystack, const RaskStr *needle) {
    int64_t hlen = str_len(haystack);
    int64_t nlen = str_len(needle);
    if (nlen == 0) return 1;
    if (nlen > hlen) return 0;
    const char *h = str_data(haystack);
    const char *n = str_data(needle);
    for (int64_t i = 0; i <= hlen - nlen; i++) {
        if (memcmp(h + i, n, (size_t)nlen) == 0) return 1;
    }
    return 0;
}

int64_t rask_string_starts_with(const RaskStr *s, const RaskStr *prefix) {
    int64_t slen = str_len(s);
    int64_t plen = str_len(prefix);
    if (plen == 0) return 1;
    if (slen < plen) return 0;
    return memcmp(str_data(s), str_data(prefix), (size_t)plen) == 0 ? 1 : 0;
}

int64_t rask_string_ends_with(const RaskStr *s, const RaskStr *suffix) {
    int64_t slen = str_len(s);
    int64_t xlen = str_len(suffix);
    if (xlen == 0) return 1;
    if (slen < xlen) return 0;
    return memcmp(str_data(s) + slen - xlen, str_data(suffix), (size_t)xlen) == 0 ? 1 : 0;
}

int64_t rask_string_find(const RaskStr *haystack, const RaskStr *needle) {
    int64_t hlen = str_len(haystack);
    int64_t nlen = str_len(needle);
    if (nlen == 0) return 0;
    if (nlen > hlen) return -1;
    const char *h = str_data(haystack);
    const char *n = str_data(needle);
    for (int64_t i = 0; i <= hlen - nlen; i++) {
        if (memcmp(h + i, n, (size_t)nlen) == 0) return i;
    }
    return -1;
}

int64_t rask_string_rfind(const RaskStr *haystack, const RaskStr *needle) {
    int64_t hlen = str_len(haystack);
    int64_t nlen = str_len(needle);
    if (nlen == 0) return hlen;
    if (nlen > hlen) return -1;
    const char *h = str_data(haystack);
    const char *n = str_data(needle);
    for (int64_t i = hlen - nlen; i >= 0; i--) {
        if (memcmp(h + i, n, (size_t)nlen) == 0) return i;
    }
    return -1;
}

int64_t rask_string_parse_int(const RaskStr *s) {
    if (str_len(s) == 0) return 0;
    return (int64_t)atoll(str_data(s));
}

double rask_string_parse_float(const RaskStr *s) {
    if (str_len(s) == 0) return 0.0;
    return atof(str_data(s));
}

// Copy a RaskStr into a NUL-terminated buffer for strtoll/strtod, which need a
// terminator that a length-counted string doesn't guarantee. Returns 0 when the
// string is longer than the buffer — no real number needs 64 characters.
static int parse_copy(const RaskStr *s, char *buf, size_t cap) {
    int64_t len = str_len(s);
    if (len < 0 || (size_t)len >= cap) return 0;
    memcpy(buf, str_data(s), (size_t)len);
    buf[len] = '\0';
    return 1;
}

// Parsing with a failure signal. Writes the value through `out` and returns 0
// on success, 1 on failure. atoll/atof can't report anything, so "0" and
// "notanumber" were indistinguishable and every parse looked successful (#472).
//
// Matches the interpreter: surrounding whitespace is trimmed, then the whole
// remaining string must be a number. Leading garbage, trailing garbage, an
// empty string and out-of-range all fail. Neither backend range-checks against
// a narrower target width — `"70000".parse()` into a u16 succeeds on both.
int64_t rask_string_parse_int_into(const RaskStr *s, int64_t *out) {
    char buf[64];
    if (!parse_copy(s, buf, sizeof buf)) return 1;

    errno = 0;
    char *end = NULL;
    long long v = strtoll(buf, &end, 10);
    if (end == buf) return 1;              // no digits consumed
    if (errno == ERANGE) return 1;         // outside i64
    while (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r') end++;
    if (*end != '\0') return 1;            // trailing garbage

    *out = (int64_t)v;
    return 0;
}

int64_t rask_string_parse_float_into(const RaskStr *s, double *out) {
    char buf[64];
    if (!parse_copy(s, buf, sizeof buf)) return 1;

    errno = 0;
    char *end = NULL;
    double v = strtod(buf, &end);
    if (end == buf) return 1;
    if (errno == ERANGE) return 1;
    while (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r') end++;
    if (*end != '\0') return 1;

    *out = v;
    return 0;
}

// ─── String-producing operations (out-param) ────────────────

void rask_string_concat(RaskStr *out, const RaskStr *a, const RaskStr *b) {
    int64_t alen = str_len(a);
    int64_t blen = str_len(b);
    int64_t total = rask_safe_add(alen, blen);
    const char *ad = str_data(a);
    const char *bd = str_data(b);
    if (total <= RASK_SSO_MAX) {
        memset(out->raw, 0, 16);
        if (alen > 0) memcpy(out->sso.data, ad, (size_t)alen);
        if (blen > 0) memcpy(out->sso.data + alen, bd, (size_t)blen);
        out->sso.remaining = (uint8_t)(15 - total);
    } else {
        uint8_t *header = (uint8_t *)rask_alloc(8 + total + 1);
        *(uint32_t *)header = 1;
        *(uint32_t *)(header + 4) = (uint32_t)total;
        if (alen > 0) memcpy(header + 8, ad, (size_t)alen);
        if (blen > 0) memcpy(header + 8 + alen, bd, (size_t)blen);
        header[8 + total] = '\0';
        out->heap.header = header;
        out->heap.tagged_len = (uint64_t)total | RASK_HEAP_FLAG;
    }
}

/* True when byte `i` of a UTF-8 string starts a character. Continuation bytes
   are 0b10xxxxxx; anything else begins one, and the end of the string counts. */
static int str_is_char_boundary(const char *d, int64_t len, int64_t i) {
    if (i == 0 || i == len) return 1;
    return ((unsigned char)d[i] & 0xC0) != 0x80;
}

void rask_string_substr(RaskStr *out, const RaskStr *s, int64_t start, int64_t end) {
    int64_t slen = str_len(s);
    /* Out of range clamps — there's no ambiguity about what was meant. */
    if (start < 0) start = 0;
    if (end > slen) end = slen;
    if (start >= end) { rask_string_new(out); return; }
    /* A cut inside a character is a different matter: it would hand back a
       `string` that isn't valid UTF-8, which the type says can't exist. The
       caller asked for something that doesn't exist, so say so rather than
       returning a nearby slice they didn't ask for. */
    const char *d = str_data(s);
    if (!str_is_char_boundary(d, slen, start) || !str_is_char_boundary(d, slen, end)) {
        rask_panic_fmt(
            "substring(%lld, %lld) cuts a character in half - "
            "these are byte offsets, and one of them lands inside a multi-byte "
            "character. `char_indices()` gives offsets that don't.",
            (long long)start, (long long)end);
    }
    str_make(out, d + start, end - start);
}

void rask_string_to_lowercase(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    // Build into a temp buffer, then make SSO or heap
    char *buf = (char *)rask_alloc(len);
    for (int64_t i = 0; i < len; i++) {
        char c = d[i];
        buf[i] = (c >= 'A' && c <= 'Z') ? c + 32 : c;
    }
    str_make(out, buf, len);
    rask_realloc(buf, len, 0);
}

void rask_string_to_uppercase(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    char *buf = (char *)rask_alloc(len);
    for (int64_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)d[i];
        buf[i] = (c >= 'a' && c <= 'z') ? (char)(c - 32) : (char)c;
    }
    str_make(out, buf, len);
    rask_realloc(buf, len, 0);
}

void rask_string_trim(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    const char *start = d;
    const char *end = d + len;
    while (start < end && (*start == ' ' || *start == '\t' || *start == '\n' || *start == '\r'))
        start++;
    while (end > start && (end[-1] == ' ' || end[-1] == '\t' || end[-1] == '\n' || end[-1] == '\r'))
        end--;
    str_make(out, start, (int64_t)(end - start));
}

void rask_string_trim_start(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    int64_t start = 0;
    while (start < len && (d[start] == ' ' || d[start] == '\t'
           || d[start] == '\n' || d[start] == '\r'))
        start++;
    str_make(out, d + start, len - start);
}

void rask_string_trim_end(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    int64_t end = len;
    while (end > 0 && (d[end - 1] == ' ' || d[end - 1] == '\t'
           || d[end - 1] == '\n' || d[end - 1] == '\r'))
        end--;
    str_make(out, d, end);
}

void rask_string_repeat(RaskStr *out, const RaskStr *s, int64_t count) {
    int64_t slen = str_len(s);
    if (slen == 0 || count <= 0) { rask_string_new(out); return; }
    int64_t total = rask_safe_mul(slen, count);
    const char *d = str_data(s);
    if (total <= RASK_SSO_MAX) {
        memset(out->raw, 0, 16);
        for (int64_t i = 0; i < count; i++)
            memcpy(out->sso.data + i * slen, d, (size_t)slen);
        out->sso.remaining = (uint8_t)(15 - total);
    } else {
        uint8_t *header = (uint8_t *)rask_alloc(8 + total + 1);
        *(uint32_t *)header = 1;
        *(uint32_t *)(header + 4) = (uint32_t)total;
        for (int64_t i = 0; i < count; i++)
            memcpy(header + 8 + i * slen, d, (size_t)slen);
        header[8 + total] = '\0';
        out->heap.header = header;
        out->heap.tagged_len = (uint64_t)total | RASK_HEAP_FLAG;
    }
}

void rask_string_reverse(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (len == 0) { rask_string_new(out); return; }
    const char *d = str_data(s);
    char *buf = (char *)rask_alloc(len);
    for (int64_t i = 0; i < len; i++)
        buf[i] = d[len - 1 - i];
    str_make(out, buf, len);
    rask_realloc(buf, len, 0);
}

void rask_string_replace(RaskStr *out, const RaskStr *s, const RaskStr *from, const RaskStr *to) {
    int64_t slen = str_len(s);
    int64_t flen = str_len(from);
    if (slen == 0) { rask_string_new(out); return; }
    if (flen == 0) {
        // No match possible — copy input
        str_make(out, str_data(s), slen);
        return;
    }

    int64_t tlen = str_len(to);
    const char *sd = str_data(s);
    const char *fd = str_data(from);
    const char *td = str_data(to);

    // Count occurrences
    int64_t count = 0;
    const char *p = sd;
    const char *end = sd + slen;
    while (p + flen <= end) {
        if (memcmp(p, fd, (size_t)flen) == 0) { count++; p += flen; }
        else p++;
    }

    int64_t new_len = rask_safe_add(slen, rask_safe_mul(count, tlen - flen));
    char *buf = (char *)rask_alloc(new_len);
    char *dst = buf;
    p = sd;
    while (p < end) {
        if (p + flen <= end && memcmp(p, fd, (size_t)flen) == 0) {
            if (tlen > 0) memcpy(dst, td, (size_t)tlen);
            dst += tlen;
            p += flen;
        } else {
            *dst++ = *p++;
        }
    }
    str_make(out, buf, new_len);
    rask_realloc(buf, new_len, 0);
}

// Replace the first `n` occurrences (S: replacen). n <= 0 leaves the string
// alone; a larger n than there are matches just replaces them all.
void rask_string_replacen(RaskStr *out, const RaskStr *s, const RaskStr *from,
                          const RaskStr *to, int64_t n) {
    int64_t slen = str_len(s);
    int64_t flen = str_len(from);
    if (slen == 0) { rask_string_new(out); return; }
    if (flen == 0 || n <= 0) { str_make(out, str_data(s), slen); return; }

    int64_t tlen = str_len(to);
    const char *sd = str_data(s);
    const char *fd = str_data(from);
    const char *td = str_data(to);
    const char *end = sd + slen;

    int64_t count = 0;
    const char *p = sd;
    while (p + flen <= end && count < n) {
        if (memcmp(p, fd, (size_t)flen) == 0) { count++; p += flen; }
        else p++;
    }

    int64_t new_len = rask_safe_add(slen, rask_safe_mul(count, tlen - flen));
    char *buf = (char *)rask_alloc(new_len);
    char *dst = buf;
    int64_t done = 0;
    p = sd;
    while (p < end) {
        if (done < n && p + flen <= end && memcmp(p, fd, (size_t)flen) == 0) {
            if (tlen > 0) memcpy(dst, td, (size_t)tlen);
            dst += tlen;
            p += flen;
            done++;
        } else {
            *dst++ = *p++;
        }
    }
    str_make(out, buf, new_len);
    rask_realloc(buf, new_len, 0);
}

// Number of Unicode scalars, not bytes (S: char_count). Counts the bytes that
// aren't UTF-8 continuation bytes.
int64_t rask_string_char_count(const RaskStr *s) {
    int64_t len = str_len(s);
    const unsigned char *p = (const unsigned char *)str_data(s);
    int64_t n = 0;
    for (int64_t i = 0; i < len; i++) {
        if ((p[i] & 0xC0) != 0x80) n++;
    }
    return n;
}

// True when every byte is ASCII (0x00–0x7F).
int64_t rask_string_str_is_ascii(const RaskStr *s) {
    int64_t len = str_len(s);
    const unsigned char *p = (const unsigned char *)str_data(s);
    for (int64_t i = 0; i < len; i++) {
        if (p[i] > 0x7F) return 0;
    }
    return 1;
}

// A one-character string from a Unicode scalar (S: from_char).
void rask_string_from_char(RaskStr *out, int64_t cp) {
    char buf[4];
    int64_t n;
    uint32_t c = (uint32_t)cp;
    if (c < 0x80) {
        buf[0] = (char)c; n = 1;
    } else if (c < 0x800) {
        buf[0] = (char)(0xC0 | (c >> 6));
        buf[1] = (char)(0x80 | (c & 0x3F));
        n = 2;
    } else if (c < 0x10000) {
        buf[0] = (char)(0xE0 | (c >> 12));
        buf[1] = (char)(0x80 | ((c >> 6) & 0x3F));
        buf[2] = (char)(0x80 | (c & 0x3F));
        n = 3;
    } else {
        buf[0] = (char)(0xF0 | (c >> 18));
        buf[1] = (char)(0x80 | ((c >> 12) & 0x3F));
        buf[2] = (char)(0x80 | ((c >> 6) & 0x3F));
        buf[3] = (char)(0x80 | (c & 0x3F));
        n = 4;
    }
    str_make(out, buf, n);
}

// ─── Split / lines / chars → Vec ────────────────────────────

RaskVec *rask_string_lines(const RaskStr *s) {
    RaskVec *v = rask_vec_new(16); // elem_size = sizeof(RaskStr) = 16
    int64_t slen = str_len(s);
    if (slen == 0) return v;
    const char *p = str_data(s);
    const char *end = p + slen;
    while (p < end) {
        const char *nl = (const char *)memchr(p, '\n', (size_t)(end - p));
        int64_t len = nl ? (int64_t)(nl - p) : (int64_t)(end - p);
        RaskStr line;
        str_make(&line, p, len);
        rask_vec_push(v, &line);
        p = nl ? nl + 1 : end;
    }
    return v;
}

RaskVec *rask_string_split(const RaskStr *s, const RaskStr *sep) {
    RaskVec *v = rask_vec_new(16);
    int64_t slen = str_len(s);
    int64_t sep_len = str_len(sep);
    const char *p = str_data(s);
    const char *end = p + slen;
    const char *sepd = str_data(sep);

    if (sep_len == 0) {
        for (int64_t i = 0; i < slen; i++) {
            RaskStr c;
            str_make_sso(&c, p + i, 1);
            rask_vec_push(v, &c);
        }
        return v;
    }

    while (p <= end) {
        const char *found = NULL;
        if (p < end) {
            for (const char *q = p; q + sep_len <= end; q++) {
                if (memcmp(q, sepd, (size_t)sep_len) == 0) {
                    found = q;
                    break;
                }
            }
        }
        int64_t chunk = found ? (int64_t)(found - p) : (int64_t)(end - p);
        RaskStr part;
        str_make(&part, p, chunk);
        rask_vec_push(v, &part);
        if (!found) break;
        p = found + sep_len;
    }
    return v;
}

RaskVec *rask_string_split_whitespace(const RaskStr *s) {
    RaskVec *v = rask_vec_new(16);
    int64_t slen = str_len(s);
    if (slen == 0) return v;
    const char *p = str_data(s);
    const char *end = p + slen;
    while (p < end) {
        while (p < end && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r'))
            p++;
        if (p >= end) break;
        const char *start = p;
        while (p < end && *p != ' ' && *p != '\t' && *p != '\n' && *p != '\r')
            p++;
        RaskStr tok;
        str_make(&tok, start, p - start);
        rask_vec_push(v, &tok);
    }
    return v;
}

RaskVec *rask_string_chars(const RaskStr *s) {
    RaskVec *v = rask_vec_new(8);
    int64_t len = str_len(s);
    const char *d = str_data(s);
    for (int64_t i = 0; i < len; i++) {
        int64_t ch = (int64_t)(uint8_t)d[i];
        rask_vec_push(v, &ch);
    }
    return v;
}

// ─── Builder operations (out-param) ─────────────────────────
// Builder always works in heap mode. Promotes SSO to heap on first use.

// Ensure s is a sole-owner heap string. Returns header pointer.
// Writes the promoted/detached value to *out.
static uint8_t *builder_ensure_heap(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    if (!str_is_heap(s)) {
        // Promote SSO to heap
        const char *d = str_data(s);
        int64_t cap = len < 8 ? 8 : len;
        uint8_t *header = (uint8_t *)rask_alloc(8 + cap + 1);
        *(uint32_t *)header = 1;
        *(uint32_t *)(header + 4) = (uint32_t)cap;
        if (len > 0) memcpy(header + 8, d, (size_t)len);
        header[8 + len] = '\0';
        out->heap.header = header;
        out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
        return header;
    }
    uint32_t *rc = heap_rc(s);
    if (*rc != 1 && *rc != RASK_RC_SENTINEL) {
        // Shared — detach (COW)
        const char *d = str_data(s);
        int64_t cap = len;
        uint8_t *header = (uint8_t *)rask_alloc(8 + cap + 1);
        *(uint32_t *)header = 1;
        *(uint32_t *)(header + 4) = (uint32_t)cap;
        if (len > 0) memcpy(header + 8, d, (size_t)len);
        header[8 + len] = '\0';
        __atomic_sub_fetch(rc, 1, __ATOMIC_ACQ_REL);
        out->heap.header = header;
        out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
        return header;
    }
    if (*rc == RASK_RC_SENTINEL) {
        // Literal — create mutable copy
        const char *d = str_data(s);
        int64_t cap = len;
        uint8_t *header = (uint8_t *)rask_alloc(8 + cap + 1);
        *(uint32_t *)header = 1;
        *(uint32_t *)(header + 4) = (uint32_t)cap;
        if (len > 0) memcpy(header + 8, d, (size_t)len);
        header[8 + len] = '\0';
        out->heap.header = header;
        out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
        return header;
    }
    // Sole owner — use as-is
    *out = *s;
    return out->heap.header;
}

static void builder_grow(RaskStr *out, int64_t needed) {
    uint32_t cap = heap_cap(out);
    if (needed <= (int64_t)cap) return;
    int64_t new_cap = cap ? cap : 8;
    while (new_cap < needed) {
        if (new_cap > INT64_MAX / 2) rask_panic("string capacity overflow");
        new_cap *= 2;
    }
    out->heap.header = (uint8_t *)rask_realloc(out->heap.header,
        8 + cap + 1, 8 + new_cap + 1);
    *(uint32_t *)(out->heap.header + 4) = (uint32_t)new_cap;
}

void rask_string_push_byte(RaskStr *out, const RaskStr *s, uint8_t byte) {
    builder_ensure_heap(out, s);
    int64_t len = str_len(out);
    builder_grow(out, len + 1);
    out->heap.header[8 + len] = byte;
    len++;
    out->heap.header[8 + len] = '\0';
    out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
}

void rask_string_push_char(RaskStr *out, const RaskStr *s, int32_t cp) {
    uint8_t buf[4];
    int n;
    if (cp < 0) { *out = *s; return; }
    else if (cp <= 0x7F) { buf[0] = (uint8_t)cp; n = 1; }
    else if (cp <= 0x7FF) {
        buf[0] = 0xC0 | (uint8_t)(cp >> 6);
        buf[1] = 0x80 | (uint8_t)(cp & 0x3F);
        n = 2;
    } else if (cp <= 0xFFFF) {
        if (cp >= 0xD800 && cp <= 0xDFFF) { *out = *s; return; }
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
    } else { *out = *s; return; }

    builder_ensure_heap(out, s);
    int64_t len = str_len(out);
    builder_grow(out, len + n);
    memcpy(out->heap.header + 8 + len, buf, (size_t)n);
    len += n;
    out->heap.header[8 + len] = '\0';
    out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
}

void rask_string_append(RaskStr *out, const RaskStr *s, const RaskStr *other) {
    int64_t olen = str_len(other);
    if (olen == 0) { *out = *s; return; }
    const char *od = str_data(other);
    builder_ensure_heap(out, s);
    int64_t len = str_len(out);
    builder_grow(out, len + olen);
    memcpy(out->heap.header + 8 + len, od, (size_t)olen);
    len += olen;
    out->heap.header[8 + len] = '\0';
    out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
}

void rask_string_append_cstr(RaskStr *out, const RaskStr *s, const char *cstr) {
    if (!cstr) { *out = *s; return; }
    int64_t clen = (int64_t)strlen(cstr);
    if (clen == 0) { *out = *s; return; }
    builder_ensure_heap(out, s);
    int64_t len = str_len(out);
    builder_grow(out, len + clen);
    memcpy(out->heap.header + 8 + len, cstr, (size_t)clen);
    len += clen;
    out->heap.header[8 + len] = '\0';
    out->heap.tagged_len = (uint64_t)len | RASK_HEAP_FLAG;
}

void rask_string_push_str(RaskStr *out, const RaskStr *s, const RaskStr *other) {
    rask_string_append(out, s, other);
}

// ─── Conversion to string ───────────────────────────────────

void rask_i64_to_string(RaskStr *out, int64_t val) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%lld", (long long)val);
    rask_string_from(out, buf);
}

void rask_u64_to_string(RaskStr *out, uint64_t val) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%llu", (unsigned long long)val);
    rask_string_from(out, buf);
}

void rask_bool_to_string(RaskStr *out, int64_t val) {
    rask_string_from(out, val ? "true" : "false");
}

// Render a double the way the interpreter does: the shortest digit string
// that reads back as the same value, never in exponent form. `%g` alone gives
// 6 significant digits, so 1234567.75 printed as 1.23457e+06 — both a loss of
// digits and a different shape from the interpreter's output.
//
// `buf` should be RASK_F64_BUF_SIZE bytes: a large magnitude in fixed notation
// needs room for every digit before the decimal point.
void rask_fmt_double(char *buf, size_t n, double val) {
    if (isnan(val)) { snprintf(buf, n, "NaN"); return; }
    if (isinf(val)) { snprintf(buf, n, val < 0 ? "-inf" : "inf"); return; }

    int prec = 1;
    for (; prec < 17; prec++) {
        snprintf(buf, n, "%.*g", prec, val);
        if (strtod(buf, NULL) == val) break;
    }
    if (prec >= 17) snprintf(buf, n, "%.17g", val);

    // %g switches to exponent form once the magnitude passes the precision.
    // Re-render those with the same significant digits, spelled out.
    if (strchr(buf, 'e')) {
        int exp10 = (int)floor(log10(fabs(val)));
        int decimals = prec - 1 - exp10;
        if (decimals < 0) decimals = 0;
        if (decimals > 320) decimals = 320;
        snprintf(buf, n, "%.*f", decimals, val);
    }
}

// Same idea for f32. Widening to double first and formatting as a double
// spells out the f32's exact binary value — 0.1f comes back as
// 0.10000000149011612 instead of 0.1, because the round-trip is checked
// against the wrong width.
void rask_fmt_float(char *buf, size_t n, float val) {
    if (isnan(val)) { snprintf(buf, n, "NaN"); return; }
    if (isinf(val)) { snprintf(buf, n, val < 0 ? "-inf" : "inf"); return; }

    int prec = 1;
    for (; prec < 9; prec++) {
        snprintf(buf, n, "%.*g", prec, (double)val);
        if (strtof(buf, NULL) == val) break;
    }
    if (prec >= 9) snprintf(buf, n, "%.9g", (double)val);

    if (strchr(buf, 'e')) {
        int exp10 = (int)floorf(log10f(fabsf(val)));
        int decimals = prec - 1 - exp10;
        if (decimals < 0) decimals = 0;
        if (decimals > 60) decimals = 60;
        snprintf(buf, n, "%.*f", decimals, (double)val);
    }
}

void rask_f64_to_string(RaskStr *out, double val) {
    char buf[RASK_F64_BUF_SIZE];
    rask_fmt_double(buf, sizeof(buf), val);
    rask_string_from(out, buf);
}

void rask_f32_to_string(RaskStr *out, float val) {
    char buf[RASK_F64_BUF_SIZE];
    rask_fmt_float(buf, sizeof(buf), val);
    rask_string_from(out, buf);
}

void rask_char_to_string(RaskStr *out, int32_t codepoint) {
    RaskStr empty;
    rask_string_new(&empty);
    rask_string_push_char(out, &empty, codepoint);
    // If the push produced a heap string from an empty builder, check if
    // the result fits in SSO. For single chars (1-4 bytes), it always does,
    // but the builder always produces heap. Compact to SSO if possible.
    if (str_is_heap(out)) {
        int64_t len = str_len(out);
        if (len <= RASK_SSO_MAX) {
            const char *d = str_data(out);
            uint8_t *header = out->heap.header;
            str_make_sso(out, d, len);
            // Free the heap allocation
            uint32_t cap = *(uint32_t *)(header + 4);
            rask_realloc(header, 8 + cap + 1, 0);
        }
    }
}

// ─── Format specs (std.fmt/S1) ──────────────────────────────
//
// The spec itself is parsed at compile time; what arrives here is one piece
// of it at a time. The type token picks a base conversion, then the width /
// align / fill triple pads the result — the same two stages the interpreter
// runs, so the two backends render a spec identically.

// Base 2, 8 or 16. Negative values render their two's-complement bit pattern,
// which is what a hex or binary spec is asking to see.
void rask_i64_to_base(RaskStr *out, int64_t val, int64_t base, int64_t upper) {
    char buf[72];
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    uint64_t v = (uint64_t)val;
    if (base < 2 || base > 16) base = 10;

    int i = (int)sizeof(buf) - 1;
    buf[i] = '\0';
    if (v == 0) {
        buf[--i] = '0';
    } else {
        while (v != 0 && i > 0) {
            buf[--i] = digits[v % (uint64_t)base];
            v /= (uint64_t)base;
        }
    }
    rask_string_from(out, buf + i);
}

void rask_u64_to_base(RaskStr *out, uint64_t val, int64_t base, int64_t upper) {
    rask_i64_to_base(out, (int64_t)val, base, upper);
}

void rask_f64_to_precision(RaskStr *out, double val, int64_t precision) {
    if (precision < 0) { rask_f64_to_string(out, val); return; }
    if (precision > 300) precision = 300;
    char buf[RASK_F64_BUF_SIZE];
    snprintf(buf, sizeof(buf), "%.*f", (int)precision, val);
    rask_string_from(out, buf);
}

void rask_f64_to_exp(RaskStr *out, double val) {
    char buf[64];
    snprintf(buf, sizeof(buf), "%e", val);
    rask_string_from(out, buf);
}

// Keep the first `count` characters — a precision on a string truncates it.
void rask_string_truncate_chars(RaskStr *out, const RaskStr *s, int64_t count) {
    if (count < 0) { *out = *s; rask_string_clone(out); return; }
    const char *data = str_data(s);
    int64_t len = str_len(s);
    int64_t i = 0, seen = 0;
    while (i < len && seen < count) {
        unsigned char c = (unsigned char)data[i];
        i += (c < 0x80) ? 1 : (c < 0xE0) ? 2 : (c < 0xF0) ? 3 : 4;
        seen++;
    }
    if (i > len) i = len;
    str_make(out, data, i);
}

// align: 0 left, 1 right, 2 center. Width counts characters, not bytes.
void rask_string_pad(RaskStr *out, const RaskStr *s, int64_t width, int64_t align, int32_t fill) {
    int64_t count = str_char_count(s);
    if (width <= 0 || count >= width) {
        *out = *s;
        rask_string_clone(out);
        return;
    }
    int64_t padding = width - count;
    int64_t left = (align == 0) ? 0 : (align == 2) ? padding / 2 : padding;
    int64_t right = padding - left;

    // Encode the fill character once, then repeat it.
    char fill_buf[5];
    int fill_len = 0;
    uint32_t cp = (uint32_t)fill;
    if (cp < 0x80) {
        fill_buf[fill_len++] = (char)cp;
    } else if (cp < 0x800) {
        fill_buf[fill_len++] = (char)(0xC0 | (cp >> 6));
        fill_buf[fill_len++] = (char)(0x80 | (cp & 0x3F));
    } else if (cp < 0x10000) {
        fill_buf[fill_len++] = (char)(0xE0 | (cp >> 12));
        fill_buf[fill_len++] = (char)(0x80 | ((cp >> 6) & 0x3F));
        fill_buf[fill_len++] = (char)(0x80 | (cp & 0x3F));
    } else {
        fill_buf[fill_len++] = (char)(0xF0 | (cp >> 18));
        fill_buf[fill_len++] = (char)(0x80 | ((cp >> 12) & 0x3F));
        fill_buf[fill_len++] = (char)(0x80 | ((cp >> 6) & 0x3F));
        fill_buf[fill_len++] = (char)(0x80 | (cp & 0x3F));
    }

    int64_t body = str_len(s);
    int64_t total = body + (left + right) * fill_len;
    char *buf = (char *)rask_alloc((size_t)total + 1);
    if (!buf) { *out = *s; rask_string_clone(out); return; }

    int64_t pos = 0;
    for (int64_t i = 0; i < left; i++) { memcpy(buf + pos, fill_buf, (size_t)fill_len); pos += fill_len; }
    memcpy(buf + pos, str_data(s), (size_t)body);
    pos += body;
    for (int64_t i = 0; i < right; i++) { memcpy(buf + pos, fill_buf, (size_t)fill_len); pos += fill_len; }
    buf[pos] = '\0';

    str_make(out, buf, pos);
    rask_realloc(buf, (size_t)total + 1, 0);
}

// `{:debug}` on a string quotes it; on anything else debug and display agree
// for the primitives, so only these two need a runtime of their own.
void rask_string_debug(RaskStr *out, const RaskStr *s) {
    int64_t len = str_len(s);
    const char *data = str_data(s);
    char *buf = (char *)rask_alloc((size_t)len + 3);
    if (!buf) { *out = *s; rask_string_clone(out); return; }
    buf[0] = '"';
    memcpy(buf + 1, data, (size_t)len);
    buf[len + 1] = '"';
    buf[len + 2] = '\0';
    str_make(out, buf, len + 2);
    rask_realloc(buf, (size_t)len + 3, 0);
}

void rask_char_debug(RaskStr *out, int32_t codepoint) {
    RaskStr inner;
    rask_char_to_string(&inner, codepoint);
    int64_t len = str_len(&inner);
    char *buf = (char *)rask_alloc((size_t)len + 3);
    if (!buf) { *out = inner; return; }
    buf[0] = '\'';
    memcpy(buf + 1, str_data(&inner), (size_t)len);
    buf[len + 1] = '\'';
    buf[len + 2] = '\0';
    str_make(out, buf, len + 2);
    rask_realloc(buf, (size_t)len + 3, 0);
    rask_string_free(&inner);
}

// ─── Char predicates ────────────────────────────────────────

int64_t rask_char_is_digit(int32_t c) {
    return (c >= '0' && c <= '9') ? 1 : 0;
}

int64_t rask_char_is_ascii(int32_t c) {
    return (c >= 0 && c <= 127) ? 1 : 0;
}

int64_t rask_char_is_alphabetic(int32_t c) {
    if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z')) return 1;
    if (c > 127) return 1;
    return 0;
}

int64_t rask_char_is_numeric(int32_t c) {
    if (c >= '0' && c <= '9') return 1;
    if (c >= 0x00B2 && c <= 0x00B3) return 1;
    if (c == 0x00B9) return 1;
    if (c >= 0x00BC && c <= 0x00BE) return 1;
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

int64_t rask_char_to_int(int32_t c) {
    return (int64_t)c;
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

// ─── Filesystem ─────────────────────────────────────────────

RaskVec *rask_fs_list_dir(const RaskStr *path) {
    RaskVec *v = rask_vec_new(16);
    int64_t plen = str_len(path);
    if (plen == 0) return v;

    const char *pd = str_data(path);
    // str_data returns null-terminated pointer for SSO (zeroed bytes)
    // and for heap (explicit null). Safe to pass to opendir.
    DIR *d = opendir(pd);
    if (!d) return v;

    struct dirent *entry;
    while ((entry = readdir(d)) != NULL) {
        if (entry->d_name[0] == '.' && (entry->d_name[1] == '\0' ||
            (entry->d_name[1] == '.' && entry->d_name[2] == '\0')))
            continue;
        RaskStr name;
        rask_string_from(&name, entry->d_name);
        rask_vec_push(v, &name);
    }
    closedir(d);
    return v;
}

// ─── Map iteration ──────────────────────────────────────────

extern RaskVec *rask_map_entries(const void *map);
RaskVec *rask_map_iter(const void *map) {
    return rask_map_entries(map);
}

// ─── StringBuilder ──────────────────────────────────────────
//
// Growable byte buffer backed by realloc. UTF-8 valid by construction
// (only string and char data enters through the API).

typedef struct {
    char  *data;
    int64_t len;
    int64_t cap;
} RaskStringBuilder;

int64_t rask_string_builder_new(void) {
    RaskStringBuilder *sb = (RaskStringBuilder *)rask_alloc(sizeof(RaskStringBuilder));
    sb->data = NULL;
    sb->len = 0;
    sb->cap = 0;
    return (int64_t)(uintptr_t)sb;
}

int64_t rask_string_builder_with_capacity(int64_t cap) {
    RaskStringBuilder *sb = (RaskStringBuilder *)rask_alloc(sizeof(RaskStringBuilder));
    sb->data = (char *)rask_alloc((size_t)cap);
    sb->len = 0;
    sb->cap = cap;
    return (int64_t)(uintptr_t)sb;
}

static void sb_grow(RaskStringBuilder *sb, int64_t extra) {
    int64_t needed = sb->len + extra;
    if (needed <= sb->cap) return;
    int64_t new_cap = sb->cap < 16 ? 16 : sb->cap;
    while (new_cap < needed) new_cap *= 2;
    sb->data = (char *)realloc(sb->data, (size_t)new_cap);
    sb->cap = new_cap;
}

void rask_string_builder_append(int64_t handle, int64_t str_ptr) {
    RaskStringBuilder *sb = (RaskStringBuilder *)(uintptr_t)handle;
    const RaskStr *s = (const RaskStr *)(uintptr_t)str_ptr;
    int64_t slen = str_len(s);
    if (slen <= 0) return;
    sb_grow(sb, slen);
    memcpy(sb->data + sb->len, str_data(s), (size_t)slen);
    sb->len += slen;
}

void rask_string_builder_append_char(int64_t handle, int64_t codepoint) {
    RaskStringBuilder *sb = (RaskStringBuilder *)(uintptr_t)handle;
    uint32_t cp = (uint32_t)codepoint;
    char buf[4];
    int n;
    if (cp < 0x80) {
        buf[0] = (char)cp; n = 1;
    } else if (cp < 0x800) {
        buf[0] = (char)(0xC0 | (cp >> 6));
        buf[1] = (char)(0x80 | (cp & 0x3F)); n = 2;
    } else if (cp < 0x10000) {
        buf[0] = (char)(0xE0 | (cp >> 12));
        buf[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[2] = (char)(0x80 | (cp & 0x3F)); n = 3;
    } else {
        buf[0] = (char)(0xF0 | (cp >> 18));
        buf[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
        buf[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[3] = (char)(0x80 | (cp & 0x3F)); n = 4;
    }
    sb_grow(sb, n);
    memcpy(sb->data + sb->len, buf, (size_t)n);
    sb->len += n;
}

// Consume the builder, return a string. Zero-copy when possible.
void rask_string_builder_build(RaskStr *out, int64_t handle) {
    RaskStringBuilder *sb = (RaskStringBuilder *)(uintptr_t)handle;
    str_make(out, sb->data, sb->len);
    free(sb->data);
    free(sb);
}

int64_t rask_string_builder_len(int64_t handle) {
    RaskStringBuilder *sb = (RaskStringBuilder *)(uintptr_t)handle;
    return sb->len;
}

int64_t rask_string_builder_is_empty(int64_t handle) {
    RaskStringBuilder *sb = (RaskStringBuilder *)(uintptr_t)handle;
    return sb->len == 0 ? 1 : 0;
}
