// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask benchmark harness — warmup, calibrate, measure, report.
// Called from generated benchmark runner entry points.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

typedef void (*bench_fn)(void);

static int64_t clock_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

static int cmp_i64(const void *a, const void *b) {
    int64_t va = *(const int64_t *)a;
    int64_t vb = *(const int64_t *)b;
    return (va > vb) - (va < vb);
}

// Run a benchmark: warmup, auto-calibrate iterations, measure, print JSON line.
void rask_bench_run(bench_fn fn, const char *name) {
    // Warmup: 3 iterations
    for (int i = 0; i < 3; i++) {
        fn();
    }

    // Calibrate: find iteration count where total >= 100ms
    int64_t iterations = 10;
    for (;;) {
        int64_t start = clock_ns();
        for (int64_t i = 0; i < iterations; i++) {
            fn();
        }
        int64_t elapsed = clock_ns() - start;
        if (elapsed >= 100000000LL || iterations >= 10000000LL) {
            break;
        }
        iterations *= 2;
    }

    // Measure each iteration
    int64_t *timings = (int64_t *)malloc((size_t)iterations * sizeof(int64_t));
    if (!timings) {
        fprintf(stderr, "bench: allocation failed for %lld timings\n",
                (long long)iterations);
        return;
    }

    for (int64_t i = 0; i < iterations; i++) {
        int64_t start = clock_ns();
        fn();
        timings[i] = clock_ns() - start;
    }

    // Stats
    qsort(timings, (size_t)iterations, sizeof(int64_t), cmp_i64);

    int64_t total = 0;
    for (int64_t i = 0; i < iterations; i++) {
        total += timings[i];
    }

    int64_t min_ns    = timings[0];
    int64_t max_ns    = timings[iterations - 1];
    int64_t mean_ns   = total / iterations;
    int64_t median_ns = timings[iterations / 2];

    free(timings);

    printf("{\"name\":\"%s\",\"iterations\":%lld,\"min_ns\":%lld,"
           "\"max_ns\":%lld,\"mean_ns\":%lld,\"median_ns\":%lld}\n",
           name,
           (long long)iterations,
           (long long)min_ns,
           (long long)max_ns,
           (long long)mean_ns,
           (long long)median_ns);
    fflush(stdout);
}
