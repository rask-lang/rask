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
// UDP sockets share the fd table. Each datagram gets its own latency, so one
// can overtake another, and the seed picks some links to drop and repeat
// datagrams for the whole run (sim/N1). All of that is legal for UDP.
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
// How much a connection's reader can have waiting unread before a write to
// it has to wait. Real TCP has a window too, and without one two peers each
// writing a large message before reading never deadlock under sim, though
// they do for real once the windows fill. How large a real window is depends
// on the machine and the path — Linux starts a receive buffer at 128 KB and
// grows it to megabytes, a congested link can hold a few KB — so no one size
// is the true one. The seed draws each end's window, a power of two from
// MIN_WINDOW to MAX_WINDOW, the way it draws short reads and latency: seeds
// with a small window find the deadlock, and the replay line keeps it.
#define MIN_WINDOW_SHIFT 10   // 1 KB
#define WINDOW_SHIFTS    9    // ... to 256 KB

typedef enum { SOCK_LISTENER, SOCK_CONN, SOCK_UDP } SockKind;

typedef struct SimDatagram SimDatagram;

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
    size_t          window;     // most unread bytes this end holds before writers wait
    int             sick;       // sim/F4: this end's operations may fail
    int             reset;      // cut by an injected Disconnect
    int             peer_reset; // the other end was cut: reads fail, not EOF
    char            label[64];  // how the fault log names this end
    // UDP
    SimDatagram    *inbox;      // waiting, in no order; the earliest due is taken
    size_t          inbox_len, inbox_cap;
    void           *sleeper;    // a receiver asleep until the next one is due
} SimSock;

struct SimDatagram {
    char    *data;
    size_t   len;
    int      from_port;
    int64_t  due_ns;   // virtual time it arrives
    uint64_t seq;      // ties on due_ns go in send order
};

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
    static const char *const names[] = {
        "localhost", "127.0.0.1", "0.0.0.0", "::1", "[::1]", "::", "[::]",
    };
    for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); i++) {
        if (strcmp(host, names[i]) == 0) return 1;
    }
    return 0;
}

// A port number, or -1 when `s` isn't one. `atoi` read "http" as 0, which
// means "any port" to listen.
static int parse_port(const char *s) {
    if (!*s) return -1;
    int port = 0;
    for (const char *p = s; *p; p++) {
        if (*p < '0' || *p > '9') return -1;
        port = port * 10 + (*p - '0');
        if (port > 65535) return -1;
    }
    return port;
}

// Cut a connection from this end: this end's calls fail, and so does the
// peer's next read, with a reset rather than an end of stream.
static void reset_conn(SimSock *s) {
    s->reset = 1;
    if (s->peer) {
        s->peer->peer_reset = 1;
        rask_sim_notify(s->peer);
    }
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
        reset_conn(s);
        errno = ECONNRESET;
    } else {
        errno = EIO;
    }
    return 1;
}

static size_t draw_window(void) {
    return (size_t)1 << (MIN_WINDOW_SHIFT + rask_sim_fault_draw() % WINDOW_SHIFTS);
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
    int port = parse_port(port_str);
    if (port < 0) {
        errno = EINVAL;
        return -1;
    }
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
    int port = parse_port(port_str);
    if (port <= 0) {
        errno = port < 0 ? EINVAL : ECONNREFUSED;
        return -1;
    }
    SimSock *l = listener_on(port);
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
    client->window = draw_window();
    server->window = draw_window();
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

// ─── Waiting, and being cancelled while waiting ────────────
//
// A task parked on a simulated socket ends its wait when it is cancelled, the
// same as on a real one (conc.async/CN3). The wake registers at the first real
// wait, and whatever the loop is waiting for is checked before the cancel, so
// data that is already there is still taken.

typedef struct {
    RaskCancelWake wake;
    int            registered;
} SimWait;

static void wake_parked(RaskCancelWake *w) {
    rask_sim_notify(w->a);
}

// Park on `key`, or answer 1 when the task was cancelled.
static int sim_wait(SimWait *sw, const void *key, const char *what) {
    if (!sw->registered) {
        sw->wake = (RaskCancelWake){ .wake = wake_parked, .a = (void *)key };
        sw->registered = 1;
        if (rask_cancel_wait_begin(&sw->wake)) return 1;
    }
    if (rask_cancel_requested()) return 1;
    int released = rask_task_slot_release();
    rask_sim_park(key, what);
    rask_task_slot_retake(released);
    return 0;
}

static int64_t sim_wait_done(SimWait *sw, int64_t result) {
    if (sw->registered) rask_cancel_wait_end();
    return result;
}

static int64_t cancelled(SimWait *sw) {
    errno = ECANCELED;
    return sim_wait_done(sw, -1);
}

int64_t rask_sim_net_accept(int64_t fd) {
    SimSock *l = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!l) {
        errno = EBADF;
        return -1;
    }
    if (l->kind != SOCK_LISTENER) {
        errno = EINVAL;
        return -1;
    }
    SimWait sw = {0};
    while (l->q_len == 0 && !l->closed) {
        if (sim_wait(&sw, l, "accept")) return cancelled(&sw);
    }
    sim_wait_done(&sw, 0);
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
    // The latency goes first. Between the wait below and taking the bytes
    // nothing may yield, or a second reader of the same socket takes them and
    // this one reports an end of stream that never happened.
    latency();
    SimWait sw = {0};
    while (s->in_len == 0 && !s->peer_closed && !s->peer_reset && !s->closed && !s->reset) {
        if (sim_wait(&sw, s, "socket read")) return cancelled(&sw);
    }
    sim_wait_done(&sw, 0);
    if (s->closed) {
        errno = EBADF;   // closed by another task while this one waited
        return -1;
    }
    if (s->peer_reset || s->reset) {
        errno = ECONNRESET;
        return -1;
    }
    if (s->in_len == 0) return 0;   // the peer closed and everything is read
    size_t k = short_len(n < s->in_len ? n : s->in_len);
    memcpy(buf, s->in + s->in_head, k);
    s->in_head += k;
    s->in_len -= k;
    if (s->in_len == 0) s->in_head = 0;
    rask_sim_notify(s);   // a writer waiting for room parks on the reader
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
    // A full window waits for the reader, as a real send buffer does.
    SimWait sw = {0};
    while (p && !p->closed && !p->reset && !s->closed && !s->reset && p->in_len >= p->window) {
        if (sim_wait(&sw, p, "room in the peer's receive window")) return cancelled(&sw);
    }
    sim_wait_done(&sw, 0);
    if (s->closed) {
        errno = EBADF;
        return -1;
    }
    if (s->reset) {
        errno = ECONNRESET;
        return -1;
    }
    if (!p || p->closed || p->reset) {
        errno = EPIPE;
        return -1;
    }
    size_t room = p->window - p->in_len;
    size_t k = short_len(n < room ? n : room);
    if (p->in_head > 0 && p->in_head + p->in_len + k > p->in_cap) {
        memmove(p->in, p->in + p->in_head, p->in_len);
        p->in_head = 0;
    }
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
    if (s->kind == SOCK_LISTENER) {
        // Connections nobody accepted are refused, the way a real listener's
        // backlog is reset when it closes. Left queued, their clients waited
        // for a reply that could never come.
        while (s->q_len > 0) {
            SimSock *conn = s->queue[s->q_head];
            s->q_head = (s->q_head + 1) % s->q_cap;
            s->q_len--;
            conn->closed = 1;
            reset_conn(conn);
        }
    } else if (s->kind == SOCK_UDP) {
        if (s->sleeper) rask_sim_wake(s->sleeper);
        for (size_t k = 0; k < s->inbox_len; k++) free(s->inbox[k].data);
        s->inbox_len = 0;
    } else if (s->peer) {
        // Closing with bytes still unread resets the connection, as TCP does:
        // the peer learns that what it sent wasn't all read.
        if (s->in_len > 0) reset_conn(s);
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

// ─── UDP ────────────────────────────────────────────────────

#define UDP_MAX_PAYLOAD 65507
#define BAD_LINK_MAX_LATENCY_NS 5000000   // 5 ms: plenty of overtaking
#define BAD_LINK_DROP_ONE_IN 8
#define BAD_LINK_REPEAT_ONE_IN 32

// Whether a link drops, repeats and reorders datagrams, decided the first
// time it carries one and kept for the run. A healthy link only delays them,
// in order: real paths mostly do, and reordering everywhere would make every
// run the awful one.
typedef struct {
    int     from, to;
    int     bad;
    int64_t last_due;   // a healthy link delivers nothing before this
} SimLink;

static SimLink *links;
static size_t   link_count, link_cap;

static uint64_t next_seq;

// No sockets and no links, between two claim inputs (sim.h). A socket the
// last input left open is abandoned with its fd: nothing can reach it.
void rask_sim_net_reset(void) {
    fd_count = 0;
    next_port = FIRST_EPHEMERAL_PORT;
    link_count = 0;
    next_seq = 0;
}

static SimSock *udp_on(int port) {
    for (size_t i = 0; i < fd_count; i++) {
        SimSock *s = fd_table[i];
        if (s && s->kind == SOCK_UDP && !s->closed && s->port == port) return s;
    }
    return NULL;
}

static SimLink *link_between(int from, int to) {
    for (size_t i = 0; i < link_count; i++) {
        if (links[i].from == from && links[i].to == to) return &links[i];
    }
    char what[96];
    snprintf(what, sizeof(what), "datagrams :%d → :%d lost, repeated and reordered", from, to);
    int bad = rask_sim_draw_bad_link(what);
    if (link_count == link_cap) {
        link_cap = link_cap ? link_cap * 2 : 8;
        links = (SimLink *)realloc(links, link_cap * sizeof(SimLink));
        if (!links) abort();
    }
    links[link_count] = (SimLink){ .from = from, .to = to, .bad = bad };
    return &links[link_count++];
}

static void deliver(SimSock *to, const void *buf, size_t n, SimLink *link) {
    int64_t span = link->bad ? BAD_LINK_MAX_LATENCY_NS : MAX_LATENCY_NS;
    int64_t due = rask_sim_time_ns() + (int64_t)(rask_sim_fault_draw() % (uint64_t)span);
    if (!link->bad) {
        if (due < link->last_due) due = link->last_due;
        link->last_due = due;
    }
    if (to->inbox_len == to->inbox_cap) {
        to->inbox_cap = to->inbox_cap ? to->inbox_cap * 2 : 8;
        to->inbox = (SimDatagram *)realloc(to->inbox, to->inbox_cap * sizeof(SimDatagram));
        if (!to->inbox) abort();
    }
    SimDatagram *d = &to->inbox[to->inbox_len++];
    d->data = (char *)malloc(n ? n : 1);
    if (!d->data) abort();
    memcpy(d->data, buf, n);
    d->len = n;
    d->from_port = link->from;
    d->due_ns = due;
    d->seq = next_seq++;
    rask_sim_notify(to);
    if (to->sleeper) rask_sim_wake(to->sleeper);
}

// An injected IoError on a sick socket (sim/F4). A datagram socket has no
// connection to cut, so Disconnect doesn't apply.
static int udp_injected(SimSock *s, const char *op) {
    if (!s->sick || !rask_sim_fault_enabled(SIM_FAULT_IO_ERROR)) return 0;
    char what[160];
    snprintf(what, sizeof(what), "%s failed on %s", op, s->label);
    if (!rask_sim_draw_failure(what)) return 0;
    errno = EIO;
    return 1;
}

int64_t rask_sim_net_udp_bind(const char *host, const char *port_str) {
    if (!is_loopback(host)) {
        rask_sim_unsimulated("`net.udp_bind(\"%s:%s\")` — sim's network is "
                             "loopback only", host, port_str);
    }
    RASK_SIM_POINT();
    int port = parse_port(port_str);
    if (port < 0) {
        errno = EINVAL;
        return -1;
    }
    if (port == 0) {
        while (udp_on(next_port)) next_port++;
        port = next_port++;
    } else if (udp_on(port)) {
        errno = EADDRINUSE;
        return -1;
    }
    SimSock *s = (SimSock *)xcalloc(sizeof(SimSock));
    s->kind = SOCK_UDP;
    s->port = port;
    snprintf(s->label, sizeof(s->label), "UDP socket :%d", port);
    s->sick = rask_sim_draw_sick(SIM_FAULT_IO_ERROR, s->label);
    return add_fd(s);
}

int64_t rask_sim_net_send_to(int64_t fd, const void *buf, size_t n, const char *host,
                             const char *port_str) {
    SimSock *s = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!s || s->kind != SOCK_UDP || s->closed) {
        errno = EBADF;
        return -1;
    }
    if (!is_loopback(host)) {
        rask_sim_unsimulated("sending a datagram to %s:%s — sim's network is "
                             "loopback only", host, port_str);
    }
    if (n > UDP_MAX_PAYLOAD) {
        errno = EMSGSIZE;
        return -1;
    }
    if (udp_injected(s, "send")) return -1;
    int port = parse_port(port_str);
    if (port <= 0) {
        errno = EINVAL;
        return -1;
    }
    // Nobody listening: the datagram is gone, and the sender isn't told.
    SimSock *to = udp_on(port);
    if (!to) return 0;
    SimLink *link = link_between(s->port, port);
    if (link->bad && rask_sim_fault_draw() % BAD_LINK_DROP_ONE_IN == 0) return 0;
    deliver(to, buf, n, link);
    if (link->bad && rask_sim_fault_draw() % BAD_LINK_REPEAT_ONE_IN == 0) {
        deliver(to, buf, n, link);
    }
    return 0;
}

static void wake_sim_task(RaskCancelWake *w) {
    rask_sim_wake(w->a);
}

// Sleep until the next datagram is due; 1 when cancelled. A datagram due
// sooner, or a close, ends a thread's sleep early. A green task's sleep runs
// to its deadline, which only makes a datagram later, and it ends on a cancel
// by itself.
static int udp_sleep(SimSock *s, int64_t ns) {
    if (rask_fiber_active()) {
        rask_sim_sleep(ns);
        return rask_cancel_requested();
    }
    RaskCancelWake w = { .wake = wake_sim_task, .a = rask_sim_self() };
    if (rask_cancel_wait_begin(&w)) {
        rask_cancel_wait_end();
        return 1;
    }
    int released = rask_task_slot_release();
    s->sleeper = w.a;
    rask_sim_sleep(ns);
    s->sleeper = NULL;
    rask_task_slot_retake(released);
    rask_cancel_wait_end();
    return rask_cancel_requested();
}

// The datagram due first, or -1 when the inbox is empty.
static long earliest(const SimSock *s) {
    long best = -1;
    for (size_t i = 0; i < s->inbox_len; i++) {
        const SimDatagram *d = &s->inbox[i];
        if (best < 0 || d->due_ns < s->inbox[best].due_ns ||
            (d->due_ns == s->inbox[best].due_ns && d->seq < s->inbox[best].seq)) {
            best = (long)i;
        }
    }
    return best;
}

int64_t rask_sim_net_receive_from(int64_t fd, void *buf, size_t cap, char *from,
                                  size_t from_cap, int wait) {
    SimSock *s = sock_at((int)fd);
    RASK_SIM_POINT();
    if (!s || s->kind != SOCK_UDP || s->closed) {
        errno = EBADF;
        return -1;
    }
    if (udp_injected(s, "receive")) return -1;

    long pick;
    for (;;) {
        if (s->closed) {
            errno = EBADF;
            return -1;
        }
        pick = earliest(s);
        int64_t now = rask_sim_time_ns();
        if (pick >= 0 && s->inbox[pick].due_ns <= now) break;
        if (!wait) return -2;
        if (pick < 0) {
            SimWait sw = {0};
            int stop = sim_wait(&sw, s, "datagram");
            sim_wait_done(&sw, 0);
            if (stop) {
                errno = ECANCELED;
                return -1;
            }
        } else if (udp_sleep(s, s->inbox[pick].due_ns - now)) {
            errno = ECANCELED;
            return -1;
        }
    }

    SimDatagram d = s->inbox[pick];
    s->inbox[pick] = s->inbox[--s->inbox_len];
    size_t k = d.len < cap ? d.len : cap;   // a short buffer truncates, as UDP does
    memcpy(buf, d.data, k);
    free(d.data);
    snprintf(from, from_cap, "127.0.0.1:%d", d.from_port);
    return (int64_t)k;
}

#endif // RASK_SIM
