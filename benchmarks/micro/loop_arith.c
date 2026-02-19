// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: 10M integer arithmetic loop — uses bench.c harness.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

static volatile int64_t sink;

static void work(void) {
    int64_t sum = 0;
    for (int64_t i = 0; i < 10000000; i++) {
        sum += i * 3 - i / 2;
    }
    sink = sum;
}

int main(void) {
    rask_bench_run(work, "loop arith 10M");
    return 0;
}
