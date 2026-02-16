// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Channels — bounded ring buffer or rendezvous (unbuffered).
//
// Based on conc.async/CH1-CH4:
//   - Sender/Receiver are non-linear (can be dropped without close)
//   - Close-on-drop when refcount hits zero
//   - Buffered: ring buffer with capacity N
//   - Unbuffered (capacity=0): direct handoff (sender blocks until receiver)
//
// Both halves share a RaskChannel through refcounting. Senders and receivers
// each have their own refcount. When all senders drop, receivers see CLOSED.
// When all receivers drop, senders see CLOSED.

#include "rask_runtime.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <pthread.h>
#include <stdatomic.h>

// ─── Channel internals ─────────────────────────────────────

struct RaskChannel {
    // Shared state
    pthread_mutex_t mutex;
    pthread_cond_t  not_full;
    pthread_cond_t  not_empty;

    // Ring buffer
    void   *buffer;          // NULL for unbuffered
    int64_t elem_size;
    int64_t capacity;        // 0 = unbuffered
    int64_t head;            // next read position
    int64_t tail;            // next write position
    int64_t count;           // items in buffer

    // Unbuffered handoff slot
    const void *handoff_data;  // pointer to sender's data (unbuffered only)
    int         handoff_ready; // sender has data waiting
    int         handoff_taken; // receiver has copied data

    // Lifecycle
    atomic_int sender_count;
    atomic_int recver_count;
    int        closed;       // protected by mutex
};

struct RaskSender {
    RaskChannel *chan;
};

struct RaskRecver {
    RaskChannel *chan;
};

static RaskChannel *channel_alloc(int64_t elem_size, int64_t capacity) {
    RaskChannel *ch = (RaskChannel *)calloc(1, sizeof(RaskChannel));
    if (!ch) {
        fprintf(stderr, "rask: channel alloc failed\n");
        abort();
    }

    pthread_mutex_init(&ch->mutex, NULL);
    pthread_cond_init(&ch->not_full, NULL);
    pthread_cond_init(&ch->not_empty, NULL);

    ch->elem_size = elem_size;
    ch->capacity  = capacity;
    ch->head = ch->tail = ch->count = 0;
    ch->handoff_data  = NULL;
    ch->handoff_ready = 0;
    ch->handoff_taken = 0;

    if (capacity > 0) {
        ch->buffer = calloc((size_t)capacity, (size_t)elem_size);
        if (!ch->buffer) {
            fprintf(stderr, "rask: channel buffer alloc failed\n");
            abort();
        }
    }

    atomic_init(&ch->sender_count, 1);
    atomic_init(&ch->recver_count, 1);
    ch->closed = 0;

    return ch;
}

static void channel_destroy(RaskChannel *ch) {
    pthread_mutex_destroy(&ch->mutex);
    pthread_cond_destroy(&ch->not_full);
    pthread_cond_destroy(&ch->not_empty);
    free(ch->buffer);
    free(ch);
}

// Try to destroy if both sides are gone
static void channel_maybe_destroy(RaskChannel *ch) {
    int s = atomic_load_explicit(&ch->sender_count, memory_order_acquire);
    int r = atomic_load_explicit(&ch->recver_count, memory_order_acquire);
    if (s == 0 && r == 0) {
        channel_destroy(ch);
    }
}

// ─── Buffered operations ───────────────────────────────────

static int64_t buffered_send(RaskChannel *ch, const void *data) {
    pthread_mutex_lock(&ch->mutex);

    while (ch->count >= ch->capacity && !ch->closed) {
        // Check if all receivers are gone
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        pthread_cond_wait(&ch->not_full, &ch->mutex);
    }

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return RASK_CHAN_CLOSED;
    }

    char *slot = (char *)ch->buffer + ch->tail * ch->elem_size;
    memcpy(slot, data, (size_t)ch->elem_size);
    ch->tail = (ch->tail + 1) % ch->capacity;
    ch->count++;

    pthread_cond_signal(&ch->not_empty);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

static int64_t buffered_recv(RaskChannel *ch, void *data_out) {
    pthread_mutex_lock(&ch->mutex);

    while (ch->count == 0) {
        // Empty — check if senders are gone
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0) {
            pthread_mutex_unlock(&ch->mutex);
            return RASK_CHAN_CLOSED;
        }
        if (ch->closed) {
            pthread_mutex_unlock(&ch->mutex);
            return RASK_CHAN_CLOSED;
        }
        pthread_cond_wait(&ch->not_empty, &ch->mutex);
    }

    char *slot = (char *)ch->buffer + ch->head * ch->elem_size;
    memcpy(data_out, slot, (size_t)ch->elem_size);
    ch->head = (ch->head + 1) % ch->capacity;
    ch->count--;

    pthread_cond_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

static int64_t buffered_try_send(RaskChannel *ch, const void *data) {
    pthread_mutex_lock(&ch->mutex);

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return RASK_CHAN_CLOSED;
    }

    if (ch->count >= ch->capacity) {
        pthread_mutex_unlock(&ch->mutex);
        return RASK_CHAN_FULL;
    }

    char *slot = (char *)ch->buffer + ch->tail * ch->elem_size;
    memcpy(slot, data, (size_t)ch->elem_size);
    ch->tail = (ch->tail + 1) % ch->capacity;
    ch->count++;

    pthread_cond_signal(&ch->not_empty);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

static int64_t buffered_try_recv(RaskChannel *ch, void *data_out) {
    pthread_mutex_lock(&ch->mutex);

    if (ch->count == 0) {
        int64_t result = RASK_CHAN_EMPTY;
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0 ||
            ch->closed) {
            result = RASK_CHAN_CLOSED;
        }
        pthread_mutex_unlock(&ch->mutex);
        return result;
    }

    char *slot = (char *)ch->buffer + ch->head * ch->elem_size;
    memcpy(data_out, slot, (size_t)ch->elem_size);
    ch->head = (ch->head + 1) % ch->capacity;
    ch->count--;

    pthread_cond_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

// ─── Unbuffered (rendezvous) operations ────────────────────
// Sender blocks until a receiver takes the value directly.

static int64_t unbuffered_send(RaskChannel *ch, const void *data) {
    pthread_mutex_lock(&ch->mutex);

    // Wait for previous handoff to complete
    while (ch->handoff_ready && !ch->closed) {
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        pthread_cond_wait(&ch->not_full, &ch->mutex);
    }

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return RASK_CHAN_CLOSED;
    }

    // Offer data to receiver
    ch->handoff_data  = data;
    ch->handoff_ready = 1;
    ch->handoff_taken = 0;
    pthread_cond_signal(&ch->not_empty);

    // Wait until receiver copies the data
    while (!ch->handoff_taken && !ch->closed) {
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        pthread_cond_wait(&ch->not_full, &ch->mutex);
    }

    ch->handoff_ready = 0;
    ch->handoff_data  = NULL;
    ch->handoff_taken = 0;

    int was_closed = ch->closed;
    pthread_mutex_unlock(&ch->mutex);
    return was_closed ? RASK_CHAN_CLOSED : RASK_CHAN_OK;
}

static int64_t unbuffered_recv(RaskChannel *ch, void *data_out) {
    pthread_mutex_lock(&ch->mutex);

    while (!ch->handoff_ready) {
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0 ||
            ch->closed) {
            pthread_mutex_unlock(&ch->mutex);
            return RASK_CHAN_CLOSED;
        }
        pthread_cond_wait(&ch->not_empty, &ch->mutex);
    }

    // Copy from sender's data
    memcpy(data_out, ch->handoff_data, (size_t)ch->elem_size);

    // Clear ready flag BEFORE signaling sender — prevents the receiver from
    // re-entering recv and seeing the stale handoff_ready=1 before the sender
    // has a chance to reset it.
    ch->handoff_ready = 0;
    ch->handoff_taken = 1;

    // Wake sender to let it know we've taken the data
    pthread_cond_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

static int64_t unbuffered_try_send(RaskChannel *ch, const void *data) {
    (void)data;
    pthread_mutex_lock(&ch->mutex);

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return RASK_CHAN_CLOSED;
    }

    // Unbuffered try_send only succeeds if a receiver is already waiting.
    // We can't guarantee that without a rendezvous, so always return FULL.
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_FULL;
}

static int64_t unbuffered_try_recv(RaskChannel *ch, void *data_out) {
    pthread_mutex_lock(&ch->mutex);

    if (!ch->handoff_ready) {
        int64_t result = RASK_CHAN_EMPTY;
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0 ||
            ch->closed) {
            result = RASK_CHAN_CLOSED;
        }
        pthread_mutex_unlock(&ch->mutex);
        return result;
    }

    memcpy(data_out, ch->handoff_data, (size_t)ch->elem_size);
    ch->handoff_taken = 1;
    pthread_cond_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

// ─── Public API ────────────────────────────────────────────

void rask_channel_new(int64_t elem_size, int64_t capacity,
                      RaskSender **tx_out, RaskRecver **rx_out) {
    if (elem_size <= 0) {
        rask_panic("channel element size must be positive");
    }
    if (capacity < 0) {
        rask_panic("channel capacity must be non-negative");
    }

    RaskChannel *ch = channel_alloc(elem_size, capacity);

    RaskSender *tx = (RaskSender *)malloc(sizeof(RaskSender));
    RaskRecver *rx = (RaskRecver *)malloc(sizeof(RaskRecver));
    if (!tx || !rx) {
        fprintf(stderr, "rask: channel sender/receiver alloc failed\n");
        abort();
    }
    tx->chan = ch;
    rx->chan = ch;

    *tx_out = tx;
    *rx_out = rx;
}

int64_t rask_channel_send(RaskSender *tx, const void *data) {
    RaskChannel *ch = tx->chan;
    if (ch->capacity > 0) {
        return buffered_send(ch, data);
    }
    return unbuffered_send(ch, data);
}

int64_t rask_channel_recv(RaskRecver *rx, void *data_out) {
    RaskChannel *ch = rx->chan;
    if (ch->capacity > 0) {
        return buffered_recv(ch, data_out);
    }
    return unbuffered_recv(ch, data_out);
}

int64_t rask_channel_try_send(RaskSender *tx, const void *data) {
    RaskChannel *ch = tx->chan;
    if (ch->capacity > 0) {
        return buffered_try_send(ch, data);
    }
    return unbuffered_try_send(ch, data);
}

int64_t rask_channel_try_recv(RaskRecver *rx, void *data_out) {
    RaskChannel *ch = rx->chan;
    if (ch->capacity > 0) {
        return buffered_try_recv(ch, data_out);
    }
    return unbuffered_try_recv(ch, data_out);
}

RaskSender *rask_sender_clone(RaskSender *tx) {
    atomic_fetch_add_explicit(&tx->chan->sender_count, 1, memory_order_relaxed);
    RaskSender *clone = (RaskSender *)malloc(sizeof(RaskSender));
    if (!clone) {
        fprintf(stderr, "rask: sender clone alloc failed\n");
        abort();
    }
    clone->chan = tx->chan;
    return clone;
}

void rask_sender_drop(RaskSender *tx) {
    RaskChannel *ch = tx->chan;
    free(tx);

    if (atomic_fetch_sub_explicit(&ch->sender_count, 1, memory_order_acq_rel) == 1) {
        // Last sender dropped — wake any blocked receivers
        pthread_mutex_lock(&ch->mutex);
        ch->closed = 1;
        pthread_cond_broadcast(&ch->not_empty);
        pthread_mutex_unlock(&ch->mutex);
        channel_maybe_destroy(ch);
    }
}

void rask_recver_drop(RaskRecver *rx) {
    RaskChannel *ch = rx->chan;
    free(rx);

    if (atomic_fetch_sub_explicit(&ch->recver_count, 1, memory_order_acq_rel) == 1) {
        // Last receiver dropped — wake any blocked senders
        pthread_mutex_lock(&ch->mutex);
        ch->closed = 1;
        pthread_cond_broadcast(&ch->not_full);
        pthread_mutex_unlock(&ch->mutex);
        channel_maybe_destroy(ch);
    }
}
