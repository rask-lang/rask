// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: Map insert 10k — calls Rask C runtime directly.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

typedef struct RaskMap RaskMap;
RaskMap *rask_map_new(int64_t key_size, int64_t val_size);
void     rask_map_free(RaskMap *m);
int64_t  rask_map_insert(RaskMap *m, const void *key, const void *val);

static void work(void) {
    RaskMap *m = rask_map_new(8, 8);
    for (int64_t i = 0; i < 10000; i++) {
        int64_t val = i * 2;
        rask_map_insert(m, &i, &val);
    }
    rask_map_free(m);
}

int main(void) {
    rask_bench_run(work, "map insert 10k");
    return 0;
}
