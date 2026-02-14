// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Minimal Rask runtime — provides built-in functions for native-compiled programs.
// Linked with the object file produced by rask-codegen.

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

void rask_print_string(const char *ptr) {
    if (ptr) {
        fputs(ptr, stdout);
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

// ─── Entry point ──────────────────────────────────────────────────

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    rask_main();
    return 0;
}
