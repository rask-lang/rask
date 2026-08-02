// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// JSON value tree + shape-driven typed decoding.
//
// Two halves. The parser turns a string into a RaskJsonVal tree (RFC 8259,
// nesting and all). The decoder walks that tree against a *shape* — a
// description of the target type the compiler builds at the call site — and
// writes the fields straight into the destination's storage.
//
// The shape exists because the codegen backend has no reflection at runtime.
// `json.decode<User>(s)` lowers to a handful of rask_json_shape_* calls that
// spell out User's fields (name, offset, kind), then one rask_json_decode_into.
// Nesting, arrays, and maps recurse here in C instead of unrolling into MIR.

#include "rask_runtime.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// ─── Value tree ───────────────────────────────────────────────────

struct RaskJsonVal {
    uint8_t kind;
    union {
        int8_t b;
        double num;
        RaskStr str;
        struct {
            RaskJsonVal **items;
            int32_t len;
            int32_t cap;
        } arr;
        struct {
            RaskStr *keys;
            RaskJsonVal **vals;
            int32_t len;
            int32_t cap;
        } obj;
    } as;
};

// Nesting cap. The parser recurses, so a hostile "[[[[[…" would otherwise walk
// off the stack before it ever reached the decoder.
#define JSON_MAX_DEPTH 256

// ─── Error reporting ──────────────────────────────────────────────

static _Thread_local char json_err_msg[512];
static _Thread_local int64_t json_err_kind = RASK_JSON_OK;

static void json_fail(int64_t kind, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(json_err_msg, sizeof(json_err_msg), fmt, ap);
    va_end(ap);
    json_err_kind = kind;
}

int64_t rask_json_error_kind(void) {
    return json_err_kind;
}

void rask_json_error_message(RaskStr *out) {
    rask_string_from_bytes(out, json_err_msg, (int64_t)strlen(json_err_msg));
}

// ─── Parser ───────────────────────────────────────────────────────

typedef struct {
    const char *p;
    const char *end;
    int depth;
} JsonScan;

static RaskJsonVal *json_val_new(uint8_t kind) {
    RaskJsonVal *v = (RaskJsonVal *)rask_alloc((int64_t)sizeof(RaskJsonVal));
    memset(v, 0, sizeof(RaskJsonVal));
    v->kind = kind;
    return v;
}

void rask_json_tree_free(RaskJsonVal *v) {
    if (!v) return;
    switch (v->kind) {
        case RASK_JSON_STR:
            rask_string_free(&v->as.str);
            break;
        case RASK_JSON_ARR:
            for (int32_t i = 0; i < v->as.arr.len; i++) {
                rask_json_tree_free(v->as.arr.items[i]);
            }
            rask_free(v->as.arr.items);
            break;
        case RASK_JSON_OBJ:
            for (int32_t i = 0; i < v->as.obj.len; i++) {
                rask_string_free(&v->as.obj.keys[i]);
                rask_json_tree_free(v->as.obj.vals[i]);
            }
            rask_free(v->as.obj.keys);
            rask_free(v->as.obj.vals);
            break;
        default:
            break;
    }
    rask_free(v);
}

static void json_arr_push(RaskJsonVal *v, RaskJsonVal *item) {
    if (v->as.arr.len == v->as.arr.cap) {
        int32_t old = v->as.arr.cap;
        int32_t cap = old ? old * 2 : 8;
        v->as.arr.items = (RaskJsonVal **)rask_realloc(
            v->as.arr.items,
            (int64_t)old * (int64_t)sizeof(RaskJsonVal *),
            (int64_t)cap * (int64_t)sizeof(RaskJsonVal *));
        v->as.arr.cap = cap;
    }
    v->as.arr.items[v->as.arr.len++] = item;
}

// Last value wins for a repeated key (J5) — replace in place so the key order
// still reflects first appearance, which is what a Map<string, T> decode sees.
static void json_obj_put(RaskJsonVal *v, RaskStr *key, RaskJsonVal *val) {
    for (int32_t i = 0; i < v->as.obj.len; i++) {
        if (rask_string_eq(&v->as.obj.keys[i], key)) {
            rask_string_free(key);
            rask_json_tree_free(v->as.obj.vals[i]);
            v->as.obj.vals[i] = val;
            return;
        }
    }
    if (v->as.obj.len == v->as.obj.cap) {
        int32_t old = v->as.obj.cap;
        int32_t cap = old ? old * 2 : 8;
        v->as.obj.keys = (RaskStr *)rask_realloc(
            v->as.obj.keys,
            (int64_t)old * (int64_t)sizeof(RaskStr),
            (int64_t)cap * (int64_t)sizeof(RaskStr));
        v->as.obj.vals = (RaskJsonVal **)rask_realloc(
            v->as.obj.vals,
            (int64_t)old * (int64_t)sizeof(RaskJsonVal *),
            (int64_t)cap * (int64_t)sizeof(RaskJsonVal *));
        v->as.obj.cap = cap;
    }
    v->as.obj.keys[v->as.obj.len] = *key;
    v->as.obj.vals[v->as.obj.len] = val;
    v->as.obj.len++;
}

static void json_skip_space(JsonScan *s) {
    while (s->p < s->end) {
        char c = *s->p;
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') s->p++;
        else break;
    }
}

static int64_t json_offset(const JsonScan *s, const char *base) {
    return (int64_t)(s->p - base);
}

// Growable byte buffer for a string with escapes.
typedef struct {
    char *data;
    int64_t len;
    int64_t cap;
} JsonStrBuf;

static void jsb_push(JsonStrBuf *b, const char *src, int64_t n) {
    if (b->len + n > b->cap) {
        int64_t old = b->cap;
        int64_t cap = old ? old * 2 : 32;
        while (cap < b->len + n) cap *= 2;
        b->data = (char *)rask_realloc(b->data, old, cap);
        b->cap = cap;
    }
    memcpy(b->data + b->len, src, (size_t)n);
    b->len += n;
}

static void jsb_push_utf8(JsonStrBuf *b, uint32_t cp) {
    char tmp[4];
    if (cp < 0x80) {
        tmp[0] = (char)cp;
        jsb_push(b, tmp, 1);
    } else if (cp < 0x800) {
        tmp[0] = (char)(0xC0 | (cp >> 6));
        tmp[1] = (char)(0x80 | (cp & 0x3F));
        jsb_push(b, tmp, 2);
    } else if (cp < 0x10000) {
        tmp[0] = (char)(0xE0 | (cp >> 12));
        tmp[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
        tmp[2] = (char)(0x80 | (cp & 0x3F));
        jsb_push(b, tmp, 3);
    } else {
        tmp[0] = (char)(0xF0 | (cp >> 18));
        tmp[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
        tmp[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
        tmp[3] = (char)(0x80 | (cp & 0x3F));
        jsb_push(b, tmp, 4);
    }
}

static int json_hex4(JsonScan *s, uint32_t *out) {
    if (s->end - s->p < 4) return 0;
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) {
        char c = s->p[i];
        uint32_t d;
        if (c >= '0' && c <= '9') d = (uint32_t)(c - '0');
        else if (c >= 'a' && c <= 'f') d = (uint32_t)(c - 'a' + 10);
        else if (c >= 'A' && c <= 'F') d = (uint32_t)(c - 'A' + 10);
        else return 0;
        v = v * 16 + d;
    }
    s->p += 4;
    *out = v;
    return 1;
}

// Parses a quoted string into `out`. Returns 0 and sets the error on failure.
static int json_scan_string(JsonScan *s, const char *base, RaskStr *out) {
    if (s->p >= s->end || *s->p != '"') {
        json_fail(RASK_JSON_ERR_PARSE, "expected a string at byte %lld",
                  (long long)json_offset(s, base));
        return 0;
    }
    s->p++;
    const char *start = s->p;
    int escaped = 0;
    while (s->p < s->end && *s->p != '"') {
        if (*s->p == '\\') {
            escaped = 1;
            s->p += 2;
        } else {
            s->p++;
        }
    }
    if (s->p >= s->end) {
        json_fail(RASK_JSON_ERR_PARSE, "unterminated string starting at byte %lld",
                  (long long)(start - base - 1));
        return 0;
    }
    if (!escaped) {
        rask_string_from_bytes(out, start, (int64_t)(s->p - start));
        s->p++;
        return 1;
    }

    // Re-scan, this time expanding escapes.
    JsonStrBuf b = {NULL, 0, 0};
    const char *stop = s->p;
    s->p = start;
    while (s->p < stop) {
        if (*s->p != '\\') {
            const char *run = s->p;
            while (s->p < stop && *s->p != '\\') s->p++;
            jsb_push(&b, run, (int64_t)(s->p - run));
            continue;
        }
        s->p++;
        if (s->p >= stop) break;
        char c = *s->p++;
        switch (c) {
            case '"':  jsb_push(&b, "\"", 1); break;
            case '\\': jsb_push(&b, "\\", 1); break;
            case '/':  jsb_push(&b, "/", 1); break;
            case 'b':  jsb_push(&b, "\b", 1); break;
            case 'f':  jsb_push(&b, "\f", 1); break;
            case 'n':  jsb_push(&b, "\n", 1); break;
            case 'r':  jsb_push(&b, "\r", 1); break;
            case 't':  jsb_push(&b, "\t", 1); break;
            case 'u': {
                uint32_t cp;
                if (!json_hex4(s, &cp)) {
                    json_fail(RASK_JSON_ERR_PARSE, "bad \\u escape at byte %lld",
                              (long long)json_offset(s, base));
                    rask_free(b.data);
                    return 0;
                }
                // Surrogate pair: 😀 is one code point, not two.
                if (cp >= 0xD800 && cp <= 0xDBFF && stop - s->p >= 6
                    && s->p[0] == '\\' && s->p[1] == 'u') {
                    const char *save = s->p;
                    s->p += 2;
                    uint32_t lo;
                    if (json_hex4(s, &lo) && lo >= 0xDC00 && lo <= 0xDFFF) {
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    } else {
                        s->p = save;
                    }
                }
                // A lone surrogate isn't a code point; U+FFFD keeps the output
                // valid UTF-8 rather than smuggling a broken sequence through.
                if (cp >= 0xD800 && cp <= 0xDFFF) cp = 0xFFFD;
                jsb_push_utf8(&b, cp);
                break;
            }
            default:
                json_fail(RASK_JSON_ERR_PARSE, "invalid escape '\\%c' at byte %lld",
                          c, (long long)json_offset(s, base));
                rask_free(b.data);
                return 0;
        }
    }
    s->p = stop + 1;
    rask_string_from_bytes(out, b.data ? b.data : "", b.len);
    rask_free(b.data);
    return 1;
}

static RaskJsonVal *json_scan_value(JsonScan *s, const char *base);

static RaskJsonVal *json_scan_array(JsonScan *s, const char *base) {
    RaskJsonVal *v = json_val_new(RASK_JSON_ARR);
    s->p++; // '['
    json_skip_space(s);
    if (s->p < s->end && *s->p == ']') {
        s->p++;
        return v;
    }
    for (;;) {
        RaskJsonVal *item = json_scan_value(s, base);
        if (!item) {
            rask_json_tree_free(v);
            return NULL;
        }
        json_arr_push(v, item);
        json_skip_space(s);
        if (s->p < s->end && *s->p == ',') {
            s->p++;
            continue;
        }
        if (s->p < s->end && *s->p == ']') {
            s->p++;
            return v;
        }
        json_fail(RASK_JSON_ERR_PARSE, "expected ',' or ']' at byte %lld",
                  (long long)json_offset(s, base));
        rask_json_tree_free(v);
        return NULL;
    }
}

static RaskJsonVal *json_scan_object(JsonScan *s, const char *base) {
    RaskJsonVal *v = json_val_new(RASK_JSON_OBJ);
    s->p++; // '{'
    json_skip_space(s);
    if (s->p < s->end && *s->p == '}') {
        s->p++;
        return v;
    }
    for (;;) {
        json_skip_space(s);
        RaskStr key;
        if (!json_scan_string(s, base, &key)) {
            rask_json_tree_free(v);
            return NULL;
        }
        json_skip_space(s);
        if (s->p >= s->end || *s->p != ':') {
            json_fail(RASK_JSON_ERR_PARSE, "expected ':' after object key at byte %lld",
                      (long long)json_offset(s, base));
            rask_string_free(&key);
            rask_json_tree_free(v);
            return NULL;
        }
        s->p++;
        RaskJsonVal *val = json_scan_value(s, base);
        if (!val) {
            rask_string_free(&key);
            rask_json_tree_free(v);
            return NULL;
        }
        json_obj_put(v, &key, val);
        json_skip_space(s);
        if (s->p < s->end && *s->p == ',') {
            s->p++;
            continue;
        }
        if (s->p < s->end && *s->p == '}') {
            s->p++;
            return v;
        }
        json_fail(RASK_JSON_ERR_PARSE, "expected ',' or '}' at byte %lld",
                  (long long)json_offset(s, base));
        rask_json_tree_free(v);
        return NULL;
    }
}

static RaskJsonVal *json_scan_number(JsonScan *s, const char *base) {
    const char *start = s->p;
    if (s->p < s->end && *s->p == '-') s->p++;
    if (s->p < s->end && *s->p == '0') {
        s->p++;
    } else if (s->p < s->end && *s->p >= '1' && *s->p <= '9') {
        while (s->p < s->end && *s->p >= '0' && *s->p <= '9') s->p++;
    } else {
        json_fail(RASK_JSON_ERR_PARSE, "expected a digit at byte %lld",
                  (long long)json_offset(s, base));
        return NULL;
    }
    if (s->p < s->end && *s->p == '.') {
        s->p++;
        if (s->p >= s->end || *s->p < '0' || *s->p > '9') {
            json_fail(RASK_JSON_ERR_PARSE, "expected a digit after '.' at byte %lld",
                      (long long)json_offset(s, base));
            return NULL;
        }
        while (s->p < s->end && *s->p >= '0' && *s->p <= '9') s->p++;
    }
    if (s->p < s->end && (*s->p == 'e' || *s->p == 'E')) {
        s->p++;
        if (s->p < s->end && (*s->p == '+' || *s->p == '-')) s->p++;
        if (s->p >= s->end || *s->p < '0' || *s->p > '9') {
            json_fail(RASK_JSON_ERR_PARSE, "expected a digit in the exponent at byte %lld",
                      (long long)json_offset(s, base));
            return NULL;
        }
        while (s->p < s->end && *s->p >= '0' && *s->p <= '9') s->p++;
    }

    // strtod needs a terminator; the input slice may not have one.
    char tmp[64];
    int64_t n = (int64_t)(s->p - start);
    RaskJsonVal *v = json_val_new(RASK_JSON_NUM);
    if (n < (int64_t)sizeof(tmp)) {
        memcpy(tmp, start, (size_t)n);
        tmp[n] = '\0';
        v->as.num = strtod(tmp, NULL);
    } else {
        char *heap = (char *)rask_alloc(n + 1);
        memcpy(heap, start, (size_t)n);
        heap[n] = '\0';
        v->as.num = strtod(heap, NULL);
        rask_free(heap);
    }
    return v;
}

static int json_lit(JsonScan *s, const char *lit, int64_t n) {
    if (s->end - s->p < n) return 0;
    return memcmp(s->p, lit, (size_t)n) == 0;
}

static RaskJsonVal *json_scan_value(JsonScan *s, const char *base) {
    if (++s->depth > JSON_MAX_DEPTH) {
        json_fail(RASK_JSON_ERR_PARSE, "nesting deeper than %d levels", JSON_MAX_DEPTH);
        s->depth--;
        return NULL;
    }
    json_skip_space(s);
    RaskJsonVal *v = NULL;
    if (s->p >= s->end) {
        json_fail(RASK_JSON_ERR_PARSE, "unexpected end of input");
        s->depth--;
        return NULL;
    }
    char c = *s->p;
    if (c == '{') {
        v = json_scan_object(s, base);
    } else if (c == '[') {
        v = json_scan_array(s, base);
    } else if (c == '"') {
        v = json_val_new(RASK_JSON_STR);
        if (!json_scan_string(s, base, &v->as.str)) {
            rask_free(v);
            v = NULL;
        }
    } else if (json_lit(s, "true", 4)) {
        v = json_val_new(RASK_JSON_BOOL);
        v->as.b = 1;
        s->p += 4;
    } else if (json_lit(s, "false", 5)) {
        v = json_val_new(RASK_JSON_BOOL);
        v->as.b = 0;
        s->p += 5;
    } else if (json_lit(s, "null", 4)) {
        v = json_val_new(RASK_JSON_NULL);
        s->p += 4;
    } else if (c == '-' || (c >= '0' && c <= '9')) {
        v = json_scan_number(s, base);
    } else {
        json_fail(RASK_JSON_ERR_PARSE, "unexpected character '%c' at byte %lld",
                  c, (long long)json_offset(s, base));
    }
    s->depth--;
    return v;
}

RaskJsonVal *rask_json_tree_parse(const RaskStr *s) {
    json_err_kind = RASK_JSON_OK;
    json_err_msg[0] = '\0';

    const char *base = rask_string_ptr(s);
    int64_t len = rask_string_len(s);
    JsonScan scan = {base, base + len, 0};

    RaskJsonVal *v = json_scan_value(&scan, base);
    if (!v) return NULL;
    json_skip_space(&scan);
    if (scan.p != scan.end) {
        json_fail(RASK_JSON_ERR_PARSE, "trailing content after the JSON value at byte %lld",
                  (long long)json_offset(&scan, base));
        rask_json_tree_free(v);
        return NULL;
    }
    return v;
}

// ─── Shapes ───────────────────────────────────────────────────────

struct RaskJsonField {
    char *key;
    int64_t offset;
    RaskJsonShape *shape;
    int64_t flags;
};

struct RaskJsonShape {
    int64_t kind;
    int64_t slot;                 // struct byte size, or element/value slot width
    int      is_static;           // primitive singletons are never freed
    RaskJsonShape *elem;          // Vec element / Map value / T? payload
    struct RaskJsonField *fields; // struct fields
    int32_t nfields;
    int32_t cap;
};

// One shared node per primitive kind — a decode call allocates only for the
// aggregate parts of its type.
static RaskJsonShape json_prim_shapes[RASK_JSHAPE_PRIM_COUNT];
static int json_prims_ready = 0;

static int64_t json_prim_width(int64_t kind) {
    switch (kind) {
        case RASK_JSHAPE_BOOL: case RASK_JSHAPE_I8: case RASK_JSHAPE_U8:  return 1;
        case RASK_JSHAPE_I16: case RASK_JSHAPE_U16:                       return 2;
        case RASK_JSHAPE_I32: case RASK_JSHAPE_U32: case RASK_JSHAPE_F32: return 4;
        case RASK_JSHAPE_STRING:                                          return 16;
        default:                                                          return 8;
    }
}

RaskJsonShape *rask_json_shape_prim(int64_t kind) {
    if (kind < 0 || kind >= RASK_JSHAPE_PRIM_COUNT) kind = RASK_JSHAPE_I64;
    if (!json_prims_ready) {
        for (int i = 0; i < RASK_JSHAPE_PRIM_COUNT; i++) {
            json_prim_shapes[i].kind = i;
            json_prim_shapes[i].slot = json_prim_width(i);
            json_prim_shapes[i].is_static = 1;
        }
        json_prims_ready = 1;
    }
    return &json_prim_shapes[kind];
}

static RaskJsonShape *json_shape_new(int64_t kind, int64_t slot) {
    RaskJsonShape *s = (RaskJsonShape *)rask_alloc((int64_t)sizeof(RaskJsonShape));
    memset(s, 0, sizeof(RaskJsonShape));
    s->kind = kind;
    s->slot = slot;
    return s;
}

RaskJsonShape *rask_json_shape_struct(int64_t size) {
    return json_shape_new(RASK_JSHAPE_STRUCT, size);
}

RaskJsonShape *rask_json_shape_vec(RaskJsonShape *elem, int64_t elem_slot) {
    RaskJsonShape *s = json_shape_new(RASK_JSHAPE_VEC, elem_slot);
    s->elem = elem;
    return s;
}

RaskJsonShape *rask_json_shape_map(RaskJsonShape *val, int64_t val_slot) {
    RaskJsonShape *s = json_shape_new(RASK_JSHAPE_MAP, val_slot);
    s->elem = val;
    return s;
}

RaskJsonShape *rask_json_shape_opt(RaskJsonShape *inner, int64_t total_size) {
    RaskJsonShape *s = json_shape_new(RASK_JSHAPE_OPT, total_size);
    s->elem = inner;
    return s;
}

void rask_json_shape_field(RaskJsonShape *s, const RaskStr *key, int64_t offset,
                           RaskJsonShape *fs, int64_t flags) {
    if (!s) return;
    if (s->nfields == s->cap) {
        int32_t old = s->cap;
        int32_t cap = old ? old * 2 : 8;
        s->fields = (struct RaskJsonField *)rask_realloc(
            s->fields,
            (int64_t)old * (int64_t)sizeof(struct RaskJsonField),
            (int64_t)cap * (int64_t)sizeof(struct RaskJsonField));
        s->cap = cap;
    }
    int64_t klen = rask_string_len(key);
    char *copy = (char *)rask_alloc(klen + 1);
    memcpy(copy, rask_string_ptr(key), (size_t)klen);
    copy[klen] = '\0';
    struct RaskJsonField *f = &s->fields[s->nfields++];
    f->key = copy;
    f->offset = offset;
    f->shape = fs;
    f->flags = flags;
}

void rask_json_shape_free(RaskJsonShape *s) {
    if (!s || s->is_static) return;
    for (int32_t i = 0; i < s->nfields; i++) {
        rask_free(s->fields[i].key);
        rask_json_shape_free(s->fields[i].shape);
    }
    rask_free(s->fields);
    rask_json_shape_free(s->elem);
    rask_free(s);
}

// ─── Decoding ─────────────────────────────────────────────────────

static const char *json_kind_name(uint8_t kind) {
    switch (kind) {
        case RASK_JSON_NULL: return "null";
        case RASK_JSON_BOOL: return "a boolean";
        case RASK_JSON_NUM:  return "a number";
        case RASK_JSON_STR:  return "a string";
        case RASK_JSON_ARR:  return "an array";
        default:             return "an object";
    }
}

static const char *json_shape_name(int64_t kind) {
    switch (kind) {
        case RASK_JSHAPE_BOOL:   return "bool";
        case RASK_JSHAPE_STRING: return "string";
        case RASK_JSHAPE_F32:    return "f32";
        case RASK_JSHAPE_F64:    return "f64";
        case RASK_JSHAPE_VEC:    return "a list";
        case RASK_JSHAPE_MAP:    return "a map";
        case RASK_JSHAPE_STRUCT: return "a struct";
        default:                 return "an integer";
    }
}

// Writes an integer into a `width`-byte slot. Vec/Map slots are always a full
// word and readers load a whole word out of them, so a narrow value is
// sign-extended into the slot rather than left with garbage above it.
static void json_store_int(void *dst, int64_t width, int64_t v) {
    switch (width) {
        case 1: { int8_t  x = (int8_t)v;  memcpy(dst, &x, 1); break; }
        case 2: { int16_t x = (int16_t)v; memcpy(dst, &x, 2); break; }
        case 4: { int32_t x = (int32_t)v; memcpy(dst, &x, 4); break; }
        default: memcpy(dst, &v, 8); break;
    }
}

static int json_decode_into_slot(void *dst, RaskJsonShape *shape,
                                 RaskJsonVal *v, const char *path);

// A `T?` slot is [tag:8][payload]. tag 0 = Some, 1 = none (rask_mono::abi).
static int json_decode_opt(void *dst, RaskJsonShape *shape, RaskJsonVal *v,
                           const char *path) {
    int64_t tag;
    if (!v || v->kind == RASK_JSON_NULL) {
        tag = 1;
        memcpy(dst, &tag, 8);
        memset((char *)dst + RASK_OPTION_PAYLOAD_OFFSET, 0,
               (size_t)(shape->slot > RASK_OPTION_PAYLOAD_OFFSET
                        ? shape->slot - RASK_OPTION_PAYLOAD_OFFSET : 0));
        return 1;
    }
    tag = 0;
    memcpy(dst, &tag, 8);
    return json_decode_into_slot((char *)dst + RASK_OPTION_PAYLOAD_OFFSET,
                                 shape->elem, v, path);
}

static int json_decode_struct(void *dst, RaskJsonShape *shape, RaskJsonVal *v,
                              const char *path) {
    if (v->kind != RASK_JSON_OBJ) {
        json_fail(RASK_JSON_ERR_TYPE, "%s should be an object, found %s",
                  path[0] ? path : "the value", json_kind_name(v->kind));
        return 0;
    }
    for (int32_t i = 0; i < shape->nfields; i++) {
        struct RaskJsonField *f = &shape->fields[i];
        RaskJsonVal *member = NULL;
        for (int32_t j = 0; j < v->as.obj.len; j++) {
            const char *k = rask_string_ptr(&v->as.obj.keys[j]);
            int64_t klen = rask_string_len(&v->as.obj.keys[j]);
            if ((int64_t)strlen(f->key) == klen && memcmp(k, f->key, (size_t)klen) == 0) {
                member = v->as.obj.vals[j];
                break;
            }
        }
        char child[256];
        snprintf(child, sizeof(child), "%s%s%s", path, path[0] ? "." : "field ", f->key);

        if (!member) {
            // A `T?` field takes `none`; anything else has to be there (J9).
            if (f->shape->kind == RASK_JSHAPE_OPT) {
                if (!json_decode_opt((char *)dst + f->offset, f->shape, NULL, child)) return 0;
                continue;
            }
            // A field with `@default` already holds its value — the call site
            // wrote it before handing the destination over.
            if (f->flags & RASK_JFIELD_OPTIONAL) continue;
            json_fail(RASK_JSON_ERR_MISSING, "field \"%s\" not found in the JSON object", f->key);
            return 0;
        }
        if (!json_decode_into_slot((char *)dst + f->offset, f->shape, member, child)) {
            return 0;
        }
    }
    return 1;
}

static int json_decode_vec(void *dst, RaskJsonShape *shape, RaskJsonVal *v,
                           const char *path) {
    if (v->kind != RASK_JSON_ARR) {
        json_fail(RASK_JSON_ERR_TYPE, "%s should be a list, found %s",
                  path[0] ? path : "the value", json_kind_name(v->kind));
        return 0;
    }
    int64_t slot = shape->slot > 0 ? shape->slot : 8;
    RaskVec *vec = rask_vec_with_capacity(slot, v->as.arr.len);
    char stack_slot[64];
    char *cell = slot <= (int64_t)sizeof(stack_slot)
        ? stack_slot
        : (char *)rask_alloc(slot);
    for (int32_t i = 0; i < v->as.arr.len; i++) {
        memset(cell, 0, (size_t)slot);
        char child[256];
        snprintf(child, sizeof(child), "%s[%d]", path[0] ? path : "the list", i);
        if (!json_decode_into_slot(cell, shape->elem, v->as.arr.items[i], child)) {
            if (cell != stack_slot) rask_free(cell);
            rask_vec_free(vec);
            return 0;
        }
        rask_vec_push(vec, cell);
    }
    if (cell != stack_slot) rask_free(cell);
    memcpy(dst, &vec, sizeof(RaskVec *));
    return 1;
}

static int json_decode_map(void *dst, RaskJsonShape *shape, RaskJsonVal *v,
                           const char *path) {
    if (v->kind != RASK_JSON_OBJ) {
        json_fail(RASK_JSON_ERR_TYPE, "%s should be an object, found %s",
                  path[0] ? path : "the value", json_kind_name(v->kind));
        return 0;
    }
    int64_t slot = shape->slot > 0 ? shape->slot : 8;
    RaskMap *map = rask_map_new_string_keys(16, slot);
    char stack_slot[64];
    char *cell = slot <= (int64_t)sizeof(stack_slot)
        ? stack_slot
        : (char *)rask_alloc(slot);
    for (int32_t i = 0; i < v->as.obj.len; i++) {
        memset(cell, 0, (size_t)slot);
        char child[256];
        snprintf(child, sizeof(child), "%s%s%s", path, path[0] ? "." : "key ",
                 rask_string_ptr(&v->as.obj.keys[i]));
        if (!json_decode_into_slot(cell, shape->elem, v->as.obj.vals[i], child)) {
            if (cell != stack_slot) rask_free(cell);
            rask_map_free(map);
            return 0;
        }
        RaskStr key = v->as.obj.keys[i];
        rask_string_clone(&key);
        rask_map_insert(map, &key, cell);
    }
    if (cell != stack_slot) rask_free(cell);
    memcpy(dst, &map, sizeof(RaskMap *));
    return 1;
}

static int json_decode_into_slot(void *dst, RaskJsonShape *shape,
                                 RaskJsonVal *v, const char *path) {
    if (!shape) return 1;
    if (shape->kind == RASK_JSHAPE_OPT) {
        return json_decode_opt(dst, shape, v, path);
    }
    if (!v) {
        json_fail(RASK_JSON_ERR_MISSING, "%s is missing", path[0] ? path : "the value");
        return 0;
    }
    switch (shape->kind) {
        case RASK_JSHAPE_STRUCT: return json_decode_struct(dst, shape, v, path);
        case RASK_JSHAPE_VEC:    return json_decode_vec(dst, shape, v, path);
        case RASK_JSHAPE_MAP:    return json_decode_map(dst, shape, v, path);
        case RASK_JSHAPE_STRING: {
            if (v->kind != RASK_JSON_STR) break;
            RaskStr s = v->as.str;
            rask_string_clone(&s);
            memcpy(dst, &s, sizeof(RaskStr));
            return 1;
        }
        case RASK_JSHAPE_BOOL: {
            if (v->kind != RASK_JSON_BOOL) break;
            json_store_int(dst, shape->slot, v->as.b ? 1 : 0);
            return 1;
        }
        case RASK_JSHAPE_F32: {
            if (v->kind != RASK_JSON_NUM) break;
            float f = (float)v->as.num;
            memcpy(dst, &f, sizeof(float));
            return 1;
        }
        case RASK_JSHAPE_F64: {
            if (v->kind != RASK_JSON_NUM) break;
            double d = v->as.num;
            memcpy(dst, &d, sizeof(double));
            return 1;
        }
        default: {
            if (v->kind != RASK_JSON_NUM) break;
            json_store_int(dst, shape->slot, (int64_t)v->as.num);
            return 1;
        }
    }
    json_fail(RASK_JSON_ERR_TYPE, "%s should be %s, found %s",
              path[0] ? path : "the value", json_shape_name(shape->kind),
              json_kind_name(v->kind));
    return 0;
}

int64_t rask_json_decode_into(void *dst, RaskJsonShape *shape, const RaskStr *input) {
    json_err_kind = RASK_JSON_OK;
    json_err_msg[0] = '\0';

    RaskJsonVal *tree = rask_json_tree_parse(input);
    if (!tree) return json_err_kind ? json_err_kind : RASK_JSON_ERR_PARSE;

    // A failed decode leaves the destination untouched from the caller's point
    // of view — the Err branch is what gets read, and the slot is scratch.
    int ok = json_decode_into_slot(dst, shape, tree, "");
    rask_json_tree_free(tree);
    if (ok) {
        json_err_kind = RASK_JSON_OK;
        return RASK_JSON_OK;
    }
    return json_err_kind ? json_err_kind : RASK_JSON_ERR_TYPE;
}

// Zero a decode destination before the shape calls run, so an early Err leaves
// no stale words behind for a later read to trip over.
void rask_json_decode_zero(void *dst, int64_t size) {
    if (dst && size > 0) memset(dst, 0, (size_t)size);
}

// ─── Shape-driven encoding ────────────────────────────────────────
//
// The mirror of the decoder: given a value and the shape describing it, write
// the JSON out. `json.encode` unrolls scalars and structs into json_buf_* calls
// at the call site, but a Map has to be walked at runtime — there's no way to
// unroll an unknown number of keys — so its field routes through here.

static void json_write_escaped(JsonStrBuf *b, const char *s, int64_t len) {
    jsb_push(b, "\"", 1);
    for (int64_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        switch (c) {
            case '"':  jsb_push(b, "\\\"", 2); break;
            case '\\': jsb_push(b, "\\\\", 2); break;
            case '\n': jsb_push(b, "\\n", 2); break;
            case '\r': jsb_push(b, "\\r", 2); break;
            case '\t': jsb_push(b, "\\t", 2); break;
            case '\b': jsb_push(b, "\\b", 2); break;
            case '\f': jsb_push(b, "\\f", 2); break;
            default:
                if (c < 0x20) {
                    char esc[7];
                    snprintf(esc, sizeof(esc), "\\u%04x", c);
                    jsb_push(b, esc, 6);
                } else {
                    jsb_push(b, (const char *)&c, 1);
                }
        }
    }
    jsb_push(b, "\"", 1);
}

static int64_t json_load_int(const void *src, int64_t width, int is_unsigned) {
    switch (width) {
        case 1: {
            if (is_unsigned) { uint8_t v; memcpy(&v, src, 1); return (int64_t)v; }
            int8_t v; memcpy(&v, src, 1); return (int64_t)v;
        }
        case 2: {
            if (is_unsigned) { uint16_t v; memcpy(&v, src, 2); return (int64_t)v; }
            int16_t v; memcpy(&v, src, 2); return (int64_t)v;
        }
        case 4: {
            if (is_unsigned) { uint32_t v; memcpy(&v, src, 4); return (int64_t)v; }
            int32_t v; memcpy(&v, src, 4); return (int64_t)v;
        }
        default: {
            int64_t v; memcpy(&v, src, 8); return v;
        }
    }
}

static int json_shape_is_unsigned(int64_t kind) {
    return kind == RASK_JSHAPE_U8 || kind == RASK_JSHAPE_U16
        || kind == RASK_JSHAPE_U32 || kind == RASK_JSHAPE_U64;
}

static void json_write_shaped(JsonStrBuf *b, const void *src, const RaskJsonShape *shape);

static void json_write_struct(JsonStrBuf *b, const void *src, const RaskJsonShape *shape) {
    jsb_push(b, "{", 1);
    for (int32_t i = 0; i < shape->nfields; i++) {
        if (i) jsb_push(b, ",", 1);
        const struct RaskJsonField *f = &shape->fields[i];
        json_write_escaped(b, f->key, (int64_t)strlen(f->key));
        jsb_push(b, ":", 1);
        json_write_shaped(b, (const char *)src + f->offset, f->shape);
    }
    jsb_push(b, "}", 1);
}

static void json_write_shaped(JsonStrBuf *b, const void *src, const RaskJsonShape *shape) {
    if (!shape || !src) {
        jsb_push(b, "null", 4);
        return;
    }
    switch (shape->kind) {
        case RASK_JSHAPE_OPT: {
            int64_t tag;
            memcpy(&tag, src, 8);
            if (tag != 0) {
                jsb_push(b, "null", 4);
                return;
            }
            json_write_shaped(b, (const char *)src + RASK_OPTION_PAYLOAD_OFFSET, shape->elem);
            return;
        }
        case RASK_JSHAPE_STRUCT:
            json_write_struct(b, src, shape);
            return;
        case RASK_JSHAPE_VEC: {
            RaskVec *vec;
            memcpy(&vec, src, sizeof(RaskVec *));
            jsb_push(b, "[", 1);
            int64_t n = vec ? rask_vec_len(vec) : 0;
            for (int64_t i = 0; i < n; i++) {
                if (i) jsb_push(b, ",", 1);
                json_write_shaped(b, rask_vec_get_unchecked(vec, i), shape->elem);
            }
            jsb_push(b, "]", 1);
            return;
        }
        case RASK_JSHAPE_MAP: {
            RaskMap *map;
            memcpy(&map, src, sizeof(RaskMap *));
            jsb_push(b, "{", 1);
            if (map) {
                // keys() and values() walk the same slots in the same order, so
                // index i lines up across the two.
                RaskVec *keys = rask_map_keys(map);
                RaskVec *vals = rask_map_values(map);
                int64_t n = rask_vec_len(keys);
                for (int64_t i = 0; i < n; i++) {
                    if (i) jsb_push(b, ",", 1);
                    const RaskStr *k = (const RaskStr *)rask_vec_get_unchecked(keys, i);
                    json_write_escaped(b, rask_string_ptr(k), rask_string_len(k));
                    jsb_push(b, ":", 1);
                    json_write_shaped(b, rask_vec_get_unchecked(vals, i), shape->elem);
                }
                rask_vec_free(keys);
                rask_vec_free(vals);
            }
            jsb_push(b, "}", 1);
            return;
        }
        case RASK_JSHAPE_STRING: {
            const RaskStr *s = (const RaskStr *)src;
            json_write_escaped(b, rask_string_ptr(s), rask_string_len(s));
            return;
        }
        case RASK_JSHAPE_BOOL: {
            int64_t v = json_load_int(src, shape->slot, 1);
            if (v) jsb_push(b, "true", 4);
            else jsb_push(b, "false", 5);
            return;
        }
        case RASK_JSHAPE_F32: {
            float f;
            memcpy(&f, src, sizeof(float));
            char num[RASK_F64_BUF_SIZE];
            rask_fmt_double(num, sizeof(num), (double)f);
            jsb_push(b, num, (int64_t)strlen(num));
            return;
        }
        case RASK_JSHAPE_F64: {
            double d;
            memcpy(&d, src, sizeof(double));
            char num[RASK_F64_BUF_SIZE];
            rask_fmt_double(num, sizeof(num), d);
            jsb_push(b, num, (int64_t)strlen(num));
            return;
        }
        default: {
            int unsig = json_shape_is_unsigned(shape->kind);
            int64_t v = json_load_int(src, shape->slot, unsig);
            char num[32];
            int n = unsig
                ? snprintf(num, sizeof(num), "%llu", (unsigned long long)v)
                : snprintf(num, sizeof(num), "%lld", (long long)v);
            jsb_push(b, num, n);
            return;
        }
    }
}

void rask_json_encode_shaped(RaskStr *out, const void *src, RaskJsonShape *shape) {
    JsonStrBuf b = {NULL, 0, 0};
    json_write_shaped(&b, src, shape);
    rask_string_from_bytes(out, b.data ? b.data : "null", b.len ? b.len : 4);
    rask_free(b.data);
}
