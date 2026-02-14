// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Minimal Rask runtime — provides built-in functions for native-compiled programs.
// Linked with the object file produced by rask-codegen.

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

// Forward declaration — user's main function, exported from the Rask module as rask_main
extern void rask_main(void);

// ─── Print functions ──────────────────────────────────────────────

void rask_print_i64(int64_t val) {
    printf("%lld", (long long)val);
}

void rask_print_bool(int8_t val) {
    printf("%s", val ? "true" : "false");
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

// ─── Entry point ──────────────────────────────────────────────────

int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    rask_main();
    return 0;
}
