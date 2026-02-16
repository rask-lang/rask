// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Synchronization primitives (conc.sync/SY1-SY4).
//
// Mutex<T>:   exclusive access via closure (conc.sync/MX1-MX2)
// Shared<T>:  multiple-reader / exclusive-writer via closure (conc.sync/R1-R3)
//
// Both use closure-based access (conc.sync/CB1-CB2): the protected data
// is only reachable inside the callback, preventing reference escapes.

#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <pthread.h>

// ─── Mutex ─────────────────────────────────────────────────

struct RaskMutex {
    pthread_mutex_t lock;
    void           *data;
    int64_t         data_size;
};

RaskMutex *rask_mutex_new(const void *initial_data, int64_t data_size) {
    if (data_size <= 0) {
        rask_panic("Mutex data size must be positive");
    }

    RaskMutex *m = (RaskMutex *)malloc(sizeof(RaskMutex));
    if (!m) {
        fprintf(stderr, "rask: mutex alloc failed\n");
        abort();
    }

    pthread_mutex_init(&m->lock, NULL);
    m->data_size = data_size;
    m->data = malloc((size_t)data_size);
    if (!m->data) {
        fprintf(stderr, "rask: mutex data alloc failed\n");
        abort();
    }

    memcpy(m->data, initial_data, (size_t)data_size);
    return m;
}

void rask_mutex_free(RaskMutex *m) {
    if (!m) return;
    pthread_mutex_destroy(&m->lock);
    free(m->data);
    free(m);
}

void rask_mutex_lock(RaskMutex *m, RaskAccessFn f, void *ctx) {
    pthread_mutex_lock(&m->lock);
    f(m->data, ctx);
    pthread_mutex_unlock(&m->lock);
}

int64_t rask_mutex_try_lock(RaskMutex *m, RaskAccessFn f, void *ctx) {
    if (pthread_mutex_trylock(&m->lock) == 0) {
        f(m->data, ctx);
        pthread_mutex_unlock(&m->lock);
        return 1;
    }
    return 0;
}

// ─── Shared (RwLock) ───────────────────────────────────────

struct RaskShared {
    pthread_rwlock_t lock;
    void            *data;
    int64_t          data_size;
};

RaskShared *rask_shared_new(const void *initial_data, int64_t data_size) {
    if (data_size <= 0) {
        rask_panic("Shared data size must be positive");
    }

    RaskShared *s = (RaskShared *)malloc(sizeof(RaskShared));
    if (!s) {
        fprintf(stderr, "rask: shared alloc failed\n");
        abort();
    }

    pthread_rwlock_init(&s->lock, NULL);
    s->data_size = data_size;
    s->data = malloc((size_t)data_size);
    if (!s->data) {
        fprintf(stderr, "rask: shared data alloc failed\n");
        abort();
    }

    memcpy(s->data, initial_data, (size_t)data_size);
    return s;
}

void rask_shared_free(RaskShared *s) {
    if (!s) return;
    pthread_rwlock_destroy(&s->lock);
    free(s->data);
    free(s);
}

void rask_shared_read(RaskShared *s, RaskAccessFn f, void *ctx) {
    pthread_rwlock_rdlock(&s->lock);
    f(s->data, ctx);
    pthread_rwlock_unlock(&s->lock);
}

void rask_shared_write(RaskShared *s, RaskAccessFn f, void *ctx) {
    pthread_rwlock_wrlock(&s->lock);
    f(s->data, ctx);
    pthread_rwlock_unlock(&s->lock);
}

int64_t rask_shared_try_read(RaskShared *s, RaskAccessFn f, void *ctx) {
    if (pthread_rwlock_tryrdlock(&s->lock) == 0) {
        f(s->data, ctx);
        pthread_rwlock_unlock(&s->lock);
        return 1;
    }
    return 0;
}

int64_t rask_shared_try_write(RaskShared *s, RaskAccessFn f, void *ctx) {
    if (pthread_rwlock_trywrlock(&s->lock) == 0) {
        f(s->data, ctx);
        pthread_rwlock_unlock(&s->lock);
        return 1;
    }
    return 0;
}
