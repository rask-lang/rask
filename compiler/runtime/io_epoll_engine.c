// SPDX-License-Identifier: (MIT OR Apache-2.0)

// epoll-based I/O engine backend (fallback for pre-5.6 kernels).
//
// Readiness-based: submit stores the pending op, sets FD non-blocking,
// tries the operation immediately. On EAGAIN, registers with epoll.
// poll() calls epoll_wait, retries ready FDs, fires callbacks.
//
// Timeouts use a sorted linked list (good enough for reasonable counts).

#define _GNU_SOURCE  // for accept4

#include "io_engine.h"

#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <time.h>
#include <stdatomic.h>
#include <pthread.h>

// ─── Pending operation types ────────────────────────────────

typedef enum {
    OP_READ,
    OP_WRITE,
    OP_ACCEPT,
    OP_TIMEOUT,
} OpType;

typedef struct PendingOp {
    OpType        type;
    int           fd;        // -1 for timeout
    void         *buf;
    size_t        len;
    rask_io_cb    cb;
    void         *ud;
    uint64_t      deadline_ns;  // for timeouts (CLOCK_MONOTONIC)
    struct PendingOp *next;     // timeout list linkage
} PendingOp;

// ─── FD-indexed op map ──────────────────────────────────────

#define MAX_FDS 4096

typedef struct {
    RaskIoEngine  base;
    int           epoll_fd;
    PendingOp    *fd_ops[MAX_FDS];  // indexed by fd
    PendingOp    *timeouts;          // sorted by deadline
    atomic_int    pending_count;
    pthread_mutex_t lock;
} EpollEngine;

// ─── Helpers ────────────────────────────────────────────────

static int set_nonblocking(int fd) {
    int flags = fcntl(fd, F_GETFL);
    if (flags < 0) return -1;
    return fcntl(fd, F_SETFL, flags | O_NONBLOCK);
}

static uint64_t clock_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

static void register_fd(EpollEngine *ee, int fd, uint32_t events) {
    struct epoll_event ev = { .events = events | EPOLLONESHOT, .data.fd = fd };
    // Try add first, mod if already registered
    if (epoll_ctl(ee->epoll_fd, EPOLL_CTL_ADD, fd, &ev) < 0) {
        if (errno == EEXIST)
            epoll_ctl(ee->epoll_fd, EPOLL_CTL_MOD, fd, &ev);
    }
}

// ─── Submit operations ──────────────────────────────────────

static void epoll_submit_read(RaskIoEngine *e, int fd, void *buf, size_t len,
                               rask_io_cb cb, void *ud) {
    EpollEngine *ee = (EpollEngine *)e;
    set_nonblocking(fd);

    // Fast path: try immediately
    ssize_t n = read(fd, buf, len);
    if (n >= 0) { cb(ud, n, 0); return; }
    if (errno != EAGAIN && errno != EWOULDBLOCK) {
        cb(ud, -1, errno); return;
    }

    // Slow path: register with epoll
    PendingOp *op = (PendingOp *)malloc(sizeof(PendingOp));
    if (!op) { cb(ud, -1, ENOMEM); return; }
    op->type = OP_READ;
    op->fd   = fd;
    op->buf  = buf;
    op->len  = len;
    op->cb   = cb;
    op->ud   = ud;
    op->next = NULL;

    pthread_mutex_lock(&ee->lock);
    if (fd >= 0 && fd < MAX_FDS) {
        ee->fd_ops[fd] = op;
    }
    register_fd(ee, fd, EPOLLIN);
    atomic_fetch_add_explicit(&ee->pending_count, 1, memory_order_relaxed);
    pthread_mutex_unlock(&ee->lock);
}

static void epoll_submit_write(RaskIoEngine *e, int fd, const void *buf,
                                size_t len, rask_io_cb cb, void *ud) {
    EpollEngine *ee = (EpollEngine *)e;
    set_nonblocking(fd);

    ssize_t n = write(fd, buf, len);
    if (n >= 0) { cb(ud, n, 0); return; }
    if (errno != EAGAIN && errno != EWOULDBLOCK) {
        cb(ud, -1, errno); return;
    }

    PendingOp *op = (PendingOp *)malloc(sizeof(PendingOp));
    if (!op) { cb(ud, -1, ENOMEM); return; }
    op->type = OP_WRITE;
    op->fd   = fd;
    op->buf  = (void *)buf;
    op->len  = len;
    op->cb   = cb;
    op->ud   = ud;
    op->next = NULL;

    pthread_mutex_lock(&ee->lock);
    if (fd >= 0 && fd < MAX_FDS) {
        ee->fd_ops[fd] = op;
    }
    register_fd(ee, fd, EPOLLOUT);
    atomic_fetch_add_explicit(&ee->pending_count, 1, memory_order_relaxed);
    pthread_mutex_unlock(&ee->lock);
}

static void epoll_submit_accept(RaskIoEngine *e, int listen_fd,
                                 rask_io_cb cb, void *ud) {
    EpollEngine *ee = (EpollEngine *)e;
    set_nonblocking(listen_fd);

    int client = accept4(listen_fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
    if (client >= 0) { cb(ud, client, 0); return; }
    if (errno != EAGAIN && errno != EWOULDBLOCK) {
        cb(ud, -1, errno); return;
    }

    PendingOp *op = (PendingOp *)malloc(sizeof(PendingOp));
    if (!op) { cb(ud, -1, ENOMEM); return; }
    op->type = OP_ACCEPT;
    op->fd   = listen_fd;
    op->buf  = NULL;
    op->len  = 0;
    op->cb   = cb;
    op->ud   = ud;
    op->next = NULL;

    pthread_mutex_lock(&ee->lock);
    if (listen_fd >= 0 && listen_fd < MAX_FDS) {
        ee->fd_ops[listen_fd] = op;
    }
    register_fd(ee, listen_fd, EPOLLIN);
    atomic_fetch_add_explicit(&ee->pending_count, 1, memory_order_relaxed);
    pthread_mutex_unlock(&ee->lock);
}

static void epoll_submit_timeout(RaskIoEngine *e, uint64_t ns,
                                  rask_io_cb cb, void *ud) {
    EpollEngine *ee = (EpollEngine *)e;

    PendingOp *op = (PendingOp *)malloc(sizeof(PendingOp));
    if (!op) { cb(ud, -1, ENOMEM); return; }
    op->type = OP_TIMEOUT;
    op->fd   = -1;
    op->buf  = NULL;
    op->len  = 0;
    op->cb   = cb;
    op->ud   = ud;
    op->deadline_ns = clock_ns() + ns;
    op->next = NULL;

    pthread_mutex_lock(&ee->lock);
    PendingOp **pp = &ee->timeouts;
    while (*pp && (*pp)->deadline_ns <= op->deadline_ns) {
        pp = &(*pp)->next;
    }
    op->next = *pp;
    *pp = op;
    atomic_fetch_add_explicit(&ee->pending_count, 1, memory_order_relaxed);
    pthread_mutex_unlock(&ee->lock);
}

// ─── Retry a ready FD operation ─────────────────────────────

static void retry_op(EpollEngine *ee __attribute__((unused)), PendingOp *op) {
    ssize_t n;
    switch (op->type) {
    case OP_READ:
        n = read(op->fd, op->buf, op->len);
        if (n >= 0) {
            op->cb(op->ud, n, 0);
        } else {
            op->cb(op->ud, -1, errno);
        }
        break;
    case OP_WRITE:
        n = write(op->fd, op->buf, op->len);
        if (n >= 0) {
            op->cb(op->ud, n, 0);
        } else {
            op->cb(op->ud, -1, errno);
        }
        break;
    case OP_ACCEPT: {
        int client = accept4(op->fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
        if (client >= 0) {
            op->cb(op->ud, client, 0);
        } else {
            op->cb(op->ud, -1, errno);
        }
        break;
    }
    default:
        break;
    }
}

// ─── Poll completions ───────────────────────────────────────

static int epoll_io_poll(RaskIoEngine *e, int timeout_ms) {
    EpollEngine *ee = (EpollEngine *)e;
    int fired = 0;

    pthread_mutex_lock(&ee->lock);

    // Check expired timeouts first
    uint64_t now = clock_ns();
    while (ee->timeouts && ee->timeouts->deadline_ns <= now) {
        PendingOp *op = ee->timeouts;
        ee->timeouts = op->next;
        atomic_fetch_sub_explicit(&ee->pending_count, 1, memory_order_relaxed);
        pthread_mutex_unlock(&ee->lock);
        op->cb(op->ud, 0, 0);
        free(op);
        fired++;
        pthread_mutex_lock(&ee->lock);
        now = clock_ns();
    }

    // Calculate epoll timeout: min of requested timeout and next timer deadline
    int epoll_timeout = timeout_ms;
    if (ee->timeouts) {
        uint64_t until_ns = ee->timeouts->deadline_ns - now;
        int until_ms = (int)(until_ns / 1000000ULL);
        if (until_ms < 0) until_ms = 0;
        if (epoll_timeout < 0 || until_ms < epoll_timeout) {
            epoll_timeout = until_ms;
        }
    }

    pthread_mutex_unlock(&ee->lock);

    // epoll_wait outside the lock (it's a blocking syscall)
    struct epoll_event events[64];
    int nfds = epoll_wait(ee->epoll_fd, events, 64, epoll_timeout);

    pthread_mutex_lock(&ee->lock);

    for (int i = 0; i < nfds; i++) {
        int fd = events[i].data.fd;
        if (fd < 0 || fd >= MAX_FDS) continue;

        PendingOp *op = ee->fd_ops[fd];
        if (!op) continue;

        ee->fd_ops[fd] = NULL;
        epoll_ctl(ee->epoll_fd, EPOLL_CTL_DEL, fd, NULL);
        atomic_fetch_sub_explicit(&ee->pending_count, 1, memory_order_relaxed);
        pthread_mutex_unlock(&ee->lock);
        retry_op(ee, op);
        free(op);
        fired++;
        pthread_mutex_lock(&ee->lock);
    }

    // Check timeouts again (epoll_wait may have taken time)
    if (nfds >= 0) {
        now = clock_ns();
        while (ee->timeouts && ee->timeouts->deadline_ns <= now) {
            PendingOp *op = ee->timeouts;
            ee->timeouts = op->next;
            atomic_fetch_sub_explicit(&ee->pending_count, 1,
                                       memory_order_relaxed);
            pthread_mutex_unlock(&ee->lock);
            op->cb(op->ud, 0, 0);
            free(op);
            fired++;
            pthread_mutex_lock(&ee->lock);
            now = clock_ns();
        }
    }

    pthread_mutex_unlock(&ee->lock);
    return fired;
}

static int epoll_io_pending(RaskIoEngine *e) {
    EpollEngine *ee = (EpollEngine *)e;
    return atomic_load_explicit(&ee->pending_count, memory_order_relaxed);
}

// ─── Destroy ────────────────────────────────────────────────

static void epoll_destroy(RaskIoEngine *e) {
    EpollEngine *ee = (EpollEngine *)e;

    // Free any pending FD ops
    for (int i = 0; i < MAX_FDS; i++) {
        if (ee->fd_ops[i]) {
            free(ee->fd_ops[i]);
        }
    }

    // Free timeout list
    PendingOp *t = ee->timeouts;
    while (t) {
        PendingOp *next = t->next;
        free(t);
        t = next;
    }

    pthread_mutex_destroy(&ee->lock);
    if (ee->epoll_fd >= 0) close(ee->epoll_fd);
    free(ee);
}

// ─── Create ─────────────────────────────────────────────────

RaskIoEngine *rask_io_create_epoll(void) {
    int efd = epoll_create1(EPOLL_CLOEXEC);
    if (efd < 0) return NULL;

    EpollEngine *ee = (EpollEngine *)calloc(1, sizeof(EpollEngine));
    if (!ee) { close(efd); return NULL; }

    ee->epoll_fd = efd;
    ee->timeouts = NULL;
    atomic_init(&ee->pending_count, 0);
    pthread_mutex_init(&ee->lock, NULL);

    ee->base.submit_read    = epoll_submit_read;
    ee->base.submit_write   = epoll_submit_write;
    ee->base.submit_accept  = epoll_submit_accept;
    ee->base.submit_timeout = epoll_submit_timeout;
    ee->base.poll           = epoll_io_poll;
    ee->base.pending        = epoll_io_pending;
    ee->base.destroy        = epoll_destroy;

    return &ee->base;
}

// ─── Auto-detect ────────────────────────────────────────────

RaskIoEngine *rask_io_create(void) {
    // Try io_uring first
    RaskIoEngine *e = rask_io_create_uring();
    if (e) return e;

    // Fall back to epoll
    return rask_io_create_epoll();
}
