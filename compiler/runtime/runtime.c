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
#include <signal.h>

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
    rask_panic("called unwrap on None/Err value");
}

void rask_assert_fail(void) {
    rask_panic("assertion failed");
}

void rask_panic_unwrap_at(const char *file, int32_t line, int32_t col) {
    rask_panic_at(file, line, col, "called unwrap on None/Err value");
}

void rask_assert_fail_at(const char *file, int32_t line, int32_t col) {
    rask_panic_at(file, line, col, "assertion failed");
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
    char *buf = (char *)rask_alloc((int64_t)size + 1);
    size_t n = fread(buf, 1, (size_t)size, f);
    buf[n] = '\0';
    fclose(f);
    RaskString *s = rask_string_from_bytes(buf, (int64_t)n);
    rask_free(buf);
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

// ─── File instance methods ────────────────────────────────────────
// Operate on FILE* handles returned by rask_fs_open / rask_fs_create.

void rask_file_close(int64_t file) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (f) fclose(f);
}

RaskString *rask_file_read_all(int64_t file) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return rask_string_new();
    // Read from current position to end
    long start = ftell(f);
    fseek(f, 0, SEEK_END);
    long end = ftell(f);
    fseek(f, start, SEEK_SET);
    long size = end - start;
    char *buf = (char *)rask_alloc((int64_t)size + 1);
    size_t n = fread(buf, 1, (size_t)size, f);
    buf[n] = '\0';
    RaskString *s = rask_string_from_bytes(buf, (int64_t)n);
    rask_free(buf);
    return s;
}

void rask_file_write(int64_t file, const RaskString *content) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f || !content) return;
    fwrite(rask_string_ptr(content), 1, (size_t)rask_string_len(content), f);
}

void rask_file_write_line(int64_t file, const RaskString *content) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return;
    if (content) {
        fwrite(rask_string_ptr(content), 1, (size_t)rask_string_len(content), f);
    }
    fputc('\n', f);
}

RaskVec *rask_file_lines(int64_t file) {
    RaskVec *v = rask_vec_new(sizeof(RaskString *));
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return v;
    // Rewind to start
    fseek(f, 0, SEEK_SET);
    char buf[4096];
    while (fgets(buf, sizeof(buf), f)) {
        size_t len = strlen(buf);
        if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
        if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';
        RaskString *line = rask_string_from_bytes(buf, (int64_t)len);
        rask_vec_push(v, &line);
    }
    return v;
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

int64_t rask_net_tcp_accept(int64_t listen_fd) {
    int client = accept((int)listen_fd, NULL, NULL);
    return (int64_t)client;
}

// ─── String-based socket I/O (used by Rask stdlib HTTP parser) ────

// Read up to max_len bytes from fd, return as RaskString.
RaskString *rask_io_read_string(int64_t fd, int64_t max_len) {
    if (max_len <= 0 || max_len > 1024 * 1024) max_len = 65536;
    char *buf = (char *)rask_alloc(max_len);
    int64_t total = 0;

    // Read until we have a complete HTTP request (double CRLF) or buffer full
    while (total < max_len) {
        ssize_t n = read((int)fd, buf + total, (size_t)(max_len - total));
        if (n <= 0) break;
        total += n;
        // Check for end of HTTP headers (\r\n\r\n)
        if (total >= 4) {
            for (int64_t i = total - 4; i >= 0 && i >= total - n - 3; i--) {
                if (buf[i] == '\r' && buf[i+1] == '\n' &&
                    buf[i+2] == '\r' && buf[i+3] == '\n') {
                    // Found header boundary — check Content-Length for body
                    // For simplicity, just return what we have (sufficient for
                    // simple JSON APIs where body fits in first read)
                    goto done;
                }
            }
        }
    }
done:;
    RaskString *s = rask_string_from_bytes(buf, total);
    rask_free(buf);
    return s;
}

// Write a RaskString to fd. Returns bytes written or -1.
int64_t rask_io_write_string(int64_t fd, int64_t str_ptr) {
    const RaskString *s = (const RaskString *)(uintptr_t)str_ptr;
    if (!s) return 0;
    const char *data = rask_string_ptr(s);
    int64_t len = rask_string_len(s);
    int64_t written = 0;
    while (written < len) {
        ssize_t n = write((int)fd, data + written, (size_t)(len - written));
        if (n < 0) return -1;
        written += n;
    }
    return written;
}

// Close a file descriptor.
void rask_io_close_fd(int64_t fd) {
    close((int)fd);
}

// ─── HTTP helpers (called from Rask stdlib via extern "C") ──────

// Parse HTTP/1.1 request from socket fd. Returns pointer to
// [method, path, body, headers] struct (4 x i64).
int64_t rask_http_parse_request(int64_t conn_fd) {
    RaskString *raw = rask_io_read_string(conn_fd, 65536);
    if (!raw || rask_string_len(raw) == 0) {
        // Empty request — return minimal struct
        int64_t *req = (int64_t *)rask_alloc(sizeof(int64_t) * 4);
        req[0] = (int64_t)(uintptr_t)rask_string_from_bytes("GET", 3);
        req[1] = (int64_t)(uintptr_t)rask_string_from_bytes("/", 1);
        req[2] = (int64_t)(uintptr_t)rask_string_from_bytes("", 0);
        req[3] = (int64_t)(uintptr_t)rask_map_new(8, 8);
        return (int64_t)(uintptr_t)req;
    }

    const char *data = rask_string_ptr(raw);
    int64_t len = rask_string_len(raw);

    // Find end of headers (\r\n\r\n)
    int64_t header_end = -1;
    for (int64_t i = 0; i + 3 < len; i++) {
        if (data[i] == '\r' && data[i+1] == '\n' &&
            data[i+2] == '\r' && data[i+3] == '\n') {
            header_end = i;
            break;
        }
    }
    if (header_end < 0) header_end = len;

    // Extract body (after \r\n\r\n)
    RaskString *body;
    if (header_end + 4 < len) {
        body = rask_string_from_bytes(data + header_end + 4, len - header_end - 4);
    } else {
        body = rask_string_from_bytes("", 0);
    }

    // Parse request line: "METHOD PATH HTTP/1.1\r\n"
    int64_t first_space = -1, second_space = -1;
    for (int64_t i = 0; i < header_end; i++) {
        if (data[i] == ' ') {
            if (first_space < 0) first_space = i;
            else if (second_space < 0) { second_space = i; break; }
        }
        if (data[i] == '\r') break;
    }

    RaskString *method, *path;
    if (first_space > 0 && second_space > first_space) {
        method = rask_string_from_bytes(data, first_space);
        path = rask_string_from_bytes(data + first_space + 1,
                                       second_space - first_space - 1);
    } else {
        method = rask_string_from_bytes("GET", 3);
        path = rask_string_from_bytes("/", 1);
    }

    // Parse headers
    RaskMap *headers = rask_map_new(8, 8);
    int64_t line_start = -1;
    // Find start of second line (after first \r\n)
    for (int64_t i = 0; i < header_end; i++) {
        if (data[i] == '\r' && i + 1 < header_end && data[i+1] == '\n') {
            line_start = i + 2;
            break;
        }
    }
    if (line_start > 0) {
        int64_t pos = line_start;
        while (pos < header_end) {
            // Find end of this header line
            int64_t line_end = header_end;
            for (int64_t i = pos; i < header_end; i++) {
                if (data[i] == '\r') { line_end = i; break; }
            }
            // Find ": " separator
            int64_t colon = -1;
            for (int64_t i = pos; i + 1 < line_end; i++) {
                if (data[i] == ':' && data[i+1] == ' ') { colon = i; break; }
            }
            if (colon > pos) {
                RaskString *key = rask_string_from_bytes(data + pos, colon - pos);
                RaskString *val = rask_string_from_bytes(data + colon + 2,
                                                          line_end - colon - 2);
                int64_t key_ptr = (int64_t)(uintptr_t)key;
                int64_t val_ptr = (int64_t)(uintptr_t)val;
                rask_map_insert(headers, &key_ptr, &val_ptr);
            }
            // Skip \r\n to next line
            pos = line_end + 2;
        }
    }

    // Build HttpRequest struct: [method, path, body, headers]
    int64_t *req = (int64_t *)rask_alloc(sizeof(int64_t) * 4);
    req[0] = (int64_t)(uintptr_t)method;
    req[1] = (int64_t)(uintptr_t)path;
    req[2] = (int64_t)(uintptr_t)body;
    req[3] = (int64_t)(uintptr_t)headers;
    return (int64_t)(uintptr_t)req;
}

// Format and write HTTP response to socket fd.
// resp_ptr points to [status(i64), headers(Map*), body(String*)].
int64_t rask_http_write_response(int64_t conn_fd, int64_t response_ptr) {
    int64_t *resp = (int64_t *)(uintptr_t)response_ptr;
    int64_t status = resp[0];
    RaskMap *headers = (RaskMap *)(uintptr_t)resp[1];
    RaskString *body = (RaskString *)(uintptr_t)resp[2];

    const char *reason = "OK";
    switch ((int)status) {
        case 200: reason = "OK"; break;
        case 201: reason = "Created"; break;
        case 204: reason = "No Content"; break;
        case 400: reason = "Bad Request"; break;
        case 404: reason = "Not Found"; break;
        case 500: reason = "Internal Server Error"; break;
    }

    int64_t body_len = body ? rask_string_len(body) : 0;

    // Build response into a growable string
    RaskString *out = rask_string_new();
    char line_buf[256];
    int n = snprintf(line_buf, sizeof(line_buf),
                     "HTTP/1.1 %d %s\r\n", (int)status, reason);
    rask_string_append_cstr(out, line_buf);

    // Write user headers from Map
    if (headers && rask_map_len(headers) > 0) {
        RaskVec *keys = rask_map_keys(headers);
        for (int64_t i = 0; i < rask_vec_len(keys); i++) {
            int64_t *key_slot = (int64_t *)rask_vec_get(keys, i);
            if (!key_slot) continue;
            RaskString *key = (RaskString *)(uintptr_t)*key_slot;
            int64_t *val_slot = (int64_t *)rask_map_get(headers, key_slot);
            if (!val_slot) continue;
            RaskString *val = (RaskString *)(uintptr_t)*val_slot;
            rask_string_append_cstr(out, rask_string_ptr(key));
            rask_string_append_cstr(out, ": ");
            rask_string_append_cstr(out, rask_string_ptr(val));
            rask_string_append_cstr(out, "\r\n");
        }
        rask_vec_free(keys);
    }

    // Content-Length header
    n = snprintf(line_buf, sizeof(line_buf),
                 "Content-Length: %lld\r\n\r\n", (long long)body_len);
    rask_string_append_cstr(out, line_buf);

    // Write header + body
    rask_io_write_string(conn_fd, (int64_t)(uintptr_t)out);
    if (body_len > 0) {
        rask_io_write_string(conn_fd, (int64_t)(uintptr_t)body);
    }

    rask_string_free(out);
    return 0;
}

// Legacy stubs — kept for backward compat, but shadowed by Rask stdlib functions
int64_t rask_net_read_http_request(int64_t conn_fd) {
    return rask_http_parse_request(conn_fd);
}

int64_t rask_net_write_http_response(int64_t conn_fd, int64_t response_ptr) {
    return rask_http_write_response(conn_fd, response_ptr);
}

// Stub: create a Map from a static array of key-value pairs.
int64_t rask_map_from(int64_t pairs_ptr) {
    (void)pairs_ptr;
    return (int64_t)(uintptr_t)rask_map_new(8, 8);
}

// Stub: generic json.encode — returns JSON string representation.
RaskString *rask_json_encode(int64_t value_ptr) {
    return rask_string_from_bytes("{}", 2);
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
    int64_t required = rask_safe_add(b->len, needed);
    if (required <= b->cap) return;
    int64_t new_cap = b->cap;
    if (new_cap > INT64_MAX / 2) rask_panic("JSON buffer overflow");
    new_cap *= 2;
    if (new_cap < required) new_cap = required;
    b->data = (char *)rask_realloc(b->data, b->cap, new_cap);
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
    RaskJsonBuf *b = (RaskJsonBuf *)rask_alloc(sizeof(RaskJsonBuf));
    b->cap = 256;
    b->data = (char *)rask_alloc(b->cap);
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

void rask_json_buf_add_raw(RaskJsonBuf *buf, const char *key, const RaskString *raw_json) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, key, (int64_t)strlen(key));
    json_buf_append_cstr(buf, ":");
    if (raw_json) {
        json_buf_append(buf, rask_string_ptr(raw_json), rask_string_len(raw_json));
    } else {
        json_buf_append_cstr(buf, "null");
    }
    buf->field_count++;
}

RaskString *rask_json_buf_finish(RaskJsonBuf *buf) {
    json_buf_append_cstr(buf, "}");
    RaskString *s = rask_string_from_bytes(buf->data, buf->len);
    rask_free(buf->data);
    rask_free(buf);
    return s;
}

RaskString *rask_json_encode_string(const RaskString *s) {
    struct RaskJsonBuf b;
    b.cap = 256;
    b.data = (char *)rask_alloc(b.cap);
    b.len = 0;
    b.field_count = 0;
    if (s) {
        json_buf_append_escaped(&b, rask_string_ptr(s), rask_string_len(s));
    } else {
        json_buf_append_cstr(&b, "null");
    }
    RaskString *result = rask_string_from_bytes(b.data, b.len);
    rask_free(b.data);
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
    RaskJsonObj *obj = (RaskJsonObj *)rask_alloc(sizeof(RaskJsonObj));
    memset(obj, 0, sizeof(RaskJsonObj));
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
    signal(SIGPIPE, SIG_IGN);
    rask_args_init(argc, argv);
    rask_main();
    return 0;
}
