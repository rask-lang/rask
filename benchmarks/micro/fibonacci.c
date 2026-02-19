// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: recursive fibonacci — uses bench.c harness for fair comparison.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

static volatile int64_t sink;

static int64_t fib(int64_t n) {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}

static void work(void) {
    sink = fib(30);
}

int main(void) {
    rask_bench_run(work, "fibonacci 30");
    return 0;
}
