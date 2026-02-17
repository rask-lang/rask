// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Backend-agnostic I/O engine for the green scheduler.
//
// Two backends: io_uring (Linux 5.6+) and epoll fallback.
// rask_io_create() auto-detects: tries io_uring first, falls back to epoll.
//
// Operations are completion-based: submit → callback fires when done.
// The scheduler calls poll() to process completions between task switches.

#ifndef RASK_IO_ENGINE_H
#define RASK_IO_ENGINE_H

#include <stdint.h>
#include <stddef.h>

// Callback signature: result is bytes transferred (or fd for accept),
// err is errno on failure (0 on success).
typedef void (*rask_io_cb)(void *userdata, int64_t result, int err);

typedef struct RaskIoEngine RaskIoEngine;

struct RaskIoEngine {
    void (*submit_read)(RaskIoEngine *e, int fd, void *buf, size_t len,
                        rask_io_cb cb, void *ud);
    void (*submit_write)(RaskIoEngine *e, int fd, const void *buf, size_t len,
                         rask_io_cb cb, void *ud);
    void (*submit_accept)(RaskIoEngine *e, int listen_fd,
                          rask_io_cb cb, void *ud);
    void (*submit_timeout)(RaskIoEngine *e, uint64_t ns,
                           rask_io_cb cb, void *ud);

    // Process completions. Returns number of callbacks fired.
    // timeout_ms: 0 = non-blocking peek, -1 = block until at least one.
    int (*poll)(RaskIoEngine *e, int timeout_ms);

    // Pending operation count (for shutdown draining).
    int (*pending)(RaskIoEngine *e);

    void (*destroy)(RaskIoEngine *e);
};

// Auto-detect best backend. Returns NULL on failure (shouldn't happen on Linux).
RaskIoEngine *rask_io_create(void);

// Explicit constructors (used by tests or forced selection).
RaskIoEngine *rask_io_create_uring(void);
RaskIoEngine *rask_io_create_epoll(void);

#endif // RASK_IO_ENGINE_H
