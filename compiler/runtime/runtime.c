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

void rask_print_string(const RaskStr *s) {
    fputs(rask_string_ptr(s), stdout);
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

void rask_assert_fail_msg(const char *msg) {
    rask_panic(msg ? msg : "assertion failed");
}

void rask_assert_fail_msg_at(const char *msg, const char *file,
                             int32_t line, int32_t col) {
    rask_panic_at(file, line, col, msg ? msg : "assertion failed");
}

void rask_assert_fail_cmp_i64(int64_t left, int64_t right,
                              const char *op, const char *file,
                              int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: %lld %s %lld (left: %lld, right: %lld)",
             (long long)left, op ? op : "?",
             (long long)right, (long long)left, (long long)right);
    rask_panic_at(file, line, col, buf);
}

void rask_assert_fail_cmp_str(const char *left, const char *right,
                              const char *op, const char *file,
                              int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: \"%s\" %s \"%s\"",
             left ? left : "(null)", op ? op : "?",
             right ? right : "(null)");
    rask_panic_at(file, line, col, buf);
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

// Single read into a string (up to max_len bytes).
void rask_io_read_string(RaskStr *out, int64_t fd, int64_t max_len) {
    if (max_len <= 0 || max_len > 4 * 1024 * 1024) max_len = 65536;
    char *buf = (char *)rask_alloc(max_len);
    ssize_t n = read((int)fd, buf, (size_t)max_len);
    if (n < 0) n = 0;
    rask_string_from_bytes(out, buf, n);
    rask_free(buf);
}

// ─── Clone (shallow copy for i64-sized values) ───────────────────
// Strings and collection handles are pointer-sized; clone is identity.
int64_t rask_clone(int64_t value) { return value; }

// ─── CLI module ───────────────────────────────────────────────────
// cli.args() → Vec of RaskStr values (16 bytes each).

RaskVec *rask_cli_args(void) {
    RaskVec *v = rask_vec_new(16);
    int64_t count = rask_args_count();
    for (int64_t i = 0; i < count; i++) {
        const char *arg = rask_args_get(i);
        RaskStr s;
        rask_string_from(&s, arg);
        rask_vec_push(v, &s);
    }
    return v;
}

// ─── FS module ────────────────────────────────────────────────────

RaskVec *rask_fs_read_lines(const RaskStr *path) {
    RaskVec *v = rask_vec_new(16);
    const char *p = rask_string_ptr(path);

    FILE *f = fopen(p, "r");
    if (!f) return v;

    char buf[4096];
    while (fgets(buf, sizeof(buf), f)) {
        size_t len = strlen(buf);
        if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
        if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';

        RaskStr line;
        rask_string_from_bytes(&line, buf, (int64_t)len);
        rask_vec_push(v, &line);
    }

    fclose(f);
    return v;
}

// ─── IO module ────────────────────────────────────────────────────

void rask_io_read_line(RaskStr *out) {
    char buf[4096];
    if (!fgets(buf, sizeof(buf), stdin)) {
        rask_string_new(out);
        return;
    }
    size_t len = strlen(buf);
    if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
    if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';
    rask_string_from_bytes(out, buf, (int64_t)len);
}

// ─── More FS module ───────────────────────────────────────────────

void rask_fs_read_file(RaskStr *out, const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "rb");
    if (!f) { rask_string_new(out); return; }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *buf = (char *)rask_alloc((int64_t)size + 1);
    size_t n = fread(buf, 1, (size_t)size, f);
    buf[n] = '\0';
    fclose(f);
    rask_string_from_bytes(out, buf, (int64_t)n);
    rask_free(buf);
}

void rask_fs_write_file(const RaskStr *path, const RaskStr *content) {
    const char *p = rask_string_ptr(path);
    const char *c = rask_string_ptr(content);
    int64_t clen = rask_string_len(content);
    FILE *f = fopen(p, "wb");
    if (!f) return;
    fwrite(c, 1, (size_t)clen, f);
    fclose(f);
}

RaskVec *rask_fs_read_bytes(const RaskStr *path) {
    RaskVec *v = rask_vec_new(1);
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "rb");
    if (!f) return v;
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (size > 0) {
        char *buf = (char *)rask_alloc((int64_t)size);
        size_t n = fread(buf, 1, (size_t)size, f);
        for (size_t i = 0; i < n; i++) {
            uint8_t byte = (uint8_t)buf[i];
            rask_vec_push(v, &byte);
        }
        rask_free(buf);
    }
    fclose(f);
    return v;
}

void rask_fs_write_bytes(const RaskStr *path, RaskVec *data) {
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "wb");
    if (!f) return;
    int64_t len = rask_vec_len(data);
    for (int64_t i = 0; i < len; i++) {
        uint8_t *byte = (uint8_t *)rask_vec_get(data, i);
        if (byte) fwrite(byte, 1, 1, f);
    }
    fclose(f);
}

int8_t rask_fs_exists(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "r");
    if (f) { fclose(f); return 1; }
    return 0;
}

// ─── More FS module ───────────────────────────────────────────────

int64_t rask_fs_open(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "r");
    return (int64_t)(uintptr_t)f;
}

int64_t rask_fs_create(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    FILE *f = fopen(p, "w");
    return (int64_t)(uintptr_t)f;
}

void rask_fs_canonicalize(RaskStr *out, const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    char resolved[4096];
    char *r = realpath(p, resolved);
    if (!r) { rask_string_new(out); return; }
    rask_string_from(out, resolved);
}

int64_t rask_fs_copy(const RaskStr *from, const RaskStr *to) {
    const char *src = rask_string_ptr(from);
    const char *dst = rask_string_ptr(to);
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

void rask_fs_rename(const RaskStr *from, const RaskStr *to) {
    const char *s = rask_string_ptr(from);
    const char *d = rask_string_ptr(to);
    rename(s, d);
}

void rask_fs_remove(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    remove(p);
}

#include <sys/stat.h>

// Thin wrappers for libc functions whose names clash with Rask methods
// or that access C structs. Self-hosted stdlib calls these via extern "C".
int32_t rask_libc_rename(const char *from, const char *to) { return rename(from, to); }
int32_t rask_libc_remove(const char *path) { return remove(path); }
int32_t rask_libc_mkdir(const char *path, uint32_t mode) { return mkdir(path, mode); }

#include <dirent.h>
// Extract name from dirent (Rask can't access C struct fields)
const char *rask_dirent_name(void *entry) { return ((struct dirent *)entry)->d_name; }

// Stat helpers — return individual fields so Rask doesn't need struct access
int64_t rask_stat_size(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) return -1;
    return (int64_t)st.st_size;
}
int64_t rask_stat_mtime(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) return -1;
    return (int64_t)st.st_mtime;
}
int64_t rask_stat_atime(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) return -1;
    return (int64_t)st.st_atime;
}

void rask_fs_create_dir(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
    mkdir(p, 0755);
}

void rask_fs_create_dir_all(const RaskStr *path) {
    const char *p = rask_string_ptr(path);
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

void rask_fs_append_file(const RaskStr *path, const RaskStr *content) {
    const char *p = rask_string_ptr(path);
    const char *c = rask_string_ptr(content);
    int64_t clen = rask_string_len(content);
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

void rask_file_read_all(RaskStr *out, int64_t file) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) { rask_string_new(out); return; }
    // Read from current position to end
    long start = ftell(f);
    fseek(f, 0, SEEK_END);
    long end = ftell(f);
    fseek(f, start, SEEK_SET);
    long size = end - start;
    char *buf = (char *)rask_alloc((int64_t)size + 1);
    size_t n = fread(buf, 1, (size_t)size, f);
    buf[n] = '\0';
    rask_string_from_bytes(out, buf, (int64_t)n);
    rask_free(buf);
}

void rask_file_write(int64_t file, const RaskStr *content) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return;
    fwrite(rask_string_ptr(content), 1, (size_t)rask_string_len(content), f);
}

void rask_file_write_all(int64_t file, const RaskStr *content) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return;
    const char *ptr = rask_string_ptr(content);
    size_t remaining = (size_t)rask_string_len(content);
    while (remaining > 0) {
        size_t written = fwrite(ptr, 1, remaining, f);
        if (written == 0) break;
        ptr += written;
        remaining -= written;
    }
    fflush(f);
}

void rask_file_write_line(int64_t file, const RaskStr *content) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return;
    fwrite(rask_string_ptr(content), 1, (size_t)rask_string_len(content), f);
    fputc('\n', f);
}

RaskVec *rask_file_lines(int64_t file) {
    RaskVec *v = rask_vec_new(16);
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return v;
    // Rewind to start
    fseek(f, 0, SEEK_SET);
    char buf[4096];
    while (fgets(buf, sizeof(buf), f)) {
        size_t len = strlen(buf);
        if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
        if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';
        RaskStr line;
        rask_string_from_bytes(&line, buf, (int64_t)len);
        rask_vec_push(v, &line);
    }
    return v;
}

// ─── Net module ───────────────────────────────────────────────────

#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <netdb.h>

int64_t rask_net_tcp_listen(const RaskStr *addr) {
    const char *a = rask_string_ptr(addr);

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

int64_t rask_net_tcp_connect(const RaskStr *addr) {
    const char *a = rask_string_ptr(addr);

    // Parse "host:port"
    char host[256] = "127.0.0.1";
    char port_str[16] = "80";
    const char *colon = strrchr(a, ':');
    if (colon) {
        size_t hlen = (size_t)(colon - a);
        if (hlen > 0 && hlen < sizeof(host)) {
            memcpy(host, a, hlen);
            host[hlen] = '\0';
        }
        size_t plen = strlen(colon + 1);
        if (plen > 0 && plen < sizeof(port_str)) {
            memcpy(port_str, colon + 1, plen + 1);
        }
    }

    // Resolve hostname via getaddrinfo (handles both IPs and DNS names)
    struct addrinfo hints, *result;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    int err = getaddrinfo(host, port_str, &hints, &result);
    if (err != 0) return -1;

    int fd = socket(result->ai_family, result->ai_socktype, result->ai_protocol);
    if (fd < 0) {
        freeaddrinfo(result);
        return -1;
    }

    if (connect(fd, result->ai_addr, result->ai_addrlen) < 0) {
        close(fd);
        freeaddrinfo(result);
        return -1;
    }

    freeaddrinfo(result);
    return (int64_t)fd;
}

// ─── String-based socket I/O (used by Rask stdlib HTTP parser) ────

// Read up to max_len bytes from fd, return as RaskStr.
static void io_read_string(RaskStr *out, int64_t fd, int64_t max_len) {
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
                    goto done;
                }
            }
        }
    }
done:;
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
}

// Read until connection closes or max_len reached. For HTTP client responses
// where Connection: close is used.
void rask_io_read_until_close(RaskStr *out, int64_t fd, int64_t max_len) {
    if (max_len <= 0 || max_len > 4 * 1024 * 1024) max_len = 1048576;
    char *buf = (char *)rask_alloc(max_len);
    int64_t total = 0;
    while (total < max_len) {
        ssize_t n = read((int)fd, buf + total, (size_t)(max_len - total));
        if (n <= 0) break;
        total += n;
    }
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
}

// Write a RaskStr to fd. Returns bytes written or -1.
int64_t rask_io_write_string(int64_t fd, int64_t str_ptr) {
    const RaskStr *s = (const RaskStr *)(uintptr_t)str_ptr;
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
// [method, path, body, headers] — each string field is a 16-byte RaskStr.
// Layout: [RaskStr method (16B)][RaskStr path (16B)][RaskStr body (16B)][Map* headers (8B)]
int64_t rask_http_parse_request(int64_t conn_fd) {
    RaskStr raw;
    io_read_string(&raw, conn_fd, 65536);
    if (rask_string_len(&raw) == 0) {
        // Empty request — return minimal struct
        // Allocate: 3 * 16 bytes (strings) + 8 bytes (map ptr) = 56 bytes
        uint8_t *req = (uint8_t *)rask_alloc(56);
        memset(req, 0, 56);
        RaskStr *method = (RaskStr *)req;
        RaskStr *path = (RaskStr *)(req + 16);
        RaskStr *body = (RaskStr *)(req + 32);
        rask_string_from_bytes(method, "GET", 3);
        rask_string_from_bytes(path, "/", 1);
        rask_string_new(body);
        *(int64_t *)(req + 48) = (int64_t)(uintptr_t)rask_map_new(16, 16);
        return (int64_t)(uintptr_t)req;
    }

    const char *data = rask_string_ptr(&raw);
    int64_t len = rask_string_len(&raw);

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

    // Parse request line: "METHOD PATH HTTP/1.1\r\n"
    int64_t first_space = -1, second_space = -1;
    for (int64_t i = 0; i < header_end; i++) {
        if (data[i] == ' ') {
            if (first_space < 0) first_space = i;
            else if (second_space < 0) { second_space = i; break; }
        }
        if (data[i] == '\r') break;
    }

    // Allocate result: 3 RaskStr (48B) + 1 Map* (8B) = 56B
    uint8_t *req = (uint8_t *)rask_alloc(56);
    memset(req, 0, 56);
    RaskStr *method = (RaskStr *)req;
    RaskStr *path_str = (RaskStr *)(req + 16);
    RaskStr *body = (RaskStr *)(req + 32);

    if (first_space > 0 && second_space > first_space) {
        rask_string_from_bytes(method, data, first_space);
        rask_string_from_bytes(path_str, data + first_space + 1,
                               second_space - first_space - 1);
    } else {
        rask_string_from_bytes(method, "GET", 3);
        rask_string_from_bytes(path_str, "/", 1);
    }

    // Extract body (after \r\n\r\n)
    if (header_end + 4 < len) {
        rask_string_from_bytes(body, data + header_end + 4, len - header_end - 4);
    } else {
        rask_string_new(body);
    }

    // Parse headers — map stores RaskStr keys and values (16B each)
    RaskMap *headers = rask_map_new_string_keys(16, 16);
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
                RaskStr key, val;
                rask_string_from_bytes(&key, data + pos, colon - pos);
                rask_string_from_bytes(&val, data + colon + 2,
                                       line_end - colon - 2);
                rask_map_insert(headers, &key, &val);
            }
            // Skip \r\n to next line
            pos = line_end + 2;
        }
    }

    *(int64_t *)(req + 48) = (int64_t)(uintptr_t)headers;

    rask_string_free(&raw);
    return (int64_t)(uintptr_t)req;
}

// Format and write HTTP response to socket fd.
// resp_ptr points to [RaskStr status_str (16B) ... ] — but currently uses
// [status(i64), headers(Map*), body_ptr]. Keep old ABI for now.
int64_t rask_http_write_response(int64_t conn_fd, int64_t response_ptr) {
    int64_t *resp = (int64_t *)(uintptr_t)response_ptr;
    int64_t status = resp[0];
    RaskMap *headers = (RaskMap *)(uintptr_t)resp[1];
    const RaskStr *body = (const RaskStr *)(uintptr_t)resp[2];

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
    RaskStr out;
    rask_string_new(&out);
    char line_buf[256];
    snprintf(line_buf, sizeof(line_buf),
             "HTTP/1.1 %d %s\r\n", (int)status, reason);
    rask_string_append_cstr(&out, &out, line_buf);

    // Write user headers from Map
    if (headers && rask_map_len(headers) > 0) {
        RaskVec *keys = rask_map_keys(headers);
        for (int64_t i = 0; i < rask_vec_len(keys); i++) {
            RaskStr *key = (RaskStr *)rask_vec_get(keys, i);
            if (!key) continue;
            RaskStr *val = (RaskStr *)rask_map_get(headers, key);
            if (!val) continue;
            RaskStr tmp;
            rask_string_append_cstr(&tmp, &out, rask_string_ptr(key));
            rask_string_free(&out);
            rask_string_append_cstr(&out, &tmp, ": ");
            rask_string_free(&tmp);
            rask_string_append_cstr(&tmp, &out, rask_string_ptr(val));
            rask_string_free(&out);
            rask_string_append_cstr(&out, &tmp, "\r\n");
            rask_string_free(&tmp);
        }
        rask_vec_free(keys);
    }

    // Content-Length header
    snprintf(line_buf, sizeof(line_buf),
             "Content-Length: %lld\r\n\r\n", (long long)body_len);
    RaskStr tmp;
    rask_string_append_cstr(&tmp, &out, line_buf);
    rask_string_free(&out);
    out = tmp;

    // Write header + body
    rask_io_write_string(conn_fd, (int64_t)(uintptr_t)&out);
    if (body_len > 0) {
        rask_io_write_string(conn_fd, (int64_t)(uintptr_t)body);
    }

    rask_string_free(&out);
    return 0;
}

// Close a network socket (listening or connected).
void rask_net_close(int64_t fd) {
    if (fd >= 0) close((int)fd);
}

// Close an HttpServer — extracts the listener fd from the struct
// (listener is the first field) and closes it.
void rask_http_server_close(int64_t server_ptr) {
    if (server_ptr == 0) return;
    int64_t fd = *(int64_t *)(uintptr_t)server_ptr;
    if (fd >= 0) close((int)fd);
}

// Clone a socket fd via dup().
int64_t rask_net_clone(int64_t fd) {
    if (fd < 0) return -1;
    return (int64_t)dup((int)fd);
}

// Read all available data from a TCP connection into a string.
// Reads until EOF or error. Returns Result-encoded value:
// >=0 = success (string written to out), <0 = error.
int64_t rask_net_read_all(int64_t fd, int64_t out_ptr) {
    RaskStr *out = (RaskStr *)(intptr_t)out_ptr;
    char *buf = (char *)rask_alloc(65536);
    int64_t total = 0;
    int64_t cap = 65536;
    for (;;) {
        ssize_t n = read((int)fd, buf + total, (size_t)(cap - total));
        if (n <= 0) break;
        total += n;
        if (total >= cap) {
            cap *= 2;
            buf = (char *)rask_realloc(buf, cap / 2, cap);
        }
    }
    rask_string_from_bytes(out, buf, total);
    rask_free(buf);
    return 0;
}

// Write all data to a TCP connection. Returns 0 on success, -1 on error.
int64_t rask_net_write_all(int64_t fd, int64_t str_ptr) {
    const RaskStr *s = (const RaskStr *)(intptr_t)str_ptr;
    const char *data = rask_string_ptr(s);
    int64_t len = rask_string_len(s);
    int64_t written = 0;
    while (written < len) {
        ssize_t n = write((int)fd, data + written, (size_t)(len - written));
        if (n < 0) return -1;
        written += n;
    }
    return 0;
}

// Get the remote address of a TCP connection as "ip:port" string.
void rask_net_remote_addr(RaskStr *out, int64_t fd) {
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    if (getpeername((int)fd, (struct sockaddr *)&addr, &addrlen) < 0) {
        rask_string_from(out, "unknown");
        return;
    }
    char ip[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &addr.sin_addr, ip, sizeof(ip));
    char buf[64];
    snprintf(buf, sizeof(buf), "%s:%d", ip, ntohs(addr.sin_port));
    rask_string_from(out, buf);
}

// Get filesystem metadata for a path. Returns pointer to a
// [size(8B), accessed(8B), modified(8B)] struct, or NULL on error.
int64_t rask_fs_metadata(int64_t path_ptr) {
    const RaskStr *p = (const RaskStr *)(intptr_t)path_ptr;
    const char *path = rask_string_ptr(p);
    int64_t len = rask_string_len(p);
    char *cpath = (char *)rask_alloc(len + 1);
    memcpy(cpath, path, (size_t)len);
    cpath[len] = '\0';

    struct stat st;
    if (stat(cpath, &st) < 0) {
        rask_free(cpath);
        return 0;
    }
    rask_free(cpath);

    int64_t *meta = (int64_t *)rask_alloc(24);
    meta[0] = (int64_t)st.st_size;
    meta[1] = (int64_t)st.st_atime;
    meta[2] = (int64_t)st.st_mtime;
    return (int64_t)(intptr_t)meta;
}

// Metadata field accessors — meta_ptr points to [size, accessed, modified].
int64_t rask_metadata_size(int64_t meta_ptr) {
    int64_t *meta = (int64_t *)(intptr_t)meta_ptr;
    return meta ? meta[0] : 0;
}

int64_t rask_metadata_accessed(int64_t meta_ptr) {
    int64_t *meta = (int64_t *)(intptr_t)meta_ptr;
    return meta ? meta[1] : 0;
}

int64_t rask_metadata_modified(int64_t meta_ptr) {
    int64_t *meta = (int64_t *)(intptr_t)meta_ptr;
    return meta ? meta[2] : 0;
}

// ── Args parsing ───────────────────────────────────────────────
// Parse raw CLI args into an Args struct:
// [program(16B string), positional(8B Vec*), flags(8B Vec*), options(8B Map*)]
// Total: 40 bytes at returned pointer.

int64_t rask_args_parse(void) {
    int64_t count = rask_args_count();

    RaskStr *program = (RaskStr *)rask_alloc(16);
    rask_string_new(program);
    if (count > 0) {
        const char *p = rask_args_get(0);
        if (p) rask_string_from(program, p);
    }

    RaskVec *positional = rask_vec_new(16);
    RaskVec *flags = rask_vec_new(16);
    RaskMap *options = rask_map_new(16, 16);

    int past_separator = 0;
    for (int64_t i = 1; i < count; i++) {
        const char *arg = rask_args_get(i);
        if (!arg) continue;
        size_t alen = strlen(arg);

        if (past_separator) {
            RaskStr s;
            rask_string_from(&s, arg);
            rask_vec_push(positional, &s);
            continue;
        }

        if (alen == 2 && arg[0] == '-' && arg[1] == '-') {
            past_separator = 1;
            continue;
        }

        if (alen > 2 && arg[0] == '-' && arg[1] == '-') {
            // --option=value or --flag
            const char *eq = strchr(arg + 2, '=');
            if (eq) {
                RaskStr key, val;
                rask_string_from_bytes(&key, arg, (int64_t)(eq - arg));
                rask_string_from(&val, eq + 1);
                rask_map_insert(options, &key, &val);
            } else if (i + 1 < count && rask_args_get(i + 1)[0] != '-') {
                RaskStr key, val;
                rask_string_from(&key, arg);
                rask_string_from(&val, rask_args_get(i + 1));
                rask_map_insert(options, &key, &val);
                i++;
            } else {
                RaskStr f;
                rask_string_from(&f, arg);
                rask_vec_push(flags, &f);
            }
        } else if (alen > 1 && arg[0] == '-') {
            // -f or -o value
            if (alen == 2 && i + 1 < count && rask_args_get(i + 1)[0] != '-') {
                RaskStr key, val;
                rask_string_from(&key, arg);
                rask_string_from(&val, rask_args_get(i + 1));
                rask_map_insert(options, &key, &val);
                i++;
            } else {
                // Combined short flags: -vn → --v, --n
                for (size_t j = 1; j < alen; j++) {
                    char short_flag[3] = { '-', arg[j], '\0' };
                    RaskStr f;
                    rask_string_from(&f, short_flag);
                    rask_vec_push(flags, &f);
                }
            }
        } else {
            RaskStr s;
            rask_string_from(&s, arg);
            rask_vec_push(positional, &s);
        }
    }

    // Pack into a 40-byte struct: [program(16B), positional(8B), flags(8B), options(8B)]
    char *result = (char *)rask_alloc(40);
    memcpy(result, program, 16);
    rask_free(program);
    *(int64_t *)(result + 16) = (int64_t)(intptr_t)positional;
    *(int64_t *)(result + 24) = (int64_t)(intptr_t)flags;
    *(int64_t *)(result + 32) = (int64_t)(intptr_t)options;
    return (int64_t)(intptr_t)result;
}

// Args method: flag(long, short) -> bool
int64_t rask_args_flag(int64_t args_ptr, int64_t long_ptr, int64_t short_ptr) {
    char *a = (char *)(intptr_t)args_ptr;
    RaskVec *flags = (RaskVec *)(intptr_t)*(int64_t *)(a + 24);
    const RaskStr *lng = (const RaskStr *)(intptr_t)long_ptr;
    const RaskStr *sht = (const RaskStr *)(intptr_t)short_ptr;
    int64_t len = rask_vec_len(flags);
    for (int64_t i = 0; i < len; i++) {
        const RaskStr *f = (const RaskStr *)rask_vec_get(flags, i);
        if (f && (rask_string_eq(f, lng) || rask_string_eq(f, sht))) return 1;
    }
    return 0;
}

// Args method: option(long, short) -> Option<string> (NULL = None, ptr = Some)
int64_t rask_args_option(int64_t args_ptr, int64_t long_ptr, int64_t short_ptr) {
    char *a = (char *)(intptr_t)args_ptr;
    RaskMap *opts = (RaskMap *)(intptr_t)*(int64_t *)(a + 32);
    void *val = rask_map_get(opts, (const void *)(intptr_t)long_ptr);
    if (val) return (int64_t)(intptr_t)val;
    val = rask_map_get(opts, (const void *)(intptr_t)short_ptr);
    return (int64_t)(intptr_t)val;
}

// Args method: option_or(long, short, default) -> string
void rask_args_option_or(RaskStr *out, int64_t args_ptr, int64_t long_ptr,
                         int64_t short_ptr, int64_t default_ptr) {
    int64_t val_ptr = rask_args_option(args_ptr, long_ptr, short_ptr);
    if (val_ptr) {
        const RaskStr *val = (const RaskStr *)(intptr_t)val_ptr;
        rask_string_from_bytes(out, rask_string_ptr(val), rask_string_len(val));
    } else {
        const RaskStr *def = (const RaskStr *)(intptr_t)default_ptr;
        rask_string_from_bytes(out, rask_string_ptr(def), rask_string_len(def));
    }
}

// Args method: positional() -> Vec<string>
int64_t rask_args_positional(int64_t args_ptr) {
    char *a = (char *)(intptr_t)args_ptr;
    return *(int64_t *)(a + 16);
}

// Args method: program() -> string
int64_t rask_args_program(int64_t args_ptr) {
    return args_ptr; // first 16 bytes IS the program string
}

// HTTP server accept: accept TCP connection + parse HTTP request.
// Returns pointer to [request_ptr(8B), conn_fd(8B)] — two i64s.
// request_ptr points to the 56-byte Request struct from rask_http_parse_request.
// On error (accept fails), returns -1.
int64_t rask_http_server_accept(int64_t listen_fd) {
    int client = accept((int)listen_fd, NULL, NULL);
    if (client < 0) return -1;
    int64_t req_ptr = rask_http_parse_request((int64_t)client);
    int64_t *result = (int64_t *)rask_alloc(16);
    result[0] = req_ptr;
    result[1] = (int64_t)client;
    return (int64_t)(uintptr_t)result;
}

// HTTP respond: write response and close connection.
// responder_fd is the conn_fd from server_accept, response_ptr is the Response struct.
int64_t rask_http_respond(int64_t responder_fd, int64_t response_ptr) {
    int64_t rc = rask_http_write_response(responder_fd, response_ptr);
    close((int)responder_fd);
    return rc;
}

// HTTP client: send a request and return a Response struct.
// method/url are RaskStr pointers, body/headers can be 0.
// Returns pointer to [status_code(i64), headers(Map*), body(RaskStr*)] or -1 on error.
int64_t rask_http_send_request(int64_t method_ptr, int64_t url_ptr,
                               int64_t body_ptr, int64_t headers_ptr) {
    const RaskStr *url = (const RaskStr *)(uintptr_t)url_ptr;
    const RaskStr *method = (const RaskStr *)(uintptr_t)method_ptr;
    const char *url_str = rask_string_ptr(url);
    int64_t url_len = rask_string_len(url);

    // Parse url: skip "http://"
    const char *host_start = url_str;
    if (url_len > 7 && memcmp(url_str, "http://", 7) == 0) {
        host_start = url_str + 7;
    }

    // Split host:port and path
    char host[256] = {0};
    char port_str[8] = "80";
    const char *path = "/";
    const char *slash = strchr(host_start, '/');
    size_t host_part_len = slash ? (size_t)(slash - host_start) : strlen(host_start);
    if (slash) path = slash;

    // Check for port in host
    const char *colon = memchr(host_start, ':', host_part_len);
    if (colon) {
        size_t hlen = (size_t)(colon - host_start);
        if (hlen < sizeof(host)) { memcpy(host, host_start, hlen); host[hlen] = '\0'; }
        size_t plen = host_part_len - hlen - 1;
        if (plen < sizeof(port_str)) { memcpy(port_str, colon + 1, plen); port_str[plen] = '\0'; }
    } else {
        if (host_part_len < sizeof(host)) { memcpy(host, host_start, host_part_len); host[host_part_len] = '\0'; }
    }

    // Connect
    struct addrinfo hints = { .ai_family = AF_INET, .ai_socktype = SOCK_STREAM };
    struct addrinfo *res = NULL;
    if (getaddrinfo(host, port_str, &hints, &res) != 0 || !res) return -1;
    int fd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (fd < 0) { freeaddrinfo(res); return -1; }
    if (connect(fd, res->ai_addr, res->ai_addrlen) < 0) {
        close(fd); freeaddrinfo(res); return -1;
    }
    freeaddrinfo(res);

    // Build request
    const char *method_str = rask_string_ptr(method);
    const RaskStr *body = body_ptr ? (const RaskStr *)(uintptr_t)body_ptr : NULL;
    int64_t body_len = body ? rask_string_len(body) : 0;

    RaskStr req;
    rask_string_new(&req);
    char line[512];
    snprintf(line, sizeof(line), "%s %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n",
             method_str, path, host);
    rask_string_append_cstr(&req, &req, line);
    if (body_len > 0) {
        snprintf(line, sizeof(line), "Content-Length: %lld\r\n", (long long)body_len);
        RaskStr tmp;
        rask_string_append_cstr(&tmp, &req, line);
        rask_string_free(&req);
        req = tmp;
    }
    {
        RaskStr tmp;
        rask_string_append_cstr(&tmp, &req, "\r\n");
        rask_string_free(&req);
        req = tmp;
    }

    rask_io_write_string(fd, (int64_t)(uintptr_t)&req);
    if (body_len > 0) {
        rask_io_write_string(fd, (int64_t)(uintptr_t)body);
    }
    rask_string_free(&req);

    // Read response
    RaskStr resp_raw;
    rask_io_read_until_close(&resp_raw, fd, 1048576);
    close(fd);

    const char *rdata = rask_string_ptr(&resp_raw);
    int64_t rlen = rask_string_len(&resp_raw);

    // Parse status code from "HTTP/1.1 200 OK\r\n"
    int64_t status_code = 0;
    if (rlen > 12 && memcmp(rdata, "HTTP/", 5) == 0) {
        const char *sp = strchr(rdata, ' ');
        if (sp) status_code = atoi(sp + 1);
    }

    // Find end of headers
    int64_t hdr_end = -1;
    for (int64_t i = 0; i + 3 < rlen; i++) {
        if (rdata[i] == '\r' && rdata[i+1] == '\n' && rdata[i+2] == '\r' && rdata[i+3] == '\n') {
            hdr_end = i; break;
        }
    }
    if (hdr_end < 0) hdr_end = rlen;

    // Parse response headers
    RaskMap *resp_headers = rask_map_new_string_keys(16, 16);
    // Skip status line
    int64_t lstart = -1;
    for (int64_t i = 0; i < hdr_end; i++) {
        if (rdata[i] == '\r' && i + 1 < hdr_end && rdata[i+1] == '\n') {
            lstart = i + 2; break;
        }
    }
    if (lstart > 0) {
        int64_t pos = lstart;
        while (pos < hdr_end) {
            int64_t lend = hdr_end;
            for (int64_t i = pos; i < hdr_end; i++) {
                if (rdata[i] == '\r') { lend = i; break; }
            }
            int64_t colon_pos = -1;
            for (int64_t i = pos; i + 1 < lend; i++) {
                if (rdata[i] == ':' && rdata[i+1] == ' ') { colon_pos = i; break; }
            }
            if (colon_pos > pos) {
                RaskStr key, val;
                rask_string_from_bytes(&key, rdata + pos, colon_pos - pos);
                rask_string_from_bytes(&val, rdata + colon_pos + 2, lend - colon_pos - 2);
                rask_map_insert(resp_headers, &key, &val);
            }
            pos = lend + 2;
        }
    }

    // Extract body
    RaskStr *resp_body = (RaskStr *)rask_alloc(16);
    if (hdr_end + 4 < rlen) {
        rask_string_from_bytes(resp_body, rdata + hdr_end + 4, rlen - hdr_end - 4);
    } else {
        rask_string_new(resp_body);
    }
    rask_string_free(&resp_raw);

    // Return [status_code(i64), headers(Map*), body(RaskStr*)]
    int64_t *result = (int64_t *)rask_alloc(24);
    result[0] = status_code;
    result[1] = (int64_t)(uintptr_t)resp_headers;
    result[2] = (int64_t)(uintptr_t)resp_body;
    return (int64_t)(uintptr_t)result;
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
void rask_json_encode(RaskStr *out, int64_t value_ptr) {
    (void)value_ptr;
    rask_string_from_bytes(out, "{}", 2);
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

void rask_json_buf_add_string(RaskJsonBuf *buf, const RaskStr *key, const RaskStr *val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(key), rask_string_len(key));
    json_buf_append_cstr(buf, ":");
    json_buf_append_escaped(buf, rask_string_ptr(val), rask_string_len(val));
    buf->field_count++;
}

void rask_json_buf_add_i64(RaskJsonBuf *buf, const RaskStr *key, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(key), rask_string_len(key));
    char num[32];
    snprintf(num, sizeof(num), ":%lld", (long long)val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_add_f64(RaskJsonBuf *buf, const RaskStr *key, double val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(key), rask_string_len(key));
    char num[64];
    snprintf(num, sizeof(num), ":%g", val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_add_bool(RaskJsonBuf *buf, const RaskStr *key, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(key), rask_string_len(key));
    json_buf_append_cstr(buf, val ? ":true" : ":false");
    buf->field_count++;
}

void rask_json_buf_add_raw(RaskJsonBuf *buf, const RaskStr *key, const RaskStr *raw_json) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(key), rask_string_len(key));
    json_buf_append_cstr(buf, ":");
    json_buf_append(buf, rask_string_ptr(raw_json), rask_string_len(raw_json));
    buf->field_count++;
}

void rask_json_buf_finish(RaskStr *out, RaskJsonBuf *buf) {
    json_buf_append_cstr(buf, "}");
    rask_string_from_bytes(out, buf->data, buf->len);
    rask_free(buf->data);
    rask_free(buf);
}

// ─── JSON array buffer ──────────────────────────────────────────

RaskJsonBuf *rask_json_buf_new_array(void) {
    RaskJsonBuf *b = (RaskJsonBuf *)rask_alloc(sizeof(RaskJsonBuf));
    b->cap = 256;
    b->data = (char *)rask_alloc(b->cap);
    b->len = 0;
    b->field_count = 0;
    json_buf_append_cstr(b, "[");
    return b;
}

void rask_json_buf_array_add_raw(RaskJsonBuf *buf, const RaskStr *raw_json) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append(buf, rask_string_ptr(raw_json), rask_string_len(raw_json));
    buf->field_count++;
}

void rask_json_buf_array_add_string(RaskJsonBuf *buf, const RaskStr *val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_escaped(buf, rask_string_ptr(val), rask_string_len(val));
    buf->field_count++;
}

void rask_json_buf_array_add_i64(RaskJsonBuf *buf, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    char num[32];
    snprintf(num, sizeof(num), "%lld", (long long)val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_array_add_f64(RaskJsonBuf *buf, double val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    char num[64];
    snprintf(num, sizeof(num), "%g", val);
    json_buf_append_cstr(buf, num);
    buf->field_count++;
}

void rask_json_buf_array_add_bool(RaskJsonBuf *buf, int64_t val) {
    if (buf->field_count > 0) json_buf_append_cstr(buf, ",");
    json_buf_append_cstr(buf, val ? "true" : "false");
    buf->field_count++;
}

void rask_json_buf_finish_array(RaskStr *out, RaskJsonBuf *buf) {
    json_buf_append_cstr(buf, "]");
    rask_string_from_bytes(out, buf->data, buf->len);
    rask_free(buf->data);
    rask_free(buf);
}

void rask_json_encode_string(RaskStr *out, const RaskStr *s) {
    struct RaskJsonBuf b;
    b.cap = 256;
    b.data = (char *)rask_alloc(b.cap);
    b.len = 0;
    b.field_count = 0;
    json_buf_append_escaped(&b, rask_string_ptr(s), rask_string_len(s));
    rask_string_from_bytes(out, b.data, b.len);
    rask_free(b.data);
}

void rask_json_encode_i64(RaskStr *out, int64_t val) {
    char buf[32];
    int len = snprintf(buf, sizeof(buf), "%lld", (long long)val);
    rask_string_from_bytes(out, buf, (int64_t)len);
}

// ─── JSON decode ──────────────────────────────────────────────────

#define JSON_MAX_FIELDS 64

struct RaskJsonField {
    char key[128];
    enum { JSON_STRING, JSON_NUMBER, JSON_BOOL } type;
    union {
        RaskStr str_val;
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

static void json_parse_string(RaskStr *out, const char **p) {
    if (**p != '"') { rask_string_new(out); return; }
    (*p)++;
    // Scan for closing quote to know total length
    const char *start = *p;
    int has_escapes = 0;
    while (**p && **p != '"') {
        if (**p == '\\') { has_escapes = 1; (*p)++; if (**p) (*p)++; }
        else (*p)++;
    }
    if (!has_escapes) {
        // Fast path: no escapes, just copy the raw bytes
        rask_string_from_bytes(out, start, (int64_t)(*p - start));
        if (**p == '"') (*p)++;
        return;
    }
    // Slow path: unescape. Reset and rebuild.
    *p = start;
    RaskStr s;
    rask_string_new(&s);
    while (**p && **p != '"') {
        if (**p == '\\' && *(*p + 1)) {
            char c = *(*p + 1);
            uint8_t byte;
            switch (c) {
                case '"': case '\\': case '/': byte = (uint8_t)c; break;
                case 'n': byte = '\n'; break;
                case 't': byte = '\t'; break;
                case 'r': byte = '\r'; break;
                default: byte = (uint8_t)c; break;
            }
            RaskStr tmp;
            rask_string_push_byte(&tmp, &s, byte);
            rask_string_free(&s);
            s = tmp;
            *p += 2;
        } else {
            RaskStr tmp;
            rask_string_push_byte(&tmp, &s, (uint8_t)**p);
            rask_string_free(&s);
            s = tmp;
            (*p)++;
        }
    }
    if (**p == '"') (*p)++;
    *out = s;
}

RaskJsonObj *rask_json_parse(const RaskStr *s) {
    RaskJsonObj *obj = (RaskJsonObj *)rask_alloc(sizeof(RaskJsonObj));
    memset(obj, 0, sizeof(RaskJsonObj));

    const char *p = rask_string_ptr(s);
    json_skip_ws(&p);
    if (*p != '{') return obj;
    p++;

    while (*p && *p != '}' && obj->count < JSON_MAX_FIELDS) {
        json_skip_ws(&p);
        if (*p == '}') break;
        if (*p == ',') { p++; json_skip_ws(&p); }

        if (*p != '"') break;
        RaskStr key;
        json_parse_string(&key, &p);
        struct RaskJsonField *f = &obj->fields[obj->count];
        snprintf(f->key, sizeof(f->key), "%s", rask_string_ptr(&key));
        rask_string_free(&key);

        json_skip_ws(&p);
        if (*p != ':') break;
        p++;
        json_skip_ws(&p);

        if (*p == '"') {
            f->type = JSON_STRING;
            json_parse_string(&f->str_val, &p);
        } else if (*p == 't' || *p == 'f') {
            f->type = JSON_BOOL;
            if (strncmp(p, "true", 4) == 0) { f->bool_val = 1; p += 4; }
            else if (strncmp(p, "false", 5) == 0) { f->bool_val = 0; p += 5; }
        } else if (*p == 'n' && strncmp(p, "null", 4) == 0) {
            f->type = JSON_STRING;
            rask_string_new(&f->str_val);
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

void rask_json_get_string(RaskStr *out, RaskJsonObj *obj, const char *key) {
    struct RaskJsonField *f = json_find_field(obj, key);
    if (!f || f->type != JSON_STRING) { rask_string_new(out); return; }
    // Copy the field's string value
    *out = f->str_val;
    rask_string_clone(out); // RC inc if heap
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

int64_t rask_json_decode(const RaskStr *s) {
    return (int64_t)(uintptr_t)rask_json_parse(s);
}

// ─── Error origin (ER15/ER16) ────────────────────────────────────

// Source file name for error origin formatting. Set by rask_main at startup.
static const char *rask_origin_file = "<unknown>";

void rask_set_origin_file(const char *file) {
    rask_origin_file = file;
}

// Read the origin_line field from a Result and format as a string.
// Result layout: [tag:8][origin_file:8][origin_line:8][payload:...]
void rask_result_origin(RaskStr *out, const void *result_ptr) {
    const int64_t *fields = (const int64_t *)result_ptr;
    int64_t origin_line = fields[2]; // offset 16 = origin_line
    if (origin_line > 0) {
        char buf[256];
        snprintf(buf, sizeof(buf), "%s:%lld", rask_origin_file, (long long)origin_line);
        rask_string_from(out, buf);
    } else {
        rask_string_from(out, "<no origin>");
    }
}

// ─── Resource tracking ──────────────────────────────────────────
// Simple consumed-flag tracker for ensure consumption cancellation (C1/C2).
// Each resource gets an integer ID via rask_resource_register().
// rask_resource_consume() marks it consumed.
// rask_resource_is_consumed() checks the flag (used before ensure cleanup).

#define RASK_MAX_RESOURCES 256

static struct {
    int8_t consumed;
    int64_t scope_depth;
} rask_resources[RASK_MAX_RESOURCES];
static int64_t rask_resource_next_id = 1;

int64_t rask_resource_register(int64_t scope_depth) {
    int64_t id = rask_resource_next_id++;
    if (id > 0 && id < RASK_MAX_RESOURCES) {
        rask_resources[id].consumed = 0;
        rask_resources[id].scope_depth = scope_depth;
    }
    return id;
}

void rask_resource_consume(int64_t id) {
    if (id > 0 && id < RASK_MAX_RESOURCES) {
        rask_resources[id].consumed = 1;
    }
}

int64_t rask_resource_is_consumed(int64_t id) {
    if (id > 0 && id < RASK_MAX_RESOURCES) {
        return rask_resources[id].consumed;
    }
    return 0;
}

void rask_resource_scope_check(int64_t scope_depth) {
    // Check for unconsumed resources at this scope depth.
    // For now, no-op — the ownership checker catches this statically.
    (void)scope_depth;
}

// ─── Runtime checks ──────────────────────────────────────────────

// When RASK_RUNTIME_CHECKS=1 is set, null-pointer and validity checks
// are active in the C runtime. Debug builds (RASK_DEBUG) always check.
int rask_runtime_checks_enabled = 0;

// ─── Entry point ──────────────────────────────────────────────────

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    const char *checks_env = getenv("RASK_RUNTIME_CHECKS");
    if (checks_env && checks_env[0] == '1') {
        rask_runtime_checks_enabled = 1;
    }
    rask_args_init(argc, argv);
    rask_main();
    return 0;
}
