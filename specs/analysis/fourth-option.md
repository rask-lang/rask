<!-- id: analysis.fourth-option -->
<!-- status: exploration -->
<!-- summary: Is there a fourth memory model beyond GC/RC, lifetimes, and handles? Yes: fix references at delete time instead of checking them at use time — the database answer, unclaimed at language level -->
<!-- depends: memory/pools.md, memory/borrowing.md, memory/boxes.md, memory/allocators.md -->

# The Fourth Option

The question: Rask and Hylo attack the same problem — memory safety without GC, RC,
or lifetime annotations. The known answers are lifetimes (Rust), no-references-at-all
(Hylo), and checked handles (Rask, Vale). Is there a fourth answer, and specifically:
can handles go away without reintroducing GC, RC, or lifetimes?

Short answer: yes, there is exactly one unclaimed corner in the design space, and
Rask's existing rules are the precondition for reaching it. Whether it *replaces*
handles or sits beside them is the honest question — worked through at the end.

## The map: who witnesses a deletion?

Every deref must know the target is alive. Every safe memory model is an answer to
one question: **when a value dies, who finds out, and when?**

| Witness | When they learn | Scheme | Languages |
|---------|----------------|--------|-----------|
| The collector | Eventually, by tracing | GC | Java, Go |
| The last reference | Immediately, by counting | RC | Swift, Nim, Python |
| The compiler | Before the program runs | Lifetimes / regions / second-class refs | Rust, Cyclone, Austral |
| The reader | At next access, by checking | Generational handles | Rask pools, Vale, every ECS |
| Nobody — deletion doesn't exist | — | Arenas / never-free | Zig arenas, MLKit, `mem.alloc` |
| Nobody — references don't exist | — | Pure mutable value semantics | Hylo, Swift structs |

Hardware tagging (CHERI, ARM MTE) is the reader-checks row implemented in silicon.
Epoch/frame schemes ("defer deletes to frame end") are the arena row applied
repeatedly. Every "new" memory model of the last decade lands on this table —
Vale's generational references are handles baked into fat pointers, Verona and
Vale's regions are rows 3/5, Koka's Perceus is optimized row 2.

One seat at the table is empty: **the incoming references themselves learn, at
delete time.** Deleting a value walks every reference that points at it and fixes
it — nulls it, or removes it from its list. After delete, no stale reference
exists anywhere, so a deref needs no check, no count, no lifetime, and no
generation. Liveness is an invariant maintained by `delete`, not a property
checked by readers.

This is not exotic. It's `ON DELETE SET NULL` / `ON DELETE CASCADE` — the model
relational databases have run for fifty years. Qt does it in a library (QPointer
zeroing, parent-child teardown). Kernel intrusive lists do it by hand (`list_del`
unlinks in place). The ECS world is converging on it as "relationships" (flecs).
Nobody has made it a *language's* memory model.

## Why no language has taken the seat

To fix every incoming reference at delete time, the language must be able to
**enumerate every incoming reference**. With first-class storable references
that's hopeless — a pointer can hide in any local, any struct, any array, any
thread's stack. Rust can't enumerate them. Neither can C++, which is why QPointer
needs a registry and only covers QObjects.

Rask already banned the thing that makes it hopeless. No storable references
(`mem.borrowing/S3`): a persistent reference can only live in a declared field of
a value the compiler knows about, and every local is a block-scoped borrow the
checker already tracks. Enumerability isn't a new restriction Rask would have to
add — it's the restriction Rask is built on. The pool spec even documents the
consequence backwards: handles exist *because* stored addresses were banned. The
same ban makes stored addresses safe to re-admit in one specific shape, because
now they can all be found.

## The sketch: edges instead of handles

A graph-shaped box (working name `Graph<T>`; naming comes last). Nodes live in
it like they live in a pool — it owns their memory. The difference: instead of
handles, nodes refer to each other with **edges**, declared in the schema.

<!-- test: skip -->
```rask
struct Entity {
    health: i32,
    target: Edge<Entity>?,           // becomes none when the target is deleted
    children: Edges<Entity>,         // edge list; deleted nodes drop out
    parent: Edge<Entity>? inverse(children),   // compiler-maintained inverse
}
```

The rules that make it work, all reusing existing machinery:

1. **Edges live only in node fields.** An `Edge<T>` can be stored in a field of
   a node in the same graph, nowhere else. Locals hold block-scoped borrows, as
   everywhere in Rask. This is what keeps incoming references enumerable.

2. **Every edge has a backlink.** Writing `a.target = b` registers a hidden
   backlink in `b` (an intrusive incoming-edge link — O(1) to add, O(1) to
   unlink, the kernel `list_head` trick). When the schema declares an inverse
   (`parent`/`children`), the inverse *is* the backlink — no hidden storage at
   all. A doubly-linked list's `prev` is `next`'s backlink; a tree's `parent` is
   `children`'s. The memory the mechanism needs is memory those structures
   already carry by hand today.

3. **Delete unlinks.** `graph.delete(n)` walks n's incoming edges (enumerable,
   via backlinks), sets each `Edge?` to `none` or removes it from its `Edges`
   list, unregisters n's outgoing backlinks, frees the node, returns it owned
   (so `@resource` fields follow `mem.linear`, same as `pool.remove`). O(degree),
   at an explicit call site — the cost is visible, per Transparency. No user
   code runs during the unlink walk; Rask has no destructors, so nothing can
   observe the graph mid-fixup.

4. **Traversal is plain pointers, zero checks.** `e.target` is either `none` or
   a valid node — there is no third state, so there is nothing to check. No pool
   base, no index arithmetic, no generation compare. Aliased `with` scopes get
   the same treatment pools already have (`with a.target as t, b.parent as p`
   panics if they resolve to the same node — a pointer compare at scope open,
   `mem.pools` W3's rule verbatim).

5. **Delete respects open borrows.** Deleting while a local borrows a node is
   the existing W2c-shaped compile error. Worklist algorithms that need node
   identity in local collections get it from the frozen discipline: inside
   `using frozen Graph<T>` no deletes can happen, so raw node refs in a local
   `Vec` are valid for the whole scope by construction — regions falling out of
   a rule (`PF5`) that already exists.

### How it reads

Entity targeting, today with pools:

<!-- test: skip -->
```rask
struct Entity { health: i32, target: Handle<Entity>? }

if e.target? as h {
    if entities.get(h)? as t {     // stale? wrong pool? checked here, every time
        t.health -= damage
    } else {
        e.target = none              // manual cleanup of the stale handle
    }
}
```

With edges:

<!-- test: skip -->
```rask
struct Entity { health: i32, target: Edge<Entity>? }

if e.target? as t {
    t.health -= damage             // no check exists to elide
}
```

The staleness branch is gone — not hidden, gone: `target` became `none` at the
moment the target died. The stale-handle state that pools detect at access
simply never exists.

## What it costs — honestly

The bookkeeping is conserved, not eliminated. Handles pay at every read
(generation check, double indirection); edges pay at every delete (O(in-degree)
unlink) and every edge write (O(1) backlink registration). For read-heavy
structures — which graphs, trees, and entity systems overwhelmingly are — that
trade is favorable. For churn-heavy, high-fan-in structures (10,000 edges into
one node, deleted every frame) it's worse, and honestly so: the delete's cost is
proportional to what must be fixed.

| | `Pool` + `Handle` | `Graph` + `Edge` |
|---|---|---|
| Read a reference | ~1ns check + indirection (elidable sometimes) | plain deref, nothing to elide |
| Write a reference | free (it's an integer copy) | O(1) backlink register |
| Delete | O(1), bumps generation | O(degree), unlinks |
| Stale reference | exists; caught at access | cannot exist |
| Stored size | 12 bytes | 8-byte pointer (+8–16 backlink, free when an inverse is declared) |
| Reference escapes the structure | yes — handles are values, send them anywhere | no — edges live in node fields only |
| Survives serialization / mmap | yes (`mem.relocatable`) | needs pointer fixup, or loses the row above |

The last two rows are the real finding. **Edges kill handles for topology, and
cannot replace them for identity.** An event queue holding "who died", a save
file, a network message, a handle sent to another task — those are references
that outlive scopes, escape the structure, and survive time. Anything that does
that is a *key*, and a key that names possibly-dead data must be validated when
it comes back — which is a generation check, which is a handle. That's not a
weakness of this design; it's the reason the reader-checks row of the table
exists. No scheme gets durable identity without either keeping the target alive
(GC/RC) or checking at use (handles). A fourth option for *references* exists;
a fourth option for *durable identity* provably doesn't.

Concurrency is unchanged from pools: delete-unlink is a multi-node write, so a
graph is single-task or behind `Mutex`, exactly like `Pool` today.

## Alternatives weighed and set aside

- **Scoped regions with deferred delete** (arena row): open a region, traverse
  with raw refs, deletes are tombstones, reclaim at region end. Works, needs no
  new mechanism beyond `mem.alloc` arenas — but tombstones bring back a check
  (is this node dead?) on every stored-edge hop, which is a generation check
  wearing a coat. Kept as a pattern (frozen scopes above), rejected as the model.
- **Paths as identity** (pure MVS, the Hylo-extended route): name a node by its
  path from the root of a value tree. Paths go stale under mutation in ways that
  are *silent* (the path now names a different node) — strictly worse than a
  stale handle, which at least fails loudly.
- **Vale's generational references**: handles fused into pointers. Same row of
  the table as pools; the check moves, it doesn't leave.

## Verdict

1. There is a fourth option, exactly one, and it's the database model: **fix
   references at delete time.** The taxonomy has no other empty seat — every
   scheme is a choice of witness, and the witnesses are enumerable.
2. It's uniquely available to Rask. Enumerable incoming references require
   banning storable references, which Rask already did. Rust would need to
   become a different language to take this seat; Rask needs one new box.
3. It does not abolish handles; it demotes them. Intra-structure topology
   (linked lists, trees with parent pointers, graphs, ECS relationships) moves
   to edges: faster reads, no staleness state, no generation ceremony. Durable
   identity — anything that escapes the structure or outlives a scope — keeps
   pools, because keys-that-may-be-stale must be checked, in any universe.
4. The box-family bar (`mem.boxes` appendix: "box six needs a scoped access
   discipline the five don't cover") is met — *relational* access, edges fixed
   at delete, is not exclusive/identity/read-heavy/locked/linear. This doc does
   not propose adding it; it establishes that the seat exists and what sitting
   in it would cost. Next step, if any: write the litmus programs (doubly-linked
   list, text editor undo tree, game-loop targeting) both ways and compare —
   per METRICS, not per taste.
