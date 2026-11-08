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
// heap record. A write to one of those starts from the holder, and the record
// hangs off the target, so the rack keeps a hash index keyed by (holder, target)
// to get from one to the other without walking the target's list. Every edge
// write is then O(1) whatever the in-degree; only `delete` walks a list, because
// it has to visit every edge it nulls (RK3).

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
#define RACK_EDGE_INLINE 3   // header-resident; its slot is link field `ordinal` of its node

// Doubly linked, so a record already in hand unlinks without a search.
//
// An inline record names its *target* rather than its slot. The slot follows
// from where the record sits (`inline_slot`), and the target is what every
// unlink needs: which list the record is on. Read off the slot instead, a
// write that reached the slot some other way — through a `mutate` parameter,
// say (#1807) — left the record on one list while the slot named another
// node, and the next unlink took a different node's list apart.
typedef struct RackEdge {
    int32_t          kind;
    int32_t          ordinal;  // inline: which link field of the node
    union {
        void        *holder;   // heap: void** slot, RaskVec*, or RaskMap*
        void        *target;   // inline: the node whose list it is on, or NULL
    };
    struct RackEdge *prev;
    struct RackEdge *next;
} RackEdge;

_Static_assert(sizeof(struct RackEdge) % 16 == 0, "inline edges must keep the payload 16-byte aligned");

// A foreign holder's record. It carries its target because a container holds
// one record per target it names, so the container alone doesn't pick one out
// of the index. The inline records don't need it — they are reached by
// arithmetic — so they stay 32 bytes.
//
// A container's records are also chained from the container itself
// (`hprev`/`hnext`), so a dying container can drop every record naming it —
// including one for a target whose entries were removed long ago, which the
// container's current elements can no longer lead to (#1796).
typedef struct HeapEdge {
    RackEdge         e;        // first, so a RackEdge* on the list casts back
    void            *target;
    struct HeapEdge *hprev;
    struct HeapEdge *hnext;
} HeapEdge;

// Node header. Lives immediately before the payload; `sizeof` is a multiple of
// 16 so the payload keeps the alignment the chunk allocation gave it.
typedef struct RackNode {
    RaskRack *rack;
    RackEdge *inline_in;   // node-field edges, records inline before this header
    RackEdge *heap_in;     // foreign slots, vec and map elements; reached via edge_index
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
    // What a node payload *owns*: the `offset | (kind << 28)` entries
    // `rask_owned_release` reads, the same encoding a container's elements use.
    // `fields` above is a different question — it lists the fields holding
    // links, so the delete-time fixup can find a node's edges — and a node's
    // string or vector is invisible to it. So a rack of nodes with a
    // `Vec<Link<T>>` or a `name: string` in them leaked one per node.
    int32_t  *owned;
    int32_t   owned_count;
    int32_t   inline_count;   // LINK-kind fields: one inline edge record each
    int32_t   header_bytes;   // inline records + RackNode, i.e. payload's distance from slot start
    int64_t   elem_align;     // the node type's alignment; past 16 the chunks are aligned to it
    HeapEdge *edge_pool;      // recycled heap records, chained through e.next
    // Open-addressed index over every heap record whose target is in this rack,
    // keyed as `index_key` says. Linear probing with backward-shift
    // deletion, so there are no tombstones to sweep. Capacity is a power of two
    // and the load stays at or under half.
    HeapEdge **edge_index;
    int64_t   edge_index_cap;
    int64_t   edge_index_len;
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

// `align` is a power of two.
static inline int64_t round_up(int64_t n, int64_t align) {
    return (n + align - 1) & ~(align - 1);
}

// ─── Edge records ──────────────────────────────────────────────────────────

static HeapEdge *edge_alloc(RaskRack *r) {
    HeapEdge *h = r->edge_pool;
    if (h) {
        r->edge_pool = (HeapEdge *)h->e.next;
        return h;
    }
    return (HeapEdge *)rask_alloc(sizeof(HeapEdge));
}

static void edge_release(RaskRack *r, HeapEdge *h) {
    h->e.next = (RackEdge *)r->edge_pool;
    r->edge_pool = h;
}

// ─── Container chains ──────────────────────────────────────────────────────

// Where a container keeps the head of its own records; NULL for a slot, which
// has exactly one record and no chain.
static void **holder_chain(int32_t kind, void *holder) {
    switch (kind) {
    case RACK_EDGE_VEC: return rask_vec_link_edges((RaskVec *)holder);
    case RACK_EDGE_MAP: return rask_map_link_edges((RaskMap *)holder);
    default:            return NULL;
    }
}

static void chain_push(HeapEdge *h) {
    void **head = holder_chain(h->e.kind, h->e.holder);
    h->hprev = NULL;
    h->hnext = NULL;
    if (!head) return;
    h->hnext = (HeapEdge *)*head;
    if (h->hnext) h->hnext->hprev = h;
    *head = h;
}

static void chain_unlink(HeapEdge *h) {
    void **head = holder_chain(h->e.kind, h->e.holder);
    if (!head) return;
    if (h->hprev) h->hprev->hnext = h->hnext; else *head = h->hnext;
    if (h->hnext) h->hnext->hprev = h->hprev;
    h->hprev = NULL;
    h->hnext = NULL;
}

// ─── Holder index ──────────────────────────────────────────────────────────
//
// The heap records hang off the target, but every write starts from the
// holder: "this slot used to point at X, forget that", "does this vector
// already have a record on X". Without an index both are a walk down X's list,
// which is what made one assignment cost microseconds when X was a hub
// (rask-lang/rask#981).

// What a record is keyed on besides its holder. A slot holds one target at a
// time, so the slot alone identifies its record, and retargeting it within a
// rack leaves the index untouched. A container holds many, so its records are
// keyed on (container, target).
static inline const void *index_key(int32_t kind, const void *target) {
    return kind == RACK_EDGE_SLOT ? NULL : target;
}

static inline uint64_t edge_hash(int32_t kind, const void *holder, const void *target) {
    uint64_t h = (uint64_t)(uintptr_t)holder * 0x9E3779B97F4A7C15ull;
    h ^= (uint64_t)(uintptr_t)index_key(kind, target) * 0xC2B2AE3D27D4EB4Full;
    h ^= (uint64_t)kind;
    return h ^ (h >> 29);
}

static inline int edge_matches(const HeapEdge *h, int32_t kind, const void *holder, const void *target) {
    return h->e.holder == holder && h->e.kind == kind &&
           index_key(kind, h->target) == index_key(kind, target);
}

static HeapEdge *index_find(const RaskRack *r, int32_t kind, void *holder, void *target) {
    if (r->edge_index_cap == 0) return NULL;
    uint64_t mask = (uint64_t)r->edge_index_cap - 1;
    for (uint64_t i = edge_hash(kind, holder, target) & mask;; i = (i + 1) & mask) {
        HeapEdge *h = r->edge_index[i];
        if (!h) return NULL;
        if (edge_matches(h, kind, holder, target)) return h;
    }
}

static void index_place(HeapEdge **tab, uint64_t mask, HeapEdge *h) {
    uint64_t i = edge_hash(h->e.kind, h->e.holder, h->target) & mask;
    while (tab[i]) i = (i + 1) & mask;
    tab[i] = h;
}

static void index_insert(RaskRack *r, HeapEdge *h) {
    if ((r->edge_index_len + 1) * 2 > r->edge_index_cap) {
        int64_t cap = r->edge_index_cap ? r->edge_index_cap * 2 : 64;
        HeapEdge **tab = (HeapEdge **)rask_alloc(cap * (int64_t)sizeof(HeapEdge *));
        if (!tab) rask_panic("rack: out of memory growing the edge index");
        memset(tab, 0, (size_t)cap * sizeof(HeapEdge *));
        for (int64_t i = 0; i < r->edge_index_cap; i++) {
            if (r->edge_index[i]) index_place(tab, (uint64_t)cap - 1, r->edge_index[i]);
        }
        rask_free(r->edge_index);
        r->edge_index = tab;
        r->edge_index_cap = cap;
    }
    index_place(r->edge_index, (uint64_t)r->edge_index_cap - 1, h);
    r->edge_index_len++;
}

// Remove `h`, then pull back any later entry of the same probe run that would
// otherwise be cut off from its home slot by the hole.
static void index_remove(RaskRack *r, HeapEdge *h) {
    uint64_t mask = (uint64_t)r->edge_index_cap - 1;
    uint64_t i = edge_hash(h->e.kind, h->e.holder, h->target) & mask;
    while (r->edge_index[i] != h) i = (i + 1) & mask;
    for (uint64_t j = (i + 1) & mask; r->edge_index[j]; j = (j + 1) & mask) {
        HeapEdge *m = r->edge_index[j];
        uint64_t home = edge_hash(m->e.kind, m->e.holder, m->target) & mask;
        // `m` may move into the hole at `i` only if its home is not in (i, j].
        int movable = (i <= j) ? (home <= i || home > j) : (home <= i && home > j);
        if (movable) {
            r->edge_index[i] = m;
            i = j;
        }
    }
    r->edge_index[i] = NULL;
    r->edge_index_len--;
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

// The node an inline record sits in front of, and the slot it stands for: link
// field `ordinal` of that node's type.
static void **inline_slot(RackEdge *e) {
    RackNode *n = (RackNode *)((char *)e + (size_t)(e->ordinal + 1) * sizeof(RackEdge));
    const RaskRack *r = n->rack;
    if (!r) return NULL;
    int32_t k = 0;
    for (int32_t i = 0; i < r->field_count; i++) {
        if (r->fields[i * 2] != RASK_RACK_FIELD_LINK) continue;
        if (k == e->ordinal) return (void **)((char *)n + sizeof(RackNode) + r->fields[i * 2 + 1]);
        k++;
    }
    return NULL;
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

// Record that a *foreign* holder now points at `target`: a slot outside the
// rack, or a container element. A container keeps one record per target
// however many of its entries name it; a slot keeps one record, full stop.
static void edge_register(int32_t kind, void *holder, void *target) {
    RackNode *n = node_of(target);
    RaskRack *r = n->rack;
    HeapEdge *found = index_find(r, kind, holder, target);
    if (found) {
        if (found->target == target) return;
        // A slot record found by the slot alone but naming another node: the
        // storage was reused without its old edge being forgotten. The slot
        // holds `target` now, so the record moves.
        list_unlink(&node_of(found->target)->heap_in, &found->e);
        found->target = target;
        list_push(&n->heap_in, &found->e);
        return;
    }
    HeapEdge *h = edge_alloc(r);
    if (!h) rask_panic("rack: out of memory recording an edge");
    // Whole-struct: `edge_alloc` hands back either a fresh block or one off the
    // free pool with stale fields still in it.
    *h = (HeapEdge){ .e = { .kind = kind, .holder = holder }, .target = target };
    list_push(&n->heap_in, &h->e);
    index_insert(r, h);
    chain_push(h);
}

// Forget the edge `holder` -> `target`. A no-op if it was never recorded, which
// keeps `rask_link_set` safe to call on a slot the rack has never seen.
static void edge_unregister(int32_t kind, void *holder, void *target) {
    RaskRack *r = node_of(target)->rack;
    // A dead target: `fix_incoming` already released every record on it. A
    // slot can still name it only if nothing ever recorded that slot.
    if (!r) return;
    HeapEdge *h = index_find(r, kind, holder, target);
    if (!h) return;
    index_remove(r, h);
    // The record's own target, not the argument: a slot record is found by the
    // slot alone, and a stale one can name a different node of this rack.
    list_unlink(&node_of(h->target)->heap_in, &h->e);
    chain_unlink(h);
    edge_release(r, h);
}

// A container is being freed: drop every record that names it. Its targets are
// all live — `delete` releases the records on a node as it dies — so each
// still has a rack and a list to leave.
void rask_link_holder_died(void **head) {
    while (head && *head) {
        HeapEdge *h = (HeapEdge *)*head;
        RackNode *n = node_of(h->target);
        index_remove(n->rack, h);
        list_unlink(&n->heap_in, &h->e);
        chain_unlink(h);
        edge_release(n->rack, h);
    }
}

// `node.field = target` for a link declared directly on a node. The record is
// inline, so both the unlink and the splice are O(1) however many other things
// point at either node — this is the write the old design charged O(in-degree)
// for.
static void inline_retarget(RackEdge *e, void *target) {
    if (e->target == target) return;
    if (e->target) list_unlink(&node_of(e->target)->inline_in, e);
    e->target = rask_link_is_none(target) ? NULL : target;
    if (e->target) list_push(&node_of(e->target)->inline_in, e);
}

static void node_edge_set(char *payload, int32_t k, void **slot, void *target) {
    *slot = target;
    inline_retarget(inline_edge(node_of(payload), k), target);
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
    *slot = target;
    if (!rask_link_is_none(old) && !rask_link_is_none(target)) {
        RaskRack *r = node_of(old)->rack;
        HeapEdge *h = r && r == node_of(target)->rack
                          ? index_find(r, RACK_EDGE_SLOT, slot, old) : NULL;
        if (h) {
            // Same rack, and the index is keyed on the slot alone, so the
            // record stays where it is in the index and only changes lists.
            list_unlink(&node_of(h->target)->heap_in, &h->e);
            h->target = target;
            list_push(&node_of(target)->heap_in, &h->e);
            return;
        }
    }
    if (!rask_link_is_none(old)) edge_unregister(RACK_EDGE_SLOT, slot, old);
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

// The links an element of struct type holds (`Vec<Cursor>`), recorded against
// the container the same way a bare link element is: one record per
// (container, target), naming no position.
typedef struct {
    int32_t kind;
    void   *holder;
} RegisterCtx;

static void register_slot_cb(void **slot, void *ctx) {
    RegisterCtx *c = (RegisterCtx *)ctx;
    if (!rask_link_is_none(*slot)) edge_register(c->kind, c->holder, *slot);
}

void rask_link_register_element_fields(RaskVec *v, const void *elem) {
    if (!v || !elem) return;
    RegisterCtx c = { RACK_EDGE_VEC, v };
    rask_elem_links((char *)elem, rask_vec_elem_strs(v), register_slot_cb, &c);
}

void rask_link_register_entry_fields(RaskMap *m, const void *value) {
    if (!m || !value) return;
    RegisterCtx c = { RACK_EDGE_MAP, m };
    rask_elem_links((char *)value, rask_map_val_strs(m), register_slot_cb, &c);
}

static void register_entry_cb(char *value, void *ctx) {
    rask_link_register_entry_fields((RaskMap *)ctx, value);
}

// Null each link slot in an element that names the dying node.
typedef struct {
    void   *dead;
    int64_t fixed;
} NullCtx;

static void null_slot_cb(void **slot, void *ctx) {
    NullCtx *c = (NullCtx *)ctx;
    if (*slot == c->dead) {
        *slot = RASK_LINK_NONE;
        c->fixed++;
    }
}

typedef struct {
    const RaskElemStrs *strs;
    NullCtx            *null;
} NullValuesCtx;

static void null_value_cb(char *value, void *ctx) {
    NullValuesCtx *c = (NullValuesCtx *)ctx;
    rask_elem_links(value, c->strs, null_slot_cb, c->null);
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

// A struct's storage is going away — its frame is returning. The records its
// link slots made name addresses the next frame will reuse. Containers in it
// drop their own records when they are freed.
void rask_link_forget_struct(void *base, int64_t field_count, const int32_t *fields) {
    if (!base || field_count <= 0 || !fields) return;
    char *p = (char *)base;
    for (int64_t i = 0; i < field_count; i++) {
        if (fields[i * 2] == RASK_RACK_FIELD_LINK) rask_link_forget((void **)(p + fields[i * 2 + 1]));
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
    if (rask_vec_holds_struct_links(v)) {
        for (int64_t i = 0; i < n; i++) {
            rask_link_register_element_fields(v, rask_vec_get_unchecked(v, i));
        }
        return;
    }
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(v, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_register(RACK_EDGE_VEC, v, elem);
    }
}

void rask_link_register_map(RaskMap *m) {
    if (!m) return;
    if (rask_map_holds_struct_links(m)) {
        rask_map_each_value(m, register_entry_cb, m);
        return;
    }
    RaskVec *vals = rask_map_values(m);
    int64_t n = rask_vec_len(vals);
    for (int64_t i = 0; i < n; i++) {
        void *elem;
        memcpy(&elem, rask_vec_get_unchecked(vals, i), sizeof(elem));
        if (!rask_link_is_none(elem)) edge_register(RACK_EDGE_MAP, m, elem);
    }
    rask_vec_free(vals);
}

static void forget_vec_edges(RaskVec *v);
static void forget_map_edges(RaskMap *m);

// Forget whatever edge `slot` holds, without writing it. Emitted where a holder
// dies while its target is still live — the rack must not keep the address of
// storage that is going away.
void rask_link_forget(void **slot) {
    void *old = *slot;
    if (!rask_link_is_none(old)) edge_unregister(RACK_EDGE_SLOT, slot, old);
}

// The container half of the same thing: a field holding a `Vec`/`Map` of links
// is overwritten, so the container it held is about to be nobody's. Its records
// name it by address and live on the targets, which have no way to find out
// that the holder is gone — so a later delete of one of those targets walks a
// record for a vector the program has finished with, and reads its length
// through a pointer nothing keeps alive any more (#983).
//
// Emitted at the assignment, against the *old* value, right before the store.
void rask_link_forget_vec(RaskVec *v) {
    forget_vec_edges(v);
}

void rask_link_forget_map(RaskMap *m) {
    forget_map_edges(m);
}

// ─── Descriptor walks ──────────────────────────────────────────────────────
//
// The descriptor lists which fields of `T` hold links. Codegen builds one per
// node type from the struct layout; it is what lets `insert` record the edges
// a node literal already carries, and what lets `delete` drop the edges the
// dying node holds.

// Every record naming the container goes, whatever it still holds: an entry
// removed earlier left its record behind, and only the chain can find it.
static void forget_vec_edges(RaskVec *v) {
    if (v) rask_link_holder_died(rask_vec_link_edges(v));
}

static void forget_map_edges(RaskMap *m) {
    if (m) rask_link_holder_died(rask_map_link_edges(m));
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
            *e = (RackEdge){ .kind = RACK_EDGE_INLINE, .ordinal = k };
            inline_retarget(e, *slot);
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
            inline_retarget(inline_edge(node_of(payload), k), RASK_LINK_NONE);
            // A callee writing this slot through a `mutate` parameter can't
            // know it is inside a node, and records it as foreign storage.
            // That record names the slot, which dies with the node.
            if (!rask_link_is_none(*slot)) edge_unregister(RACK_EDGE_SLOT, slot, *slot);
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
    char *chunk = (char *)rask_alloc_aligned(r->stride * RACK_CHUNK_SLOTS, r->elem_align);
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
static void rack_describe(RaskRack *r, int64_t elem_size, int64_t elem_align,
                          int32_t field_count, const int32_t *fields,
                          int32_t owned_count, const int32_t *owned) {
    if (r->elem_size != 0) return;
    if (elem_size <= 0) rask_panic("rack element size must be positive");
    r->elem_size = elem_size;
    r->elem_align = elem_align;
    if (owned_count > 0 && owned) {
        int64_t bytes = (int64_t)owned_count * (int64_t)sizeof(int32_t);
        r->owned = (int32_t *)rask_alloc(bytes);
        if (r->owned) {
            memcpy(r->owned, owned, (size_t)bytes);
            r->owned_count = owned_count;
        }
    }
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
    // An `@align(64)` node pads the front of each slot up to its alignment,
    // and its chunks come aligned to match: the records still sit right
    // before the header, which sits right before the payload.
    int64_t align = elem_align > 16 ? elem_align : 16;
    r->header_bytes = (int32_t)round_up(
        (int64_t)r->inline_count * (int64_t)sizeof(RackEdge) + (int64_t)sizeof(RackNode), align);
    r->stride = round_up((int64_t)r->header_bytes + r->elem_size, align);
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

// A link's hash, and the one thing about a node that doesn't move: its slot.
//
// A link *is* the node's address (mem.racks/RK2), so hashing its bytes hashes
// whatever the allocator handed out — which moves with however much the program
// allocated before the rack was built. `determinism/D7` doesn't promise a map
// iteration order, so that can't produce a wrong answer; what it breaks is the
// other half of D7, where sim derives the map seed from the sim seed so a
// replay walks the buckets in the same order. The seed replays, the address
// doesn't (#1268).
//
// The slot index replays. Equality is still the address — two links are equal
// when they name the same node — so this only decides which bucket a key lands
// in, and equal keys hash equal because the same node has the same slot for as
// long as it lives.
//
// Links from two different racks can share a slot number and collide. That
// costs a probe, not an answer.
int64_t rask_link_slot(const void *link) {
    if (rask_link_is_none(link)) return -1;
    return node_of((void *)link)->slot_index;
}

int64_t rask_rack_contains(const RaskRack *r, const void *link) {
    if (!r || rask_link_is_none(link)) return 0;
    const RackNode *n = node_of((void *)link);
    if (n->rack != r) return 0;
    if (n->slot_index < 0 || n->slot_index >= r->dir_cap) return 0;
    return r->directory[n->slot_index] == link ? 1 : 0;
}

void *rask_rack_insert(RaskRack *r, const void *value, int64_t elem_size,
                       int64_t field_count, const int32_t *fields,
                       int64_t owned_count, const int32_t *owned,
                       int64_t elem_align) {
    if (!r) return NULL;
    rack_describe(r, elem_size, elem_align, (int32_t)field_count, fields,
                  (int32_t)owned_count, owned);

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
        *inline_edge(n, k) = (RackEdge){ .kind = RACK_EDGE_INLINE, .ordinal = k };
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
        void **slot = inline_slot(ie);
        if (slot && *slot == payload) {
            *slot = RASK_LINK_NONE;
            if (stats_enabled()) g_edges_fixed++;
        }
        ie->target = NULL;
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
            // Elements of struct type keep their place and lose the link
            // inside them, the way a struct field does.
            if (rask_vec_holds_struct_links(v)) {
                NullCtx nc = { payload, 0 };
                for (int64_t i = 0; i < rask_vec_len(v); i++) {
                    rask_elem_links(rask_vec_get_unchecked(v, i), rask_vec_elem_strs(v), null_slot_cb, &nc);
                }
                fixed = nc.fixed;
                break;
            }
            for (int64_t i = rask_vec_len(v) - 1; i >= 0; i--) {
                void *elem;
                memcpy(&elem, rask_vec_get_unchecked(v, i), sizeof(elem));
                if (elem == payload) { rask_vec_remove_at(v, i, NULL); fixed++; }
            }
            break;
        }
        case RACK_EDGE_MAP: {
            RaskMap *m = (RaskMap *)e->holder;
            if (rask_map_holds_struct_links(m)) {
                NullCtx nc = { payload, 0 };
                NullValuesCtx vc = { rask_map_val_strs(m), &nc };
                rask_map_each_value(m, null_value_cb, &vc);
                fixed = nc.fixed;
                break;
            }
            fixed = rask_map_drop_value_ptr(m, payload);
            break;
        }
        }
        if (stats_enabled()) g_edges_fixed += fixed;
        HeapEdge *h = (HeapEdge *)e;
        index_remove(r, h);
        chain_unlink(h);
        edge_release(r, h);
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
    // The node owned its strings and containers; the rack owned the node.
    rask_owned_release_all(payload, r->owned, r->owned_count);
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
    RaskVec *out = rask_vec_new(8, NULL, 0, 0);
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
    // First every node's own outgoing edges leave whatever lists they are on.
    // Those records live in this rack's chunks, so leaving one on a node of
    // another rack let that rack's next delete walk into freed memory (#1797).
    // A node's containers go with it, and take their records along.
    for (int64_t i = 0; i < r->high_water; i++) {
        if (i < r->dir_cap && r->directory[i]) {
            char *payload = (char *)r->directory[i];
            forget_own_edges(r, payload);
            rask_owned_release_all(payload, r->owned, r->owned_count);
        }
    }
    // What is left pointing in comes from outside. A node of another rack is
    // nulled, or its own teardown would unlink a record from this rack's
    // freed headers. A heap record is freed, and leaves its container's chain
    // so the container doesn't walk it later; a struct slot it names is
    // nulled too. This rack's own payloads aren't
    // touched: nothing may read them after this, and #1587 is a place where
    // something still does.
    for (int64_t i = 0; i < r->high_water; i++) {
        if (i < r->dir_cap && r->directory[i]) {
            char *payload = (char *)r->directory[i];
            RackNode *n = node_of(payload);
            for (RackEdge *e = n->inline_in, *next; e; e = next) {
                next = e->next;
                void **slot = inline_slot(e);
                if (slot && *slot == payload) *slot = RASK_LINK_NONE;
                e->target = NULL;
                e->prev = NULL;
                e->next = NULL;
            }
            n->inline_in = NULL;
            for (RackEdge *e = n->heap_in, *next; e; e = next) {
                next = e->next;
                // A struct field outside the rack — a local's, say, whose
                // frame forgets it on the way out. Nulled like any other link
                // into the rack, so that forget finds nothing to follow.
                if (e->kind == RACK_EDGE_SLOT) {
                    void **slot = (void **)e->holder;
                    if (*slot == payload) *slot = RASK_LINK_NONE;
                }
                chain_unlink((HeapEdge *)e);
                rask_free(e);
            }
            n->heap_in = NULL;
        }
    }
    HeapEdge *pooled = r->edge_pool;
    while (pooled) {
        HeapEdge *next = (HeapEdge *)pooled->e.next;
        rask_free(pooled);
        pooled = next;
    }
    rask_free(r->edge_index);
    for (int64_t i = 0; i < r->chunk_count; i++) rask_free(r->chunks[i]);
    rask_free(r->chunks);
    rask_free(r->directory);
    rask_free(r->free_list);
    rask_free(r->fields);
    rask_free(r->owned);
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

static void snapshot_remap_slot_cb(void **slot, void *ctx) {
    void *mapped = snapshot_target(((SnapshotCtx *)ctx)->rack, *slot,
                                   ((SnapshotCtx *)ctx)->origin, ((SnapshotCtx *)ctx)->n_slots);
    if (mapped) *slot = mapped;
}

typedef struct {
    const RaskElemStrs *strs;
    SnapshotCtx        *snap;
} SnapshotValuesCtx;

static void snapshot_remap_value_cb(char *value, void *ctx) {
    SnapshotValuesCtx *c = (SnapshotValuesCtx *)ctx;
    rask_elem_links(value, c->strs, snapshot_remap_slot_cb, c->snap);
}

static void rask_map_remap_link_values(RaskMap *m, const RaskRack *r, void **origin, int64_t n_slots) {
    SnapshotCtx ctx = { r, origin, n_slots };
    if (rask_map_holds_struct_links(m)) {
        SnapshotValuesCtx vc = { rask_map_val_strs(m), &ctx };
        rask_map_each_value(m, snapshot_remap_value_cb, &vc);
        return;
    }
    rask_map_map_values_ptr(m, snapshot_remap_cb, &ctx);
}

RaskRack *rask_rack_snapshot(const RaskRack *r) {
    if (!r) return NULL;
    RaskRack *copy = rask_rack_new();
    if (!copy) return NULL;
    // A rack nothing was ever inserted into has no shape yet, and neither
    // does its copy.
    if (r->elem_size != 0) {
        rack_describe(copy, r->elem_size, r->elem_align, r->field_count, r->fields,
                      r->owned_count, r->owned);
    }

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
        void *dst = rask_rack_insert(copy, NULL, r->elem_size, r->field_count, r->fields,
                                     r->owned_count, r->owned, r->elem_align);
        if (!dst) continue;
        memcpy(dst, src, (size_t)r->elem_size);
        // The copy holds the original's strings and containers now, so it needs
        // its own reference to each — a refcount for a string, a real copy for
        // a nested container. Without it whichever rack died second read memory
        // that was gone, which is the same relation `rask_vec_clone` has to its
        // source.
        rask_owned_retain_all((char *)dst, r->owned, r->owned_count);
        origin[i] = dst;
    }

    // Now the edges. A link is rewritten only if it names a node of *this* rack;
    // a cross-rack edge points somewhere this snapshot has no copy of, and
    // rewriting it would invent one.
    for (int64_t i = 0; i < n_slots; i++) {
        char *dst = (char *)origin[i];
        if (!dst) continue;
        // The node this one was copied from, so a field can be asked whether it
        // still shares the original's container — see the VEC case.
        const char *src_node =
            (i < r->dir_cap) ? (const char *)r->directory[i] : NULL;
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
                // The copy shares the original's vector until *something*
                // clones it: the memcpy above copied the pointer, not the
                // elements. `rask_owned_retain_all` is that something whenever
                // the node's owned descriptor names this field — so cloning
                // again here overwrote the handle it wrote and leaked the
                // vector it had just made. One per node with a
                // `Vec<Link<T>>` field; `p13_rack_snapshot.rk` leaked nine.
                //
                // Asking the original settles it without assuming the two
                // descriptions agree about which fields they cover.
                RaskVec *cur = (RaskVec *)*slot;
                int shares_original =
                    src_node && cur == *(RaskVec **)(src_node + field_offset(r, f));
                RaskVec *fresh = shares_original ? rask_vec_clone(cur) : cur;
                *slot = fresh;
                int64_t n = rask_vec_len(fresh);
                if (rask_vec_holds_struct_links(fresh)) {
                    SnapshotCtx sc = { r, origin, n_slots };
                    for (int64_t e = 0; e < n; e++) {
                        rask_elem_links(rask_vec_get_unchecked(fresh, e), rask_vec_elem_strs(fresh),
                                        snapshot_remap_slot_cb, &sc);
                    }
                } else {
                    for (int64_t e = 0; e < n; e++) {
                        void **cell = (void **)rask_vec_get_unchecked(fresh, e);
                        void *mapped = snapshot_target(r, *cell, origin, n_slots);
                        if (mapped) *cell = mapped;
                    }
                }
                rask_link_register_vec(fresh);
                break;
            }
            case RASK_RACK_FIELD_MAP: {
                // Same as the vector above.
                RaskMap *cur = (RaskMap *)*slot;
                int shares_original =
                    src_node && cur == *(RaskMap **)(src_node + field_offset(r, f));
                RaskMap *fresh = shares_original ? rask_map_clone(cur) : cur;
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
