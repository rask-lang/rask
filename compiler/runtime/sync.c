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
#include <stdatomic.h>
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

    RaskMutex *m = (RaskMutex *)rask_alloc(sizeof(RaskMutex));

    pthread_mutex_init(&m->lock, NULL);
    m->data_size = data_size;
    m->data = rask_alloc(data_size);

    memcpy(m->data, initial_data, (size_t)data_size);
    return m;
}

void rask_mutex_free(RaskMutex *m) {
    if (!m) return;
    pthread_mutex_destroy(&m->lock);
    rask_free(m->data);
    rask_free(m);
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
    _Atomic int64_t  refcount;
};

RaskShared *rask_shared_new(const void *initial_data, int64_t data_size) {
    if (data_size <= 0) {
        rask_panic("Shared data size must be positive");
    }

    RaskShared *s = (RaskShared *)rask_alloc(sizeof(RaskShared));

    pthread_rwlock_init(&s->lock, NULL);
    s->data_size = data_size;
    s->data = rask_alloc(data_size);

    atomic_store(&s->refcount, 1);
    memcpy(s->data, initial_data, (size_t)data_size);
    return s;
}

void rask_shared_free(RaskShared *s) {
    if (!s) return;
    if (atomic_fetch_sub(&s->refcount, 1) > 1) return;
    pthread_rwlock_destroy(&s->lock);
    rask_free(s->data);
    rask_free(s);
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

// ─── i64-based codegen wrappers ────────────────────────────
//
// Rask closure layout (see closures.rs): [func_ptr | env...]
// Calling convention: func_ptr(env_ptr, args...) where env_ptr = closure + 8.

#define CLOSURE_FUNC(cl)  (*(int64_t *)(intptr_t)(cl))
#define CLOSURE_ENV(cl)   ((cl) + 8)

typedef int64_t (*RaskClosureFn1)(int64_t env, int64_t arg);
typedef void    (*RaskClosureVoidFn1)(int64_t env, int64_t arg);

int64_t rask_shared_new_i64(int64_t value) {
    RaskShared *s = rask_shared_new(&value, sizeof(int64_t));
    return (int64_t)(intptr_t)s;
}

int64_t rask_shared_read_i64(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);

    pthread_rwlock_rdlock(&s->lock);
    int64_t data = *(int64_t *)s->data;
    int64_t result = fn(env, data);
    pthread_rwlock_unlock(&s->lock);
    return result;
}

int64_t rask_shared_write_i64(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);

    pthread_rwlock_wrlock(&s->lock);
    int64_t data = *(int64_t *)s->data;
    int64_t new_data = fn(env, data);
    *(int64_t *)s->data = new_data;
    pthread_rwlock_unlock(&s->lock);
    return new_data;
}

int64_t rask_shared_clone_i64(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    atomic_fetch_add(&s->refcount, 1);
    return shared;
}

void rask_shared_drop_i64(int64_t shared) {
    rask_shared_free((RaskShared *)(intptr_t)shared);
}
