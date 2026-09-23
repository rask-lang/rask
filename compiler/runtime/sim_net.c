// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Sockets under sim: a loopback network inside the test's process (sim/B3's
// v1 surface). A listener bound in the test accepts connections from tasks in
// the same test; the bytes travel through in-memory pipes.
//
// A sim socket is an fd number from SIM_FD_BASE up, so it passes through every
// `int64_t fd` the stdlib already carries. runtime.c's socket calls ask
// `rask_sim_net_owns` first and route here.
//
// Two things a real network does and correct code survives are always on
// (sim/F1, C4): a read or write may move fewer bytes than asked, and data
// arrives after a latency. Both are drawn from the fault stream, so they
// replay from the seed and never shift the schedule.
//
// Built only with -DRASK_SIM.

#include "rask_runtime.h"

#ifdef RASK_SIM

#include "sim.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define SIM_FD_BASE (1 << 24)
#define FIRST_EPHEMERAL_PORT 49152
#define MAX_LATENCY_NS 500000   // 0.5 ms

typedef enum { SOCK_LISTENER, SOCK_CONN } SockKind;

typedef struct SimSock {
    SockKind        kind;
    int             refs;       // fds pointing here (dup)
    int             closed;
    int             port;       // listener: bound port; conn: local port
    // Listener
    struct SimSock **queue;
    size_t          q_head, q_len, q_cap;
    // Connection
    struct SimSock *peer;
    int             remote_port;
    char           *in;
    size_t          in_head, in_len, in_cap;
    int             peer_closed;
    int             sick;       // sim/F4: this end's operations may fail
    int             reset;      // cut by an injected Disconnect
    int             peer_reset; // the other end was cut: reads fail, not EOF
    char            label[64];  // how the fault log names this end
} SimSock;

static SimSock **fd_table;
static size_t    fd_count, fd_cap;
static int       next_port = FIRST_EPHEMERAL_PORT;

static void *xcalloc(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) abort();
    return p;
}

static int add_fd(SimSock *s) {
    if (fd_count == fd_cap) {
        fd_cap = fd_cap ? fd_cap * 2 : 16;
        fd_table = (SimSock **)realloc(fd_table, fd_cap * sizeof(SimSock *));
        if (!fd_table) abort();
    }
    fd_table[fd_count] = s;
    s->refs++;
    return SIM_FD_BASE + (int)fd_count++;
}

static SimSock *sock_at(int fd) {
    size_t i = (size_t)(fd - SIM_FD_BASE);
    return i < fd_count ? fd_table[i] : NULL;
}

int rask_sim_net_owns(int64_t fd) {
    return fd >= SIM_FD_BASE && (size_t)(fd - SIM_FD_BASE) < fd_count;
}

static int is_loopback(const char *host) {
    return strcmp(host, "localhost") == 0 || strcmp(host, "127.0.0.1") == 0 ||
           strcmp(host, "0.0.0.0") == 0;
}

static SimSock *listener_on(int port) {
    for (size_t i = 0; i < fd_count; i++) {
        SimSock *s = fd_table[i];
        if (s && s->kind == SOCK_LISTENER && !s->closed && s->port == port) return s;
    }
    return NULL;
}

// Fewer bytes than asked, sometimes: half the time all of it, otherwise a
// seeded prefix of at least one.
static size_t short_len(size_t n) {
    if (n <= 1) return n;
    uint64_t r = rask_sim_fault_draw();
    if (r & 1) return n;
    return 1 + (size_t)((r >> 1) % n);
}

// sim/F4, F5: an injected fault on a sick end. Either the operation fails
// with no effect, or (Disconnect) the connection is cut: this call fails, and
// so does the peer's next read — a reset, not an end of stream, so a reader
// can't mistake the bytes it got so far for the whole message.
static int injected(SimSock *s, const char *op) {
    if (s->reset) {
        errno = ECONNRESET;
        return 1;
    }
    if (!s->sick) return 0;
    int io = rask_sim_fault_enabled(SIM_FAULT_IO_ERROR);
    int cut = rask_sim_fault_enabled(SIM_FAULT_DISCONNECT);
    if (io && cut) {
        if (rask_sim_fault_draw() & 1) io = 0;
        else cut = 0;
    }
    char what[160];
    snprintf(what, sizeof(what), cut ? "%s reset during %s" : "%s failed on %s",
             cut ? s->label : op, cut ? op : s->label);
    if (!rask_sim_draw_failure(what)) return 0;
    if (cut) {
        s->reset = 1;
        if (s->peer) {
            s->peer->peer_reset = 1;
            rask_sim_notify(s->peer);
        }
        errno = ECONNRESET;
    } else {
        errno = EIO;
    }
    return 1;
}

static void draw_sickness(SimSock *s) {
    s->sick = rask_sim_draw_sick(SIM_FAULT_IO_ERROR | SIM_FAULT_DISCONNECT, s->label);
}

static void latency(void) {
    rask_sim_sleep((int64_t)(rask_sim_fault_draw() % MAX_LATENCY_NS));
}

// ─── The calls runtime.c routes here ────────────────────────

int64_t rask_sim_net_listen(const char *host, const char *port_str) {
    if (!is_loopback(host)) {
        rask_sim_unsimulated("`net.tcp_listen(\"%s:%s\")` — sim's network is "
                             "loopback only", host, port_str);
    }
    RASK_SIM_POINT();
    int port = atoi(port_str);
    if (port == 0) {
        while (listener_on(next_port)) next_port++;
        port = next_port++;
    } else if (listener_on(port)) {
        errno = EADDRINUSE;
        return -1;
    }
    SimSock *s = (SimSock *)xcalloc(sizeof(SimSock));
    s->kind = SOCK_LISTENER;
    s->port = port;
    return add_fd(s);
}

int64_t rask_sim_net_connect(const char *host, const char *port_str) {
    if (!is_loopback(host)) {
        rask_sim_unsimulated("`net.tcp_connect(\"%s:%s\")` — sim's network is "
                             "loopback only", host, port_str);
    }
    RASK_SIM_POINT();
    latency();
    SimSock *l = listener_on(atoi(port_str));
    if (!l) {
        errno = ECONNREFUSED;
        return -1;
    }
    SimSock *client = (SimSock *)xcalloc(sizeof(SimSock));
    SimSock *server = (SimSock *)xcalloc(sizeof(SimSock));
    client->kind = server->kind = SOCK_CONN;
    client->peer = server;
    server->peer = client;
    client->port = next_port++;
    client->remote_port = l->port;
    server->port = l->port;
    server->remote_port = client->port;
    snprintf(client->label, sizeof(client->label), "connection to :%d", l->port);
    snprintf(server->label, sizeof(server->label), "connection from :%d", client->port);
    draw_sickness(client);
    draw_sickness(server);

    if (l->q_len == l->q_cap) {
        size_t cap = l->q_cap ? l->q_cap * 2 : 8;
        SimSock **q = (SimSock **)xcalloc(cap * sizeof(SimSock *));
        for (size_t i = 0; i < l->q_len; i++) q[i] = l->queue[(l->q_head + i) % l->q_cap];
        free(l->queue);
        l->queue = q;
        l->q_head = 0;
        l->q_cap = cap;
    }
    l->queue[(l->q_head + l->q_len++) % l->q_cap] = server;
    rask_sim_notify(l);
    return add_fd(client);
}

int64_t rask_sim_net_accept(int64_t fd) {
    SimSock *l = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!l || l->kind != SOCK_LISTENER) {
        errno = EINVAL;
        return -1;
    }
    while (l->q_len == 0 && !l->closed) {
        rask_sim_park(l, "accept");
    }
    if (l->closed) {
        errno = EBADF;
        return -1;
    }
    SimSock *conn = l->queue[l->q_head];
    l->q_head = (l->q_head + 1) % l->q_cap;
    l->q_len--;
    return add_fd(conn);
}

int64_t rask_sim_net_read(int64_t fd, void *buf, size_t n) {
    SimSock *s = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!s || s->kind != SOCK_CONN || s->closed) {
        errno = EBADF;
        return -1;
    }
    if (injected(s, "read")) return -1;
    while (s->in_len == 0 && !s->peer_closed && !s->peer_reset) {
        rask_sim_park(s, "socket read");
    }
    if (s->peer_reset) {
        errno = ECONNRESET;
        return -1;
    }
    if (s->in_len == 0) return 0;   // the peer closed and everything is read
    latency();
    size_t k = short_len(n < s->in_len ? n : s->in_len);
    memcpy(buf, s->in + s->in_head, k);
    s->in_head += k;
    s->in_len -= k;
    if (s->in_len == 0) s->in_head = 0;
    return (int64_t)k;
}

int64_t rask_sim_net_write(int64_t fd, const void *buf, size_t n) {
    SimSock *s = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!s || s->kind != SOCK_CONN || s->closed) {
        errno = EBADF;
        return -1;
    }
    if (injected(s, "write")) return -1;
    SimSock *p = s->peer;
    if (!p || p->closed || p->reset) {
        errno = EPIPE;
        return -1;
    }
    size_t k = short_len(n);
    size_t need = p->in_head + p->in_len + k;
    if (need > p->in_cap) {
        size_t cap = p->in_cap ? p->in_cap : 256;
        while (cap < need) cap *= 2;
        p->in = (char *)realloc(p->in, cap);
        if (!p->in) abort();
        p->in_cap = cap;
    }
    memcpy(p->in + p->in_head + p->in_len, buf, k);
    p->in_len += k;
    rask_sim_notify(p);
    return (int64_t)k;
}

int rask_sim_net_close(int64_t fd) {
    size_t i = (size_t)(fd - SIM_FD_BASE);
    SimSock *s = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!s) {
        errno = EBADF;
        return -1;
    }
    fd_table[i] = NULL;
    if (--s->refs > 0) return 0;
    s->closed = 1;
    rask_sim_notify(s);
    if (s->kind == SOCK_CONN && s->peer) {
        s->peer->peer_closed = 1;
        rask_sim_notify(s->peer);
    }
    return 0;
}

int64_t rask_sim_net_dup(int64_t fd) {
    SimSock *s = sock_at((int)fd);
    if (!s) {
        errno = EBADF;
        return -1;
    }
    return add_fd(s);
}

// "127.0.0.1:<port>", local or remote.
void rask_sim_net_addr(int64_t fd, int remote, char *out, size_t cap) {
    SimSock *s = sock_at((int)fd);
    if (!s) {
        snprintf(out, cap, "unknown");
        return;
    }
    int port = remote && s->kind == SOCK_CONN ? s->remote_port : s->port;
    snprintf(out, cap, "127.0.0.1:%d", port);
}

#endif // RASK_SIM
