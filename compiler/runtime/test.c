// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask test harness — run test functions, catch panics, report results as JSON.
// Called from generated test runner entry points.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
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

// Run a single test: catch panics, print JSON result line.
// Returns 0 on pass, 1 on fail.
int rask_test_run(test_fn fn, const char *name) {
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

    // Escape quotes in error message for JSON
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
