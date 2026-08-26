<!-- id: mem.racks -->
<!-- status: decided -->
<!-- summary: Rack<T> holds nodes with stable identity; Link<T> is a reference storable in a field. Delete nulls every edge pointing at the node -->
<!-- depends: memory/boxes.md, memory/linear.md, memory/parameters.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/, compiler/crates/rask-mir/, compiler/crates/rask-codegen/, compiler/runtime/rack.c -->

# Racks and Links

`Rack<T>` is where many instances of one type live with stable identity.
`Link<T>` is a reference to one of them, and unlike every other reference in
Rask it can be stored in a field.

This **replaces `Pool<T>` + `Handle<T>`** (`mem.pools`, now `deprecated`). The
job is the same — many things of one type, individually addressable,
individually removable — and the mechanism is better: a handle is a ticket you
redeem at the container and check for staleness, a link is a pointer you follow.

<!-- test: skip -->
```rask
struct Entity {
    name: string
    health: i32
    damage: i32
    target: Link<Entity>?
}

func combat_round(mutate world: Rack<Entity>) {
    for e in world.nodes() {
        if e.target? as t {
            t.health -= e.damage
        }
    }
}
```

The handle version of that loop needs three things this one doesn't: a lookup
per read (`world[h].damage`), a liveness check per hop (the target may be
deleted), and a manual `world[h].target = none` when the check fails — which is
silent if you forget it.

## The core guarantee

| Rule | Description |
|------|-------------|
| **RK1: Nodes live in a rack** | A `Rack<T>` owns its nodes' lifetime. Nodes have stable addresses: nothing the rack does moves them, so a link stays valid for as long as its node does |
| **RK2: A link is a stored reference** | `Link<T>` may be held in a struct field, which no other Rask reference may be (`mem.borrowing/B1`). That is the whole point of the type |
| **RK3: Delete nulls every incoming edge** | `rack.delete(n)` sets every `Link<T>?` field pointing at `n` to `none` before it returns. A link to a deleted node therefore cannot be observed — there is no stale link to check for, and no cleanup for the program to remember |
| **RK4: Reads are unchecked** | Following a link is a pointer dereference. RK3 is what earns that: the invalid state doesn't exist, so nothing needs testing for it |

RK3 is why the rack exists as an object. Nulling incoming edges requires knowing
who points at whom, and that index needs one home per graph.

## What the compiler enforces

| Rule | Description |
|------|-------------|
| **RK5: Use after delete is an error** | A *local* link can't be reached by the rack, so RK3 can't null it. Using one after its node is deleted is a compile error (E0328), reported as a use after free rather than as a move. A link in a `Link<T>?` *field* is fine — the rack nulls it and the `?` makes you check |
| **RK6: A link may not outlive its rack** | A link whose rack this body declared cannot be returned, assigned into a longer-lived name, or smuggled out inside a struct, tuple or array literal (E0379). A link into a rack the *caller* owns is unrestricted — that rack outlives the call |
| **RK7: Edges are optional for now** | A required edge (`Link<T>` with no `?`) is rejected (E0327): delete has no `none` to set it to, so it needs a declared policy, and construction needs a batch. Both are deferred — see below |
| **RK8: A node write asks permission** | Writing a node's field needs write access to that node, which travels with the link or the rack. See `mem.parameters/PM10` for the parameter modes and the view-versus-writer distinction |
| **RK9: An unnamed delete is declared** | A function that deletes nodes the caller didn't hand it declares `deleting` (`mem.parameters/PM8`, PM9). The call then revokes the caller's links into that rack, because which nodes died isn't knowable from outside |

## Crossing a task boundary

A link is an address, so it means nothing in another task's address space. A
graph crosses by copy:

<!-- test: skip -->
```rask
let mine = world.snapshot()          // deep copy; internal edges re-pointed
if world.corresponding(n)? as here { … }   // translate a link you hold now
```

`snapshot()` copies the nodes and re-points every internal edge at the copies,
so the receiving side has a complete, independent graph. `corresponding()`
translates a link the caller holds *now* into the equivalent node in the copy —
a link sent back the other way means nothing, so if a node needs a name that
survives the crossing, give it an id field.

## The cost, stated

Reads are free; writes are not. Assigning a link into an edge writes the
**target** as well as the holder, because the rack records the incoming edge so
that RK3 can find it later. Measured, an edge write is ~2.6 ns against ~2.9 ns
for a raw pointer store — one extra cache access, in the same tier as a bounds
check rather than the allocations, locks and I/O `CORE_DESIGN` principle 1
reserves visibility for.

Natively, a node lives in a fixed-size chunk the rack never reallocates, with its
header immediately *before* the payload. So a link is the node's address and
nothing else, and `l.health` is the same base+offset load any aggregate field
gets — no lookup, no adjustment. `Link<T>?` is that same word with the null
address for `none` — one word, no tag. That's a different sentinel from
`Handle<T>?`, which uses all-ones, and deliberately so: each niche picks the
value its own domain can't produce. A handle is index+generation, so all-ones is
impossible; a link is an address, so null is. Null also means a rack chunk
arrives with every link already reading as `none`, since chunks are zeroed.

An edge write touches the target's *header*, not any field the target declares.
So a link lent for reading (PM10) stays valid for reading: nothing observable
through it changes.

### Where an edge record lives

The write has to find the record for the slot it is overwriting, so that the old
target forgets it. Searching the old target's list for that record would make an
ordinary field assignment cost O(in-degree) — 4096 things pointing at one node
turned `a.target = b` into 4.6 µs. So records are placed where the write can
reach them by arithmetic.

A link declared directly on a node keeps its record **inline, immediately before
the node header**:

```
chunk: [ e0 | e1 | hdr | T ][ e0 | e1 | hdr | T ] ...
                     ^
                     Link<T> points here
```

Record `k` is a fixed offset from the header, so the write unlinks and re-splices
in constant time whatever the in-degree. Measured across in-degree 1 → 4096, a
retarget stays flat at ~5 ns.

Everything else that can hold a link — a field of a struct outside the rack, a
`Vec` or `Map` element — gets a heap record found by scanning. Those sit on a
separate list, so a hub with thousands of node-field edges never lengthens the
list that gets scanned.

| Rule | Description |
|------|-------------|
| **RK10: Retargeting is constant-time** | Overwriting a link declared on a node costs the same at any in-degree. The record is inline, so nothing is searched for. A link held outside a node still pays a scan of the target's much shorter foreign-holder list |

`RASK_RACK_STATS=1` reports `deletes`/`edges_fixed`/`holders_visited` on both
backends. The numbers are not comparable across them yet — a container edge
counts as one record on the interpreter and one per removed element natively,
so `l1_list_links.rk` reads 1/1 on one side and 6/6 on the other. Read them per
backend until the counters are defined (rask-lang/rask#983).

## Choosing this over the alternatives

Ask in order (`analysis.storage-consolidation`):

1. One value, one owner? → a plain field.
2. Many values? → `Vec` or `Map`, unless…
3. …they reference each other and can be deleted? → **`Rack` + `Link`**.
4. Several accessors share one mutable value? → `Shared<T>`.

Step 3 is plural by construction. A one-node graph has no edges — a single node
can only point at itself — so there is nothing for RK3 to maintain and no reason
to reach for a rack. `Heap<T>` if it needs the heap, `Shared<T>` if it's shared.

## Deferred

Named here so the gaps are on the record rather than discovered:

- **Required edges** (RK7) and, with them, a delete policy. Set-to-`none` is
  complete only while every edge is optional; admitting `Link<T>` makes one of
  cascade or restrict mandatory. Both wait on batch construction.
- **Retiring `mem.pools`.** Native lowering has landed, which was the condition
  (rask-lang/rask#908), but the pool corpus hasn't been converted yet.
- **Slab affordances.** The backing store is a slab already; whether to promise
  contiguous iteration, or offer an explicit `compact()`, wants measurement
  first (`analysis.fourth-option`).
- **A link escaping inside a collection.** RK6 covers returns, assignments and
  aggregate literals; a link pushed into a `Vec` that then escapes is not yet
  caught (rask-lang/rask#941).
- **Pushing a link into a container still scans.** One record per (container,
  target) means `vec.push(link)` checks whether that pair is already recorded,
  and that check walks the target's foreign-holder list. Node-field edges are on
  the other list, so they cost it nothing — measured flat at ~3.5 ns whether one
  node points at the target or 4096 do. What it is still linear in is the number
  of *foreign* holders: 4096 of those make the same push 9.4 µs. Ordinary graphs
  have few, but nothing enforces that, and it is the last place in the model
  where in-degree leaks into a write (rask-lang/rask#981).
- **Structural mutation under concurrency.** Edges may only connect co-owned
  nodes, and the current answer for sharing a graph is a lock around the
  ownership root. Staged structural mutation is designed and unbuilt.

## Why not a read-only link type

`LinkView<T>` was considered and rejected. Read-only is a parameter mode, not a
type (`mem.parameters/PM10`): a plain link parameter is a view, a `mutate` one
writes. A separate type would have to either propagate along every edge or leak
in one hop, and it would put `mut` in a type position, which no other box does.
The mode gets the same guarantee out of machinery the language already has.

## Error messages

```
ERROR [mem.racks/RK5]: use after delete
   |
 3 |  rack.delete(n)
   |  -------------- the node `n` names was deleted here
 4 |  println("{n.name}")
   |            ^ `n` points at freed memory from here on
   |
FIX: move the reads above the delete, or store the link in a `Link<T>?` field
WHY: `delete` frees the node, so every name for it dies at once. A field can
     survive because the rack nulls it; a local can't be reached by the rack.
```

```
ERROR [mem.racks/RK6]: link outlives its rack
   |
 3 |  return n
   |  ^^^^^^^^ `n` lives in `s`, and `s` dies when this function returns
   |
FIX: return the node's data, or take the rack as a parameter so it outlives
     the call
WHY: the nodes live in the rack, so when the rack goes out of scope the node
     goes with it. No delete happened, so RK5 never looks at this.
```
