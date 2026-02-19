// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: handle overhead — uses same pool.c runtime as Rask.
// Proves overhead is the pool design, not the language.

#include <stdint.h>
#include <string.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

typedef struct RaskPool RaskPool;
typedef struct RaskVec  RaskVec;

RaskPool *rask_pool_new(int64_t elem_size);
void      rask_pool_free(RaskPool *p);
int64_t   rask_pool_insert_packed(RaskPool *p, const void *elem);
void     *rask_pool_get_packed(const RaskPool *p, int64_t packed);
int64_t   rask_pool_remove_packed(RaskPool *p, int64_t packed);

RaskVec  *rask_vec_new(int64_t elem_size);
void      rask_vec_free(RaskVec *v);
int64_t   rask_vec_push(RaskVec *v, const void *elem);
void     *rask_vec_get(const RaskVec *v, int64_t index);
int64_t   rask_vec_len(const RaskVec *v);

// Sequential read: insert 1k, read each via handle
static void sequential_read(void) {
    RaskPool *p = rask_pool_new(8);
    RaskVec *handles = rask_vec_new(8);

    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = rask_pool_insert_packed(p, &i);
        rask_vec_push(handles, &h);
    }

    int64_t sum = 0;
    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = *(int64_t *)rask_vec_get(handles, i);
        int64_t *val = (int64_t *)rask_pool_get_packed(p, h);
        sum += *val;
    }

    (void)sum;
    rask_vec_free(handles);
    rask_pool_free(p);
}

// Random read: stride-7 access pattern (tests cache behavior)
static void random_read(void) {
    RaskPool *p = rask_pool_new(8);
    RaskVec *handles = rask_vec_new(8);

    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = rask_pool_insert_packed(p, &i);
        rask_vec_push(handles, &h);
    }

    int64_t sum = 0;
    int64_t idx = 0;
    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = *(int64_t *)rask_vec_get(handles, idx);
        int64_t *val = (int64_t *)rask_pool_get_packed(p, h);
        sum += *val;
        idx = (idx + 7) % 1000;
    }

    (void)sum;
    rask_vec_free(handles);
    rask_pool_free(p);
}

// Churn remove: insert 1k, remove 20%, re-insert
static void churn_remove(void) {
    RaskPool *p = rask_pool_new(8);
    RaskVec *handles = rask_vec_new(8);

    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = rask_pool_insert_packed(p, &i);
        rask_vec_push(handles, &h);
    }

    for (int64_t i = 0; i < 1000; i += 5) {
        int64_t h = *(int64_t *)rask_vec_get(handles, i);
        rask_pool_remove_packed(p, h);
    }

    for (int64_t i = 0; i < 200; i++) {
        int64_t val = i * 10;
        rask_pool_insert_packed(p, &val);
    }

    rask_vec_free(handles);
    rask_pool_free(p);
}

// Churn read: insert 1k, read 800 (skip every 5th)
static void churn_read(void) {
    RaskPool *p = rask_pool_new(8);
    RaskVec *handles = rask_vec_new(8);

    for (int64_t i = 0; i < 1000; i++) {
        int64_t h = rask_pool_insert_packed(p, &i);
        rask_vec_push(handles, &h);
    }

    int64_t sum = 0;
    for (int64_t i = 0; i < 1000; i++) {
        if (i % 5 != 0) {
            int64_t h = *(int64_t *)rask_vec_get(handles, i);
            int64_t *val = (int64_t *)rask_pool_get_packed(p, h);
            sum += *val;
        }
    }

    (void)sum;
    rask_vec_free(handles);
    rask_pool_free(p);
}

int main(void) {
    rask_bench_run(sequential_read, "handle sequential read 1k");
    rask_bench_run(random_read,     "handle random read 1k");
    rask_bench_run(churn_remove,    "handle churn remove 1k");
    rask_bench_run(churn_read,      "handle churn read 800");
    return 0;
}
