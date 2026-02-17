// SPDX-License-Identifier: (MIT OR Apache-2.0)

// io_uring I/O engine backend.
//
// Uses raw syscalls (no liburing dependency). Completion-based: prep SQE
// with opcode + userdata, submit, then reap CQEs on poll.
//
// Requires Linux 5.6+ for IORING_OP_READ/WRITE. Falls back gracefully
// if io_uring_setup returns -ENOSYS.

#include "io_engine.h"

#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <linux/io_uring.h>
#include <stdatomic.h>
#include <pthread.h>

// ─── Syscall wrappers ───────────────────────────────────────

static int io_uring_setup(unsigned entries, struct io_uring_params *p) {
    return (int)syscall(__NR_io_uring_setup, entries, p);
}

static int io_uring_enter(int fd, unsigned to_submit, unsigned min_complete,
                          unsigned flags) {
    return (int)syscall(__NR_io_uring_enter, fd, to_submit, min_complete,
                        flags, NULL, 0);
}

// ─── Completion callback storage ────────────────────────────

typedef struct {
    rask_io_cb cb;
    void      *ud;
} IoOp;

// ─── Engine state ───────────────────────────────────────────

#define URING_ENTRIES 256

typedef struct {
    RaskIoEngine  base;
    int           ring_fd;

    // SQ ring (mapped)
    uint32_t     *sq_head;
    uint32_t     *sq_tail;
    uint32_t     *sq_ring_mask;
    uint32_t     *sq_array;
    struct io_uring_sqe *sqes;

    // CQ ring (mapped)
    uint32_t     *cq_head;
    uint32_t     *cq_tail;
    uint32_t     *cq_ring_mask;
    struct io_uring_cqe *cqes;

    void         *sq_ring_ptr;
    size_t        sq_ring_size;
    void         *cq_ring_ptr;
    size_t        cq_ring_size;
    void         *sqe_ptr;
    size_t        sqe_size;

    atomic_int    pending_count;

    // Op storage: indexed by SQE index
    IoOp          ops[URING_ENTRIES];
    // Freelist for op slots
    uint32_t      free_slots[URING_ENTRIES];
    int           free_top;

    // Timeout specs storage (io_uring needs stable memory for TIMEOUT ops).
    // Matches struct __kernel_timespec layout but avoids header dependency.
    struct { int64_t tv_sec; long long tv_nsec; } timeouts[URING_ENTRIES];

    // Protects SQ/CQ ring access and slot allocation from concurrent workers.
    pthread_mutex_t lock;
} UringEngine;

// ─── SQE allocation ─────────────────────────────────────────

static int alloc_slot(UringEngine *ue) {
    if (ue->free_top <= 0) return -1;
    return (int)ue->free_slots[--ue->free_top];
}

static void free_slot(UringEngine *ue, uint32_t slot) {
    ue->free_slots[ue->free_top++] = slot;
}

static struct io_uring_sqe *get_sqe(UringEngine *ue) {
    uint32_t tail = atomic_load_explicit((_Atomic uint32_t *)ue->sq_tail,
                                          memory_order_relaxed);
    uint32_t head = atomic_load_explicit((_Atomic uint32_t *)ue->sq_head,
                                          memory_order_acquire);
    uint32_t mask = *ue->sq_ring_mask;

    if (tail - head >= URING_ENTRIES) return NULL; // SQ full

    uint32_t idx = tail & mask;
    struct io_uring_sqe *sqe = &ue->sqes[idx];
    memset(sqe, 0, sizeof(*sqe));

    ue->sq_array[idx] = idx;
    atomic_store_explicit((_Atomic uint32_t *)ue->sq_tail, tail + 1,
                          memory_order_release);
    return sqe;
}

// Submit all queued SQEs to the kernel.
static int flush_sq(UringEngine *ue) {
    int ret = io_uring_enter(ue->ring_fd, URING_ENTRIES, 0, 0);
    if (ret < 0 && errno != EBUSY) return -errno;
    return ret >= 0 ? ret : 0;
}

// ─── Submit operations ──────────────────────────────────────

static void uring_submit_read(RaskIoEngine *e, int fd, void *buf, size_t len,
                               rask_io_cb cb, void *ud) {
    UringEngine *ue = (UringEngine *)e;
    pthread_mutex_lock(&ue->lock);

    int slot = alloc_slot(ue);
    if (slot < 0) { pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    ue->ops[slot].cb = cb;
    ue->ops[slot].ud = ud;

    struct io_uring_sqe *sqe = get_sqe(ue);
    if (!sqe) { free_slot(ue, (uint32_t)slot); pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    sqe->opcode  = IORING_OP_READ;
    sqe->fd      = fd;
    sqe->addr    = (uint64_t)(uintptr_t)buf;
    sqe->len     = (uint32_t)len;
    sqe->off     = (uint64_t)-1; // current file position
    sqe->user_data = (uint64_t)slot;

    atomic_fetch_add_explicit(&ue->pending_count, 1, memory_order_relaxed);
    flush_sq(ue);
    pthread_mutex_unlock(&ue->lock);
}

static void uring_submit_write(RaskIoEngine *e, int fd, const void *buf,
                                size_t len, rask_io_cb cb, void *ud) {
    UringEngine *ue = (UringEngine *)e;
    pthread_mutex_lock(&ue->lock);

    int slot = alloc_slot(ue);
    if (slot < 0) { pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    ue->ops[slot].cb = cb;
    ue->ops[slot].ud = ud;

    struct io_uring_sqe *sqe = get_sqe(ue);
    if (!sqe) { free_slot(ue, (uint32_t)slot); pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    sqe->opcode  = IORING_OP_WRITE;
    sqe->fd      = fd;
    sqe->addr    = (uint64_t)(uintptr_t)buf;
    sqe->len     = (uint32_t)len;
    sqe->off     = (uint64_t)-1;
    sqe->user_data = (uint64_t)slot;

    atomic_fetch_add_explicit(&ue->pending_count, 1, memory_order_relaxed);
    flush_sq(ue);
    pthread_mutex_unlock(&ue->lock);
}

static void uring_submit_accept(RaskIoEngine *e, int listen_fd,
                                 rask_io_cb cb, void *ud) {
    UringEngine *ue = (UringEngine *)e;
    pthread_mutex_lock(&ue->lock);

    int slot = alloc_slot(ue);
    if (slot < 0) { pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    ue->ops[slot].cb = cb;
    ue->ops[slot].ud = ud;

    struct io_uring_sqe *sqe = get_sqe(ue);
    if (!sqe) { free_slot(ue, (uint32_t)slot); pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    sqe->opcode  = IORING_OP_ACCEPT;
    sqe->fd      = listen_fd;
    sqe->addr    = 0;
    sqe->addr2   = 0;
    sqe->user_data = (uint64_t)slot;

    atomic_fetch_add_explicit(&ue->pending_count, 1, memory_order_relaxed);
    flush_sq(ue);
    pthread_mutex_unlock(&ue->lock);
}

static void uring_submit_timeout(RaskIoEngine *e, uint64_t ns,
                                  rask_io_cb cb, void *ud) {
    UringEngine *ue = (UringEngine *)e;
    pthread_mutex_lock(&ue->lock);

    int slot = alloc_slot(ue);
    if (slot < 0) { pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    ue->ops[slot].cb = cb;
    ue->ops[slot].ud = ud;

    ue->timeouts[slot].tv_sec  = (long long)(ns / 1000000000ULL);
    ue->timeouts[slot].tv_nsec = (long long)(ns % 1000000000ULL);

    struct io_uring_sqe *sqe = get_sqe(ue);
    if (!sqe) { free_slot(ue, (uint32_t)slot); pthread_mutex_unlock(&ue->lock); cb(ud, -1, ENOMEM); return; }

    sqe->opcode  = IORING_OP_TIMEOUT;
    sqe->fd      = -1;
    sqe->addr    = (uint64_t)(uintptr_t)&ue->timeouts[slot];
    sqe->len     = 1;
    sqe->off     = 0;
    sqe->user_data = (uint64_t)slot;

    atomic_fetch_add_explicit(&ue->pending_count, 1, memory_order_relaxed);
    flush_sq(ue);
    pthread_mutex_unlock(&ue->lock);
}

// ─── Poll completions ───────────────────────────────────────

static int uring_poll(RaskIoEngine *e, int timeout_ms) {
    UringEngine *ue = (UringEngine *)e;

    if (timeout_ms != 0) {
        unsigned flags = IORING_ENTER_GETEVENTS;
        io_uring_enter(ue->ring_fd, 0, 1, flags);
    }

    pthread_mutex_lock(&ue->lock);

    int fired = 0;
    uint32_t mask = *ue->cq_ring_mask;

    for (;;) {
        uint32_t head = atomic_load_explicit((_Atomic uint32_t *)ue->cq_head,
                                              memory_order_acquire);
        uint32_t tail = atomic_load_explicit((_Atomic uint32_t *)ue->cq_tail,
                                              memory_order_acquire);
        if (head == tail) break;

        struct io_uring_cqe *cqe = &ue->cqes[head & mask];
        uint32_t slot = (uint32_t)cqe->user_data;

        rask_io_cb cb = NULL;
        void *ud = NULL;
        int64_t result = 0;
        int err = 0;

        if (slot < URING_ENTRIES && ue->ops[slot].cb) {
            cb = ue->ops[slot].cb;
            ud = ue->ops[slot].ud;
            result = cqe->res;
            if (cqe->res < 0) {
                err = -(int)cqe->res;
                result = -1;
            }
            ue->ops[slot].cb = NULL;
            free_slot(ue, slot);
            atomic_fetch_sub_explicit(&ue->pending_count, 1,
                                       memory_order_relaxed);
        }

        atomic_store_explicit((_Atomic uint32_t *)ue->cq_head, head + 1,
                              memory_order_release);
        fired++;

        // Fire callback outside lock to avoid deadlock (callback may re-submit)
        if (cb) {
            pthread_mutex_unlock(&ue->lock);
            cb(ud, result, err);
            pthread_mutex_lock(&ue->lock);
        }
    }

    pthread_mutex_unlock(&ue->lock);
    return fired;
}

static int uring_pending(RaskIoEngine *e) {
    UringEngine *ue = (UringEngine *)e;
    return atomic_load_explicit(&ue->pending_count, memory_order_relaxed);
}

// ─── Destroy ────────────────────────────────────────────────

static void uring_destroy(RaskIoEngine *e) {
    UringEngine *ue = (UringEngine *)e;
    pthread_mutex_destroy(&ue->lock);
    if (ue->sq_ring_ptr)
        munmap(ue->sq_ring_ptr, ue->sq_ring_size);
    if (ue->cq_ring_ptr && ue->cq_ring_ptr != ue->sq_ring_ptr)
        munmap(ue->cq_ring_ptr, ue->cq_ring_size);
    if (ue->sqe_ptr)
        munmap(ue->sqe_ptr, ue->sqe_size);
    if (ue->ring_fd >= 0)
        close(ue->ring_fd);
    free(ue);
}

// ─── Create ─────────────────────────────────────────────────

RaskIoEngine *rask_io_create_uring(void) {
    struct io_uring_params params;
    memset(&params, 0, sizeof(params));

    int fd = io_uring_setup(URING_ENTRIES, &params);
    if (fd < 0) return NULL;

    UringEngine *ue = (UringEngine *)calloc(1, sizeof(UringEngine));
    if (!ue) { close(fd); return NULL; }

    ue->ring_fd = fd;
    atomic_init(&ue->pending_count, 0);
    pthread_mutex_init(&ue->lock, NULL);

    // Map SQ ring
    ue->sq_ring_size = params.sq_off.array +
                       params.sq_entries * sizeof(uint32_t);
    ue->sq_ring_ptr = mmap(NULL, ue->sq_ring_size, PROT_READ | PROT_WRITE,
                           MAP_SHARED | MAP_POPULATE, fd,
                           IORING_OFF_SQ_RING);
    if (ue->sq_ring_ptr == MAP_FAILED) goto fail;

    ue->sq_head      = (uint32_t *)((char *)ue->sq_ring_ptr + params.sq_off.head);
    ue->sq_tail      = (uint32_t *)((char *)ue->sq_ring_ptr + params.sq_off.tail);
    ue->sq_ring_mask = (uint32_t *)((char *)ue->sq_ring_ptr + params.sq_off.ring_mask);
    ue->sq_array     = (uint32_t *)((char *)ue->sq_ring_ptr + params.sq_off.array);

    // Map SQEs
    ue->sqe_size = params.sq_entries * sizeof(struct io_uring_sqe);
    ue->sqe_ptr = mmap(NULL, ue->sqe_size, PROT_READ | PROT_WRITE,
                       MAP_SHARED | MAP_POPULATE, fd,
                       IORING_OFF_SQES);
    if (ue->sqe_ptr == MAP_FAILED) goto fail;
    ue->sqes = (struct io_uring_sqe *)ue->sqe_ptr;

    // Map CQ ring
    ue->cq_ring_size = params.cq_off.cqes +
                       params.cq_entries * sizeof(struct io_uring_cqe);
    if (params.features & IORING_FEAT_SINGLE_MMAP) {
        ue->cq_ring_ptr = ue->sq_ring_ptr;
    } else {
        ue->cq_ring_ptr = mmap(NULL, ue->cq_ring_size, PROT_READ | PROT_WRITE,
                               MAP_SHARED | MAP_POPULATE, fd,
                               IORING_OFF_CQ_RING);
        if (ue->cq_ring_ptr == MAP_FAILED) goto fail;
    }

    ue->cq_head      = (uint32_t *)((char *)ue->cq_ring_ptr + params.cq_off.head);
    ue->cq_tail      = (uint32_t *)((char *)ue->cq_ring_ptr + params.cq_off.tail);
    ue->cq_ring_mask = (uint32_t *)((char *)ue->cq_ring_ptr + params.cq_off.ring_mask);
    ue->cqes         = (struct io_uring_cqe *)((char *)ue->cq_ring_ptr + params.cq_off.cqes);

    // Init freelist
    ue->free_top = URING_ENTRIES;
    for (int i = 0; i < URING_ENTRIES; i++) {
        ue->free_slots[i] = (uint32_t)i;
    }

    // Wire up vtable
    ue->base.submit_read    = uring_submit_read;
    ue->base.submit_write   = uring_submit_write;
    ue->base.submit_accept  = uring_submit_accept;
    ue->base.submit_timeout = uring_submit_timeout;
    ue->base.poll           = uring_poll;
    ue->base.pending        = uring_pending;
    ue->base.destroy        = uring_destroy;

    return &ue->base;

fail:
    uring_destroy(&ue->base);
    return NULL;
}
