// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask runtime — print functions, I/O, resource tracking, and entry point.
// Collection and string implementations live in vec.c, map.c, pool.c, string.c.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>

// Forward declaration — user's main function, exported from the Rask module as rask_main
extern void rask_main(void);

// ─── Print functions ──────────────────────────────────────────────

void rask_print_i64(int64_t val) {
    printf("%lld", (long long)val);
}

void rask_print_bool(int8_t val) {
    printf("%s", val ? "true" : "false");
}

void rask_print_f64(double val) {
    printf("%g", val);
}

void rask_print_f32(float val) {
    printf("%g", (double)val);
}

void rask_print_char(int32_t codepoint) {
    if (codepoint < 0x80) {
        putchar(codepoint);
    } else if (codepoint < 0x800) {
        putchar(0xC0 | (codepoint >> 6));
        putchar(0x80 | (codepoint & 0x3F));
    } else if (codepoint < 0x10000) {
        putchar(0xE0 | (codepoint >> 12));
        putchar(0x80 | ((codepoint >> 6) & 0x3F));
        putchar(0x80 | (codepoint & 0x3F));
    } else {
        putchar(0xF0 | (codepoint >> 18));
        putchar(0x80 | ((codepoint >> 12) & 0x3F));
        putchar(0x80 | ((codepoint >> 6) & 0x3F));
        putchar(0x80 | (codepoint & 0x3F));
    }
}

void rask_print_u64(uint64_t val) {
    printf("%llu", (unsigned long long)val);
}

void rask_print_string(const RaskString *s) {
    if (s) {
        fputs(rask_string_ptr(s), stdout);
    }
}

void rask_print_newline(void) {
    putchar('\n');
}

// ─── Runtime support ──────────────────────────────────────────────

void rask_exit(int64_t code) {
    exit((int)code);
}

void rask_panic_unwrap(void) {
    fprintf(stderr, "panic: called unwrap on None/Err value\n");
    abort();
}

void rask_assert_fail(void) {
    fprintf(stderr, "panic: assertion failed\n");
    abort();
}

// ─── I/O primitives ──────────────────────────────────────────────
// Thin wrappers around POSIX syscalls. Return values match POSIX
// conventions: bytes transferred on success, -1 on error.

int64_t rask_io_open(const char *path, int64_t flags, int64_t mode) {
    return (int64_t)open(path, (int)flags, (mode_t)mode);
}

int64_t rask_io_close(int64_t fd) {
    return (int64_t)close((int)fd);
}

int64_t rask_io_read(int64_t fd, void *buf, int64_t len) {
    return (int64_t)read((int)fd, buf, (size_t)len);
}

int64_t rask_io_write(int64_t fd, const void *buf, int64_t len) {
    return (int64_t)write((int)fd, buf, (size_t)len);
}

// ─── Resource tracking ───────────────────────────────────────────
// Runtime enforcement for must-consume (linear) types.
// Simple fixed-size tracker — production would use a growable array.

#define RASK_MAX_RESOURCES 1024

struct rask_resource_entry {
    int64_t id;
    int64_t scope_depth;
    int     active;
};

static struct rask_resource_entry rask_resources[RASK_MAX_RESOURCES];
static int64_t rask_next_resource_id = 1;

int64_t rask_resource_register(int64_t scope_depth) {
    int64_t id = rask_next_resource_id++;
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (!rask_resources[i].active) {
            rask_resources[i].id = id;
            rask_resources[i].scope_depth = scope_depth;
            rask_resources[i].active = 1;
            return id;
        }
    }
    fprintf(stderr, "panic: resource tracker overflow\n");
    abort();
}

void rask_resource_consume(int64_t resource_id) {
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (rask_resources[i].active && rask_resources[i].id == resource_id) {
            rask_resources[i].active = 0;
            return;
        }
    }
    fprintf(stderr, "panic: consuming unknown resource %lld\n", (long long)resource_id);
    abort();
}

void rask_resource_scope_check(int64_t scope_depth) {
    for (int i = 0; i < RASK_MAX_RESOURCES; i++) {
        if (rask_resources[i].active && rask_resources[i].scope_depth == scope_depth) {
            fprintf(stderr, "panic: unconsumed resource at scope depth %lld\n",
                    (long long)scope_depth);
            abort();
        }
    }
}

// ─── Clone (shallow copy for i64-sized values) ───────────────────
// Strings and collection handles are pointer-sized; clone is identity.
int64_t rask_clone(int64_t value) { return value; }

// ─── CLI module ───────────────────────────────────────────────────
// cli.args() → Vec of RaskString* pointers.

RaskVec *rask_cli_args(void) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    int64_t count = rask_args_count();
    for (int64_t i = 0; i < count; i++) {
        const char *arg = rask_args_get(i);
        RaskString *s = rask_string_from(arg);
        rask_vec_push(v, &s);
    }
    return v;
}

// ─── FS module ────────────────────────────────────────────────────

RaskVec *rask_fs_read_lines(const RaskString *path) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    const char *p = path ? rask_string_ptr(path) : "";

    FILE *f = fopen(p, "r");
    if (!f) return v;

    char buf[4096];
    while (fgets(buf, sizeof(buf), f)) {
        size_t len = strlen(buf);
        if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
        if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';

        RaskString *line = rask_string_from_bytes(buf, (int64_t)len);
        rask_vec_push(v, &line);
    }

    fclose(f);
    return v;
}

// ─── IO module ────────────────────────────────────────────────────

RaskString *rask_io_read_line(void) {
    char buf[4096];
    if (!fgets(buf, sizeof(buf), stdin)) {
        return rask_string_new();
    }
    size_t len = strlen(buf);
    if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
    if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';
    return rask_string_from_bytes(buf, (int64_t)len);
}

// ─── More FS module ───────────────────────────────────────────────

RaskString *rask_fs_read_file(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    FILE *f = fopen(p, "rb");
    if (!f) return rask_string_new();
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = (char *)malloc((size_t)size + 1);
    if (!buf) { fclose(f); return rask_string_new(); }
    fread(buf, 1, (size_t)size, f);
    buf[size] = '\0';
    fclose(f);
    RaskString *s = rask_string_from_bytes(buf, (int64_t)size);
    free(buf);
    return s;
}

void rask_fs_write_file(const RaskString *path, const RaskString *content) {
    const char *p = path ? rask_string_ptr(path) : "";
    const char *c = content ? rask_string_ptr(content) : "";
    int64_t clen = content ? rask_string_len(content) : 0;
    FILE *f = fopen(p, "wb");
    if (!f) return;
    fwrite(c, 1, (size_t)clen, f);
    fclose(f);
}

int8_t rask_fs_exists(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    FILE *f = fopen(p, "r");
    if (f) { fclose(f); return 1; }
    return 0;
}

// ─── More FS module ───────────────────────────────────────────────

int64_t rask_fs_open(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    FILE *f = fopen(p, "r");
    return (int64_t)(uintptr_t)f;
}

int64_t rask_fs_create(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    FILE *f = fopen(p, "w");
    return (int64_t)(uintptr_t)f;
}

RaskString *rask_fs_canonicalize(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    char resolved[4096];
    char *r = realpath(p, resolved);
    if (!r) return rask_string_new();
    return rask_string_from(resolved);
}

int64_t rask_fs_copy(const RaskString *from, const RaskString *to) {
    const char *src = from ? rask_string_ptr(from) : "";
    const char *dst = to ? rask_string_ptr(to) : "";
    FILE *in = fopen(src, "rb");
    if (!in) return -1;
    FILE *out = fopen(dst, "wb");
    if (!out) { fclose(in); return -1; }
    char buf[4096];
    int64_t total = 0;
    size_t n;
    while ((n = fread(buf, 1, sizeof(buf), in)) > 0) {
        fwrite(buf, 1, n, out);
        total += (int64_t)n;
    }
    fclose(in);
    fclose(out);
    return total;
}

void rask_fs_rename(const RaskString *from, const RaskString *to) {
    const char *s = from ? rask_string_ptr(from) : "";
    const char *d = to ? rask_string_ptr(to) : "";
    rename(s, d);
}

void rask_fs_remove(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    remove(p);
}

#include <sys/stat.h>

void rask_fs_create_dir(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    mkdir(p, 0755);
}

void rask_fs_create_dir_all(const RaskString *path) {
    const char *p = path ? rask_string_ptr(path) : "";
    char tmp[4096];
    snprintf(tmp, sizeof(tmp), "%s", p);
    for (char *c = tmp + 1; *c; c++) {
        if (*c == '/') {
            *c = '\0';
            mkdir(tmp, 0755);
            *c = '/';
        }
    }
    mkdir(tmp, 0755);
}

void rask_fs_append_file(const RaskString *path, const RaskString *content) {
    const char *p = path ? rask_string_ptr(path) : "";
    const char *c = content ? rask_string_ptr(content) : "";
    int64_t clen = content ? rask_string_len(content) : 0;
    FILE *f = fopen(p, "ab");
    if (!f) return;
    fwrite(c, 1, (size_t)clen, f);
    fclose(f);
}

// ─── Net module ───────────────────────────────────────────────────

#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>

int64_t rask_net_tcp_listen(const RaskString *addr) {
    const char *a = addr ? rask_string_ptr(addr) : "0.0.0.0:0";

    // Parse "host:port"
    char host[256] = "0.0.0.0";
    int port = 0;
    const char *colon = strrchr(a, ':');
    if (colon) {
        size_t hlen = (size_t)(colon - a);
        if (hlen > 0 && hlen < sizeof(host)) {
            memcpy(host, a, hlen);
            host[hlen] = '\0';
        }
        port = atoi(colon + 1);
    }

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;

    int opt = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in sa;
    memset(&sa, 0, sizeof(sa));
    sa.sin_family = AF_INET;
    sa.sin_port = htons((uint16_t)port);
    inet_pton(AF_INET, host, &sa.sin_addr);

    if (bind(fd, (struct sockaddr *)&sa, sizeof(sa)) < 0) {
        close(fd);
        return -1;
    }
    if (listen(fd, 128) < 0) {
        close(fd);
        return -1;
    }
    return (int64_t)fd;
}

// ─── JSON module ──────────────────────────────────────────────────

// Growable JSON buffer
struct RaskJsonBuf {
    char *data;
    int64_t len;
    int64_t cap;
    int field_count;
};

static void json_buf_grow(struct RaskJsonBuf *b, int64_t needed) {
    if (b->len + needed <= b->cap) return;
    int64_t new_cap = b->cap * 2;
    if (new_cap < b->len + needed) new_cap = b->len + needed;
    b->data = (char *)realloc(b->data, (size_t)new_cap);
    b->cap = new_cap;
}

static void json_buf_append(struct RaskJsonBuf *b, const char *s, int64_t len) {
    json_buf_grow(b, len);
    memcpy(b->data + b->len, s, (size_t)len);
    b->len += len;
}

static void json_buf_append_cstr(struct RaskJsonBuf *b, const char *s) {
    json_buf_append(b, s, (int64_t)strlen(s));
}

static void json_buf_append_escaped(struct RaskJsonBuf *b, const char *s, int64_t len) {
    json_buf_append(b, "\"", 1);
    for (int64_t i = 0; i < len; i++) {
        char c = s[i];
        switch (c) {
            case '"':  json_buf_append(b, "\\\"", 2); break;
            case '\\': json_buf_append(b, "\\\\", 2); break;
            case '\n': json_buf_append(b, "\\n", 2); break;
            case '\r': json_buf_append(b, "\\r", 2); break;
            case '\t': json_buf_append(b, "\\t", 2); break;
            default:   json_buf_append(b, &c, 1); break;
        }
    }
    json_buf_append(b, "\"", 1);
}

RaskJsonBuf *rask_json_buf_new(void) {
    RaskJsonBuf *b = (RaskJsonBuf *)malloc(sizeof(RaskJsonBuf));
    b->cap = 256;
    b->data = (char *)malloc((size_t)b->cap);
    b->len = 0;
    b->field_count = 0;
    json_buf_append_cstr(b, "{");
    return b;
}

void rask_json_buf_add_string(RaskJsonBuf *buf, const char *key, const RaskString *val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, key, (int64_t)strlen(key));
    json_buf_append_cstr(buf, ":");
    if (val) {
        json_buf_append_escaped(buf, rask_string_ptr(val), rask_string_len(val));
    } else {
        json_buf_append_cstr(buf, "null");
    }
    buf->field_count++;
}

void rask_json_buf_add_i64(RaskJsonBuf *buf, const char *key, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, key, (int64_t)strlen(key));
    char num[32];
    snprintf(num, sizeof(num), ":%lld", (long long)val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_add_f64(RaskJsonBuf *buf, const char *key, double val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, key, (int64_t)strlen(key));
    char num[64];
    snprintf(num, sizeof(num), ":%g", val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_add_bool(RaskJsonBuf *buf, const char *key, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, key, (int64_t)strlen(key));
    json_buf_append_cstr(buf, val ? ":true" : ":false");
    buf->field_count++;
}

RaskString *rask_json_buf_finish(RaskJsonBuf *buf) {
    json_buf_append_cstr(buf, "}");
    RaskString *s = rask_string_from_bytes(buf->data, buf->len);
    free(buf->data);
    free(buf);
    return s;
}

RaskString *rask_json_encode_string(const RaskString *s) {
    struct RaskJsonBuf b;
    b.cap = 256;
    b.data = (char *)malloc((size_t)b.cap);
    b.len = 0;
    b.field_count = 0;
    if (s) {
        json_buf_append_escaped(&b, rask_string_ptr(s), rask_string_len(s));
    } else {
        json_buf_append_cstr(&b, "null");
    }
    RaskString *result = rask_string_from_bytes(b.data, b.len);
    free(b.data);
    return result;
}

RaskString *rask_json_encode_i64(int64_t val) {
    char buf[32];
    int len = snprintf(buf, sizeof(buf), "%lld", (long long)val);
    return rask_string_from_bytes(buf, (int64_t)len);
}

// ─── JSON decode ──────────────────────────────────────────────────

#define JSON_MAX_FIELDS 64

struct RaskJsonField {
    char key[128];
    enum { JSON_STRING, JSON_NUMBER, JSON_BOOL } type;
    union {
        RaskString *str_val;
        double num_val;
        int8_t bool_val;
    };
};

struct RaskJsonObj {
    struct RaskJsonField fields[JSON_MAX_FIELDS];
    int count;
};

static void json_skip_ws(const char **p) {
    while (**p == ' ' || **p == '\t' || **p == '\n' || **p == '\r') (*p)++;
}

static RaskString *json_parse_string(const char **p) {
    if (**p != '"') return rask_string_new();
    (*p)++;
    RaskString *s = rask_string_new();
    while (**p && **p != '"') {
        if (**p == '\\' && *(*p + 1)) {
            char c = *(*p + 1);
            switch (c) {
                case '"': case '\\': case '/':
                    rask_string_push_byte(s, (uint8_t)c); break;
                case 'n': rask_string_push_byte(s, '\n'); break;
                case 't': rask_string_push_byte(s, '\t'); break;
                case 'r': rask_string_push_byte(s, '\r'); break;
                default: rask_string_push_byte(s, (uint8_t)c); break;
            }
            *p += 2;
        } else {
            rask_string_push_byte(s, (uint8_t)**p);
            (*p)++;
        }
    }
    if (**p == '"') (*p)++;
    return s;
}

RaskJsonObj *rask_json_parse(const RaskString *s) {
    RaskJsonObj *obj = (RaskJsonObj *)calloc(1, sizeof(RaskJsonObj));
    if (!s) return obj;

    const char *p = rask_string_ptr(s);
    json_skip_ws(&p);
    if (*p != '{') return obj;
    p++;

    while (*p && *p != '}' && obj->count < JSON_MAX_FIELDS) {
        json_skip_ws(&p);
        if (*p == '}') break;
        if (*p == ',') { p++; json_skip_ws(&p); }

        if (*p != '"') break;
        RaskString *key = json_parse_string(&p);
        struct RaskJsonField *f = &obj->fields[obj->count];
        snprintf(f->key, sizeof(f->key), "%s", rask_string_ptr(key));
        rask_string_free(key);

        json_skip_ws(&p);
        if (*p != ':') break;
        p++;
        json_skip_ws(&p);

        if (*p == '"') {
            f->type = JSON_STRING;
            f->str_val = json_parse_string(&p);
        } else if (*p == 't' || *p == 'f') {
            f->type = JSON_BOOL;
            if (strncmp(p, "true", 4) == 0) { f->bool_val = 1; p += 4; }
            else if (strncmp(p, "false", 5) == 0) { f->bool_val = 0; p += 5; }
        } else if (*p == 'n' && strncmp(p, "null", 4) == 0) {
            f->type = JSON_STRING;
            f->str_val = NULL;
            p += 4;
        } else {
            f->type = JSON_NUMBER;
            char *end;
            f->num_val = strtod(p, &end);
            p = end;
        }
        obj->count++;
    }
    return obj;
}

static struct RaskJsonField *json_find_field(RaskJsonObj *obj, const char *key) {
    if (!obj) return NULL;
    for (int i = 0; i < obj->count; i++) {
        if (strcmp(obj->fields[i].key, key) == 0) return &obj->fields[i];
    }
    return NULL;
}

RaskString *rask_json_get_string(RaskJsonObj *obj, const char *key) {
    struct RaskJsonField *f = json_find_field(obj, key);
    if (!f || f->type != JSON_STRING) return rask_string_new();
    return f->str_val ? rask_string_clone(f->str_val) : rask_string_new();
}

int64_t rask_json_get_i64(RaskJsonObj *obj, const char *key) {
    struct RaskJsonField *f = json_find_field(obj, key);
    if (!f || f->type != JSON_NUMBER) return 0;
    return (int64_t)f->num_val;
}

double rask_json_get_f64(RaskJsonObj *obj, const char *key) {
    struct RaskJsonField *f = json_find_field(obj, key);
    if (!f || f->type != JSON_NUMBER) return 0.0;
    return f->num_val;
}

int8_t rask_json_get_bool(RaskJsonObj *obj, const char *key) {
    struct RaskJsonField *f = json_find_field(obj, key);
    if (!f || f->type != JSON_BOOL) return 0;
    return f->bool_val;
}

int64_t rask_json_decode(const RaskString *s) {
    return (int64_t)(uintptr_t)rask_json_parse(s);
}

// ─── Entry point ──────────────────────────────────────────────────

int main(int argc, char **argv) {
    rask_args_init(argc, argv);
    rask_main();
    return 0;
}
