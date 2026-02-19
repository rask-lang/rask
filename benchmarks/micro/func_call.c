// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: 10M function calls.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

static volatile int64_t sink;

__attribute__((noinline))
static int64_t add(int64_t a, int64_t b) {
    return a + b;
}

static void work(void) {
    int64_t sum = 0;
    for (int64_t i = 0; i < 10000000; i++) {
        sum = add(sum, i);
    }
    sink = sum;
}

int main(void) {
    rask_bench_run(work, "func call 10M");
    return 0;
}
