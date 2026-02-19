// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C raw array baseline: same operations as handle_overhead.c but with
// direct array indexing. No generation checks, no pool indirection.
// This is the theoretical minimum — completely unsafe.

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

#define N 1000

// Sequential read: direct array[i] access
static void sequential_read(void) {
    int64_t *data = (int64_t *)malloc(N * sizeof(int64_t));
    int64_t *indices = (int64_t *)malloc(N * sizeof(int64_t));

    for (int64_t i = 0; i < N; i++) {
        data[i] = i;
        indices[i] = i;
    }

    int64_t sum = 0;
    for (int64_t i = 0; i < N; i++) {
        sum += data[indices[i]];
    }

    (void)sum;
    free(indices);
    free(data);
}

// Random read: stride-7 access pattern
static void random_read(void) {
    int64_t *data = (int64_t *)malloc(N * sizeof(int64_t));
    int64_t *indices = (int64_t *)malloc(N * sizeof(int64_t));

    for (int64_t i = 0; i < N; i++) {
        data[i] = i;
        indices[i] = i;
    }

    int64_t sum = 0;
    int64_t idx = 0;
    for (int64_t i = 0; i < N; i++) {
        sum += data[indices[idx]];
        idx = (idx + 7) % N;
    }

    (void)sum;
    free(indices);
    free(data);
}

// Churn remove: simulate remove + re-insert
static void churn_remove(void) {
    int64_t *data = (int64_t *)malloc(N * sizeof(int64_t));
    int8_t  *alive = (int8_t *)malloc(N * sizeof(int8_t));

    for (int64_t i = 0; i < N; i++) {
        data[i] = i;
        alive[i] = 1;
    }

    for (int64_t i = 0; i < N; i += 5) {
        alive[i] = 0;
    }

    int64_t fill = 0;
    for (int64_t i = 0; i < N && fill < 200; i++) {
        if (!alive[i]) {
            data[i] = fill * 10;
            alive[i] = 1;
            fill++;
        }
    }

    free(alive);
    free(data);
}

// Churn read: insert 1k, read 800 (skip every 5th)
static void churn_read(void) {
    int64_t *data = (int64_t *)malloc(N * sizeof(int64_t));

    for (int64_t i = 0; i < N; i++) {
        data[i] = i;
    }

    int64_t sum = 0;
    for (int64_t i = 0; i < N; i++) {
        if (i % 5 != 0) {
            sum += data[i];
        }
    }

    (void)sum;
    free(data);
}

int main(void) {
    rask_bench_run(sequential_read, "handle sequential read 1k");
    rask_bench_run(random_read,     "handle random read 1k");
    rask_bench_run(churn_remove,    "handle churn remove 1k");
    rask_bench_run(churn_read,      "handle churn read 800");
    return 0;
}
