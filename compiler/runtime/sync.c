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
    RASK_CHECK_NONNULL(s, "Shared.read: shared handle is null");
    pthread_rwlock_rdlock(&s->lock);
    f(s->data, ctx);
    pthread_rwlock_unlock(&s->lock);
}

void rask_shared_write(RaskShared *s, RaskAccessFn f, void *ctx) {
    RASK_CHECK_NONNULL(s, "Shared.write: shared handle is null");
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

// ─── Staged access (conc.sync/ST1–ST4) ─────────────────────
//
// `with box.staged() as v { … }` binds a working copy under the exclusive lock.
// Every non-panic exit commits it as one move (ST2); unwinding discards it (ST3),
// so a panic between two field writes leaves survivors the last committed state
// rather than a half-written one.
//
// The commit/discard split rides the machinery already here: codegen schedules
// the commit as the block's inline cleanup, which every ordinary exit chains
// through, and the acquire registers the *discard* on the held-access stack that
// `rask_panic` drains. A panic therefore skips the commit and runs the discard
// without either path knowing about the other.
//
// One frame per live staged block, on a per-thread stack. conc.sync/DL1 rejects
// nested `with` on sync boxes, so the depth is one in practice; a list costs
// nothing and keeps a lock taken through a function call honest.

typedef struct StagedFrame {
    int64_t             handle;   // the box, as codegen names it
    void               *scratch;  // the working copy
    void               *payload;  // where a commit puts it back
    int64_t             size;
    struct StagedFrame *next;
} StagedFrame;

static __thread StagedFrame *tl_staged = NULL;

// Copy the payload aside and hand back the copy's address. The caller has
// already taken the lock.
static int64_t staged_begin(int64_t handle, void *payload, int64_t size,
                            RaskReleaseFn discard) {
    StagedFrame *f = (StagedFrame *)malloc(sizeof(StagedFrame));
    void *scratch = rask_alloc(size);
    if (!f || !scratch) {
        rask_panic("out of memory staging a locked value");
    }
    memcpy(scratch, payload, (size_t)size);
    f->handle  = handle;
    f->scratch = scratch;
    f->payload = payload;
    f->size    = size;
    f->next    = tl_staged;
    tl_staged  = f;
    rask_access_push(discard, handle);
    return (int64_t)(intptr_t)scratch;
}

// Unlink this box's frame. Returns NULL when there isn't one, which means the
// block already exited — a commit and a discard both racing one exit is the bug
// this guards, and doing nothing twice is the safe answer.
static StagedFrame *staged_take(int64_t handle) {
    StagedFrame **link = &tl_staged;
    while (*link) {
        if ((*link)->handle == handle) {
            StagedFrame *f = *link;
            *link = f->next;
            return f;
        }
        link = &(*link)->next;
    }
    return NULL;
}

static void staged_free(StagedFrame *f) {
    rask_realloc(f->scratch, f->size, 0);
    free(f);
}

// ST2: commit the copy as one move, then release. `rask_access_pop` cancels the
// discard this box registered — the block exited the ordinary way.
static int staged_commit(int64_t handle) {
    StagedFrame *f = staged_take(handle);
    if (!f) return 0;
    memcpy(f->payload, f->scratch, (size_t)f->size);
    staged_free(f);
    rask_access_pop(handle);
    return 1;
}

// ST3: drop the copy uncommitted. Reached from the unwind drain, so the access
// entry is already gone.
static int staged_abandon(int64_t handle) {
    StagedFrame *f = staged_take(handle);
    if (!f) return 0;
    staged_free(f);
    return 1;
}

void rask_mutex_staged_discard(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    if (staged_abandon(mutex)) {
        pthread_mutex_unlock(&m->lock);
    }
}

int64_t rask_mutex_staged_acquire(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    pthread_mutex_lock(&m->lock);
    return staged_begin(mutex, m->data, m->data_size, rask_mutex_staged_discard);
}

// The working copy's address, for a word-sized payload codegen loaded into a
// local and has to store back before the commit.
int64_t rask_mutex_staged_data(int64_t mutex) {
    StagedFrame *f = tl_staged;
    while (f) {
        if (f->handle == mutex) return (int64_t)(intptr_t)f->scratch;
        f = f->next;
    }
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    return (int64_t)(intptr_t)m->data;
}

void rask_mutex_staged_commit(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    if (staged_commit(mutex)) {
        pthread_mutex_unlock(&m->lock);
    }
}

void rask_shared_staged_discard(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    if (staged_abandon(shared)) {
        pthread_rwlock_unlock(&s->lock);
    }
}

int64_t rask_shared_staged_acquire(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    pthread_rwlock_wrlock(&s->lock);
    return staged_begin(shared, s->data, s->data_size, rask_shared_staged_discard);
}

int64_t rask_shared_staged_data(int64_t shared) {
    StagedFrame *f = tl_staged;
    while (f) {
        if (f->handle == shared) return (int64_t)(intptr_t)f->scratch;
        f = f->next;
    }
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    return (int64_t)(intptr_t)s->data;
}

void rask_shared_staged_commit(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    if (staged_commit(shared)) {
        pthread_rwlock_unlock(&s->lock);
    }
}

// The closure form, for the same reason `rask_shared_write_ptr` exists: it is
// what `stdlib/sync.rk`'s `@native` names, and a declaration whose symbol
// doesn't exist fails at codegen rather than at the declaration (#1007).
int64_t rask_shared_staged_ptr(int64_t shared, int64_t closure) {
    RaskClosureFn1 fn = (RaskClosureFn1)(intptr_t)CLOSURE_FUNC(closure);
    int64_t env = CLOSURE_ENV(closure);
    int64_t scratch = rask_shared_staged_acquire(shared);
    int64_t result = fn(env, scratch);
    rask_shared_staged_commit(shared);
    return result;
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

// Acquire/release pair for the direct `mutex.lock().method(args)` form. Unlike
// the closure wrapper above, the method call happens in the caller's frame, so
// it returns aggregates (a `T or E` result) through the normal ABI. Acquire
// locks and hands back the data pointer; release unlocks.
int64_t rask_mutex_acquire(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    pthread_mutex_lock(&m->lock);
    // U3/U4: a panic before the inline release would leave this held forever.
    rask_access_push(rask_mutex_release, mutex);
    return (int64_t)(intptr_t)m->data;
}

void rask_mutex_release(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    rask_access_pop(mutex);
    pthread_mutex_unlock(&m->lock);
}

// The payload's address without touching the lock. `with m.lock() as v { ... }`
// loads a word-sized payload into a local, so the local has to be written back
// before the lock is released — and acquire already consumed its own result.
// Only ever called while the lock is held.
int64_t rask_mutex_data(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    return (int64_t)(intptr_t)m->data;
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

// Acquire/release for the direct `shared.read()/.write()` guard form, mirroring
// the Mutex pair. The following method or field access runs in the caller's
// frame so it can return aggregates. Read takes a shared lock, write exclusive.
int64_t rask_shared_read_acquire(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    pthread_rwlock_rdlock(&s->lock);
    rask_access_push(rask_shared_release, shared);   // U3/U4
    return (int64_t)(intptr_t)s->data;
}

int64_t rask_shared_write_acquire(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    pthread_rwlock_wrlock(&s->lock);
    rask_access_push(rask_shared_release, shared);   // U3/U4
    return (int64_t)(intptr_t)s->data;
}

// The payload's address without touching the lock — the Shared counterpart of
// rask_mutex_data. Only ever called while the lock is held.
int64_t rask_shared_data(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    return (int64_t)(intptr_t)s->data;
}

void rask_shared_release(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    rask_access_pop(shared);
    pthread_rwlock_unlock(&s->lock);
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


// ─── Cell ───────────────────────────────────────────────────
//
// mem.cell: single-owner interior mutability. No lock — a Cell is not
// shareable across tasks, so `get`/`set` are a plain copy in and out of a
// heap slot. The slot keeps the value at a stable address, which is what
// lets a `const` binding hand out a mutable interior.

typedef struct {
    int64_t data_size;
    void   *data;
} RaskCell;

int64_t rask_cell_new(int64_t data_ptr, int64_t data_size) {
    if (data_size <= 0) data_size = 8;
    RaskCell *c = (RaskCell *)rask_alloc(sizeof(RaskCell));
    c->data_size = data_size;
    c->data = rask_alloc(data_size);
    if (data_ptr) {
        memcpy(c->data, (const void *)(intptr_t)data_ptr, (size_t)data_size);
    } else {
        memset(c->data, 0, (size_t)data_size);
    }
    return (int64_t)(intptr_t)c;
}

// The slot's address. Codegen loads a scalar through it, or copies an
// aggregate out of it — same convention as Shared's guard pointer.
int64_t rask_cell_get(int64_t cell) {
    RaskCell *c = (RaskCell *)(intptr_t)cell;
    RASK_CHECK_NONNULL(c, "Cell.get: cell is null");
    return (int64_t)(intptr_t)c->data;
}

void rask_cell_set(int64_t cell, int64_t data_ptr) {
    RaskCell *c = (RaskCell *)(intptr_t)cell;
    RASK_CHECK_NONNULL(c, "Cell.set: cell is null");
    if (data_ptr) {
        memcpy(c->data, (const void *)(intptr_t)data_ptr, (size_t)c->data_size);
    }
}

// Swap in the new value, hand back the old one's address. The old value has to
// outlive the call, so it goes in its own allocation rather than being returned
// out of the slot we're about to overwrite.
int64_t rask_cell_replace(int64_t cell, int64_t data_ptr) {
    RaskCell *c = (RaskCell *)(intptr_t)cell;
    RASK_CHECK_NONNULL(c, "Cell.replace: cell is null");
    void *old = rask_alloc(c->data_size);
    memcpy(old, c->data, (size_t)c->data_size);
    if (data_ptr) {
        memcpy(c->data, (const void *)(intptr_t)data_ptr, (size_t)c->data_size);
    }
    return (int64_t)(intptr_t)old;
}

void rask_cell_free(int64_t cell) {
    RaskCell *c = (RaskCell *)(intptr_t)cell;
    if (!c) return;
    rask_free(c->data);
    rask_free(c);
}

// ─── get / set / replace under a lock ──────────────────────────────────────
//
// conc.sync puts these on `Shared<T, S>` for every strategy, not just `Local`.
// The `Local` versions above are a plain copy in and out of a heap slot; under
// a lock they are the same copy with the lock held. Without these, `get` on a
// locking box type-checked and then failed to link (`Function not found:
// Shared_get`) — the interpreter answered it because it has one uniform box.
//
// `get` returns the slot's address, matching the Cell and guard-pointer
// convention: codegen loads a scalar through it or copies an aggregate out. The
// lock is released before the caller reads, which is the same window `read()`
// hands out for an inline access — a single-expression shorthand, not a
// critical section (CE6).

int64_t rask_shared_get(int64_t shared) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RASK_CHECK_NONNULL(s, "Shared.get: box is null");
    return (int64_t)(intptr_t)s->data;
}

void rask_shared_set(int64_t shared, int64_t data_ptr) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RASK_CHECK_NONNULL(s, "Shared.set: box is null");
    if (!data_ptr) return;
    pthread_rwlock_wrlock(&s->lock);
    memcpy(s->data, (const void *)(intptr_t)data_ptr, (size_t)s->data_size);
    pthread_rwlock_unlock(&s->lock);
}

int64_t rask_shared_replace(int64_t shared, int64_t data_ptr) {
    RaskShared *s = (RaskShared *)(intptr_t)shared;
    RASK_CHECK_NONNULL(s, "Shared.replace: box is null");
    void *old = rask_alloc(s->data_size);
    pthread_rwlock_wrlock(&s->lock);
    memcpy(old, s->data, (size_t)s->data_size);
    if (data_ptr) {
        memcpy(s->data, (const void *)(intptr_t)data_ptr, (size_t)s->data_size);
    }
    pthread_rwlock_unlock(&s->lock);
    return (int64_t)(intptr_t)old;
}

int64_t rask_mutex_get(int64_t mutex) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    RASK_CHECK_NONNULL(m, "Shared.get: box is null");
    return (int64_t)(intptr_t)m->data;
}

void rask_mutex_set(int64_t mutex, int64_t data_ptr) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    RASK_CHECK_NONNULL(m, "Shared.set: box is null");
    if (!data_ptr) return;
    pthread_mutex_lock(&m->lock);
    memcpy(m->data, (const void *)(intptr_t)data_ptr, (size_t)m->data_size);
    pthread_mutex_unlock(&m->lock);
}

int64_t rask_mutex_replace(int64_t mutex, int64_t data_ptr) {
    RaskMutex *m = (RaskMutex *)(intptr_t)mutex;
    RASK_CHECK_NONNULL(m, "Shared.replace: box is null");
    void *old = rask_alloc(m->data_size);
    pthread_mutex_lock(&m->lock);
    memcpy(old, m->data, (size_t)m->data_size);
    if (data_ptr) {
        memcpy(m->data, (const void *)(intptr_t)data_ptr, (size_t)m->data_size);
    }
    pthread_mutex_unlock(&m->lock);
    return (int64_t)(intptr_t)old;
}
