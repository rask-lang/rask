// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask test harness — run test functions, catch panics, report results as JSON.
// Called from generated test runner entry points.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <time.h>

typedef void (*test_fn)(void);

// Panic recovery declarations from panic.c
extern RaskPanicCtx *rask_panic_install(void);
extern void          rask_panic_remove(void);
extern jmp_buf      *rask_panic_jmpbuf(void);
extern void          rask_panic_activate(void);
extern char         *rask_panic_take_message(void);

static int64_t clock_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

// Thread-local test state for skip/expect_fail
static __thread int rask_test_skipped = 0;
static __thread const char *rask_test_skip_reason = NULL;
static __thread int rask_test_expects_fail = 0;

void rask_test_skip(const char *reason) {
    rask_test_skipped = 1;
    rask_test_skip_reason = reason;
    extern void rask_panic(const char *msg);
    rask_panic(reason);
}

// Just set the skip flag — caller handles unwinding via panic
void rask_test_skip_flag(void) {
    rask_test_skipped = 1;
}

void rask_test_expect_fail(void) {
    rask_test_expects_fail = 1;
}

// assert_eq(got, expected) — compare two i64 values, panic with diff on mismatch
void rask_assert_eq(int64_t got, int64_t expected) {
    if (got != expected) {
        char buf[256];
        snprintf(buf, sizeof(buf),
                 "assert_eq failed\n  got:      %lld\n  expected: %lld",
                 (long long)got, (long long)expected);
        extern void rask_panic(const char *msg);
        rask_panic(buf);
    }
}

// Thread-local check failure tracking
static __thread int rask_check_failures = 0;
static __thread char rask_check_last_msg[512] = {0};

// check_fail — record failure without unwinding (test continues)
void rask_check_fail(const char *msg) {
    rask_check_failures++;
    if (msg) {
        snprintf(rask_check_last_msg, sizeof(rask_check_last_msg), "%s", msg);
    } else {
        snprintf(rask_check_last_msg, sizeof(rask_check_last_msg), "check failed");
    }
    fprintf(stderr, "check failed: %s\n", rask_check_last_msg);
}

// Run a single test: catch panics, print JSON result line.
// Returns 0 on pass, 1 on fail.
int rask_test_run(test_fn fn, const char *name) {
    // Reset per-test state
    rask_test_skipped = 0;
    rask_test_skip_reason = NULL;
    rask_test_expects_fail = 0;
    rask_check_failures = 0;
    rask_check_last_msg[0] = '\0';

    rask_panic_install();
    jmp_buf *jb = rask_panic_jmpbuf();

    int64_t start = clock_ns();
    int failed = 0;
    char *error_msg = NULL;

    if (setjmp(*jb) == 0) {
        rask_panic_activate();
        fn();
    } else {
        // Returned via longjmp from rask_panic
        failed = 1;
        error_msg = rask_panic_take_message();
    }

    int64_t elapsed_ns = clock_ns() - start;
    rask_panic_remove();

    int was_skipped = rask_test_skipped;
    int expects_fail = rask_test_expects_fail;

    // Handle skipped tests — use panic message as skip reason
    if (was_skipped) {
        printf("{\"name\":\"%s\",\"passed\":true,\"duration_ns\":%lld,\"skipped\":\"",
               name, (long long)elapsed_ns);
        const char *reason = error_msg ? error_msg : (rask_test_skip_reason ? rask_test_skip_reason : "");
        for (const char *p = reason; *p; p++) {
            if (*p == '"') printf("\\\"");
            else if (*p == '\\') printf("\\\\");
            else if (*p == '\n') printf("\\n");
            else putchar(*p);
        }
        printf("\"}\n");
        if (error_msg) free(error_msg);
        fflush(stdout);
        return 0;
    }

    // Handle expect_fail: invert pass/fail
    if (expects_fail) {
        if (failed) {
            // Expected failure occurred — pass
            if (error_msg) free(error_msg);
            printf("{\"name\":\"%s\",\"passed\":true,\"duration_ns\":%lld}\n",
                   name, (long long)elapsed_ns);
            fflush(stdout);
            return 0;
        } else {
            // Expected failure but test passed — fail
            printf("{\"name\":\"%s\",\"passed\":false,\"duration_ns\":%lld,\"error\":\"expected failure but test passed\"}\n",
                   name, (long long)elapsed_ns);
            fflush(stdout);
            return 1;
        }
    }

    // Check failures also count as failure
    if (!failed && rask_check_failures > 0) {
        failed = 1;
        // Use last check message as error
        error_msg = strdup(rask_check_last_msg);
    }

    // Normal case: escape quotes in error message for JSON
    if (failed) {
        printf("{\"name\":\"%s\",\"passed\":false,\"duration_ns\":%lld,\"error\":\"",
               name, (long long)elapsed_ns);
        if (error_msg) {
            for (const char *p = error_msg; *p; p++) {
                if (*p == '"') printf("\\\"");
                else if (*p == '\\') printf("\\\\");
                else if (*p == '\n') printf("\\n");
                else putchar(*p);
            }
            free(error_msg);
        } else {
            printf("(unknown)");
        }
        printf("\"}\n");
    } else {
        printf("{\"name\":\"%s\",\"passed\":true,\"duration_ns\":%lld}\n",
               name, (long long)elapsed_ns);
    }
    fflush(stdout);

    return failed;
}
