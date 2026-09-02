// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rack — nodes with stable addresses and delete-time edge fixup (mem.racks).
//
// A link is the node's address, so the two things the layout has to give are a
// node that never moves and a place to hang the list of edges pointing at it.
// Both come out of one decision: allocate nodes in fixed-size chunks and keep a
// header immediately *before* the payload.
//
//     chunk: [ hdr | T ][ hdr | T ][ hdr | T ] ...
//                   ^
//                   Link<T> points here
//
// The header sitting before the payload is what makes `link.field` an ordinary
// base+offset load — codegen emits the same access it would for a pointer to a
// struct, with no adjustment. Chunks are never reallocated, so a node's address
// is fixed for as long as the node lives (RK1).
//
// Each node's header holds the *incoming* edges — the link words pointing at it.
// `delete` walks them and nulls each one, so the cost follows in-degree rather
// than rack size (RK3), and a dead link never exists to be checked (RK4).
//
// Edges come in two homes, and the split is what keeps a write off the critical
// path. A link declared directly on a node gets its edge record inline, laid out
// immediately *before* the node header:
//
//     chunk: [ e0 | e1 | hdr | T ][ e0 | e1 | hdr | T ] ...
//                          ^
//                          Link<T> points here
//
// Record `k` sits at a fixed offset from the header, so writing that field finds
// its record by arithmetic and unlinks in O(1). Everything else that can hold a
// link — a field of a struct outside the rack, a `Vec` or `Map` element — gets a
// heap record found by scanning. Those live on a separate list so a hub with
// thousands of node-field edges never makes the scanned list longer.

#include "rask_runtime.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define RACK_CHUNK_SLOTS 64

// One incoming edge, and where it lives.
//
// A struct field names its slot exactly, so an overwrite unlinks the old target
// precisely and nothing accumulates however many times the field is written. A
// container names the container and no position — positions shift under
// insertion and rehashing — so it is one record per (container, target) pair and
// the fixup drops every match it finds. The fixup re-checks that a slot still
// holds the node before nulling it, so a record that no longer describes reality
// is recognised rather than trusted.
#define RACK_EDGE_SLOT   0
#define RACK_EDGE_VEC    1
#define RACK_EDGE_MAP    2
#define RACK_EDGE_INLINE 3   // header-resident; holder is a slot in the node payload

// Doubly linked, so a record already in hand unlinks without a search. The old
// `target` field was written and never read — `prev` costs nothing over it.
typedef struct RackEdge {
    int32_t          kind;
    void            *holder;   // void** slot, RaskVec*, or RaskMap*
    struct RackEdge *prev;
    struct RackEdge *next;
} RackEdge;

_Static_assert(sizeof(struct RackEdge) % 16 == 0, "inline edges must keep the payload 16-byte aligned");

// Node header. Lives immediately before the payload; `sizeof` is a multiple of
// 16 so the payload keeps the alignment the chunk allocation gave it.
typedef struct RackNode {
    RaskRack *rack;
    RackEdge *inline_in;   // node-field edges, records inline before this header
    RackEdge *heap_in;     // foreign slots, vec and map elements; found by scanning
    int64_t   slot_index;
} RackNode;

_Static_assert(sizeof(RackNode) % 16 == 0, "node payload must stay 16-byte aligned");

struct RaskRack {
    int64_t   elem_size;
    int64_t   stride;         // sizeof(RackNode) + elem_size, rounded to 16
    int64_t   len;            // live nodes
    int64_t   high_water;     // slots ever handed out
    char    **chunks;
    int64_t   chunk_count;
    int64_t   chunk_cap;
    void    **directory;      // slot index -> payload pointer, NULL when free
    int64_t   dir_cap;
    int64_t  *free_list;
    int64_t   free_len;
    int64_t   free_cap;
    int32_t  *fields;         // (kind, byte offset) pairs — see RASK_RACK_FIELD_*
    int32_t   field_count;
    int32_t   inline_count;   // LINK-kind fields: one inline edge record each
    int32_t   header_bytes;   // inline records + RackNode, i.e. payload's distance from slot start
    RackEdge *edge_pool;      // recycled edge records
    uint32_t  rack_id;
    // Set on a snapshot: which rack it copied, and slot index -> copied payload.
    // `corresponding` is the only reader.
    RaskRack *origin_rack;
    void    **origin_map;
    int64_t   origin_len;
};

static uint32_t g_next_rack_id = 1;

// Fixup counters, for the delete-cost question the model is judged on.
// `RASK_RACK_STATS=1` prints the totals at exit — same three numbers the
// interpreter reports, so the two can be compared directly.
static int64_t g_deletes = 0;
static int64_t g_edges_fixed = 0;
static int64_t g_holders_visited = 0;
static int g_stats_checked = 0;
static int g_stats_on = 0;

static int stats_enabled(void) {
    if (!g_stats_checked) {
        g_stats_checked = 1;
        g_stats_on = getenv("RASK_RACK_STATS") != NULL;
    }
    return g_stats_on;
}

void rask_rack_print_stats(void) {
    if (!stats_enabled()) return;
    fprintf(stderr, "rack stats: deletes=%lld edges_fixed=%lld holders_visited=%lld\n",
            (long long)g_deletes, (long long)g_edges_fixed, (long long)g_holders_visited);
}

static inline RackNode *node_of(void *payload) {
    return (RackNode *)((char *)payload - sizeof(RackNode));
}

static inline int64_t round_up16(int64_t n) {
    return (n + 15) & ~(int64_t)15;
}

// ─── Edge records ──────────────────────────────────────────────────────────

static RackEdge *edge_alloc(RaskRack *r) {
    RackEdge *e = r->edge_pool;
    if (e) {
        r->edge_pool = e->next;
        return e;
    }
    return (RackEdge *)rask_alloc(sizeof(RackEdge));
}

static void edge_release(RaskRack *r, RackEdge *e) {
    e->next = r->edge_pool;
    r->edge_pool = e;
}

// ─── Doubly linked incoming lists ──────────────────────────────────────────
//
// Splice and unsplice. A record already in hand leaves its list without a
// search, which is the whole reason the inline home is worth having.

static inline void list_push(RackEdge **head, RackEdge *e) {
    e->prev = NULL;
    e->next = *head;
    if (*head) (*head)->prev = e;
    *head = e;
}

static inline void list_unlink(RackEdge **head, RackEdge *e) {
    if (e->prev) e->prev->next = e->next; else *head = e->next;
    if (e->next) e->next->prev = e->prev;
    e->prev = NULL;
    e->next = NULL;
}

// Inline record `k` of a node, counting LINK-kind fields in descriptor order.
// Laid out before the header so the arithmetic needs only `k` — the count lives
// on the rack, and reaching *it* would mean already having the node.
static inline RackEdge *inline_edge(RackNode *n, int32_t k) {
    return (RackEdge *)((char *)n - (size_t)(k + 1) * sizeof(RackEdge));
}

// Which LINK field of the node type starts at `offset`, as an ordinal among
// LINK fields only. -1 when the offset names something else — a nested
// aggregate's link, say, which keeps the scanned home.
static int32_t link_ordinal(const RaskRack *r, int64_t offset) {
    int32_t k = 0;
    for (int32_t i = 0; i < r->field_count; i++) {
        if (r->fields[i * 2] != RASK_RACK_FIELD_LINK) continue;
        if ((int64_t)r->fields[i * 2 + 1] == offset) return k;
        k++;
    }
    return -1;
}

static int edge_recorded(void *target, int32_t kind, void *holder) {
    for (RackEdge *e = node_of(target)->heap_in; e; e = e->next) {
        if (e->kind == kind && e->holder == holder) return 1;
    }
    return 0;
}

// Record that a *foreign* holder now points at `target`: a slot outside the
// rack, or a container element. These are found by scanning, so they get their
// own list — a hub with thousands of node-field edges must not lengthen it.
static void edge_register(int32_t kind, void *holder, void *target) {
    RackNode *n = node_of(target);
    RaskRack *r = n->rack;
    if (kind != RACK_EDGE_SLOT && edge_recorded(target, kind, holder)) {
        return;   // one record per (container, target), however many entries match
    }
    RackEdge *e = edge_alloc(r);
    e->kind = kind;
    e->holder = holder;
    list_push(&n->heap_in, e);
}

// Forget the edge `holder` -> `target`. A no-op if it was never recorded, which
// keeps `rask_link_set` safe to call on a slot the rack has never seen.
static void edge_unregister(int32_t kind, void *holder, void *target) {
    RackNode *n = node_of(target);
    RaskRack *r = n->rack;
    for (RackEdge *e = n->heap_in; e; e = e->next) {
        if (e->kind == kind && e->holder == holder) {
            list_unlink(&n->heap_in, e);
            edge_release(r, e);
            return;
        }
    }
}

// `node.field = target` for a link declared directly on a node. The record is
// inline, so both the unlink and the splice are O(1) however many other things
// point at either node — this is the write the old design charged O(in-degree)
// for.
static void node_edge_set(char *payload, int32_t k, void **slot, void *target) {
    RackEdge *e = inline_edge(node_of(payload), k);
    void *old = *slot;
    if (old == target) return;
    if (!rask_link_is_none(old)) list_unlink(&node_of(old)->inline_in, e);
    *slot = target;
    if (!rask_link_is_none(target)) list_push(&node_of(target)->inline_in, e);
}

// ─── Link stores ───────────────────────────────────────────────────────────

// `holder.edge = target` — write the slot and keep the target's incoming list
// in step. This is the one write the model charges for: the edge write touches
// the *target's* header as well as the holder (mem.racks, "The cost, stated").
void rask_link_set(void **slot, void *target) {
    void *old = *slot;
    if (old == target) {
        // Self-assignment must not drop the backlink it would re-add.
        return;
    }
    if (!rask_link_is_none(old)) edge_unregister(RACK_EDGE_SLOT, slot, old);
    *slot = target;
    if (!rask_link_is_none(target)) edge_register(RACK_EDGE_SLOT, slot, target);
}

// `payload.<field at offset> = target`, where `payload` is a node of some rack.
// Codegen emits this instead of `rask_link_set` whenever it can see statically
// that the holder is a node, which is the overwhelmingly common edge write.
//
// Falls back to the scanned path for an offset the descriptor doesn't name as a
// top-level link — a link inside a nested aggregate, which has no inline record.
void rask_link_set_node(void *payload, int64_t offset, void *target) {
    void **slot = (void **)((char *)payload + offset);
    RackNode *n = node_of(payload);
    RaskRack *r = n->rack;
    int32_t k = r ? link_ordinal(r, offset) : -1;
    if (k < 0) {
        rask_link_set(slot, target);
        return;
    }
    node_edge_set((char *)payload, k, slot, target);
}

// A link stored in a `Vec<Link<T>>`. The record names the vector, not the
// position: `push`, `remove` and `sort` all move elements around, and a record
// that named an index would be wrong by the next call.
void rask_link_register_element(RaskVec *v, void *target) {
    if (!v || rask_link_is_none(target)) return;
    edge_register(RACK_EDGE_VEC, v, target);
}

// Same for a `Map<K, Link<T>>` value.
void rask_link_register_entry(RaskMap *m, void *target) {
    if (!m || rask_link_is_none(target)) return;
    edge_register(RACK_EDGE_MAP, m, target);
}

// Record the edges a struct's own fields carry, against the storage it is in.
//
// `insert` does this for a node from the rack's descriptor. Everything else that
// holds links needs it too — `let c = Cursor { at: victim }` puts an edge in a
// field no assignment ever wrote, so nothing had recorded it and `delete` could
// not find the slot to null.
void rask_link_register_struct(void *base, int64_t field_count, const int32_t *fields) {
    if (!base || field_count <= 0 || !fields) return;
    char *p = (char *)base;
    for (int64_t i = 0; i < field_count; i++) {
        int32_t kind = fields[i * 2];
        void **slot = (void **)(p + fields[i * 2 + 1]);
        switch (kind) {
        case RASK_RACK_FIELD_LINK:
            if (!rask_link_is_none(*slot)) edge_register(RACK_EDGE_SLOT, slot, *slot);
            break;
        case RASK_RACK_FIELD_VEC:
            rask_link_register_vec((RaskVec *)*slot);
            break;
        case RASK_RACK_FIELD_MAP:
            rask_link_register_map((RaskMap *)*slot);
            break;
        }
    }
}

// Record every link already sitting in a container.
//
// For a container that arrives whole rather than element by element — the
// classic being `h.list = h.list.filter(…)`, where `filter` builds a fresh
// vector whose entries no push ever recorded.
void rask_link_register_vec(RaskVec *v) {
    if (!v) return;
    int64_t n = rask_vec_len(v);
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(v, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_register(RACK_EDGE_VEC, v, elem);
    }
}

void rask_link_register_map(RaskMap *m) {
    if (!m) return;
    RaskVec *vals = rask_map_values(m);
    int64_t n = rask_vec_len(vals);
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(vals, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_register(RACK_EDGE_MAP, m, elem);
    }
    rask_vec_free(vals);
}

// Forget whatever edge `slot` holds, without writing it. Emitted where a holder
// dies while its target is still live — the rack must not keep the address of
// storage that is going away.
void rask_link_forget(void **slot) {
    void *old = *slot;
    if (!rask_link_is_none(old)) edge_unregister(RACK_EDGE_SLOT, slot, old);
}

// ─── Descriptor walks ──────────────────────────────────────────────────────
//
// The descriptor lists which fields of `T` hold links. Codegen builds one per
// node type from the struct layout; it is what lets `insert` record the edges
// a node literal already carries, and what lets `delete` drop the edges the
// dying node holds.

static void forget_vec_edges(RaskVec *v) {
    if (!v) return;
    int64_t n = rask_vec_len(v);
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(v, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_unregister(RACK_EDGE_VEC, v, elem);
    }
}

static void forget_map_edges(RaskMap *m) {
    if (!m) return;
    RaskVec *vals = rask_map_values(m);
    int64_t n = rask_vec_len(vals);
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(vals, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_unregister(RACK_EDGE_MAP, m, elem);
    }
    rask_vec_free(vals);
}

static inline int32_t field_kind(const RaskRack *r, int32_t i) { return r->fields[i * 2]; }
static inline int32_t field_offset(const RaskRack *r, int32_t i) { return r->fields[i * 2 + 1]; }

// Record the edges a node's own fields carry. `insert` runs this because a node
// literal is built before the node has an identity — `rack.insert(Node { peer:
// head })` names an edge nothing has recorded yet.
static void register_own_edges(const RaskRack *r, char *payload) {
    int32_t k = 0;
    for (int32_t i = 0; i < r->field_count; i++) {
        void **slot = (void **)(payload + field_offset(r, i));
        switch (field_kind(r, i)) {
        case RASK_RACK_FIELD_LINK: {
            RackEdge *e = inline_edge(node_of(payload), k);
            e->kind = RACK_EDGE_INLINE;
            e->holder = slot;
            e->prev = NULL;
            e->next = NULL;
            if (!rask_link_is_none(*slot)) list_push(&node_of(*slot)->inline_in, e);
            k++;
            break;
        }
        case RASK_RACK_FIELD_VEC:
            rask_link_register_vec((RaskVec *)*slot);
            break;
        case RASK_RACK_FIELD_MAP:
            rask_link_register_map((RaskMap *)*slot);
            break;
        }
    }
}

// Drop them again. A dying node's containers die with it, so every target they
// name has to forget them — otherwise a later delete walks a record pointing at
// a freed vector.
static void forget_own_edges(const RaskRack *r, char *payload) {
    int32_t k = 0;
    for (int32_t i = 0; i < r->field_count; i++) {
        void **slot = (void **)(payload + field_offset(r, i));
        switch (field_kind(r, i)) {
        case RASK_RACK_FIELD_LINK: {
            if (!rask_link_is_none(*slot)) {
                list_unlink(&node_of(*slot)->inline_in, inline_edge(node_of(payload), k));
            }
            k++;
            break;
        }
        case RASK_RACK_FIELD_VEC:
            forget_vec_edges((RaskVec *)*slot);
            break;
        case RASK_RACK_FIELD_MAP:
            forget_map_edges((RaskMap *)*slot);
            break;
        }
    }
}

// ─── Storage ───────────────────────────────────────────────────────────────

static int grow_chunks(RaskRack *r) {
    if (r->chunk_count == r->chunk_cap) {
        int64_t cap = r->chunk_cap ? r->chunk_cap * 2 : 4;
        char **next = (char **)rask_realloc(r->chunks, r->chunk_cap * (int64_t)sizeof(char *),
                                            cap * (int64_t)sizeof(char *));
        if (!next) return 0;
        r->chunks = next;
        r->chunk_cap = cap;
    }
    char *chunk = (char *)rask_alloc((size_t)r->stride * RACK_CHUNK_SLOTS);
    if (!chunk) return 0;
    memset(chunk, 0, (size_t)r->stride * RACK_CHUNK_SLOTS);
    r->chunks[r->chunk_count++] = chunk;
    return 1;
}

static int grow_directory(RaskRack *r, int64_t needed) {
    if (needed <= r->dir_cap) return 1;
    int64_t cap = r->dir_cap ? r->dir_cap : 16;
    while (cap < needed) cap *= 2;
    void **next = (void **)rask_realloc(r->directory, r->dir_cap * (int64_t)sizeof(void *),
                                        cap * (int64_t)sizeof(void *));
    if (!next) return 0;
    memset(next + r->dir_cap, 0, (size_t)(cap - r->dir_cap) * sizeof(void *));
    r->directory = next;
    r->dir_cap = cap;
    return 1;
}

static void *payload_at(const RaskRack *r, int64_t index) {
    char *chunk = r->chunks[index / RACK_CHUNK_SLOTS];
    return chunk + (index % RACK_CHUNK_SLOTS) * r->stride + r->header_bytes;
}

// ─── Public API ────────────────────────────────────────────────────────────

// The node type's shape arrives with the first insert, not here: `Rack.new()`
// has no argument to read `T` off, exactly as `Pool.new()` doesn't.
static void rack_describe(RaskRack *r, int64_t elem_size, int32_t field_count,
                          const int32_t *fields) {
    if (r->elem_size != 0) return;
    r->elem_size = elem_size > 0 ? elem_size : 8;
    if (field_count > 0 && fields) {
        int64_t bytes = (int64_t)field_count * 2 * (int64_t)sizeof(int32_t);
        r->fields = (int32_t *)rask_alloc(bytes);
        if (r->fields) {
            memcpy(r->fields, fields, (size_t)bytes);
            r->field_count = field_count;
            for (int32_t i = 0; i < field_count; i++) {
                if (fields[i * 2] == RASK_RACK_FIELD_LINK) r->inline_count++;
            }
        }
    }
    // One inline record per top-level link field, ahead of the header. Both are
    // multiples of 16, so the payload keeps the alignment the chunk gave it.
    r->header_bytes = (int32_t)((size_t)r->inline_count * sizeof(RackEdge) + sizeof(RackNode));
    r->stride = round_up16((int64_t)r->header_bytes + r->elem_size);
}

RaskRack *rask_rack_new(void) {
    RaskRack *r = (RaskRack *)rask_alloc(sizeof(RaskRack));
    if (!r) return NULL;
    memset(r, 0, sizeof(*r));
    r->rack_id = g_next_rack_id++;
    if (stats_enabled()) {
        static int registered = 0;
        if (!registered) { registered = 1; atexit(rask_rack_print_stats); }
    }
    return r;
}

int64_t rask_rack_len(const RaskRack *r) {
    return r ? r->len : 0;
}

int64_t rask_rack_is_empty(const RaskRack *r) {
    return (!r || r->len == 0) ? 1 : 0;
}

int64_t rask_rack_contains(const RaskRack *r, const void *link) {
    if (!r || rask_link_is_none(link)) return 0;
    const RackNode *n = node_of((void *)link);
    if (n->rack != r) return 0;
    if (n->slot_index < 0 || n->slot_index >= r->dir_cap) return 0;
    return r->directory[n->slot_index] == link ? 1 : 0;
}

void *rask_rack_insert(RaskRack *r, const void *value, int64_t elem_size,
                       int64_t field_count, const int32_t *fields) {
    if (!r) return NULL;
    rack_describe(r, elem_size, (int32_t)field_count, fields);

    int64_t index;
    if (r->free_len > 0) {
        index = r->free_list[--r->free_len];
    } else {
        index = r->high_water;
        if (index / RACK_CHUNK_SLOTS >= r->chunk_count) {
            if (!grow_chunks(r)) return NULL;
        }
        if (!grow_directory(r, index + 1)) return NULL;
        r->high_water++;
    }

    char *payload = (char *)payload_at(r, index);
    RackNode *n = node_of(payload);
    n->rack = r;
    n->inline_in = NULL;
    n->heap_in = NULL;
    n->slot_index = index;
    // A reused slot still holds the previous tenant's records. They are already
    // unlinked from wherever they pointed; clear them so the fresh node starts
    // with records that describe it.
    for (int32_t k = 0; k < r->inline_count; k++) {
        RackEdge *e = inline_edge(n, k);
        e->kind = RACK_EDGE_INLINE;
        e->holder = NULL;
        e->prev = NULL;
        e->next = NULL;
    }

    if (value) {
        memcpy(payload, value, (size_t)r->elem_size);
    } else {
        memset(payload, 0, (size_t)r->elem_size);
    }

    r->directory[index] = payload;
    r->len++;

    // The literal may already carry edges — `rack.insert(Node { peer: head })`
    // builds the struct before the node has an identity, so this is where those
    // edges get recorded.
    register_own_edges(r, payload);
    return payload;
}

// Null every edge pointing at `payload` and hand the records back.
static void fix_incoming(RaskRack *r, char *payload) {
    RackNode *n = node_of(payload);

    // Inline records first. Each names a slot in some node's payload, and that
    // node outlives this one or is being deleted alongside it, so the slot is
    // always there to write. The record itself is not ours to free — it lives in
    // its own node's header — so it is only detached.
    RackEdge *ie = n->inline_in;
    n->inline_in = NULL;
    while (ie) {
        RackEdge *next = ie->next;
        if (stats_enabled()) g_holders_visited++;
        void **slot = (void **)ie->holder;
        if (slot && *slot == payload) {
            *slot = RASK_LINK_NONE;
            if (stats_enabled()) g_edges_fixed++;
        }
        ie->prev = NULL;
        ie->next = NULL;
        ie = next;
    }

    RackEdge *e = n->heap_in;
    n->heap_in = NULL;
    while (e) {
        RackEdge *next = e->next;
        if (stats_enabled()) g_holders_visited++;
        int64_t fixed = 0;
        switch (e->kind) {
        case RACK_EDGE_SLOT: {
            // The record names the slot *and* what it should contain. A slot
            // whose holder died without unregistering no longer holds this
            // node, so the mismatch is caught here rather than scribbling on
            // storage that has moved on.
            void **slot = (void **)e->holder;
            if (*slot == payload) { *slot = RASK_LINK_NONE; fixed = 1; }
            break;
        }
        case RACK_EDGE_VEC: {
            // A list of live things loses the entry rather than holding a
            // `none`. Every match goes, not just the first: one record covers
            // the whole vector however many entries point here.
            RaskVec *v = (RaskVec *)e->holder;
            for (int64_t i = rask_vec_len(v) - 1; i >= 0; i--) {
                void *elem;
                memcpy(&elem, rask_vec_get_unchecked(v, i), sizeof(elem));
                if (elem == payload) { rask_vec_remove_at(v, i, NULL); fixed++; }
            }
            break;
        }
        case RACK_EDGE_MAP:
            fixed = rask_map_drop_value_ptr((RaskMap *)e->holder, payload);
            break;
        }
        if (stats_enabled()) g_edges_fixed += fixed;
        edge_release(r, e);
        e = next;
    }
}

static void release_slot(RaskRack *r, char *payload) {
    RackNode *n = node_of(payload);
    int64_t index = n->slot_index;

    // Free the slot first, so nothing the fixup touches can reach the dying
    // node through the rack.
    if (index >= 0 && index < r->dir_cap) r->directory[index] = NULL;
    r->len--;

    forget_own_edges(r, payload);
    fix_incoming(r, payload);

    n->rack = NULL;
    n->slot_index = -1;
    memset(payload, 0, (size_t)r->elem_size);

    if (r->free_len == r->free_cap) {
        int64_t cap = r->free_cap ? r->free_cap * 2 : 16;
        int64_t *next = (int64_t *)rask_realloc(r->free_list, r->free_cap * (int64_t)sizeof(int64_t),
                                                cap * (int64_t)sizeof(int64_t));
        if (!next) return;
        r->free_list = next;
        r->free_cap = cap;
    }
    r->free_list[r->free_len++] = index;
}

void rask_rack_delete(RaskRack *r, void *link) {
    if (!r || rask_link_is_none(link)) return;
    if (!rask_rack_contains(r, link)) return;
    if (stats_enabled()) g_deletes++;
    release_slot(r, (char *)link);
}

void rask_rack_clear(RaskRack *r) {
    if (!r) return;
    // Walk the high-water mark rather than the free list: a `clear` has to null
    // edges as it goes, or a root edge outside the rack survives it (RK3 covers
    // clear too, not just delete).
    for (int64_t i = 0; i < r->high_water; i++) {
        if (i < r->dir_cap && r->directory[i]) {
            if (stats_enabled()) g_deletes++;
            release_slot(r, (char *)r->directory[i]);
        }
    }
}

// Links to every live node, in slot order.
RaskVec *rask_rack_nodes(const RaskRack *r) {
    RaskVec *out = rask_vec_new(8, NULL, 0);
    if (!r) return out;
    for (int64_t i = 0; i < r->high_water; i++) {
        if (i < r->dir_cap && r->directory[i]) {
            void *p = r->directory[i];
            rask_vec_push(out, &p);
        }
    }
    return out;
}

void rask_rack_free(RaskRack *r) {
    if (!r) return;
    for (int64_t i = 0; i < r->high_water; i++) {
        if (i < r->dir_cap && r->directory[i]) {
            RackNode *n = node_of(r->directory[i]);
            RackEdge *e = n->heap_in;   // inline records die with the chunk
            while (e) {
                RackEdge *next = e->next;
                rask_free(e);
                e = next;
            }
        }
    }
    RackEdge *pooled = r->edge_pool;
    while (pooled) {
        RackEdge *next = pooled->next;
        rask_free(pooled);
        pooled = next;
    }
    for (int64_t i = 0; i < r->chunk_count; i++) rask_free(r->chunks[i]);
    rask_free(r->chunks);
    rask_free(r->directory);
    rask_free(r->free_list);
    rask_free(r->fields);
    rask_free(r->origin_map);
    rask_free(r);
}

// ─── Snapshot ──────────────────────────────────────────────────────────────
//
// The delete-time machinery pointed at a different job: delete walks the edges
// *into* one node and nulls them, a snapshot walks the edges *out of* every node
// and re-points them at the copies. Both work because the rack knows its graph.

// The copy of the node `target` names, or NULL when there is nothing to rewrite
// — `none`, or a link into a rack this snapshot isn't copying.
static void *snapshot_target(const RaskRack *r, void *target, void **origin, int64_t n_slots) {
    if (rask_link_is_none(target)) return NULL;
    RackNode *tn = node_of(target);
    if (tn->rack != r) return NULL;               // cross-rack: leave alone
    if (tn->slot_index < 0 || tn->slot_index >= n_slots) return NULL;
    return origin[tn->slot_index];
}

// Map values are reached through the map's own storage, which rack.c can't see.
// map.c walks them and calls back for each.
static void *snapshot_remap_cb(void *value, void *ctx);

typedef struct {
    const RaskRack *rack;
    void          **origin;
    int64_t         n_slots;
} SnapshotCtx;

static void *snapshot_remap_cb(void *value, void *ctx) {
    SnapshotCtx *c = (SnapshotCtx *)ctx;
    void *mapped = snapshot_target(c->rack, value, c->origin, c->n_slots);
    return mapped ? mapped : value;
}

static void rask_map_remap_link_values(RaskMap *m, const RaskRack *r, void **origin, int64_t n_slots) {
    SnapshotCtx ctx = { r, origin, n_slots };
    rask_map_map_values_ptr(m, snapshot_remap_cb, &ctx);
}

RaskRack *rask_rack_snapshot(const RaskRack *r) {
    if (!r) return NULL;
    RaskRack *copy = rask_rack_new();
    if (!copy) return NULL;
    rack_describe(copy, r->elem_size, r->field_count, r->fields);

    // Copy every node first, so every target exists before any edge is rewritten.
    // `origin` maps original slot index -> copied payload.
    int64_t n_slots = r->high_water;
    void **origin = (void **)rask_alloc((size_t)(n_slots > 0 ? n_slots : 1) * sizeof(void *));
    if (!origin) return copy;
    memset(origin, 0, (size_t)(n_slots > 0 ? n_slots : 1) * sizeof(void *));

    for (int64_t i = 0; i < n_slots; i++) {
        if (i >= r->dir_cap || !r->directory[i]) continue;
        // Insert the bytes without registering interior edges: they still name
        // the *original* nodes at this point, and registering them would make
        // the original's deletes reach into the copy.
        char *src = (char *)r->directory[i];
        void *dst = rask_rack_insert(copy, NULL, r->elem_size, r->field_count, r->fields);
        if (!dst) continue;
        memcpy(dst, src, (size_t)r->elem_size);
        origin[i] = dst;
    }

    // Now the edges. A link is rewritten only if it names a node of *this* rack;
    // a cross-rack edge points somewhere this snapshot has no copy of, and
    // rewriting it would invent one.
    for (int64_t i = 0; i < n_slots; i++) {
        char *dst = (char *)origin[i];
        if (!dst) continue;
        for (int32_t f = 0; f < r->field_count; f++) {
            void **slot = (void **)(dst + field_offset(r, f));
            switch (field_kind(r, f)) {
            case RASK_RACK_FIELD_LINK: {
                void *target = *slot;
                if (rask_link_is_none(target)) break;
                // A cross-rack edge is not rewritten — the node it names has no
                // copy here, and inventing one would be wrong. But it still has
                // to be *recorded*, or the rack that owns that node can't null
                // this slot when it deletes it (RK3), and the copy is left
                // holding a freed address.
                void *mapped = snapshot_target(r, target, origin, n_slots);
                *slot = RASK_LINK_NONE;           // nothing recorded yet
                rask_link_set_node(dst, field_offset(r, f), mapped ? mapped : target);
                break;
            }
            case RASK_RACK_FIELD_VEC: {
                // The copy shares the original's vector until this runs: the
                // memcpy above copied the pointer, not the elements.
                RaskVec *fresh = rask_vec_clone((RaskVec *)*slot);
                *slot = fresh;
                int64_t n = rask_vec_len(fresh);
                for (int64_t e = 0; e < n; e++) {
                    void **cell = (void **)rask_vec_get_unchecked(fresh, e);
                    void *mapped = snapshot_target(r, *cell, origin, n_slots);
                    if (mapped) *cell = mapped;
                }
                rask_link_register_vec(fresh);
                break;
            }
            case RASK_RACK_FIELD_MAP: {
                RaskMap *fresh = rask_map_clone((RaskMap *)*slot);
                *slot = fresh;
                rask_map_remap_link_values(fresh, r, origin, n_slots);
                rask_link_register_map(fresh);
                break;
            }
            }
        }
    }

    // Remember where the copy came from, so `corresponding` can translate.
    copy->origin_rack = (RaskRack *)r;
    copy->origin_map = origin;
    copy->origin_len = n_slots;
    return copy;
}

// This snapshot's copy of the node `link` names, or NULL if that node isn't in
// here. A link is an address, so it can't name anything in a different
// allocation — this is the one translation at the boundary.
void *rask_rack_corresponding(const RaskRack *r, const void *link) {
    if (!r || rask_link_is_none(link) || !r->origin_map) return RASK_LINK_NONE;
    const RackNode *n = node_of((void *)link);
    if (n->rack != r->origin_rack) return RASK_LINK_NONE;
    if (n->slot_index < 0 || n->slot_index >= r->origin_len) return RASK_LINK_NONE;
    void *mapped = r->origin_map[n->slot_index];
    return mapped ? mapped : RASK_LINK_NONE;
}
