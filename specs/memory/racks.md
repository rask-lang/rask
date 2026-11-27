<!-- id: mem.racks -->
<!-- status: decided -->
<!-- summary: Rack<T> holds nodes with stable identity; Link<T> is a reference storable in a field. Delete nulls every edge pointing at the node -->
<!-- depends: memory/shared-rack-heap.md, memory/linear.md, memory/parameters.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/, compiler/crates/rask-mir/, compiler/crates/rask-codegen/, compiler/runtime/rack.c -->

# Racks and Links

`Rack<T>` is where many instances of one type live with stable identity.
`Link<T>` is a reference to one of them, and unlike every other reference in
Rask it can be stored in a field.

This **replaced `Pool<T>` + `Handle<T>`**, which are gone (rask-lang/rask#908). The
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
| **RK2: A link is a stored reference** | `Link<T>` may be held in a struct field, which no other Rask reference may be (`mem.borrowing/B1`). That is the whole point of the type. It is a machine word naming a node, so it copies rather than moves — pushing a link into a `Vec` leaves you with the link you started with, and the rack still owns the node. What kills a link is the delete, not the handing over: see RK5. A link reads like its node: `l.field` is the node's field and `l.method()` the node's method, borrowing the node in place. Only identity (`==`, `hash`) and `clone` are the link's own, and a `take self` method can't be called through one, since it would move the node out of the rack (E0910) |
| **RK3: Delete nulls every incoming edge** | `rack.delete(n)` sets every `Link<T>?` field pointing at `n` to `none` before it returns. A link to a deleted node therefore cannot be observed — there is no stale link to check for, and no cleanup for the program to remember |
| **RK4: Reads are unchecked** | Following a link is a pointer dereference. RK3 is what earns that: the invalid state doesn't exist, so nothing needs testing for it |
| **RK15: Every stored link is an edge** | RK3 covers a link wherever it is stored: a field at any depth, a tuple or array element, a `Vec` or `Map` entry, and a container held in a local. A slot is set to `none`; a container entry is removed. A *local link* is a root the compiler guards (RK5); a *local container* of links is storage the rack maintains. A loop over a vector of links keeps its place across those removals: it visits each entry still present when it gets there, once. An entry added during the loop isn't visited, and a loop whose body deletes the node that owns the vector ends there |

RK3 is why the rack exists as an object. Nulling incoming edges requires knowing
who points at whom, and that index needs one home per graph.

RK15 settles a case the two neighbouring rules leave open. A local link is a
root because the rack can't reach it, so the compiler stops you using it after
a delete. A local vector of links can be reached: it registers its entries like
any other holder. So a cloned child list is a list of live children, and
deleting while walking one is ordinary code:

<!-- test: skip -->
```rask
func delete_subtree(deleting scene: Scene, take n: Link<SceneNode>) {
    let kids = n.children.clone()
    for c in kids {
        delete_subtree(deleting scene, c)    // removes c from kids, and from n.children
    }
    scene.nodes.delete(n)
}
```

The loop doesn't skip the child after each removal, and it doesn't run off the
end. It walks the vector through a cursor that a removal moves back.

## What the compiler enforces

| Rule | Description |
|------|-------------|
| **RK5: Use after delete is an error** | A *local* link can't be reached by the rack, so RK3 can't null it. Using one after its node is deleted is a compile error (E0328), reported as a use after free rather than as a move. A link in a `Link<T>?` *field* is fine — the rack nulls it and the `?` makes you check. A closure that deletes through a rack it captured is that delete at each call: a named delete kills the link it names, an unnamed one every local link into the rack. And a `with` or `for mutate` over a node's field (`with a.items[0]`) holds a place in that node, so deleting the node, or a delete that picks its own nodes (`clear`, a `deleting` call), is a compile error until the block ends (E0928); deleting a different node is fine, since nodes never move (RK1) |
| **RK6: A link may not outlive its rack** | A link whose rack this body declared cannot be returned, assigned into a longer-lived name, or smuggled out inside a struct, tuple or array literal (E0379). A link into a rack the *caller* owns is unrestricted — that rack outlives the call |
| **RK7: Edges are optional for now** | A required edge (`Link<T>` with no `?`) is rejected (E0327): delete has no `none` to set it to, so it needs a declared policy, and construction needs a batch. Both are deferred — see below |
| **RK8: A node write asks permission** | Writing a node's field needs write access to that node, which travels with the link or the rack. See `mem.parameters/PM10` for the parameter modes and the view-versus-writer distinction |
| **RK9: An unnamed delete is declared** | A function that deletes nodes the caller didn't hand it declares `deleting` (`mem.parameters/PM8`, PM9). The call then revokes the caller's links into that rack, because which nodes died isn't knowable from outside |
| **RK11: Links compare, they don't order** | `a == b` asks whether two links name the same node, which is meaningful and stable. `<`, `<=`, `>`, `>=`, `compare`, and `sort`/`min`/`max` over a collection of links are rejected (E0406): a link is an address, so the answer would come from the allocator and change with anything the program allocated earlier. Order by something the nodes declare — `links.sort_by_key(|l| l.id)` |
| **RK12: A node has a slot number** | Every node carries the index of the slot it occupies, and the rack keeps a directory from index to node. A link is an address, but a node always knows its position, and the position is stable for the node's life. Deleting frees the slot; a later insert may reuse it, so a slot number names a *position*, not an identity — to name a node across anything, give it an id field |
| **RK13: Position is what crosses a boundary** | Wherever a graph has to leave this allocation, links are written as the slot numbers they point at and resolved back to addresses on arrival. `snapshot()` does this in-process, which is what `corresponding()` translates through; `mem.relocatable` does it against a byte buffer. Both are one pass over nodes plus edges, at the copy, never at a read |
| **RK14: A node is a struct** | `Rack<T>` requires `T` to be a struct (E0326). Edges are `Link<T>?` fields and RK3 is about nulling them, so a node with no fields — an `i32`, a `string`, an enum — has nothing for a rack to do and is a slower `Vec<T>`. Checked where the node type is settled, so `Rack.new()` filled by its inserts is caught too |

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
the all-ones word the pool's handles used to use, and deliberately: a niche
picks the value its own domain can't produce, and for an address that is null.
Null also means a rack chunk arrives with every link already reading as `none`,
since chunks are zeroed.

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
`Vec` or `Map` element — gets a heap record on a separate list, and the rack
keeps a hash index from holder to record. A slot's record is keyed by the slot,
since a slot names one target at a time; a container keeps one record per target
it names, keyed by (container, target). So `c.at = b` and `vec.push(b)` find
their record with one probe instead of walking the target's list. Measured
across foreign in-degree 1 → 4096, a retarget stays at ~7–13 ns and a push at
~5 ns, where both used to reach 4–9 µs.

That leaves `delete` as the only operation that walks a list, and it has to: it
visits every edge it nulls.

| Rule | Description |
|------|-------------|
| **RK10: Retargeting is constant-time** | Overwriting or storing a link costs the same at any in-degree. A link declared on a node reaches its inline record by arithmetic; any other holder reaches its heap record through the rack's index |

### Measuring it

`RASK_RACK_STATS=1` reports three numbers at exit:

| Counter | Meaning |
|---------|---------|
| `deletes` | Nodes deleted, `clear` included |
| `edges_fixed` | Slots actually rewritten: links set to `none`, plus container entries removed — one per entry, not one per container. A splice that repointed an edge before the delete leaves nothing here, because nothing had to change |
| `holders_visited` | Edge records the fixup walked, live or stale |

All three agree across backends, and they are meant to: `edges_fixed` says what
RK3 had to do, `holders_visited` says what it cost to find it. A number that
meant something different on each backend couldn't answer either question.

Agreement is not free — it says the two ends keep the *same* records, which is
a soundness property in disguise. A record for a holder that no longer exists is
a wasted visit at best and a read through dead storage at worst, so both ends
drop a dying node's own outgoing edges and both drop the records of a container
that gets replaced wholesale. Neither used to, in different places, and the
counters are how that was found (rask-lang/rask#983).

## Changing a graph from parallel tasks

Following an edge and reading a node's fields write nothing, so a task that
borrows its frame (`conc.async/B1`) and the code beside it may read one rack
at the same time. What they can't do is change its structure there: an
insert, a delete or a link write touches nodes the other side is reading. So
each side decides, a `Staged<T>` buffer records the decision, and the rack
applies it after the join, on one thread.

<!-- test: skip -->
```rask
func tick(mutate world: Rack<Entity>) {
    let h = ThreadPool.spawn(requires_pool: false) { decide(world, 0, 1000) }
    mut ops = decide(world, 1000, 2000)
    ops.append(h.join()!)
    world.apply(ops)
}

func decide(world: Rack<Entity>, lo: u64, hi: u64) -> Staged<Entity> {
    mut ops = world.staged()
    for e in world.nodes() {
        if e.id >= lo && e.id < hi && e.health <= 0 {
            ops.delete(e.id, e)
        }
    }
    return ops
}
```

| Rule | Description |
|------|-------------|
| **RK16: A task and its frame may share a graph for reading** | A borrowing task and the code before its join may both name one rack, its links and values holding them (relaxing `conc.async/B3`), as long as neither writes through one and neither stores a link anywhere except a `Staged<T>` buffer (E0962). A write is a node field, an insert, a delete, or a `mutate` use of a rack or a link. A store is putting a link in a field, a `Vec`, a `Map` or a struct held in a local, in the code or in anything it calls, or handing one back as the task's value. `for n in rack.nodes()` is a read. A `Local` box still goes on one side |
| **RK17: A buffer records, the rack doesn't change** | `rack.staged()` makes an empty `Staged<T>` for that rack. `ops.delete(key, n)`, `ops.insert(key, node)` and `ops.relink(key, n, to, f)` record a change and do nothing else. Until it applies, a node queued for deletion is an ordinary node: both sides read it as before |
| **RK18: Apply runs in key order** | `rack.apply(ops)` sorts the buffer by `key` (a `u64` the caller picks, such as an entity id) and runs it in that order. Equal keys keep the order they were recorded in, and `ops.append(other)` puts `other`'s after. So the result depends on the keys, not on how the work was split across tasks |
| **RK19: Deletes land last** | Inserts and relinks run first, in key order; then every queued delete runs, in key order. Deleting a node twice is deleting it once. A relink whose node or target is queued for deletion is skipped. An inserted node that links to a node being deleted ends with that link `none`, by the ordinary RK3 fixup. No insert in an apply reuses a slot the same apply frees |
| **RK20: A relink brings its links as arguments** | `ops.relink(key, n, to, f)` calls `f(mutate node, to)` at apply. `f` may not capture a link or a `Local` box (E0963): `n` and `to` are the only links it sees, and both are checked first |
| **RK21: A buffer belongs to its rack as it was** | Applying a buffer to a rack other than the one that made it, or after that rack has deleted anything since, panics. The buffer's links were taken from the rack as it stood, and a delete since may have freed what they name |

The buffer's links aren't edges: no record is kept for them, so nothing nulls
them. That is what lets one side record changes while the other reads, and
RK21 is what keeps it sound.

### Why deletes land last

The design note said buffers apply in task order. That makes the answer depend
on how the work was split, so RK18 orders by a key the program supplies
instead. Running every delete at the end then removes the remaining order
question: whether a relink or an insert ran before or after a delete of the
same node would otherwise decide whether a link survives.

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
- **Slab affordances.** The backing store is a slab already, and RK12 promises
  the slot number and the directory. What's still open is whether to promise
  *contiguous* iteration, or offer an explicit `compact()`; both want measurement
  first (`analysis.fourth-option`).
- **A link escaping inside a collection.** RK6 covers returns, assignments and
  aggregate literals; a link pushed into a `Vec` that then escapes is not yet
  caught (rask-lang/rask#941).
- **Parallel field writes.** Blocks sharing a rack only read it (RK16). Giving
  each block a disjoint chunk of nodes whose scalar fields it may write is the
  next tier, and needs a way to hand out chunks.

## Why not a read-only link type

`LinkView<T>` was considered and rejected. Read-only is a parameter mode, not a
type (`mem.parameters/PM10`): a plain link parameter is a view, a `mutate` one
writes. A separate type would have to either propagate along every edge or leak
in one hop, and it would put `mut` in a type position, which nothing else does.
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
ERROR [mem.racks/RK11]: `<` on `Link<T>` would go by address
   |
 4 |  if a < b { … }
   |     ^^^^^ nodes have no order of their own
   |
FIX: order by a field the nodes declare — `a.id < b.id`
WHY: a link is the address of its node, so `<` answers from wherever the
     allocator put the chunk. `==` is fine: it asks whether two links name
     the same node.
```

```
ERROR [mem.racks/RK16]: a task that shares a rack with the code beside it changes it
   |
 3 |  let h = ThreadPool.spawn { e.health = 0 }
   |                             ^^^^^^^^ this writes through a link or a rack
 4 |  let n = total(world)
   |
FIX: record the change and make it after the join: `ops.delete(key, e)` in
     the task, then `world.apply(ops)`
WHY: both sides read this rack at the same time, so a write on one changes
     what the other is reading.
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
