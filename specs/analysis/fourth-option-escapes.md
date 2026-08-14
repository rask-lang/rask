<!-- id: analysis.fourth-option-escapes -->
<!-- status: exploration -->
<!-- summary: The escape problem decomposed — handles conflated four needs; three dissolve into existing mechanisms, the fourth is naming, not referencing -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-verification.md, memory/borrowing.md -->

# Where Escaping References Go

The objection, stated at full strength: Rask has no storable references, so
`Handle<T>` was carrying that entire load — every place a program needs to
name something later, elsewhere, or across a boundary. Edges can't leave the
structure. That looks like a hole where a large amount of real code lives.

Working it through: **"escaping" was never one need.** Handles served four,
and they only looked like one because a single mechanism covered them all.
Three dissolve into mechanisms that already exist. The fourth is not a
reference problem.

## First, the rule was too strict

Earlier drafts said "edges live only in node fields." That is stricter than
soundness requires. The actual requirement is **enumerability**: a delete must
be able to find every edge pointing at the dying node. Node fields qualify
because the graph owns them — but so does anything else the graph
transitively owns.

**Revised rule: an edge may live anywhere the graph transitively owns.**

<!-- test: skip -->
```rask
struct Inventory {                 // not a node — a plain value
    items: Vec<Edge<Item>>
    favorite: Edge<Item>?
}

struct Player {                    // a node
    name: string
    inventory: Inventory           // edges nested two deep: still enumerable
}
```

This matters more than it looks. Composition works normally — a node's edges
don't have to be flattened into its top level, so ordinary struct design
survives. Combined with root edges (edge fields on the graph's owning struct)
and root indexes (`Map<K, Edge<T>>` on that struct), the enumerable region is
the whole owned tree, not one level of it.

What's excluded is narrow: locals (which are block-scoped borrows anyway) and
values owned by something *outside* the graph.

## The four needs, separated

### 1. Escape in time — event queues, undo logs, deferred work

`Event.Died(who)`, processed three frames later. The classic case for a
handle.

**Answer: the holder becomes a node.** An event queue is a graph.

<!-- test: skip -->
```rask
struct DamageEvent {
    amount: i32
    subject: Edge<Entity>?     // goes none if the subject dies first
}

// events: Graph<DamageEvent> — a field of the same world
for ev in world.events {
    if ev.subject? as target {
        target.health -= ev.amount
    }                            // dead subject: the event simply does nothing
}
```

No mechanism is added. The queue holds edges because the graph owns the
queue, and a deleted subject nulls out of pending events automatically —
which is the behaviour the handle version had to write by hand (and forgot
to, in the flagship: [#740](https://github.com/rask-lang/rask/issues/740)).
Undo stacks, job queues, and scheduled callbacks are the same shape.

### 2. Escape in scope — working sets, partitioning, worklists

Collect nodes into a local `Vec`, then iterate and mutate. Very common.

**Answer: the delete-locked scope.** These references don't need to survive a
delete; they need deletes not to happen while they're held. That's a weaker
requirement than a stale-checked reference, and it's checkable:

<!-- test: skip -->
```rask
with world.entities.pinned {          // no deletes in this scope
    mut targets = Vec.new()
    for e in world.entities {
        if e.hostile { targets.push(e) }    // plain node refs, no check
    }
    for t in targets { t.alert() }
}                                       // deletes allowed again here
```

Handles solved this by making every access checked. Pinning solves it by
making the dangerous operation impossible for a bounded region — the same
trade Rask already makes everywhere else.

### 3. Escape across sync domains — channels, tasks

Send a reference to another task.

**Answer: you couldn't safely do this with handles either.** A handle sent
cross-task still needs its pool, and touching that pool from two tasks is
exactly the race the sync-domain rule names. What pools actually shipped for
this is a hidden `Arc<Mutex>` — a lock, not a solution.

So the honest reading: this was never a *reference* capability, it was a
*shared mutable state* capability, and Rask's answer is channels. Send the
data (a copy), send a message describing the change, or send a domain id. The
owner task applies it. That is the model pools.md already blesses — "share
handles, not data; the pool stays in one thread; commands flow back to it" —
just without pretending the handle in that sentence was doing something safe.

### 4. Escape out of the process — save files, network, plugins

**Answer: this is not a reference problem, it's a naming problem.** A pointer
means nothing in a byte stream; a generational handle means nothing after a
restart (the generation counters reset). Serializing a handle was already
unsound-in-practice unless you serialized the whole pool with it.

What programs actually do — including Rask's own flagship — is use a domain
identity: `TaskId`, `UserId`, `OrderId`. The store's `by_id` map redeems it.
That code exists today, works unchanged, and never involved a handle crossing
the wire.

## The residual: when you have no domain id

A generic structure with no natural identity field still needs a stable name
sometimes — a cache keyed by node, a debugger, a plugin boundary.

**`NodeId`: a name, not a reference.** The graph mints a never-reused `u64`;
`graph.find(id)` returns `T?`.

<!-- test: skip -->
```rask
let id = world.entities.id_of(e)     // export a name
// ... later, or from elsewhere ...
if world.entities.find(id)? as e { e.wake() }
```

Is this a handle by another name? Structurally similar — a checked lookup —
so the difference has to be argued, not asserted:

| | `Handle<T>` today | `NodeId` |
|---|---|---|
| Role | the **primary** way to reference anything | an **export format** for boundaries |
| Frequency of the check | every deref, in every hot loop | once, at the boundary crossing |
| Needs the container in scope | yes (`using Pool<T>`) | yes, but only at the redemption site |
| Inside the structure | handles all the way down | edges — no checks at all |
| Reuse hazard | generation saturation ends a slot | never reused; a dead id is dead forever |

The distinction that matters: a handle is a reference you *dereference
repeatedly*; a NodeId is a name you *look up once* and then traverse from with
edges. The check doesn't disappear from the language — it stops being on the
hot path and stops being how you write ordinary code.

Whether `NodeId` ships at all is a real question. The flagship needs none of
it. A first version could omit it entirely and add it when a program
demonstrates the need, which is the more disciplined order.

## What is genuinely given up

- **A reference you can put anywhere, with no thought about who owns it.**
  Placing an edge means the graph must own the place. Most code satisfies
  this without noticing (nodes, values inside nodes, world-level fields), but
  a struct that sits outside the world and wants to point into it must hold a
  `NodeId` or a domain key instead.
- **Returning a reference from a function.** Unchanged from today — Rask
  already forbids it (`mem.borrowing/S3`), and the existing answers stand:
  return a copy or a view struct (the flagship's `TaskView`), or hand out
  scoped access with `with`.
- **Sending a live reference to another task.** Gone, and it was never real.

## Moving edges, and moving nodes

"Transfer ownership of an edge without copying" splits into two questions,
because **an edge doesn't own anything** — the graph owns the node; an edge is
a tracked non-owning reference. So there is no ownership in an edge to
transfer, but there are two real operations underneath the question.

### Moving an edge between holders

Rebinding which field holds a reference — `a.target` becomes `b.target`,
with `a` left holding nothing.

Written as copy-then-clear, that's link into the target's list for `b`, then
unlink `a`: roughly 7–10 stores. A genuine *move* is cheaper, because the
target's incoming list never needs to grow or shrink — the same backlink entry
just changes which slot owns it, so only the entry's neighbours and the two
holder slots get written: ~4–5 stores. Worth its own form:

<!-- test: skip -->
```rask
b.target = take a.target      // move: a.target becomes none, list unchanged
b.target = a.target             // copy: two edges, two backlinks
```

This matches `take` everywhere else in Rask, and the saving is real but
modest — a constant factor on an already-cheap operation, not a new
capability.

### Moving a node — the capability that was hiding here

The deeper question: can a *node* move without its references breaking?

Yes, and it falls out of machinery that already exists. Relocating a node
means walking its incoming list and writing the **new address** into each
edge — the identical walk a delete does, with a different value written.
O(in-degree), no new mechanism.

That unlocks something handles structurally cannot do:

| | `Pool` + `Handle` | `Graph` + `Edge` |
|---|---|---|
| Node identity is | the slot index | the node itself |
| Can a live node change slots? | **No** — the index *is* the handle; moving it invalidates every handle | Yes — incoming edges are rewritten to the new address |
| Arena compaction | impossible without an indirection table (which reintroduces a per-access hop) | possible |
| Reordering for locality (hot nodes contiguous) | impossible | possible |

Pools freeze layout by construction: `mem.pools/PL9` guarantees handles
survive *growth* precisely because the index never changes, which is the same
property that forbids compaction. A graph can defragment its arena, or sort
nodes into traversal order so a hot loop walks contiguous memory — an
optimization that matters exactly in the workloads pools were designed for.

**Decided: compaction is explicit, never automatic.** `graph.compact()` is a
call the programmer writes, at a point they choose. A runtime that relocates
nodes on its own schedule is a moving collector — unpredictable pauses
decided by something other than the program — which is precisely what this
design exists to avoid. The capability is layout *control*, not layout
management.

Two honest limits. Moving nodes **between graphs** is not free in the same
way: separate arenas mean the bytes are copied, and any incoming edge from the
old graph must be severed or converted (a cross-sync-domain edge is barred by
the ownership rule anyway). And relocation must respect open borrows —
moving a node someone holds is the same compile error as deleting one.

Nested `Owned<T>` fields ride along correctly: moving a node moves the owning
pointer, not the heap value it points at.

## Verdict

The escape hole is smaller than it appeared, and most of it closes with a rule
correction rather than a new feature: edges live anywhere the graph
transitively owns, which makes queues, logs, indexes, and nested value types
first-class edge holders. Scope escapes become pinned regions. Cross-task and
cross-process escapes were naming problems wearing a reference costume, and
programs already answer them with domain ids.

`Key<T>`/`NodeId` survives as an optional boundary convenience, not as the
load-bearing mechanism it was when it was called `Handle<T>`.
