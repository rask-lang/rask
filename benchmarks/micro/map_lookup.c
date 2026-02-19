// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: Map lookup 10k — calls Rask C runtime directly.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

typedef struct RaskMap RaskMap;
RaskMap *rask_map_new(int64_t key_size, int64_t val_size);
void     rask_map_free(RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);
void    *rask_map_get(const RaskMap *m, const void *key);

static volatile int64_t sink;

static void work(void) {
    RaskMap *m = rask_map_new(8, 8);
    for (int64_t i = 0; i < 10000; i++) {
        int64_t val = i * 2;
        rask_map_insert(m, &i, &val);
    }
    int64_t sum = 0;
    for (int64_t i = 0; i < 10000; i++) {
        void *p = rask_map_get(m, &i);
        if (p) sum += *(int64_t *)p;
    }
    sink = sum;
    rask_map_free(m);
}

int main(void) {
    rask_bench_run(work, "map lookup 10k");
    return 0;
}
