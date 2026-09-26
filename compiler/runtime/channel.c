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
#include "sim.h"

#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <pthread.h>
#include <stdatomic.h>
#include <sched.h>

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

    // Unbuffered handoff slot. A sender owns it from offering a value until it
    // has seen that value taken, so a second sender can't install its own in
    // between and reset `handoff_taken` under the first (#1345).
    const void *handoff_data;  // pointer to sender's data (unbuffered only)
    int         handoff_busy;  // a sender owns the slot
    int         handoff_ready; // the value is there to take
    int         handoff_taken; // a receiver has copied it

    // Lifecycle
    atomic_int sender_count;
    atomic_int recver_count;
    // Every end, of either kind. The drop that takes it to zero frees the
    // channel, after it is done with the mutex.
    atomic_int ends;
    int        closed;       // protected by mutex
};

struct RaskSender {
    RaskChannel *chan;
};

struct RaskRecver {
    RaskChannel *chan;
};

static RaskChannel *channel_alloc(int64_t elem_size, int64_t capacity) {
    RaskChannel *ch = (RaskChannel *)rask_alloc(sizeof(RaskChannel));
    memset(ch, 0, sizeof(RaskChannel));

    pthread_mutex_init(&ch->mutex, NULL);
    pthread_cond_init(&ch->not_full, NULL);
    pthread_cond_init(&ch->not_empty, NULL);

    ch->elem_size = elem_size;
    ch->capacity  = capacity;
    ch->head = ch->tail = ch->count = 0;
    ch->handoff_data  = NULL;
    ch->handoff_busy  = 0;
    ch->handoff_ready = 0;
    ch->handoff_taken = 0;

    if (capacity > 0) {
        int64_t buf_size = capacity * elem_size;
        ch->buffer = rask_alloc(buf_size);
        memset(ch->buffer, 0, (size_t)buf_size);
    }

    atomic_init(&ch->sender_count, 1);
    atomic_init(&ch->recver_count, 1);
    atomic_init(&ch->ends, 2);
    ch->closed = 0;

    return ch;
}

static void channel_destroy(RaskChannel *ch) {
    pthread_mutex_destroy(&ch->mutex);
    pthread_cond_destroy(&ch->not_full);
    pthread_cond_destroy(&ch->not_empty);
    rask_free(ch->buffer);
    rask_free(ch);
}

// Drop one end. It used to free once it read both counts as zero, but the
// last sender and the last receiver dropping together could both read that,
// and one could free while the other still held the mutex.
static void channel_end_gone(RaskChannel *ch) {
    if (atomic_fetch_sub_explicit(&ch->ends, 1, memory_order_acq_rel) == 1) {
        channel_destroy(ch);
    }
}

// ─── Waking a select ───────────────────────────────────────
//
// A `select` with no arm ready waits for *any* of its channels to change, and
// one waiter can't sit on several condvars at once. So every change to any
// channel — a value in, a value out, an end closed — moves one global epoch,
// and a waiting select sleeps until the epoch moves past the one it read before
// probing its arms. Any change after that read wakes it, so none is missed. A
// change on an unrelated channel wakes it too, and it probes and sleeps again:
// a spurious wake, not a busy loop.
//
// The wait goes through rask_task_cond_wait, so a green task parks, a sim task
// parks under the seeded scheduler (and can be reported as deadlocked, #1342),
// and a thread blocks. Channel operations skip the lock entirely while no
// select is waiting.

static atomic_ullong select_epoch = 0;
static atomic_int select_waiters = 0;
static pthread_mutex_t select_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t select_cond = PTHREAD_COND_INITIALIZER;

static void select_notify(void) {
    atomic_fetch_add_explicit(&select_epoch, 1, memory_order_seq_cst);
    if (atomic_load_explicit(&select_waiters, memory_order_seq_cst) == 0) return;
    pthread_mutex_lock(&select_lock);
    rask_task_cond_broadcast(&select_cond);
    pthread_mutex_unlock(&select_lock);
}

static void chan_signal(pthread_cond_t *c) {
    rask_task_cond_signal(c);
    select_notify();
}

static void chan_broadcast(pthread_cond_t *c) {
    rask_task_cond_broadcast(c);
    select_notify();
}

int64_t rask_select_epoch(void) {
    return (int64_t)atomic_load_explicit(&select_epoch, memory_order_seq_cst);
}

// 1 when the task was cancelled while it waited (conc.select/CN1).
int64_t rask_select_wait(int64_t seen) {
    RaskCancelWake wake = rask_cancel_wake_cond(&select_lock, &select_cond);
    int cancelled = rask_cancel_wait_begin(&wake);
    atomic_fetch_add_explicit(&select_waiters, 1, memory_order_seq_cst);
    pthread_mutex_lock(&select_lock);
    while (!cancelled &&
           (int64_t)atomic_load_explicit(&select_epoch, memory_order_seq_cst) == seen) {
        if (rask_cancel_requested()) {
            cancelled = 1;
            break;
        }
        rask_task_cond_wait(&select_cond, &select_lock, "select");
    }
    pthread_mutex_unlock(&select_lock);
    atomic_fetch_sub_explicit(&select_waiters, 1, memory_order_seq_cst);
    rask_cancel_wait_end();
    return cancelled;
}

// ─── Waiting, and being cancelled while waiting ────────────
//
// A task parked on a channel is where a cancel most often finds it
// (conc.async/CN3), so each blocking loop waits through this. The cancel wake
// is registered at the first real wait, not on every call, and dropped once
// the channel's mutex is let go. A value that arrives together with the cancel
// is still taken: the loop checks its own condition before the flag.

typedef struct {
    RaskCancelWake wake;
    int            registered;
} ChanWait;

// 1 if the task was cancelled; the caller unlocks and reports it.
static int chan_wait(RaskChannel *ch, pthread_cond_t *c, ChanWait *cw, const char *what) {
    if (!cw->registered) {
        cw->wake = rask_cancel_wake_cond(&ch->mutex, c);
        cw->registered = 1;
        if (rask_cancel_wait_begin(&cw->wake)) return 1;
    }
    if (rask_cancel_requested()) return 1;
    rask_task_cond_wait(c, &ch->mutex, what);
    return 0;
}

// After the channel's mutex is let go.
static int64_t chan_wait_done(ChanWait *cw, int64_t result) {
    if (cw->registered) rask_cancel_wait_end();
    return result;
}

// ─── Buffered operations ───────────────────────────────────

static int64_t buffered_send(RaskChannel *ch, const void *data) {
    ChanWait cw = {0};
    pthread_mutex_lock(&ch->mutex);

    while (ch->count >= ch->capacity && !ch->closed) {
        // Check if all receivers are gone
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        if (chan_wait(ch, &ch->not_full, &cw, "channel send")) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CANCELLED);
        }
    }

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return chan_wait_done(&cw, RASK_CHAN_CLOSED);
    }

    char *slot = (char *)ch->buffer + ch->tail * ch->elem_size;
    memcpy(slot, data, (size_t)ch->elem_size);
    ch->tail = (ch->tail + 1) % ch->capacity;
    ch->count++;

    chan_signal(&ch->not_empty);
    pthread_mutex_unlock(&ch->mutex);
    return chan_wait_done(&cw, RASK_CHAN_OK);
}

static int64_t buffered_recv(RaskChannel *ch, void *data_out) {
    ChanWait cw = {0};
    pthread_mutex_lock(&ch->mutex);

    while (ch->count == 0) {
        // Empty — check if senders are gone
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0 ||
            ch->closed) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CLOSED);
        }
        if (chan_wait(ch, &ch->not_empty, &cw, "channel receive")) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CANCELLED);
        }
    }

    char *slot = (char *)ch->buffer + ch->head * ch->elem_size;
    memcpy(data_out, slot, (size_t)ch->elem_size);
    ch->head = (ch->head + 1) % ch->capacity;
    ch->count--;

    chan_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return chan_wait_done(&cw, RASK_CHAN_OK);
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

    chan_signal(&ch->not_empty);
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

    chan_signal(&ch->not_full);
    pthread_mutex_unlock(&ch->mutex);
    return RASK_CHAN_OK;
}

// ─── Unbuffered (rendezvous) operations ────────────────────
// Sender blocks until a receiver takes the value directly.

// `not_full` has two kinds of waiter on an unbuffered channel — senders
// waiting for the slot, and the slot's owner waiting for its value to be
// taken — so it is broadcast: a signal can wake the wrong kind and leave the
// right one asleep.

static int64_t unbuffered_send(RaskChannel *ch, const void *data) {
    ChanWait cw = {0};
    pthread_mutex_lock(&ch->mutex);

    // Wait for the slot.
    while (ch->handoff_busy && !ch->closed) {
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        if (chan_wait(ch, &ch->not_full, &cw, "channel send")) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CANCELLED);
        }
    }

    if (ch->closed ||
        atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
        pthread_mutex_unlock(&ch->mutex);
        return chan_wait_done(&cw, RASK_CHAN_CLOSED);
    }

    // Offer the value.
    ch->handoff_busy  = 1;
    ch->handoff_data  = data;
    ch->handoff_ready = 1;
    ch->handoff_taken = 0;
    chan_signal(&ch->not_empty);

    // Wait until a receiver has copied it. Cancelled first, the offer is
    // withdrawn below and the value was never sent.
    int cancelled = 0;
    while (!ch->handoff_taken && !ch->closed) {
        if (atomic_load_explicit(&ch->recver_count, memory_order_acquire) == 0) {
            ch->closed = 1;
            break;
        }
        if (chan_wait(ch, &ch->not_full, &cw, "channel send")) {
            cancelled = 1;
            break;
        }
    }

    int delivered = ch->handoff_taken;
    ch->handoff_busy  = 0;
    ch->handoff_ready = 0;
    ch->handoff_data  = NULL;
    ch->handoff_taken = 0;
    chan_broadcast(&ch->not_full);   // the next sender's turn

    pthread_mutex_unlock(&ch->mutex);
    return chan_wait_done(&cw, delivered ? RASK_CHAN_OK
                               : cancelled ? RASK_CHAN_CANCELLED : RASK_CHAN_CLOSED);
}

// Take the offered value. The caller holds the mutex and has seen it ready.
static void unbuffered_take(RaskChannel *ch, void *data_out) {
    memcpy(data_out, ch->handoff_data, (size_t)ch->elem_size);
    // Not ready any more, so a second receiver can't take it too; the slot
    // stays the sender's until it has seen this.
    ch->handoff_ready = 0;
    ch->handoff_taken = 1;
    chan_broadcast(&ch->not_full);
}

static int64_t unbuffered_recv(RaskChannel *ch, void *data_out) {
    ChanWait cw = {0};
    pthread_mutex_lock(&ch->mutex);

    while (!ch->handoff_ready) {
        if (atomic_load_explicit(&ch->sender_count, memory_order_acquire) == 0 ||
            ch->closed) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CLOSED);
        }
        if (chan_wait(ch, &ch->not_empty, &cw, "channel receive")) {
            pthread_mutex_unlock(&ch->mutex);
            return chan_wait_done(&cw, RASK_CHAN_CANCELLED);
        }
    }

    unbuffered_take(ch, data_out);
    pthread_mutex_unlock(&ch->mutex);
    return chan_wait_done(&cw, RASK_CHAN_OK);
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

    unbuffered_take(ch, data_out);
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

    RaskSender *tx = (RaskSender *)rask_alloc(sizeof(RaskSender));
    RaskRecver *rx = (RaskRecver *)rask_alloc(sizeof(RaskRecver));
    *tx = (RaskSender){ .chan = ch };
    *rx = (RaskRecver){ .chan = ch };

    *tx_out = tx;
    *rx_out = rx;
}

int64_t rask_channel_send(RaskSender *tx, const void *data) {
    RASK_SIM_POINT();
    RASK_CHECK_NONNULL(tx, "Sender.send: tx handle is null (bad channel destructure?)");
    RaskChannel *ch = tx->chan;
    if (ch->capacity > 0) {
        return buffered_send(ch, data);
    }
    return unbuffered_send(ch, data);
}

int64_t rask_channel_recv(RaskRecver *rx, void *data_out) {
    RASK_SIM_POINT();
    RASK_CHECK_NONNULL(rx, "Receiver.recv: rx handle is null (bad channel destructure?)");
    RaskChannel *ch = rx->chan;
    if (ch->capacity > 0) {
        return buffered_recv(ch, data_out);
    }
    return unbuffered_recv(ch, data_out);
}

int64_t rask_channel_try_send(RaskSender *tx, const void *data) {
    RASK_SIM_POINT();
    RASK_CHECK_NONNULL(tx, "Sender.try_send: tx handle is null");
    RaskChannel *ch = tx->chan;
    if (ch->capacity > 0) {
        return buffered_try_send(ch, data);
    }
    return unbuffered_try_send(ch, data);
}

int64_t rask_channel_try_recv(RaskRecver *rx, void *data_out) {
    RASK_SIM_POINT();
    RaskChannel *ch = rx->chan;
    if (ch->capacity > 0) {
        return buffered_try_recv(ch, data_out);
    }
    return unbuffered_try_recv(ch, data_out);
}

RaskSender *rask_sender_clone(RaskSender *tx) {
    atomic_fetch_add_explicit(&tx->chan->sender_count, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&tx->chan->ends, 1, memory_order_relaxed);
    RaskSender *clone = (RaskSender *)rask_alloc(sizeof(RaskSender));
    *clone = (RaskSender){ .chan = tx->chan };
    return clone;
}

void rask_sender_drop(RaskSender *tx) {
    RASK_SIM_POINT();
    RaskChannel *ch = tx->chan;
    rask_free(tx);

    if (atomic_fetch_sub_explicit(&ch->sender_count, 1, memory_order_acq_rel) == 1) {
        // Last sender dropped — wake any blocked receivers
        pthread_mutex_lock(&ch->mutex);
        ch->closed = 1;
        chan_broadcast(&ch->not_empty);
        pthread_mutex_unlock(&ch->mutex);
    }
    channel_end_gone(ch);
}

void rask_recver_drop(RaskRecver *rx) {
    RASK_SIM_POINT();
    RaskChannel *ch = rx->chan;
    rask_free(rx);

    if (atomic_fetch_sub_explicit(&ch->recver_count, 1, memory_order_acq_rel) == 1) {
        // Last receiver dropped — wake any blocked senders
        pthread_mutex_lock(&ch->mutex);
        ch->closed = 1;
        chan_broadcast(&ch->not_full);
        pthread_mutex_unlock(&ch->mutex);
    }
    channel_end_gone(ch);
}

// ─── i64-based codegen wrappers ────────────────────────────
// The dispatch table passes all values as i64. These wrappers bridge
// between i64 calling convention and the typed channel API.

// `let (tx, rx) = Channel<T>.buffered(n)` reaches codegen as three calls: one
// to make the channel, then one for each half. What travels between them is the
// channel itself.
//
// It used to be a 16-byte heap pair holding the two handles, which nothing
// could free — both accessors read it and neither could know it was the last —
// so every channel leaked the pair on top of the two handles nobody dropped.
// Handing the channel over instead leaves nothing between the calls to own, and
// each accessor makes the one handle of its kind: `channel_alloc` initialises
// both counts to 1, so the handle it returns *is* that count, and dropping it
// is what closes that end.
int64_t rask_channel_new_i64(int64_t capacity) {
    return (int64_t)(intptr_t)channel_alloc(sizeof(int64_t), capacity);
}

int64_t rask_channel_get_tx(int64_t chan) {
    RaskSender *tx = (RaskSender *)rask_alloc(sizeof(RaskSender));
    *tx = (RaskSender){ .chan = (RaskChannel *)(intptr_t)chan };
    return (int64_t)(intptr_t)tx;
}

int64_t rask_channel_get_rx(int64_t chan) {
    RaskRecver *rx = (RaskRecver *)rask_alloc(sizeof(RaskRecver));
    *rx = (RaskRecver){ .chan = (RaskChannel *)(intptr_t)chan };
    return (int64_t)(intptr_t)rx;
}

int64_t rask_channel_send_i64(int64_t tx, int64_t value) {
    return rask_channel_send((RaskSender *)(intptr_t)tx, &value);
}

// Blocking receive that reports a closed channel instead of killing the task.
// `Receiver.receive` is declared `T or ReceiveError`, and while the recv
// wrappers panicked, that error branch was unreachable — a consumer that
// doesn't know how many messages to expect couldn't be written (#1067).
//
// Status is the channel's own: OK(0), or CLOSED(-1) when the last sender is
// gone and the buffer is drained.
int64_t rask_channel_recv_into(int64_t rx, int64_t out_ptr) {
    return rask_channel_recv((RaskRecver *)(intptr_t)rx, (void *)(intptr_t)out_ptr);
}

int64_t rask_channel_recv_i64(int64_t rx) {
    int64_t data = 0;
    int64_t status = rask_channel_recv((RaskRecver *)(intptr_t)rx, &data);
    if (status != RASK_CHAN_OK) {
        rask_panic("recv on closed channel");
    }
    return data;
}

void rask_sender_drop_i64(int64_t tx) {
    rask_sender_drop((RaskSender *)(intptr_t)tx);
}

void rask_recver_drop_i64(int64_t rx) {
    rask_recver_drop((RaskRecver *)(intptr_t)rx);
}

// Explicit close (CH4): drop disconnects the channel, returns Ok(unit)=0
int64_t rask_sender_close_i64(int64_t tx) {
    rask_sender_drop((RaskSender *)(intptr_t)tx);
    return 0; // Ok(())
}

int64_t rask_recver_close_i64(int64_t rx) {
    rask_recver_drop((RaskRecver *)(intptr_t)rx);
    return 0; // Ok(())
}

int64_t rask_sender_clone_i64(int64_t tx) {
    return (int64_t)(intptr_t)rask_sender_clone((RaskSender *)(intptr_t)tx);
}

int64_t rask_channel_try_send_i64(int64_t tx, int64_t value) {
    return rask_channel_try_send((RaskSender *)(intptr_t)tx, &value);
}

int64_t rask_channel_try_recv_i64(int64_t rx) {
    int64_t data = 0;
    int64_t status = rask_channel_try_recv((RaskRecver *)(intptr_t)rx, &data);
    if (status != RASK_CHAN_OK) {
        return status;
    }
    return data;
}

// Non-blocking recv into a caller buffer of the element's real size. Returns
// the status (RASK_CHAN_OK / _EMPTY / _CLOSED); codegen turns that into the
// `T or E` Result. Unlike the _i64 form this handles elements >8 bytes and
// never conflates a status with a legitimate value.
int64_t rask_channel_try_recv_into(int64_t rx, int64_t out_ptr) {
    return rask_channel_try_recv((RaskRecver *)(intptr_t)rx, (void *)(intptr_t)out_ptr);
}

// Round-robin starting offset for a native `select` (conc.select/P1): a
// plain `select` must not let one busy channel starve the rest. A real
// per-poll shuffle costs more than the fairness needs, so a shared counter
// rotating through the arms is enough — every select call advances it, so
// no single arm keeps the first probe slot forever. `select_priority`
// skips this and always probes in listed order.
static atomic_ullong rask_select_counter = 0;

int64_t rask_select_rotate(int64_t num_arms) {
    if (num_arms <= 0) {
        return 0;
    }
    unsigned long long n = atomic_fetch_add_explicit(&rask_select_counter, 1, memory_order_relaxed);
    return (int64_t)(n % (unsigned long long)num_arms);
}

// ─── Pointer-based wrappers for aggregate types ──────────
//
// These use the actual element size instead of hardcoding sizeof(int64_t).
// - new_ptr: creates channel with specified elem_size
// - send_ptr: sends elem_size bytes from data_ptr
// - recv_ptr: receives elem_size bytes into out_ptr

int64_t rask_channel_new_ptr(int64_t elem_size, int64_t capacity) {
    if (elem_size <= 0) {
        rask_panic("channel element size must be positive");
    }
    if (capacity < 0) {
        rask_panic("channel capacity must be non-negative");
    }
    return (int64_t)(intptr_t)channel_alloc(elem_size, capacity);
}

int64_t rask_channel_send_ptr(int64_t tx, int64_t data_ptr) {
    return rask_channel_send((RaskSender *)(intptr_t)tx, (const void *)(intptr_t)data_ptr);
}

int64_t rask_channel_recv_ptr(int64_t rx, int64_t out_ptr) {
    int64_t status = rask_channel_recv((RaskRecver *)(intptr_t)rx, (void *)(intptr_t)out_ptr);
    if (status != RASK_CHAN_OK) {
        rask_panic("recv on closed channel");
    }
    return out_ptr;
}




