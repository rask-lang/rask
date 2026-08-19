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
#include <errno.h>

// Forward declaration — user's main function, exported from the Rask module as rask_main
extern void rask_main(void);

// ─── Print functions ──────────────────────────────────────────────

// One implementation per type, parameterized on the stream; the stdout and
// stderr entry points below are thin wrappers. Codegen calls the wrappers
// directly — it picks the pair from the builtin's name, so there's no
// stream argument to thread through MIR.
static void rask_fput_i64(FILE *out, int64_t val) {
    fprintf(out, "%lld", (long long)val);
}

static void rask_fput_bool(FILE *out, int8_t val) {
    fputs(val ? "true" : "false", out);
}

static void rask_fput_f64(FILE *out, double val) {
    char buf[RASK_F64_BUF_SIZE];
    rask_fmt_double(buf, sizeof(buf), val);
    fputs(buf, out);
}

static void rask_fput_f32(FILE *out, float val) {
    char buf[RASK_F64_BUF_SIZE];
    rask_fmt_float(buf, sizeof(buf), val);
    fputs(buf, out);
}

static void rask_fput_char(FILE *out, int32_t codepoint) {
    if (codepoint < 0x80) {
        putc(codepoint, out);
    } else if (codepoint < 0x800) {
        putc(0xC0 | (codepoint >> 6), out);
        putc(0x80 | (codepoint & 0x3F), out);
    } else if (codepoint < 0x10000) {
        putc(0xE0 | (codepoint >> 12), out);
        putc(0x80 | ((codepoint >> 6) & 0x3F), out);
        putc(0x80 | (codepoint & 0x3F), out);
    } else {
        putc(0xF0 | (codepoint >> 18), out);
        putc(0x80 | ((codepoint >> 12) & 0x3F), out);
        putc(0x80 | ((codepoint >> 6) & 0x3F), out);
        putc(0x80 | (codepoint & 0x3F), out);
    }
}

static void rask_fput_u64(FILE *out, uint64_t val) {
    fprintf(out, "%llu", (unsigned long long)val);
}

static void rask_fput_string(FILE *out, const RaskStr *s) {
    fputs(rask_string_ptr(s), out);
}

// ─── One print call, one line ──────────────────────────────
//
// A single `println("a {b} c")` is at least two writes — the text and the
// newline — and a multi-argument one is more. Two threads printing at once
// used to splice mid-line: "line 0 from thread 2line 194 from thread 1".
// Holding the stream's lock across the whole call makes one call atomic.
// stdio's own per-call locking is recursive, so the individual fputs inside
// just re-take a lock this thread already has.
//
// The depth counter is what makes it safe to panic mid-line: a longjmp out of
// here would otherwise leave the lock held forever and deadlock every later
// print. rask_print_unlock_all() runs on the panic path and unwinds it.
static __thread int print_lock_depth;
static __thread int eprint_lock_depth;

void rask_print_lock(void)    { flockfile(stdout); print_lock_depth++; }
void rask_print_unlock(void)  { if (print_lock_depth > 0) { print_lock_depth--; funlockfile(stdout); } }
void rask_eprint_lock(void)   { flockfile(stderr); eprint_lock_depth++; }
void rask_eprint_unlock(void) { if (eprint_lock_depth > 0) { eprint_lock_depth--; funlockfile(stderr); } }

// Drop whatever this thread still holds. Called before a panic longjmps past
// the matching unlock, and before the panic reporter writes to stderr.
void rask_print_unlock_all(void) {
    while (print_lock_depth > 0)  { print_lock_depth--;  funlockfile(stdout); }
    while (eprint_lock_depth > 0) { eprint_lock_depth--; funlockfile(stderr); }
}

void rask_print_i64(int64_t val) { rask_fput_i64(stdout, val); }
void rask_print_bool(int8_t val) { rask_fput_bool(stdout, val); }
void rask_print_f64(double val) { rask_fput_f64(stdout, val); }
void rask_print_f32(float val) { rask_fput_f32(stdout, val); }
void rask_print_char(int32_t codepoint) { rask_fput_char(stdout, codepoint); }
void rask_print_u64(uint64_t val) { rask_fput_u64(stdout, val); }
void rask_print_string(const RaskStr *s) { rask_fput_string(stdout, s); }
void rask_print_newline(void) { putchar('\n'); }

void rask_eprint_i64(int64_t val) { rask_fput_i64(stderr, val); }
void rask_eprint_bool(int8_t val) { rask_fput_bool(stderr, val); }
void rask_eprint_f64(double val) { rask_fput_f64(stderr, val); }
void rask_eprint_f32(float val) { rask_fput_f32(stderr, val); }
void rask_eprint_char(int32_t codepoint) { rask_fput_char(stderr, codepoint); }
void rask_eprint_u64(uint64_t val) { rask_fput_u64(stderr, val); }
void rask_eprint_string(const RaskStr *s) { rask_fput_string(stderr, s); }
void rask_eprint_newline(void) { putc('\n', stderr); }

// ─── Runtime support ──────────────────────────────────────────────

void rask_exit(int64_t code) {
    exit((int)code);
}

// struct.targets/EX4: an error returned from main is a failed run — status 1.
// A panic is 101 and goes through rask_panic instead. `msg` is optional; when
// the error type has no message() there's nothing to print but the fact.
// Debug aid: fill the stack region that later frames will occupy with a nonzero
// pattern. A codegen path that reads a slot it never wrote sees 0 on a
// freshly-mapped stack and looks correct, which is why such bugs only show up
// after a program has run a while. Poisoning makes the read deterministic.
// Opt-in via RASK_POISON_STACK; when off this costs one load.
__attribute__((noinline)) void rask_poison_stack(void) {
    static int enabled = -1;
    if (enabled < 0) {
        const char *e = getenv("RASK_POISON_STACK");
        enabled = (e && *e && *e != '0') ? 1 : 0;
    }
    if (!enabled) {
        return;
    }
    volatile unsigned char buf[192 * 1024];
    memset((void *)buf, 0xAA, sizeof buf);
    (void)buf[0];
}

_Noreturn void rask_main_error_exit(const RaskStr *msg) {
    fflush(stdout);
    if (msg && rask_string_len(msg) > 0) {
        fprintf(stderr, "error: %.*s\n", (int)rask_string_len(msg), rask_string_ptr(msg));
    } else {
        fprintf(stderr, "error: main returned an error\n");
    }
    exit(1);
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

// Chars reach here as scalars, same as integers — but reporting `120 == 121`
// for `'x' == 'y'` tells the reader nothing. Print the characters.
void rask_assert_fail_cmp_char(int64_t left, int64_t right,
                               const char *op, const char *file,
                               int32_t line, int32_t col) {
    RaskStr ls, rs;
    rask_char_to_string(&ls, (int32_t)left);
    rask_char_to_string(&rs, (int32_t)right);
    const char *lbuf = rask_string_ptr(&ls);
    const char *rbuf = rask_string_ptr(&rs);
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: '%s' %s '%s' (left: '%s', right: '%s')",
             lbuf, op ? op : "?", rbuf, lbuf, rbuf);
    rask_panic_at(file, line, col, buf);
}

// The two string operands arrive as `RaskStr *`, not as C strings. Read as
// `const char *` they printed the struct's first bytes — which for a short
// string *are* the characters, because those live inline, and for anything
// past the inline cap are a pointer. So a long string's assertion message came
// out as four bytes of garbage while a short one looked perfect (#848).
void rask_assert_fail_cmp_str(const RaskStr *left, const RaskStr *right,
                              const char *op, const char *file,
                              int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: \"%.*s\" %s \"%.*s\"",
             left ? (int)rask_string_len(left) : 6,
             left ? rask_string_ptr(left) : "(null)",
             op ? op : "?",
             right ? (int)rask_string_len(right) : 6,
             right ? rask_string_ptr(right) : "(null)");
    rask_panic_at(file, line, col, buf);
}

void rask_assert_fail_cmp_f64(double left, double right,
                              const char *op, const char *file,
                              int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: %g %s %g (left: %g, right: %g)",
             left, op ? op : "?", right, left, right);
    rask_panic_at(file, line, col, buf);
}

// assert_eq reports got/expected rather than left/right (testing A4): the
// first argument is what the code produced, the second what the test wants.
// The comparison itself happens in generated code — these only format.
static _Noreturn void assert_eq_fail_fmt(const char *got, const char *expected,
                                         const char *file, int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assert_eq failed\n  got:      %s\n  expected: %s", got, expected);
    rask_panic_at(file, line, col, buf);
}

void rask_assert_eq_fail_i64(int64_t got, int64_t expected,
                             const char *file, int32_t line, int32_t col) {
    char g[32], e[32];
    snprintf(g, sizeof(g), "%lld", (long long)got);
    snprintf(e, sizeof(e), "%lld", (long long)expected);
    assert_eq_fail_fmt(g, e, file, line, col);
}

void rask_assert_eq_fail_bool(int64_t got, int64_t expected,
                              const char *file, int32_t line, int32_t col) {
    assert_eq_fail_fmt(got ? "true" : "false", expected ? "true" : "false",
                       file, line, col);
}

void rask_assert_eq_fail_char(int64_t got, int64_t expected,
                              const char *file, int32_t line, int32_t col) {
    RaskStr gs, es;
    rask_char_to_string(&gs, (int32_t)got);
    rask_char_to_string(&es, (int32_t)expected);
    char g[16], e[16];
    snprintf(g, sizeof(g), "'%s'", rask_string_ptr(&gs));
    snprintf(e, sizeof(e), "'%s'", rask_string_ptr(&es));
    assert_eq_fail_fmt(g, e, file, line, col);
}

void rask_assert_eq_fail_f64(double got, double expected,
                             const char *file, int32_t line, int32_t col) {
    char g[RASK_F64_BUF_SIZE], e[RASK_F64_BUF_SIZE];
    rask_fmt_double(g, sizeof(g), got);
    rask_fmt_double(e, sizeof(e), expected);
    assert_eq_fail_fmt(g, e, file, line, col);
}

void rask_assert_eq_fail_str(const RaskStr *got, const RaskStr *expected,
                             const char *file, int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assert_eq failed\n  got:      \"%.*s\"\n  expected: \"%.*s\"",
             got ? (int)rask_string_len(got) : 6,
             got ? rask_string_ptr(got) : "(null)",
             expected ? (int)rask_string_len(expected) : 6,
             expected ? rask_string_ptr(expected) : "(null)");
    rask_panic_at(file, line, col, buf);
}

// Aggregates (structs, enums, tuples) compare fine but have no one-line
// rendering here, so the message names the failure without a value diff.
void rask_assert_eq_fail(const char *file, int32_t line, int32_t col) {
    rask_panic_at(file, line, col, "assert_eq failed: values differ");
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

// Writes the line, or says why there isn't one. Distinguishing EOF from a
// blank line is what stops every `loop { }` over stdin spinning once the input
// runs out; distinguishing EOF from a read *error* is what stops a failure
// being reported as end-of-input (#682).
int64_t rask_io_read_line(RaskStr *out, RaskStr *err_out) {
    char buf[4096];
    rask_string_new(err_out);
    if (!fgets(buf, sizeof(buf), stdin)) {
        rask_string_new(out);
        if (ferror(stdin)) {
            rask_string_from(err_out, rask_io_error_text(errno));
            return RASK_STROUT_ERROR;
        }
        return RASK_STROUT_EOF;
    }
    size_t len = strlen(buf);
    if (len > 0 && buf[len - 1] == '\n') buf[--len] = '\0';
    if (len > 0 && buf[len - 1] == '\r') buf[--len] = '\0';
    rask_string_from_bytes(out, buf, (int64_t)len);
    return RASK_STROUT_OK;
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

// Did `fopen` fail? The handle *is* the value a `File` carries, and a failed
// open is NULL — so `fs.open` can check it and build the IoError in Rask,
// where both backends read the same source. Before this the NULL sailed
// through as a successful `File`, and the failure only surfaced on the first
// read as "file handle is closed" (#858).
int64_t rask_file_is_null(int64_t file) {
    return file == 0 ? 1 : 0;
}

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

// ─── errno → IoError ──────────────────────────────────────────────
//
// The self-hosted `fs.*` functions used to throw errno away and hand back
// `IoError.Other("could not open file")`, while the interpreter built the
// real variant from Rust's std::io::Error — so one failure read two
// different ways depending on the backend (#674). These three let the Rask
// side rebuild what the interpreter produces. The interpreter is the
// reference, so the wording below matches its `std::io::Error` output.

// `errno` is a macro over a thread-local, so Rask can't name it directly.
int32_t rask_io_errno(void) { return errno; }

// IoError variant index for an errno, in stdlib/io.rk declaration order.
// The E* values differ across platforms, so the mapping lives on this side
// rather than as numbers hardcoded in Rask.
int32_t rask_io_error_kind(int32_t err) {
    switch (err) {
        case ENOENT: return 0;                  // NotFound
        case EACCES: case EPERM: return 1;      // PermissionDenied
        case EEXIST: return 2;                  // AlreadyExists
        case EPIPE: return 3;                   // BrokenPipe
        case ECONNRESET: return 4;              // ConnectionReset
        case ETIMEDOUT: return 5;               // TimedOut
        default: return 7;                      // Other
    }
}

// "No such file or directory (os error 2)" — the exact shape Rust's
// std::io::Error prints, so both backends say the same thing. The buffer is
// thread-local and the caller copies out of it immediately via string.from_raw.
const char *rask_io_error_text(int32_t err) {
    static _Thread_local char buf[256];
    snprintf(buf, sizeof(buf), "%s (os error %d)", strerror(err), err);
    return buf;
}

// strlen under a name Rask can declare without clashing with fs.rk's own
// `strlen` extern — one C symbol may only be declared once across stdlib.
uint64_t rask_io_cstr_len(const char *s) { return s ? (uint64_t)strlen(s) : 0; }

#include <dirent.h>
// Extract name from dirent (Rask can't access C struct fields)
const char *rask_dirent_name(void *entry) { return ((struct dirent *)entry)->d_name; }

// Stat helpers — return individual fields so Rask doesn't need struct access
// One stat, then read the fields off it. Rask can't hold a `struct stat`, and
// the three separate helpers below are three syscalls on a file that can change
// between them — plus none of them can tell "size 0" from "no such file". These
// report failure once and leave errno set for `IoError.last_os_error()` (#674).
static __thread struct stat rask_stat_buf;

int32_t rask_stat_load(const char *path) {
    return stat(path, &rask_stat_buf) == 0 ? 0 : -1;
}
// Unsigned: st_size is non-negative after a successful stat, and `Metadata.size`
// is a u64 — `i64 as u64` is a sign reinterpret the cast rules reject (CV3).
uint64_t rask_stat_loaded_size(void) { return (uint64_t)rask_stat_buf.st_size; }
int64_t rask_stat_loaded_mtime(void) { return (int64_t)rask_stat_buf.st_mtime; }
int64_t rask_stat_loaded_atime(void) { return (int64_t)rask_stat_buf.st_atime; }

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

// Read from the current position to EOF. Returns 0 on success, 1 on failure —
// `File.read_text` is `string or IoError`, and the caller needs the tag.
//
// Chunked rather than sized by ftell/fseek: a pipe or a terminal has no size to
// seek to, and a stream opened write-only reports one anyway (0), so the old
// version answered Ok("") for a file it could not read at all.
int64_t rask_file_read_all(RaskStr *out, int64_t file, RaskStr *err_out) {
    FILE *f = (FILE *)(uintptr_t)file;
    rask_string_new(err_out);
    if (!f) {
        rask_string_new(out);
        rask_string_from(err_out, "file handle is closed");
        return RASK_STROUT_ERROR;
    }

    size_t cap = 4096, len = 0;
    char *buf = (char *)rask_alloc((int64_t)cap);
    for (;;) {
        if (len == cap) {
            size_t new_cap = cap * 2;
            char *grown = (char *)rask_alloc((int64_t)new_cap);
            memcpy(grown, buf, len);
            rask_free(buf);
            buf = grown;
            cap = new_cap;
        }
        size_t n = fread(buf + len, 1, cap - len, f);
        len += n;
        if (n == 0) break;
    }
    if (ferror(f)) {
        // The reason, not just the fact. Reading a write-only descriptor is
        // EBADF, and "unexpected end of file" said nothing about that (#682).
        rask_string_from(err_out, rask_io_error_text(errno));
        rask_free(buf);
        rask_string_new(out);
        return RASK_STROUT_ERROR;
    }
    rask_string_from_bytes(out, buf, (int64_t)len);
    rask_free(buf);
    return RASK_STROUT_OK;
}

// Returns a RaskVec<u8>* (cast to int64_t), or -1 if the handle is null.
int64_t rask_file_read_bytes(int64_t file) {
    FILE *f = (FILE *)(uintptr_t)file;
    if (!f) return -1;
    long start = ftell(f);
    fseek(f, 0, SEEK_END);
    long end = ftell(f);
    fseek(f, start, SEEK_SET);
    long size = end - start;
    if (size < 0) size = 0;
    char *buf = (char *)rask_alloc((int64_t)size + 1);
    size_t n = fread(buf, 1, (size_t)size, f);
    RaskVec *v = rask_vec_from_static(buf, (int64_t)n, 1);
    rask_free(buf);
    return (int64_t)(uintptr_t)v;
}

// Writes a RaskVec<u8> to the file. Returns 0 on success, -1 on a null handle.
int64_t rask_file_write_bytes(int64_t file, int64_t vec_ptr) {
    if (!file) return -1;
    RaskVec *v = (RaskVec *)(uintptr_t)vec_ptr;
    rask_fwrite_vec(file, v);
    return 0;
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

// Split "host:port" on the last colon. Returns 0 when there is no colon or
// either half is unusable — an address is host *and* port, and guessing a
// missing half is how "not-an-address" came to bind 0.0.0.0:0 and report
// success (#863).
static int net_split_addr(const RaskStr *addr, char *host, size_t host_cap,
                          char *port, size_t port_cap) {
    const char *a = rask_string_ptr(addr);
    const char *colon = strrchr(a, ':');
    if (!colon) return 0;
    size_t hlen = (size_t)(colon - a);
    size_t plen = strlen(colon + 1);
    if (hlen == 0 || hlen >= host_cap) return 0;
    if (plen == 0 || plen >= port_cap) return 0;
    memcpy(host, a, hlen);
    host[hlen] = '\0';
    memcpy(port, colon + 1, plen + 1);
    return 1;
}

int64_t rask_net_tcp_listen(const RaskStr *addr) {
    char host[256];
    char port_str[16];
    if (!net_split_addr(addr, host, sizeof(host), port_str, sizeof(port_str))) {
        return -2;
    }

    // getaddrinfo rather than inet_pton, so "localhost:0" resolves the way it
    // does on the interpreter side — and so a name that resolves to nothing is
    // a failure instead of a silent 0.0.0.0.
    struct addrinfo hints, *result;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;

    if (getaddrinfo(host, port_str, &hints, &result) != 0) {
        return -2;
    }

    int fd = socket(result->ai_family, result->ai_socktype, result->ai_protocol);
    if (fd < 0) {
        freeaddrinfo(result);
        return -1;
    }

    int opt = 1;
    setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    if (bind(fd, result->ai_addr, result->ai_addrlen) < 0) {
        close(fd);
        freeaddrinfo(result);
        return -1;
    }
    freeaddrinfo(result);
    if (listen(fd, 128) < 0) {
        close(fd);
        return -1;
    }
    return (int64_t)fd;
}

// True when a handle is a failed syscall's -1. The check can't live in the
// adapter that turns a negative return into the error side, because that
// adapter has no way to build an `IoError` — it's a Rask enum — and it left the
// raw -1 in the payload, which traps when matched (#863).
int8_t rask_net_is_invalid(int64_t handle) {
    return handle < 0 ? 1 : 0;
}

// -2 specifically: the address parsed but named nothing that resolves.
// `getaddrinfo` doesn't set errno, so asking `last_os_error()` about it
// answered "Success (os error 0)" for a failure (#863).
int8_t rask_net_is_unresolved(int64_t handle) {
    return handle == -2 ? 1 : 0;
}

int64_t rask_net_tcp_accept(int64_t listen_fd) {
    int client = accept((int)listen_fd, NULL, NULL);
    return (int64_t)client;
}

int64_t rask_net_tcp_connect(const RaskStr *addr) {
    char host[256];
    char port_str[16];
    if (!net_split_addr(addr, host, sizeof(host), port_str, sizeof(port_str))) {
        return -2;
    }

    // Resolve hostname via getaddrinfo (handles both IPs and DNS names)
    struct addrinfo hints, *result;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    int err = getaddrinfo(host, port_str, &hints, &result);
    if (err != 0) return -2;

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

// ─── Standard streams ─────────────────────────────────────────────
//
// Through stdio rather than write(2), so a handle's output interleaves with
// `println` in the order the program wrote it — `println` is buffered, and a
// raw fd write would jump the queue.
//
// The stream is named by a number the Rask handle carries: 1 = stdout,
// 2 = stderr, 0 = stdin. `Stdout`/`Stderr`/`Stdin` used to be empty structs with
// no native entry point for anything (#859), which left the stream id nowhere to
// live.
static FILE *std_stream(int64_t which) {
    if (which == 2) return stderr;
    if (which == 0) return stdin;
    return stdout;
}

int64_t rask_io_std_write_text(int64_t which, int64_t str_ptr) {
    const RaskStr *s = (const RaskStr *)(uintptr_t)str_ptr;
    if (!s) return 0;
    FILE *f = std_stream(which);
    int64_t len = rask_string_len(s);
    if (len <= 0) return 0;
    size_t n = fwrite(rask_string_ptr(s), 1, (size_t)len, f);
    return n == (size_t)len ? len : -1;
}

// The bytes are gathered first: a `Vec<u8>` is contiguous only when the runtime
// built it, and compiled Rask code gives every element its own slot (#863).
int64_t rask_io_std_write_bytes(int64_t which, int64_t vec_ptr) {
    const RaskVec *v = (const RaskVec *)(uintptr_t)vec_ptr;
    int64_t len = rask_vec_len(v);
    if (len <= 0) return 0;
    char *bytes = (char *)rask_alloc(len);
    for (int64_t i = 0; i < len; i++) {
        const uint8_t *b = (const uint8_t *)rask_vec_get(v, i);
        bytes[i] = b ? (char)*b : 0;
    }
    size_t n = fwrite(bytes, 1, (size_t)len, std_stream(which));
    rask_free(bytes);
    return n == (size_t)len ? len : -1;
}

int64_t rask_io_std_flush(int64_t which) {
    return fflush(std_stream(which)) == 0 ? 0 : -1;
}

// Read up to `max` bytes from stdin, stopping at end of input. Returns a
// `Vec<u8>` cast to i64.
int64_t rask_io_std_read_bytes(int64_t max) {
    RaskVec *v = rask_vec_new(1);
    if (max <= 0) return (int64_t)(uintptr_t)v;
    for (int64_t i = 0; i < max; i++) {
        int c = fgetc(stdin);
        if (c == EOF) break;
        uint8_t byte = (uint8_t)c;
        rask_vec_push(v, &byte);
    }
    return (int64_t)(uintptr_t)v;
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
            // Append in place. Routing through a temp and freeing the
            // accumulator frees the buffer the temp just took ownership of —
            // the append primitive reuses a sole-owned buffer (#414).
            rask_string_append_cstr(&out, &out, rask_string_ptr(key));
            rask_string_append_cstr(&out, &out, ": ");
            rask_string_append_cstr(&out, &out, rask_string_ptr(val));
            rask_string_append_cstr(&out, &out, "\r\n");
        }
        rask_vec_free(keys);
    }

    // Content-Length header
    snprintf(line_buf, sizeof(line_buf),
             "Content-Length: %lld\r\n\r\n", (long long)body_len);
    rask_string_append_cstr(&out, &out, line_buf);

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

// Read all available data from a TCP connection into a Vec<u8>.
// Returns the RaskVec* (cast to int64_t), or -1 on error.
int64_t rask_net_read_bytes(int64_t fd) {
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
    RaskVec *v = rask_vec_from_static(buf, total, 1);
    rask_free(buf);
    return (int64_t)(uintptr_t)v;
}

// Write all bytes in a Vec<u8> to a TCP connection. Returns 0 on success, -1 on error.
//
// The bytes are gathered before the write because a `Vec<u8>` is only
// contiguous when the runtime built it. Compiled Rask code gives every element
// its own 8-byte slot, so taking element 0's address as the start of a byte
// buffer sent every second byte as seven NULs: "hello" left as
// "h\0\0\0\0\0\0\0e\0…" and the far end read one character (#863). Same
// per-element read `rask_fs_write_bytes` already does, with one syscall instead
// of one per byte.
int64_t rask_net_write_bytes(int64_t fd, int64_t vec_ptr) {
    const RaskVec *v = (const RaskVec *)(intptr_t)vec_ptr;
    int64_t len = rask_vec_len(v);
    if (len <= 0) return 0;
    char *bytes = (char *)rask_alloc(len);
    for (int64_t i = 0; i < len; i++) {
        const uint8_t *b = (const uint8_t *)rask_vec_get(v, i);
        bytes[i] = b ? (char)*b : 0;
    }
    int64_t written = 0;
    while (written < len) {
        ssize_t n = write((int)fd, bytes + written, (size_t)(len - written));
        if (n < 0) {
            rask_free(bytes);
            return -1;
        }
        written += n;
    }
    rask_free(bytes);
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

// Local address a listener/connection is bound to (TcpListener.local_addr).
void rask_net_local_addr(RaskStr *out, int64_t fd) {
    struct sockaddr_in addr;
    socklen_t addrlen = sizeof(addr);
    if (getsockname((int)fd, (struct sockaddr *)&addr, &addrlen) < 0) {
        rask_string_from(out, "unknown");
        return;
    }
    char ip[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &addr.sin_addr, ip, sizeof(ip));
    char buf[64];
    snprintf(buf, sizeof(buf), "%s:%d", ip, ntohs(addr.sin_port));
    rask_string_from(out, buf);
}

// `fs.metadata` is implemented in Rask now (stdlib/fs.rk). The C version
// returned NULL on failure, which codegen handed back as a valid `Metadata`,
// so the `catch` never ran and reading a field went through null — a `T or E`
// needs its error built on the Rask side (#674). `rask_stat_load` and the
// `rask_stat_loaded_*` readers above are what replaced it.


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
        rask_string_append_cstr(&req, &req, line);
    }
    rask_string_append_cstr(&req, &req, "\r\n");

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
    char num[RASK_F64_BUF_SIZE + 1];
    num[0] = ':';
    rask_fmt_double(num + 1, sizeof(num) - 1, val);
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
    char num[RASK_F64_BUF_SIZE];
    rask_fmt_double(num, sizeof(num), val);
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

// Flush stdout, then let the signal kill us as it would have. stdout is fully
// buffered when it's a pipe, so everything printed before a crash used to be
// lost — which puts the crash earlier than it really was, every time (#605).
// Reset to the default handler and re-raise rather than exiting, so the parent
// still sees "died on signal N".
static void rask_fatal_signal(int sig) {
    fflush(stdout);
    fflush(stderr);
    signal(sig, SIG_DFL);
    raise(sig);
}

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    signal(SIGSEGV, rask_fatal_signal);
    signal(SIGILL, rask_fatal_signal);
    signal(SIGBUS, rask_fatal_signal);
    signal(SIGFPE, rask_fatal_signal);
    signal(SIGABRT, rask_fatal_signal);
    const char *checks_env = getenv("RASK_RUNTIME_CHECKS");
    if (checks_env && checks_env[0] == '1') {
        rask_runtime_checks_enabled = 1;
    }
    rask_args_init(argc, argv);
    rask_poison_stack();
    rask_main();
    // O4: a detached task's panic report can't be lost to process exit.
    rask_await_detached_tasks();
    return 0;
}
