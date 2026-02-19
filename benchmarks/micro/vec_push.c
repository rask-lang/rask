// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: Vec push 10k — calls Rask C runtime directly, uses bench.c harness.

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

typedef struct RaskVec RaskVec;
RaskVec *rask_vec_new(int64_t elem_size);
void     rask_vec_free(RaskVec *v);
int64_t  rask_vec_push(RaskVec *v, const void *elem);

static void work(void) {
    RaskVec *v = rask_vec_new(8);
    for (int64_t i = 0; i < 10000; i++) {
        rask_vec_push(v, &i);
    }
    rask_vec_free(v);
}

int main(void) {
    rask_bench_run(work, "vec push 10k");
    return 0;
}
