// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Synchronization primitives (conc.sync/SY1-SY4).
//
// Mutex<T>:   exclusive access via `with` blocks (conc.sync/MX1-MX2)
// Shared<T>:  multiple-reader / exclusive-writer via `with` blocks (conc.sync/R1-R3)
//
// Primary access is `with`-based blocks (conc.sync/WS1-WS4): the protected data
// is only reachable inside the block, preventing reference escapes.
// Non-blocking variants (try_read/try_write/try_lock) use closures.

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
    _Atomic int64_t refcount;
};

RaskMutex *rask_mutex_new(const void *initial_data, int64_t data_size) {
    if (data_size <= 0) {
        rask_panic("Mutex data size must be positive");
    }

    RaskMutex *m = (RaskMutex *)rask_alloc(sizeof(RaskMutex));

    pthread_mutex_init(&m->lock, NULL);
    m->data_size = data_size;
    m->data = rask_alloc(data_size);

    atomic_store(&m->refcount, 1);
    memcpy(m->data, initial_data, (size_t)data_size);
    return m;
}

void rask_mutex_free(RaskMutex *m) {
    if (!m) return;
    if (atomic_fetch_sub(&m->refcount, 1) > 1) return;
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

// ─── Mutex i64/ptr codegen wrappers ──────────────────────

int64_t rask_mutex_new_ptr(int64_t data_ptr, int64_t data_size) {
    RaskMutex *m = rask_mutex_new((const void *)(intptr_t)data_ptr, data_size);
    return (int64_t)(intptr_t)m;
}

int64_t rask_mutex_lock_ptr(int64_t mutex, int64_t closure) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);

    pthread_mutex_lock(&m->lock);
    int64_t result = fn(env, (int64_t)(intptr_t)m->data);
    pthread_mutex_unlock(&m->lock);
    return result;
}

int64_t rask_mutex_try_lock_ptr(int64_t mutex, int64_t closure) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    if (pthread_mutex_trylock(&m->lock) == 0) {
        RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
        int64_t env = CLOSURE_ENV(closure);
        int64_t result = fn(env, (int64_t)(intptr_t)m->data);
        pthread_mutex_unlock(&m->lock);
        return result;
    }
    return 0; // lock not acquired
}

int64_t rask_mutex_clone(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    atomic_fetch_add(&m->refcount, 1);
    return mutex;
}

void rask_mutex_drop(int64_t mutex) {
    rask_mutex_free((RaskMutex *)(intptr_t)mutex);
}

// ─── Pointer-based wrappers for aggregate types ──────────
//
// These work with any data size. The closure receives a pointer to
// the data inside the Shared, not a copy. For write, modifications
// happen in-place through the pointer (no copy-back needed).

int64_t rask_shared_new_ptr(int64_t data_ptr, int64_t data_size) {
    RaskShared *s = rask_shared_new((const void *)(intptr_t)data_ptr, data_size);
    return (int64_t)(intptr_t)s;
}

int64_t rask_shared_read_ptr(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);

    pthread_rwlock_rdlock(&s->lock);
    int64_t result = fn(env, (int64_t)(intptr_t)s->data);
    pthread_rwlock_unlock(&s->lock);
    return result;
}

int64_t rask_shared_write_ptr(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);

    pthread_rwlock_wrlock(&s->lock);
    int64_t result = fn(env, (int64_t)(intptr_t)s->data);
    pthread_rwlock_unlock(&s->lock);
    return result;
}

// Non-blocking read: returns 1+result on success, 0 if contended (R3)
int64_t rask_shared_try_read_ptr(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    if (pthread_rwlock_tryrdlock(&s->lock) == 0) {
        RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
        int64_t env = CLOSURE_ENV(closure);
        int64_t result = fn(env, (int64_t)(intptr_t)s->data);
        pthread_rwlock_unlock(&s->lock);
        // Encode as Option: tag=0 (Some) in high bits, payload in low bits
        // For i64 results, pack as (result << 1) | 1 to distinguish from None(0)
        return (result << 1) | 1;
    }
    return 0; // None
}

// Non-blocking write: returns 1+result on success, 0 if contended (R3)
int64_t rask_shared_try_write_ptr(int64_t shared, int64_t closure) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    if (pthread_rwlock_trywrlock(&s->lock) == 0) {
        RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
        int64_t env = CLOSURE_ENV(closure);
        int64_t result = fn(env, (int64_t)(intptr_t)s->data);
        *(int64_t *)s->data = result;
        pthread_rwlock_unlock(&s->lock);
        return (result << 1) | 1;
    }
    return 0; // None
}

