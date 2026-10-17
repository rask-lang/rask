// SPDX-License-Identifier: (MIT OR Apache-2.0)

// QUIC for Ting, on ngtcp2's core library, with no TLS.
//
// ngtcp2 does the transport: packets, acks, loss recovery, congestion
// control, flow control, streams and datagrams. It hands everything to do
// with cryptography to callbacks. Here those callbacks are deliberately
// empty: the "cipher" copies bytes and appends a zero tag, header protection
// masks with zeros, and the handshake is four tiny messages that swap
// transport parameters. Nothing is secret and nothing is authenticated. That
// is what barebones means here, and real TLS replaces these callbacks later.
//
// What it buys: the whole stack is libc plus ngtcp2, and ngtcp2 reads no clock
// and no randomness of its own. Time comes from clock_gettime and randomness
// from getrandom, both of which sim answers from the seed (sim/B7), so a run
// under `rask test --sim` replays.
//
// Rask moves datagrams between a UdpSocket and this file. Each connection
// carries two kinds of message: reliable ones, length-prefixed on one
// bidirectional stream, and unreliable ones, each a QUIC DATAGRAM frame.

#include <ngtcp2/ngtcp2.h>

#include <arpa/inet.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/socket.h>
#include <time.h>

#define TAG_LEN 16
#define IV_LEN 12
#define SECRET_LEN 32
#define CID_LEN 18
#define MAX_PACKET 1200
#define ADDR_LEN 64

// ─── Byte queues ────────────────────────────────────────────

typedef struct {
    uint8_t *data;
    size_t len, cap;
} Bytes;

static int bytes_put(Bytes *b, const void *p, size_t n) {
    if (b->len + n > b->cap) {
        size_t cap = b->cap ? b->cap : 256;
        while (cap < b->len + n) cap *= 2;
        uint8_t *d = realloc(b->data, cap);
        if (!d) return -1;
        b->data = d;
        b->cap = cap;
    }
    memcpy(b->data + b->len, p, n);
    b->len += n;
    return 0;
}

// Drop the first n bytes.
static void bytes_drop(Bytes *b, size_t n) {
    if (n >= b->len) {
        b->len = 0;
        return;
    }
    memmove(b->data, b->data + n, b->len - n);
    b->len -= n;
}

static void put_u32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

static uint32_t get_u32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

// ─── State ──────────────────────────────────────────────────

// One reliable message, framed. ngtcp2 doesn't copy stream data: it keeps
// pointers into it to resend from, until the bytes are acknowledged. So each
// message sits in an allocation of its own that never moves, and is freed
// only once all of it is acked. (A single growing buffer moved under ngtcp2
// on realloc and resent the wrong bytes after a loss.)
typedef struct Chunk {
    struct Chunk *next;
    uint64_t offset;         // stream offset of data[0]
    size_t len;
    size_t sent;             // bytes handed to ngtcp2
    uint8_t data[];
} Chunk;

typedef struct Tq Tq;

typedef struct {
    Tq *tq;
    ngtcp2_conn *conn;
    struct sockaddr_in remote;
    char peer[ADDR_LEN];
    int64_t stream;          // our stream for reliable messages, -1 until open
    // Reliable, sending: unacknowledged messages oldest first, the first
    // not yet fully handed to ngtcp2, and the stream offset the next one gets.
    Chunk *out_head, *out_tail, *out_next;
    uint64_t out_end;
    // Reliable, receiving: bytes not yet a whole message.
    Bytes in;
    // Unreliable, sending: [len u32][payload] each.
    Bytes dgrams;
    // The fake handshake: what has arrived at each encryption level, and what
    // we sent, which ngtcp2 reads from until it's acknowledged.
    Bytes hs[3];
    uint8_t *hs_sent[4];
    int n_hs_sent;
    int done;                // handshake completed
} Conn;

// Messages ready for Rask: [reliable u8][peer len u8][peer][len u32][payload].
struct Tq {
    int server;
    struct sockaddr_in local;
    Conn **conns;
    size_t nconns;
    Bytes ready;
    size_t ready_read;
    uint64_t rng;
    char error[128];
};

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
}

static void fill_random(Tq *tq, uint8_t *p, size_t n) {
    for (size_t i = 0; i < n; i++) {
        uint64_t z = (tq->rng += 0x9e3779b97f4a7c15ULL);
        z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
        z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
        p[i] = (uint8_t)(z ^ (z >> 31));
    }
}

static int parse_addr(const char *s, struct sockaddr_in *out) {
    char host[ADDR_LEN];
    const char *colon = strrchr(s, ':');
    if (!colon || (size_t)(colon - s) >= sizeof host) return -1;
    memcpy(host, s, (size_t)(colon - s));
    host[colon - s] = '\0';
    memset(out, 0, sizeof *out);
    out->sin_family = AF_INET;
    out->sin_port = htons((uint16_t)atoi(colon + 1));
    if (strcmp(host, "localhost") == 0) strcpy(host, "127.0.0.1");
    return inet_pton(AF_INET, host, &out->sin_addr) == 1 ? 0 : -1;
}

static void format_addr(const struct sockaddr_in *a, char *out, size_t cap) {
    char ip[INET_ADDRSTRLEN];
    inet_ntop(AF_INET, &a->sin_addr, ip, sizeof ip);
    snprintf(out, cap, "%s:%d", ip, ntohs(a->sin_port));
}

static ngtcp2_path path_of(Tq *tq, Conn *c) {
    ngtcp2_path p;
    memset(&p, 0, sizeof p);
    p.local.addr = (ngtcp2_sockaddr *)&tq->local;
    p.local.addrlen = sizeof tq->local;
    p.remote.addr = (ngtcp2_sockaddr *)&c->remote;
    p.remote.addrlen = sizeof c->remote;
    return p;
}

// ─── The null cipher ────────────────────────────────────────

static int dummy;
static const uint8_t zero_iv[IV_LEN];
static const uint8_t zero_secret[SECRET_LEN];

static ngtcp2_crypto_ctx null_ctx(void) {
    ngtcp2_crypto_ctx ctx;
    memset(&ctx, 0, sizeof ctx);
    ctx.aead.native_handle = &dummy;
    ctx.aead.max_overhead = TAG_LEN;
    ctx.md.native_handle = &dummy;
    ctx.hp.native_handle = &dummy;
    ctx.max_encryption = UINT64_MAX;
    ctx.max_decryption_failure = UINT64_MAX;
    return ctx;
}

static const ngtcp2_crypto_aead_ctx null_aead_ctx = { &dummy };
static const ngtcp2_crypto_cipher_ctx null_cipher_ctx = { &dummy };

static int cb_encrypt(uint8_t *dest, const ngtcp2_crypto_aead *aead,
                      const ngtcp2_crypto_aead_ctx *aead_ctx, const uint8_t *plaintext,
                      size_t plaintextlen, const uint8_t *nonce, size_t noncelen,
                      const uint8_t *aad, size_t aadlen) {
    (void)aead; (void)aead_ctx; (void)nonce; (void)noncelen; (void)aad; (void)aadlen;
    memmove(dest, plaintext, plaintextlen);
    memset(dest + plaintextlen, 0, TAG_LEN);
    return 0;
}

static int cb_decrypt(uint8_t *dest, const ngtcp2_crypto_aead *aead,
                      const ngtcp2_crypto_aead_ctx *aead_ctx, const uint8_t *ciphertext,
                      size_t ciphertextlen, const uint8_t *nonce, size_t noncelen,
                      const uint8_t *aad, size_t aadlen) {
    (void)aead; (void)aead_ctx; (void)nonce; (void)noncelen; (void)aad; (void)aadlen;
    if (ciphertextlen < TAG_LEN) return NGTCP2_ERR_DECRYPT;
    memmove(dest, ciphertext, ciphertextlen - TAG_LEN);
    return 0;
}

static int cb_hp_mask(uint8_t *dest, const ngtcp2_crypto_cipher *hp,
                      const ngtcp2_crypto_cipher_ctx *hp_ctx, const uint8_t *sample) {
    (void)hp; (void)hp_ctx; (void)sample;
    memset(dest, 0, NGTCP2_HP_MASKLEN);
    return 0;
}

static int cb_update_key(ngtcp2_conn *conn, uint8_t *rx_secret, uint8_t *tx_secret,
                         ngtcp2_crypto_aead_ctx *rx_aead_ctx, uint8_t *rx_iv,
                         ngtcp2_crypto_aead_ctx *tx_aead_ctx, uint8_t *tx_iv,
                         const uint8_t *current_rx_secret, const uint8_t *current_tx_secret,
                         size_t secretlen, void *user_data) {
    (void)conn; (void)current_rx_secret; (void)current_tx_secret; (void)user_data;
    memset(rx_secret, 0, secretlen);
    memset(tx_secret, 0, secretlen);
    *rx_aead_ctx = null_aead_ctx;
    *tx_aead_ctx = null_aead_ctx;
    memset(rx_iv, 0, IV_LEN);
    memset(tx_iv, 0, IV_LEN);
    return 0;
}

static void cb_delete_aead(ngtcp2_conn *conn, ngtcp2_crypto_aead_ctx *ctx, void *user_data) {
    (void)conn; (void)ctx; (void)user_data;
}

static void cb_delete_cipher(ngtcp2_conn *conn, ngtcp2_crypto_cipher_ctx *ctx, void *user_data) {
    (void)conn; (void)ctx; (void)user_data;
}

static int install_initial(ngtcp2_conn *conn) {
    ngtcp2_crypto_ctx ctx = null_ctx();
    ngtcp2_conn_set_initial_crypto_ctx(conn, &ctx);
    return ngtcp2_conn_install_initial_key(conn, &null_aead_ctx, zero_iv, &null_cipher_ctx,
                                           &null_aead_ctx, zero_iv, &null_cipher_ctx, IV_LEN);
}

static int install_handshake(ngtcp2_conn *conn) {
    ngtcp2_crypto_ctx ctx = null_ctx();
    ngtcp2_conn_set_crypto_ctx(conn, &ctx);
    if (ngtcp2_conn_install_rx_handshake_key(conn, &null_aead_ctx, zero_iv, IV_LEN, &null_cipher_ctx) ||
        ngtcp2_conn_install_tx_handshake_key(conn, &null_aead_ctx, zero_iv, IV_LEN, &null_cipher_ctx)) {
        return NGTCP2_ERR_CALLBACK_FAILURE;
    }
    return 0;
}

static int install_1rtt(ngtcp2_conn *conn) {
    if (ngtcp2_conn_install_rx_key(conn, zero_secret, SECRET_LEN, &null_aead_ctx, zero_iv, IV_LEN,
                                   &null_cipher_ctx) ||
        ngtcp2_conn_install_tx_key(conn, zero_secret, SECRET_LEN, &null_aead_ctx, zero_iv, IV_LEN,
                                   &null_cipher_ctx)) {
        return NGTCP2_ERR_CALLBACK_FAILURE;
    }
    return 0;
}

// ─── The fake handshake ─────────────────────────────────────
//
// client → Initial    "TQCH" + client transport parameters
// server → Initial    "TQSH"
// server → Handshake  "TQEE" + server transport parameters
// client → Handshake  "TQFN"
//
// Each message is [tag 4][len u16][body], and may arrive in pieces.

static int send_hs(Conn *c, ngtcp2_encryption_level level, const char *tag,
                   const uint8_t *body, size_t len) {
    if (c->n_hs_sent == 4) return NGTCP2_ERR_CALLBACK_FAILURE;
    uint8_t *msg = malloc(6 + len);
    if (!msg) return NGTCP2_ERR_CALLBACK_FAILURE;
    memcpy(msg, tag, 4);
    msg[4] = (uint8_t)len;
    msg[5] = (uint8_t)(len >> 8);
    if (len) memcpy(msg + 6, body, len);
    c->hs_sent[c->n_hs_sent++] = msg;   // freed with the connection
    return ngtcp2_conn_submit_crypto_data(c->conn, level, msg, 6 + len);
}

static int send_params(Conn *c, ngtcp2_encryption_level level, const char *tag) {
    uint8_t params[256];
    ngtcp2_ssize n = ngtcp2_conn_encode_local_transport_params(c->conn, params, sizeof params);
    if (n < 0) return NGTCP2_ERR_CALLBACK_FAILURE;
    return send_hs(c, level, tag, params, (size_t)n);
}

static int on_hs_message(Conn *c, ngtcp2_encryption_level level, const uint8_t *tag,
                         const uint8_t *body, size_t len) {
    ngtcp2_conn *conn = c->conn;
    if (c->tq->server && level == NGTCP2_ENCRYPTION_LEVEL_INITIAL && !memcmp(tag, "TQCH", 4)) {
        if (ngtcp2_conn_decode_and_set_remote_transport_params(conn, body, len)) {
            return NGTCP2_ERR_CALLBACK_FAILURE;
        }
        int rv = send_hs(c, NGTCP2_ENCRYPTION_LEVEL_INITIAL, "TQSH", NULL, 0);
        if (!rv) rv = install_handshake(conn);
        if (!rv) rv = send_params(c, NGTCP2_ENCRYPTION_LEVEL_HANDSHAKE, "TQEE");
        if (!rv) rv = install_1rtt(conn);
        return rv;
    }
    if (c->tq->server && level == NGTCP2_ENCRYPTION_LEVEL_HANDSHAKE && !memcmp(tag, "TQFN", 4)) {
        ngtcp2_conn_tls_handshake_completed(conn);
        return 0;
    }
    if (!c->tq->server && level == NGTCP2_ENCRYPTION_LEVEL_INITIAL && !memcmp(tag, "TQSH", 4)) {
        return install_handshake(conn);
    }
    if (!c->tq->server && level == NGTCP2_ENCRYPTION_LEVEL_HANDSHAKE && !memcmp(tag, "TQEE", 4)) {
        if (ngtcp2_conn_decode_and_set_remote_transport_params(conn, body, len)) {
            return NGTCP2_ERR_CALLBACK_FAILURE;
        }
        int rv = install_1rtt(conn);
        if (!rv) rv = send_hs(c, NGTCP2_ENCRYPTION_LEVEL_HANDSHAKE, "TQFN", NULL, 0);
        if (!rv) ngtcp2_conn_tls_handshake_completed(conn);
        return rv;
    }
    return NGTCP2_ERR_CALLBACK_FAILURE;
}

static int cb_recv_crypto_data(ngtcp2_conn *conn, ngtcp2_encryption_level level, uint64_t offset,
                               const uint8_t *data, size_t datalen, void *user_data) {
    (void)conn; (void)offset;
    Conn *c = user_data;
    if (level > NGTCP2_ENCRYPTION_LEVEL_1RTT) return NGTCP2_ERR_CALLBACK_FAILURE;
    Bytes *b = &c->hs[level];
    if (bytes_put(b, data, datalen)) return NGTCP2_ERR_CALLBACK_FAILURE;
    while (b->len >= 6) {
        size_t len = (size_t)b->data[4] | (size_t)b->data[5] << 8;
        if (b->len < 6 + len) break;
        int rv = on_hs_message(c, level, b->data, b->data + 6, len);
        if (rv) return rv;
        bytes_drop(b, 6 + len);
    }
    return 0;
}

static int cb_client_initial(ngtcp2_conn *conn, void *user_data) {
    Conn *c = user_data;
    if (install_initial(conn)) return NGTCP2_ERR_CALLBACK_FAILURE;
    return send_params(c, NGTCP2_ENCRYPTION_LEVEL_INITIAL, "TQCH");
}

static int cb_recv_client_initial(ngtcp2_conn *conn, const ngtcp2_cid *dcid, void *user_data) {
    (void)dcid; (void)user_data;
    return install_initial(conn) ? NGTCP2_ERR_CALLBACK_FAILURE : 0;
}

static int cb_version_negotiation(ngtcp2_conn *conn, uint32_t version,
                                  const ngtcp2_cid *client_dcid, void *user_data) {
    (void)version; (void)client_dcid; (void)user_data;
    return install_initial(conn) ? NGTCP2_ERR_CALLBACK_FAILURE : 0;
}

// Our server never sends Retry, but ngtcp2 requires a client to handle one:
// it means start over with the Initial keys for the new connection ID.
static int cb_recv_retry(ngtcp2_conn *conn, const ngtcp2_pkt_hd *hd, void *user_data) {
    (void)hd; (void)user_data;
    return install_initial(conn) ? NGTCP2_ERR_CALLBACK_FAILURE : 0;
}

static int cb_handshake_completed(ngtcp2_conn *conn, void *user_data) {
    (void)conn;
    ((Conn *)user_data)->done = 1;
    return 0;
}

// ─── Randomness and connection IDs ──────────────────────────

static void cb_rand(uint8_t *dest, size_t destlen, const ngtcp2_rand_ctx *rand_ctx) {
    fill_random(rand_ctx->native_handle, dest, destlen);
}

static int cb_new_cid(ngtcp2_conn *conn, ngtcp2_cid *cid, uint8_t *token, size_t cidlen,
                      void *user_data) {
    (void)conn;
    Conn *c = user_data;
    fill_random(c->tq, cid->data, cidlen);
    cid->datalen = cidlen;
    fill_random(c->tq, token, NGTCP2_STATELESS_RESET_TOKENLEN);
    return 0;
}

static int cb_path_challenge(ngtcp2_conn *conn, uint8_t *data, void *user_data) {
    (void)conn;
    fill_random(((Conn *)user_data)->tq, data, NGTCP2_PATH_CHALLENGE_DATALEN);
    return 0;
}

// ─── Messages ───────────────────────────────────────────────

static int ready_put(Tq *tq, int reliable, const char *peer, const uint8_t *data, size_t len) {
    uint8_t head[2];
    uint8_t lenb[4];
    head[0] = (uint8_t)reliable;
    head[1] = (uint8_t)strlen(peer);
    put_u32(lenb, (uint32_t)len);
    if (bytes_put(&tq->ready, head, 2) || bytes_put(&tq->ready, peer, head[1]) ||
        bytes_put(&tq->ready, lenb, 4) || bytes_put(&tq->ready, data, len)) {
        return -1;
    }
    return 0;
}

static int cb_recv_stream_data(ngtcp2_conn *conn, uint32_t flags, int64_t stream_id,
                               uint64_t offset, const uint8_t *data, size_t datalen,
                               void *user_data, void *stream_user_data) {
    (void)flags; (void)offset; (void)stream_user_data;
    Conn *c = user_data;
    if (bytes_put(&c->in, data, datalen)) return NGTCP2_ERR_CALLBACK_FAILURE;
    while (c->in.len >= 4) {
        size_t len = get_u32(c->in.data);
        if (c->in.len < 4 + len) break;
        if (ready_put(c->tq, 1, c->peer, c->in.data + 4, len)) return NGTCP2_ERR_CALLBACK_FAILURE;
        bytes_drop(&c->in, 4 + len);
    }
    // Keep the window open: the bytes are ours now.
    ngtcp2_conn_extend_max_stream_offset(conn, stream_id, datalen);
    ngtcp2_conn_extend_max_offset(conn, datalen);
    return 0;
}

// Acks arrive in increasing offset order without overlap, so every message
// that ends at or before this one's end is done.
static int cb_acked_stream_data(ngtcp2_conn *conn, int64_t stream_id, uint64_t offset,
                                uint64_t datalen, void *user_data, void *stream_user_data) {
    (void)conn; (void)stream_id; (void)stream_user_data;
    Conn *c = user_data;
    uint64_t end = offset + datalen;
    while (c->out_head && c->out_head->offset + c->out_head->len <= end) {
        Chunk *done = c->out_head;
        c->out_head = done->next;
        if (c->out_tail == done) c->out_tail = NULL;
        if (c->out_next == done) c->out_next = done->next;
        free(done);
    }
    return 0;
}

static int cb_recv_datagram(ngtcp2_conn *conn, uint32_t flags, const uint8_t *data,
                            size_t datalen, void *user_data) {
    (void)conn; (void)flags;
    Conn *c = user_data;
    return ready_put(c->tq, 0, c->peer, data, datalen) ? NGTCP2_ERR_CALLBACK_FAILURE : 0;
}

// ─── Connections ────────────────────────────────────────────

static ngtcp2_callbacks callbacks(int server) {
    ngtcp2_callbacks cb;
    memset(&cb, 0, sizeof cb);
    if (server) {
        cb.recv_client_initial = cb_recv_client_initial;
    } else {
        cb.client_initial = cb_client_initial;
        cb.recv_retry = cb_recv_retry;
    }
    cb.recv_crypto_data = cb_recv_crypto_data;
    cb.handshake_completed = cb_handshake_completed;
    cb.encrypt = cb_encrypt;
    cb.decrypt = cb_decrypt;
    cb.hp_mask = cb_hp_mask;
    cb.recv_stream_data = cb_recv_stream_data;
    cb.acked_stream_data_offset = cb_acked_stream_data;
    cb.recv_datagram = cb_recv_datagram;
    cb.rand = cb_rand;
    cb.get_new_connection_id = cb_new_cid;
    cb.update_key = cb_update_key;
    cb.delete_crypto_aead_ctx = cb_delete_aead;
    cb.delete_crypto_cipher_ctx = cb_delete_cipher;
    cb.get_path_challenge_data = cb_path_challenge;
    cb.version_negotiation = cb_version_negotiation;
    return cb;
}

static void settings_for(Tq *tq, ngtcp2_settings *s, ngtcp2_transport_params *p) {
    ngtcp2_settings_default(s);
    s->initial_ts = now_ns();
    s->rand_ctx.native_handle = tq;
    s->max_tx_udp_payload_size = MAX_PACKET;
    s->no_pmtud = 1;
    ngtcp2_transport_params_default(p);
    p->initial_max_streams_bidi = 1;
    p->initial_max_stream_data_bidi_local = 1 << 20;
    p->initial_max_stream_data_bidi_remote = 1 << 20;
    p->initial_max_data = 4 << 20;
    p->max_idle_timeout = 30 * NGTCP2_SECONDS;
    p->max_datagram_frame_size = MAX_PACKET;
}

static Conn *conn_add(Tq *tq, const struct sockaddr_in *remote) {
    Conn **list = realloc(tq->conns, (tq->nconns + 1) * sizeof *list);
    if (!list) return NULL;
    tq->conns = list;
    Conn *c = calloc(1, sizeof *c);
    if (!c) return NULL;
    c->tq = tq;
    c->remote = *remote;
    c->stream = -1;
    format_addr(remote, c->peer, sizeof c->peer);
    tq->conns[tq->nconns++] = c;
    return c;
}

static Conn *conn_for(Tq *tq, const char *peer) {
    for (size_t i = 0; i < tq->nconns; i++) {
        if (strcmp(tq->conns[i]->peer, peer) == 0) return tq->conns[i];
    }
    return NULL;
}

static void conn_free(Conn *c) {
    if (c->conn) ngtcp2_conn_del(c->conn);
    while (c->out_head) {
        Chunk *next = c->out_head->next;
        free(c->out_head);
        c->out_head = next;
    }
    free(c->in.data);
    free(c->dgrams.data);
    for (int i = 0; i < 3; i++) free(c->hs[i].data);
    for (int i = 0; i < c->n_hs_sent; i++) free(c->hs_sent[i]);
    free(c);
}

// ─── What Rask calls ────────────────────────────────────────

// A client connected to `remote`, or a server when `remote` is NULL. NULL on
// a bad address.
Tq *ting_quic_new(const char *local, const char *remote) {
    Tq *tq = calloc(1, sizeof *tq);
    if (!tq) return NULL;
    if (parse_addr(local, &tq->local)) {
        free(tq);
        return NULL;
    }
    if (getrandom(&tq->rng, sizeof tq->rng, 0) != (ssize_t)sizeof tq->rng) tq->rng = now_ns();
    tq->server = remote == NULL;
    if (tq->server) return tq;

    struct sockaddr_in r;
    if (parse_addr(remote, &r)) {
        free(tq);
        return NULL;
    }
    Conn *c = conn_add(tq, &r);
    if (!c) {
        free(tq);
        return NULL;
    }
    ngtcp2_cid dcid, scid;
    dcid.datalen = scid.datalen = CID_LEN;
    fill_random(tq, dcid.data, CID_LEN);
    fill_random(tq, scid.data, CID_LEN);
    ngtcp2_settings s;
    ngtcp2_transport_params p;
    settings_for(tq, &s, &p);
    ngtcp2_callbacks cb = callbacks(0);
    ngtcp2_path path = path_of(tq, c);
    if (ngtcp2_conn_client_new(&c->conn, &dcid, &scid, &path, NGTCP2_PROTO_VER_V1, &cb, &s, &p,
                               NULL, c)) {
        tq->nconns = 0;
        conn_free(c);
        free(tq->conns);
        free(tq);
        return NULL;
    }
    return tq;
}

void ting_quic_free(Tq *tq) {
    if (!tq) return;
    for (size_t i = 0; i < tq->nconns; i++) conn_free(tq->conns[i]);
    free(tq->conns);
    free(tq->ready.data);
    free(tq);
}

// A datagram that arrived from `from`. 0, or -1 when it can't be read (a
// stray packet is dropped, as QUIC drops them; the connection carries on).
int64_t ting_quic_receive(Tq *tq, const uint8_t *pkt, int64_t len, const char *from) {
    struct sockaddr_in r;
    if (len <= 0 || parse_addr(from, &r)) return -1;
    char peer[ADDR_LEN];
    format_addr(&r, peer, sizeof peer);
    Conn *c = conn_for(tq, peer);
    if (!c) {
        if (!tq->server) return -1;
        ngtcp2_pkt_hd hd;
        if (ngtcp2_accept(&hd, pkt, (size_t)len) != 0) return -1;
        c = conn_add(tq, &r);
        if (!c) return -1;
        ngtcp2_cid scid;
        scid.datalen = CID_LEN;
        fill_random(tq, scid.data, CID_LEN);
        ngtcp2_settings s;
        ngtcp2_transport_params p;
        settings_for(tq, &s, &p);
        p.original_dcid = hd.dcid;
        p.original_dcid_present = 1;
        ngtcp2_callbacks cb = callbacks(1);
        ngtcp2_path path = path_of(tq, c);
        if (ngtcp2_conn_server_new(&c->conn, &hd.scid, &scid, &path, hd.version, &cb, &s, &p, NULL,
                                   c)) {
            tq->nconns--;
            conn_free(c);
            return -1;
        }
    }
    ngtcp2_path path = path_of(tq, c);
    int rv = ngtcp2_conn_read_pkt(c->conn, &path, NULL, pkt, (size_t)len, now_ns());
    if (rv) {
        snprintf(tq->error, sizeof tq->error, "%s: %s", peer, ngtcp2_strerror(rv));
        return -1;
    }
    return 0;
}

// Queue a message for `peer`. 0, or -1 when there's no connection to it.
int64_t ting_quic_send(Tq *tq, const char *peer, const uint8_t *data, int64_t len,
                       int64_t reliable) {
    Conn *c = conn_for(tq, peer);
    if (!c || len < 0) return -1;
    uint8_t lenb[4];
    put_u32(lenb, (uint32_t)len);
    if (!reliable) {
        if (bytes_put(&c->dgrams, lenb, 4) || bytes_put(&c->dgrams, data, (size_t)len)) return -1;
        return 0;
    }
    Chunk *m = malloc(sizeof *m + 4 + (size_t)len);
    if (!m) return -1;
    m->next = NULL;
    m->offset = c->out_end;
    m->len = 4 + (size_t)len;
    m->sent = 0;
    memcpy(m->data, lenb, 4);
    memcpy(m->data + 4, data, (size_t)len);
    c->out_end += m->len;
    if (c->out_tail) c->out_tail->next = m;
    else c->out_head = m;
    c->out_tail = m;
    if (!c->out_next) c->out_next = m;
    return 0;
}

// Write one packet for any connection into `out`, and its destination into
// `to`. The packet's length, or 0 when nothing wants sending now. Timers
// that are due fire first.
int64_t ting_quic_next_packet(Tq *tq, uint8_t *out, int64_t cap, char *to, int64_t to_cap) {
    uint64_t ts = now_ns();
    for (size_t i = 0; i < tq->nconns; i++) {
        Conn *c = tq->conns[i];
        if (!c->conn) continue;
        if (ngtcp2_conn_get_expiry(c->conn) <= ts) {
            ngtcp2_conn_handle_expiry(c->conn, ts);
        }
        ngtcp2_path path = path_of(tq, c);
        ngtcp2_ssize n = 0;

        // One unreliable message per packet, while any are queued.
        if (c->done && c->dgrams.len >= 4) {
            size_t len = get_u32(c->dgrams.data);
            int accepted = 0;
            n = ngtcp2_conn_write_datagram(c->conn, &path, NULL, out, (size_t)cap, &accepted, 0, 0,
                                           c->dgrams.data + 4, len, ts);
            // Too big for a packet, or the peer takes none: it's dropped,
            // which an unreliable message is allowed to be.
            if (accepted || n == NGTCP2_ERR_INVALID_ARGUMENT) bytes_drop(&c->dgrams, 4 + len);
            if (n < 0) n = 0;
        }

        // Then the reliable stream. Each side opens its own once the
        // handshake is done and sends on it, and reads whatever the other
        // side opened, so neither waits on the other to speak first.
        if (n == 0) {
            if (c->done && c->stream < 0) {
                ngtcp2_conn_open_bidi_stream(c->conn, &c->stream, NULL);
            }
            // Up to 16 unsent messages, as one vector each.
            ngtcp2_vec v[16];
            size_t nv = 0;
            int64_t stream = -1;
            if (c->stream >= 0) {
                for (Chunk *m = c->out_next; m && nv < 16; m = m->next) {
                    v[nv].base = m->data + m->sent;
                    v[nv].len = m->len - m->sent;
                    nv++;
                }
                if (nv) stream = c->stream;
            }
            ngtcp2_ssize taken = -1;
            n = ngtcp2_conn_writev_stream(c->conn, &path, NULL, out, (size_t)cap, &taken, 0, stream,
                                          nv ? v : NULL, nv, ts);
            if (n == NGTCP2_ERR_STREAM_DATA_BLOCKED) {
                // Flow control says wait: still send acks and the rest.
                taken = -1;
                n = ngtcp2_conn_writev_stream(c->conn, &path, NULL, out, (size_t)cap, NULL, 0, -1,
                                              NULL, 0, ts);
            }
            // Mark what ngtcp2 took as sent; it resends from these bytes
            // itself if they're lost.
            for (size_t left = taken > 0 ? (size_t)taken : 0; left > 0 && c->out_next;) {
                Chunk *m = c->out_next;
                size_t k = m->len - m->sent < left ? m->len - m->sent : left;
                m->sent += k;
                left -= k;
                if (m->sent == m->len) c->out_next = m->next;
            }
            if (n < 0) {
                snprintf(tq->error, sizeof tq->error, "%s: %s", c->peer, ngtcp2_strerror((int)n));
                n = 0;
            }
        }
        if (n > 0) {
            snprintf(to, (size_t)to_cap, "%s", c->peer);
            return n;
        }
    }
    return 0;
}

// The next delivered message: copies its bytes into `out` and its sender into
// `from`, and says whether it was reliable. Its length, or -1 when none is
// waiting (or `out` is too small, and it stays queued).
int64_t ting_quic_next_message(Tq *tq, uint8_t *out, int64_t cap, char *from, int64_t from_cap,
                               int64_t *reliable) {
    Bytes *b = &tq->ready;
    if (tq->ready_read >= b->len) {
        b->len = 0;
        tq->ready_read = 0;
        return -1;
    }
    const uint8_t *p = b->data + tq->ready_read;
    size_t peer_len = p[1];
    size_t len = get_u32(p + 2 + peer_len);
    if ((int64_t)len > cap) return -1;
    *reliable = p[0];
    snprintf(from, (size_t)from_cap, "%.*s", (int)peer_len, (const char *)p + 2);
    memcpy(out, p + 6 + peer_len, len);
    tq->ready_read += 6 + peer_len + len;
    return (int64_t)len;
}

// Handshake done with `peer`: 1, 0 not yet, -1 no such connection.
int64_t ting_quic_connected(Tq *tq, const char *peer) {
    Conn *c = conn_for(tq, peer);
    return c ? c->done : -1;
}

// Reliable messages to `peer` not yet acknowledged.
int64_t ting_quic_unacked(Tq *tq, const char *peer) {
    Conn *c = conn_for(tq, peer);
    if (!c) return -1;
    int64_t n = 0;
    for (Chunk *m = c->out_head; m; m = m->next) n++;
    return n;
}

// What the last failure said, for an error message.
const char *ting_quic_error(Tq *tq) {
    return tq->error;
}
