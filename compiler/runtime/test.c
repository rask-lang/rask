// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask test harness — run test functions, catch panics, report results as JSON.
// Called from generated test runner entry points.

#include "rask_runtime.h"
#include "sim.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <setjmp.h>
#include <time.h>
#include <unistd.h>

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

// Thread-local check failure tracking.
//
// `check` exists to collect several failures in one run (std.testing/A2), so
// reporting only the last one throws away the reason anyone reached for it —
// a test with three failing checks read the same as a test with one. The
// interpreter has always joined them with "; "; this matches it.
static __thread int rask_check_failures = 0;
static __thread char rask_check_last_msg[2048] = {0};

// check_fail — record failure without unwinding (test continues)
//
// The comparison reporters build their own "check failed: a == b (…)" and pass
// it whole, so nothing is prefixed here. A hand-written `check(cond, "msg")`
// passes only the message, and `rask_check_fail_msg_at` below is what gives it
// the same `file:line:` an `assert` with a message gets — without it a failing
// check read like an ordinary `print`.
void rask_check_fail(const char *msg) {
    rask_check_failures++;
    const char *text = msg ? msg : "check failed";

    size_t used = strlen(rask_check_last_msg);
    if (used == 0) {
        snprintf(rask_check_last_msg, sizeof(rask_check_last_msg), "%s", text);
    } else if (used + 2 < sizeof(rask_check_last_msg)) {
        // Truncation is silent by design past this point: the buffer is
        // thread-local and fixed, and a test with enough failing checks to fill
        // 2KB has already told the reader what they needed from the first few.
        snprintf(rask_check_last_msg + used, sizeof(rask_check_last_msg) - used,
                 "; %s", text);
    }
    fprintf(stderr, "%s\n", text);
}

void rask_check_fail_msg_at(const char *msg, const char *file,
                            int32_t line, int32_t col) {
    char buf[RASK_PANIC_MSG_MAX];
    if (file) {
        (void)col;
        snprintf(buf, sizeof(buf), "%s:%d: %s", file, (int)line,
                 msg ? msg : "check failed");
    } else {
        snprintf(buf, sizeof(buf), "%s", msg ? msg : "check failed");
    }
    rask_check_fail(buf);
}

// A test name is a string literal, so it can hold a quote, a backslash or a
// newline. The error message was escaped for the JSON line and the name wasn't,
// so a name containing `"` ended the JSON string early and the CLI's reader
// stopped there — the rest of the name simply vanished (#849).
static void json_print_escaped(const char *s) {
    for (const char *p = s; *p; p++) {
        if (*p == '"') printf("\\\"");
        else if (*p == '\\') printf("\\\\");
        else if (*p == '\n') printf("\\n");
        else if (*p == '\r') printf("\\r");
        else if (*p == '\t') printf("\\t");
        else putchar(*p);
    }
}

// Every result record starts with the mark the runner passed in
// RASK_TEST_RECORD_MARK, so the runner can tell records from what the tests
// print. It is taken out of the environment before any Rask code runs, so a
// test can't read it, and a program the test starts doesn't inherit it. Run
// by hand, with no mark, the records are bare JSON lines.
static char *record_mark_value;

__attribute__((constructor)) static void record_mark_take(void) {
    const char *mark = getenv("RASK_TEST_RECORD_MARK");
    record_mark_value = strdup(mark ? mark : "");
    unsetenv("RASK_TEST_RECORD_MARK");
}

static const char *record_mark(void) {
    return record_mark_value ? record_mark_value : "";
}

// ─── Sim mode ──────────────────────────────────────────────
//
// A sim test runs alone in its process (sim/I6): the runner starts the binary
// once per test, picking it in RASK_SIM_TEST and handing over its seed in
// RASK_SIM_SEED. Every other test is skipped without a word, and the process
// exits as soon as the chosen one is reported, so no task, environment
// variable or allocation outlives the test that made it.

#ifdef RASK_SIM
extern void rask_const_free(void);   // generated with the module constants

static const char *sim_current_name;

static void sim_print_position(void) {
    printf(",\"sim_step\":%lld,\"sim_time_ns\":%lld",
           (long long)rask_sim_step(), (long long)rask_sim_time_ns());
    if (rask_sim_sick_log()[0]) {
        printf(",\"sim_sick\":\"");
        json_print_escaped(rask_sim_sick_log());
        printf("\"");
    }
    if (rask_sim_fault_log()[0]) {
        printf(",\"sim_faults\":\"");
        json_print_escaped(rask_sim_fault_log());
        printf("\"");
    }
}

// A failure that can't unwind to the test's setjmp — a deadlock is noticed on
// whichever thread tried to schedule, not on the test's own.
_Noreturn void rask_test_sim_fail(const char *msg) {
    printf("%s{\"name\":\"", record_mark());
    json_print_escaped(sim_current_name ? sim_current_name : "");
    printf("\",\"passed\":false,\"duration_ns\":0,\"error\":\"");
    json_print_escaped(msg);
    printf("\"");
    sim_print_position();
    printf("}\n");
    fflush(NULL);
    _exit(1);
}

// Sim starts before module constants initialise, so a constant sees the
// same world the tests do: a Map built there uses the run's hash seed, and a
// constant that reads the clock or `random` reads the simulated ones.
static long sim_want;

static void sim_start(void) {
    const char *want = getenv("RASK_SIM_TEST");
    const char *seed = getenv("RASK_SIM_SEED");
    if (!want || !seed) {
        fprintf(stderr, "sim: RASK_SIM_TEST and RASK_SIM_SEED must both be set — "
                        "run sim binaries through `rask test --sim`\n");
        _exit(2);
    }
    // Set only by `--max-steps`; the default budget lives here.
    const char *steps = getenv("RASK_SIM_MAX_STEPS");
    long long max_steps = steps ? strtoll(steps, NULL, 10) : 0;
    if (max_steps <= 0) max_steps = 10000000;
    sim_want = strtol(want, NULL, 10);
    rask_sim_begin(strtoull(seed, NULL, 10), (int64_t)max_steps);
}

// Returns 1 when `name` is the test this process was started for.
// RASK_SIM_TEST is the test's position in the binary, counting from 0, so two
// tests can never answer to one request.
static int sim_select(const char *name) {
    static long next_index;
    if (next_index++ != sim_want) return 0;
    sim_current_name = name;
    return 1;
}
#endif

// Catch panics, print the JSON result line. Returns 0 on pass, 1 on fail.
static int test_run_one(test_fn fn, const char *name) {
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
        printf("%s{\"name\":\"", record_mark());
           json_print_escaped(name);
           printf("\",\"passed\":true,\"duration_ns\":%lld,\"skipped\":\"",
               (long long)elapsed_ns);
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
            printf("%s{\"name\":\"", record_mark());
               json_print_escaped(name);
               printf("\",\"passed\":true,\"duration_ns\":%lld}\n",
                   (long long)elapsed_ns);
            fflush(stdout);
            return 0;
        } else {
            // Expected failure but test passed — fail
            printf("%s{\"name\":\"", record_mark());
               json_print_escaped(name);
               printf("\",\"passed\":false,\"duration_ns\":%lld,\"error\":\"expected failure but test passed\"}\n",
                   (long long)elapsed_ns);
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
        printf("%s{\"name\":\"", record_mark());
           json_print_escaped(name);
           printf("\",\"passed\":false,\"duration_ns\":%lld,\"error\":\"",
               (long long)elapsed_ns);
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
        printf("\"");
#ifdef RASK_SIM
        sim_print_position();
#endif
        printf("}\n");
    } else {
        printf("%s{\"name\":\"", record_mark());
           json_print_escaped(name);
           printf("\",\"passed\":true,\"duration_ns\":%lld}\n",
               (long long)elapsed_ns);
    }
    fflush(stdout);

    return failed;
}

// First call of the test runner's entry point, ahead of the module constants.
void rask_test_start(void) {
#ifdef RASK_SIM
    sim_start();
#endif
}

// Run a single test. Returns 0 on pass, 1 on fail.
int rask_test_run(test_fn fn, const char *name) {
#ifdef RASK_SIM
    if (!sim_select(name)) return 0;
    int failed = test_run_one(fn, name);
    fflush(NULL);
    // What `main` does at exit, which this process never reaches: without it
    // RASK_LEAK_CHECK was silent under sim whatever the test leaked. Only on a
    // pass — a failed test's leak is the failure's, not a second finding.
    if (!failed) {
        rask_await_detached_tasks();
        rask_const_free();
        rask_leak_check();   // exits 97 when something is still held
    }
    _exit(failed);
#else
    return test_run_one(fn, name);
#endif
}
