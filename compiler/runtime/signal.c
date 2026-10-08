// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Signal delivery to channels — `os.signals` (std.os/SG1–SG3).
//
// The channel is made in Rask (`stdlib/os.rk`) and its sender handed here.
// A handler may only do async-signal-safe work, so it sets a pending bit and
// writes a byte to a self-pipe; a reader thread wakes on the pipe and does the
// sending, with locks and all.
//
// The value sent is a `Signal`: a fieldless enum, so one 8-byte word holding
// the variant's index. The table below is in the enum's declaration order.
//
// Each signal has one listener (SG2: the last registration wins). Replacing
// one drops the runtime's handle on the old channel, so once the old receiver
// has nothing else registered, it reads as closed.
//
// SG3 is answered when the next signal arrives rather than at the drop: a
// send to a channel whose receiver is gone puts the default action back and
// raises the signal again, so the process does what it would have done had
// nothing been registered. Nothing in between can tell the difference.

#include "rask_runtime.h"

#include <errno.h>
#include <stdint.h>

#if defined(_WIN32)
#define RASK_NO_SIGNALS 1
#else
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <unistd.h>
#endif

#ifndef RASK_NO_SIGNALS

// `Signal`'s variants, by index.
static const int SIGNAL_NUMBERS[] = { SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2 };
#define SIGNAL_COUNT ((int64_t)(sizeof(SIGNAL_NUMBERS) / sizeof(SIGNAL_NUMBERS[0])))

static pthread_mutex_t listeners_lock = PTHREAD_MUTEX_INITIALIZER;
static RaskSender *listeners[SIGNAL_COUNT];

// Variants raised and not yet delivered, one bit per index.
static atomic_uint_fast64_t pending;
// Write end of the self-pipe; -1 until the reader starts.
static atomic_int pipe_write = -1;

static void on_signal(int signo) {
    int saved = errno;
    for (int64_t i = 0; i < SIGNAL_COUNT; i++) {
        if (SIGNAL_NUMBERS[i] == signo) {
            atomic_fetch_or(&pending, (uint_fast64_t)1 << i);
        }
    }
    int fd = atomic_load(&pipe_write);
    if (fd >= 0) {
        char byte = 1;
        ssize_t r = write(fd, &byte, 1);
        (void)r;
    }
    errno = saved;
}

static void *signal_reader(void *arg) {
    int fd = (int)(intptr_t)arg;
    char buf[64];
    for (;;) {
        ssize_t n = read(fd, buf, sizeof buf);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return NULL;
        uint_fast64_t raised = atomic_exchange(&pending, 0);
        for (int64_t i = 0; i < SIGNAL_COUNT; i++) {
            if (!(raised & ((uint_fast64_t)1 << i))) continue;
            pthread_mutex_lock(&listeners_lock);
            RaskSender *tx = listeners[i];
            // A full channel drops the signal rather than block every other
            // listener behind a slow one.
            int64_t status = tx ? rask_channel_try_send(tx, &i) : RASK_CHAN_OK;
            int orphaned = status == RASK_CHAN_CLOSED;
            if (orphaned) {
                listeners[i] = NULL;
                rask_sender_drop(tx);
                signal(SIGNAL_NUMBERS[i], SIG_DFL);
            }
            pthread_mutex_unlock(&listeners_lock);
            if (orphaned) {
                kill(getpid(), SIGNAL_NUMBERS[i]);
            }
        }
    }
}

// The errno that stopped the reader from starting, or 0.
static int reader_failed;

static void start_reader_once(void) {
    int fds[2];
    if (pipe(fds) != 0) {
        reader_failed = errno;
        return;
    }
    pthread_t thread;
    int err = pthread_create(&thread, NULL, signal_reader, (void *)(intptr_t)fds[0]);
    if (err != 0) {
        close(fds[0]);
        close(fds[1]);
        reader_failed = err;
        return;
    }
    pthread_detach(thread);
    atomic_store(&pipe_write, fds[1]);
}

// Register `tx` for every `Signal` in `list`, taking ownership of `tx`.
// 0, or a negative errno.
int64_t rask_os_signal_forward(int64_t tx_handle, const RaskVec *list) {
    RaskSender *tx = (RaskSender *)(intptr_t)tx_handle;
    static pthread_once_t once = PTHREAD_ONCE_INIT;
    pthread_once(&once, start_reader_once);
    if (reader_failed != 0) {
        rask_sender_drop(tx);
        return -(int64_t)reader_failed;
    }
    int64_t len = rask_vec_len(list);
    pthread_mutex_lock(&listeners_lock);
    for (int64_t k = 0; k < len; k++) {
        int64_t i = *(const int64_t *)rask_vec_get(list, k);
        if (i < 0 || i >= SIGNAL_COUNT) continue;
        RaskSender *old = listeners[i];
        listeners[i] = rask_sender_clone(tx);
        if (old) rask_sender_drop(old);
        struct sigaction sa = {0};
        sa.sa_handler = on_signal;
        sigemptyset(&sa.sa_mask);
        sa.sa_flags = SA_RESTART;
        sigaction(SIGNAL_NUMBERS[i], &sa, NULL);
    }
    pthread_mutex_unlock(&listeners_lock);
    rask_sender_drop(tx);
    return 0;
}

// At exit. The registrations live as long as the process, so these are the
// process's to give back; held, they read as leaks to RASK_LEAK_CHECK.
void rask_signals_release(void) {
    pthread_mutex_lock(&listeners_lock);
    for (int64_t i = 0; i < SIGNAL_COUNT; i++) {
        if (listeners[i]) {
            rask_sender_drop(listeners[i]);
            listeners[i] = NULL;
        }
    }
    pthread_mutex_unlock(&listeners_lock);
}

#else

void rask_signals_release(void) {}

int64_t rask_os_signal_forward(int64_t tx_handle, const RaskVec *list) {
    (void)list;
    rask_sender_drop((RaskSender *)(intptr_t)tx_handle);
    return -(int64_t)ENOSYS;
}

#endif
