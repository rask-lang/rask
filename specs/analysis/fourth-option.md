<!-- id: analysis.fourth-option -->
<!-- status: exploration -->
<!-- summary: Is there a fourth memory model beyond GC/RC, lifetimes, and handles? Yes: fix references at delete time instead of checking them at use time — the database answer, unclaimed at language level -->
<!-- depends: memory/pools.md, memory/borrowing.md, memory/boxes.md, memory/allocators.md -->

# The Fourth Option

## The whole idea, plainly

When you delete something, everything pointing at it is set to `none` — right
then, automatically. Dead pointers don't linger, so following a pointer needs
no check: it's either `none` or it's alive. That's the model.

It works because each node keeps a small list of who's pointing at it, so a
delete can find them all and fix them. Databases have done this for fifty
years — it's `ON DELETE SET NULL`.

Every other memory model answers the same question differently: *when a value
dies, who finds out?* A garbage collector finds out later, by scanning.
Refcounting finds out immediately, by counting. Rust's compiler finds out
before the program runs, by proving. Handles make the *reader* find out, by
checking a ticket number at every use. This model makes the *pointers* find
out, at the moment of death.

Only Rask can take it: to fix every pointer at a node, you have to be able to
find every pointer at that node. C and Rust can't — a pointer can hide in any
local, any array, any thread. Rask already bans storing references outside
declared fields, so they're all findable. The restriction handles were built
on is exactly what makes handles replaceable.

## Where the exploration stands

Ten documents; this one is the entry point. The decisions, consolidated —
a spec draft starts here.

**The shape**

| | Decided |
|---|---|
| Types | `Store<T>` (where nodes live), `Link<T>` (one reference). No plural type — `Vec<Link<T>>` and `Map<K, Link<T>>` are edge-aware underneath |
| Reference semantics | An edge goes `none` when its target dies. That's the whole model |
| Representation | Raw pointers. `mem.relocatable` stays keys-only |
| Where edges may live | Anywhere the graph transitively owns — nodes, values inside nodes, graph-owned containers, root fields. Not locals (block-scoped borrows instead) |
| Unlink timing | **Eager** at the apply point. `@lazy` deferred |
| Delete policy | **Set-to-`none` only.** Cascade and restrict deferred; if cascade ships it needs a direction-explicit name and a `delete_cascade(n)` call site |
| Ownership | Composition by value (`Entity { body: Body }`), not a policy |
| Concurrency | Staged structural mutation, no lock on the hot path. Three parallel tiers: per-node, frozen, staged |
| Atomicity | Batches — validate then apply, no rollback. Also how required-edge cycles get built |
| Compaction | Possible (relocation rewrites incoming edges) and **explicit only** — never automatic |
| Escapes | Domain ids at process/sync boundaries. `NodeId` deferred |
| Pool / Handle | Pool folds into `Store<T>`; `Handle` becomes boundary-only, if it's needed at all |
| `Heap<T>` | **Kept.** Different rung of the ownership ladder — exclusively-owned heap data that nothing else references, and unlike a node it can be returned and moved. An AST wants it: movable, half the memory, free delete |

**Deferred on purpose:** `@lazy`, cascade/restrict, `NodeId`. Each failed the
"does a real program demand this yet?" test. That the core keeps surviving
scrutiny while the accessories keep failing it is itself a signal.

### Does this grow the type zoo?

No — it shrinks it by one, and stratifies what's left.

| Type | Status after this change |
|---|---|
| `Vec`, `Map` | untouched |
| `Pool` → `Store` | renamed, not added |
| `Handle` → `Link` | renamed, not added |
| `WeakHandle` | **deleted** — its whole job was surviving a stale reference, and stale references stop existing |
| `Owned` → `Heap<T>` | renamed. Same job, sharper boundary: exclusively-owned heap data, and unlike a node it can be returned |
| `Cell<T>`, `Mutex<T>` | **folded into `Shared<T>`** as strategies — see [consolidation](storage-type-consolidation.md) |
| `Atomic<T>` | kept, own API, but reclassified out of the storage question |

Also gone, though they aren't types: `using Pool<T>` context clauses,
`frozen`, and the generation-coalescing compiler pass.

**What a reader must know, by stratum:**

- **Day one:** `Vec`, `Map`, `string`, `T?`, `T or E`. Unchanged by any of
  this.
- **When things reference each other and can be deleted:** `Store` + `Link`.
- **When several accessors share one mutable value:** `Shared<T>`, plus a
  strategy (`Readers` / `Mutex`) if it crosses tasks.
- **Orthogonal, not part of the sequence:** `Heap<T>` when data is recursive
  or large; `Atomic<T>` for a measured contended counter.

Five memory types total, and no stratum is entered until a program needs it.
For comparison: Rust reaches for `Box`, `Rc`, `Arc`, `RefCell`, `Cell`,
`Mutex`, `RwLock`, `Weak` — more types, and the selection is harder because
several combine (`Arc<Mutex<T>>`, `Rc<RefCell<T>>`). Go has almost none,
which is the honest counterexample; it buys that with a garbage collector.

### Open questions

**Unsolved mechanisms** — no worked design yet; a spec draft has to answer
these first:

1. **Index backlinks across a `Map` rehash.** A `Map<K, Link<T>>` moves its
   entries when it grows. The backlink has to find the entry again. Probably
   means the backlink carries the key rather than a slot address, but that's
   a guess, not a design.
2. **The delete-locked scope.** Named repeatedly (collect nodes into a local
   `Vec`, then work through them), never specified. What opens it, what's
   forbidden inside, how it interacts with `mutate` parameters.
3. **Batch semantics.** Validate-then-apply is decided; the details aren't.
   What exactly is validated, what a rejection returns, whether batches
   nest, what happens on panic inside one.
4. **Root link registration.** Links on the owning struct (a list's
   `head`/`tail`, an editor's selection) are load-bearing in the litmus
   programs and have no stated registration rule.

**Specified nowhere yet, but no obvious difficulty:** iteration guarantees
over a store (the `PF1`–`PF4` equivalents), the diagnostics for every new
error path, and what `mem.relocatable` becomes when it's keys-only.

**Untested, not unsolved:**

- **Nothing is measured.** Every performance claim in these documents is
  analytic. The read-path claim (a plain deref versus a checked one) is the
  load-bearing one and the easiest to measure with a prototype.
- **The `Local` default has no corpus example.** Not one program shares a
  mutable value between closures in a single task, so the case `Shared<T>`
  defaults to is unrepresented in the evidence.
- **Migration cost is unsized.** Ten specs, two backends, the whole example
  corpus. Nobody has counted it.

**Companion documents:** [litmus](fourth-option-litmus.md) (three programs
both ways, scored) · [in practice](fourth-option-in-practice.md) (worked
example, costs, what it retires) · [adversarial](fourth-option-adversarial.md)
(16 attacks) · [concurrency](fourth-option-concurrency.md) ·
[escapes](fourth-option-escapes.md) · [verification](fourth-option-verification.md)
(soundness, strictly-better, the flagship).

---

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

A graph-shaped box (working name `Store<T>`; naming comes last). Nodes live in
it like they live in a pool — it owns their memory. The difference: instead of
handles, nodes refer to each other with **edges**, declared in the schema.

<!-- test: skip -->
```rask
struct Entity {
    health: i32,
    target: Link<Entity>?,           // becomes none when the target is deleted
    children: Vec<Link<Entity>>,     // edge list; deleted nodes drop out
    parent: Link<Entity>? inverse(children),   // compiler-maintained inverse
}
```

The rules that make it work, all reusing existing machinery:

1. **Links live only in node fields.** An `Link<T>` can be stored in a field of
   a node in the same graph, nowhere else. Locals hold block-scoped borrows, as
   everywhere in Rask. This is what keeps incoming references enumerable.

2. **Every edge has a backlink.** Writing `a.target = b` registers a hidden
   backlink in `b` (an intrusive incoming-edge link — O(1) to add, O(1) to
   unlink, the kernel `list_head` trick). When the schema declares an inverse
   (`parent`/`children`), the inverse *is* the backlink — no hidden storage at
   all. A doubly-linked list's `prev` is `next`'s backlink; a tree's `parent` is
   `children`'s. The memory the mechanism needs is memory those structures
   already carry by hand today.

3. **Delete unlinks.** `store.delete(n)` walks n's incoming edges (enumerable,
   via backlinks), sets each `Link?` to `none` or removes it from its list
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
   `using frozen Store<T>` no deletes can happen, so raw node refs in a local
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
struct Entity { health: i32, target: Link<Entity>? }

if e.target? as t {
    t.health -= damage             // no check exists to elide
}
```

The staleness branch is gone — not hidden, gone: `target` became `none` at the
moment the target died. The stale-handle state that pools detect at access
simply never exists.

### Runtime safety

Handles are runtime-checked: the stale state exists, `pool[h]` detects it and
panics, `get(h)?` asks the reader to remember to be careful. Links move death
into the type: an edge to something that can die is `Link<T>?`, it becomes
`none` when the target dies, and the compiler forces the branch. Reads cannot
panic — there is no stale state left to detect.

What remains at runtime, exhaustively:

- `!` on a none edge — explicit, same as any optional.
- Aliased `with` scopes — two edges resolving to the same node: pointer compare
  at scope open, panic on duplicate. Pools' W3 rule verbatim, same cost.
- Non-optional edges (`owner: Link<Player>`, no `?`) need a declared delete
  policy, and it's the database trio: **cascade** (delete propagates to the
  holder), **restrict** (the *delete* fails — error or panic at the one delete
  site, not scattered across reads), or disallow non-optional edges entirely.

### No context clauses

`using Pool<T>` auto-resolution (`mem.context`) exists because a handle is a
detached reference — a naked integer, useless without its pool, so the pool
must be smuggled into every function that touches one. An edge is never
detached: it's reachable only through a borrowed node, and deref is a plain
pointer that needs no container to resolve. Traversal functions take a node
borrow and nothing else:

<!-- test: skip -->
```rask
func damage(e: Entity, amount: i32) {    // no using clause
    if e.target? as t { t.health -= amount }
}
```

Still needed: the graph in hand for structural ops (`insert`/`delete`, like
pools), `with` blocks for multi-statement access (it's a box), and one rule in
exchange — a borrowed node counts as a borrow of the graph, so `delete` while
any node borrow is live is a compile error. That rule is what makes checkless
borrows sound; it's Vec's "no push while an element is borrowed", same shape.

## What it costs — honestly

The bookkeeping is conserved, not eliminated. Handles pay at every read
(generation check, double indirection); edges pay at every delete (O(in-degree)
unlink) and every edge write (O(1) backlink registration). For read-heavy
structures — which graphs, trees, and entity systems overwhelmingly are — that
trade is favorable. For churn-heavy, high-fan-in structures (10,000 edges into
one node, deleted every frame) it's worse, and honestly so: the delete's cost is
proportional to what must be fixed.

| | `Pool` + `Handle` | `Store` + `Link` |
|---|---|---|
| Read a reference | ~1ns check + indirection (elidable sometimes) | plain deref, nothing to elide |
| Write a reference | free (it's an integer copy) | O(1) backlink register |
| Delete | O(1), bumps generation | O(degree), unlinks |
| Stale reference | exists; caught at access | cannot exist |
| Stored size | 12 bytes | 8-byte pointer (+8–16 backlink, free when an inverse is declared) |
| Reference escapes the structure | yes — handles are values, send them anywhere | no — edges live in node fields only |
| Survives serialization / mmap | yes (`mem.relocatable`) | needs pointer fixup, or loses the row above |

The last two rows are the real finding. **Links kill handles for topology, and
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

## The use-case census, and Pool folding into Graph

Walking every pool use case in the specs and examples:

| Use case today | Lands on |
|---|---|
| Graphs, trees with parent pointers, linked lists | Links |
| ECS relationships (`entity.body`, `target`, `children`) | Links — cross-graph works; delete touches both graphs, like a foreign key across tables |
| Observer lists, in-world caches, event nodes | Links, when the holder lives in the graph |
| Iterate-and-delete loops | Graph iteration, same shape as pools |
| Ordered views (`line_order: Vec<Handle<Line>>`, text_editor) | Root edge containers — an ordered `Vec<Link<Line>>` on the owner; entries drop at delete |
| Secondary indexes (`by_name: Map<string, Handle<Pkg>>`, package_manager; `by_id: Map<TaskId, Handle<Task>>`, validation store) | Root `Map<K, Link<T>>` — delete removes the entry, the database's index-maintenance move. Needs spec: the backlink must carry the key (or survive rehash) |
| Chunked parallel iteration (game_loop's aspirational `spawn` over handle chunks) | Scoped parallel iteration under a delete-locked scope — disjoint node sets, no keys, and none of the `Arc<Mutex>` pools currently smuggle in for cross-task `using` |
| References serialized out (save files, network) | Keys — though the validation flagship's actual escaping identity is `TaskId`, a user-level ID redeemed through the `by_id` index, not a `Handle`. Even the web-service case prefers domain keys + a maintained index |
| References held by unsynchronized concurrent holders | Keys |

What's left of `Pool` after edges take topology is small: a registry that hands
out checked keys. That doesn't earn a separate box. **Direction (decided): Pool
folds into Graph.** One box, two reference kinds — `Link<T>` inside (checkless,
fixed at delete), `Key<T>` escaping (a Copy value, Send, storable anywhere,
redeemed via `store.get(k)?`). `Key` is today's `Handle` with its honest name;
a keys-only graph with no edge fields is today's `Pool`. Box count shrinks by
one.

### The boundary theorem, poked

First draft of the limit said: anything escaping in *time* (queues, logs) or
*space* (other tasks) must be a checked key. Poked at, that's too strong. The
delete-witness model reaches further than scopes:

- **Time escapes can stay in the model** by living in the graph. An event
  node holding `who: Link<Entity>?` gets fixed at delete like any other edge —
  processing the event later reads `none`, no generation, no panic path. The
  queue being *data in the world* is enough.
- **Even cross-task holders could in principle be fixed**: a registered root
  behind a `Mutex` that delete locks and walks — QPointer's registry, done
  soundly. Declined, but for cost (every delete locks every registered root;
  contention), not impossibility.

What actually forces a checked key is narrower: **references the delete cannot
reach** — bytes already serialized outside the process, and holders outside the
synchronization domain. Inside the process, key-vs-fixable-root is a tradeoff:
a key is 12 bytes, Copy, Send, zero coordination; a fixable root demands
registration and locking at delete. Keys stay the right *default* for escape
because coordination-free is worth more than checkless — but they're only
*forced* at the process/sync boundary. (Coarser checks exist too — one
graph-wide version stamp invalidating all keys on any delete — same category,
cheaper, blunter. And "linear keys that delete must collect back" just makes
death wait for the name, which is RESTRICT wearing ownership's coat.)

### Representation (decided)

Links compile to raw pointers. An index representation (base + index, still
checkless — index reuse is safe without generations because stale edges can't
exist) would keep pools' serialization/mmap story, at one add per hop paid
everywhere. Declined: the relocatability story is narrow in practice and
doesn't justify taxing every traversal. Graphs that need to serialize walk
themselves through Encode like everything else; `mem.relocatable` stays a
keys-only feature.

### Eager or lazy: the unlink's timing is implementation-free

Link-vs-key is two choices, only one of them forced. The *semantics* — checked
key or self-nulling edge — is user-visible and must be picked per reference.
The *timing* of the unlink is not observable in single-threaded code: a reader
cannot distinguish "nulled eagerly at delete" from "nulled lazily before I
looked." That freedom admits a hybrid:

**Tombstone delete + deferred unlink.** `delete` marks the node dead and
returns — O(1), a handle-remove's cost. A read of a not-yet-healed edge checks
one flag in the target's header (same cache line as the data it was about to
load), sees dead, self-nulls — after which that edge is a plain pointer again.
Remaining unlinks amortize onto later graph operations or an explicit
`store.flush_deletes()`; memory is reused when the backlink list drains.
Observationally identical to eager edges: a node or `none`, never a panic,
never a stale value.

The impossibility theorem, stated exactly: **every incoming edge must either
be individually written to `none` (sometime), or checked at every read
(forever).** Zero fixup writes and zero read checks over the same window
can't coexist. Everything else is sliding along that frontier — eager unlink
(checkless reads, O(degree) delete), lazy (O(1) delete, one-flag reads until
healed), per-field policy in the schema. Prior art for the lazy end is RCU
("readers proceed checklessly, reclamation deferred") and epoch-based
reclamation.

The lazy variant also dissolves the litmus's TC regression: eager `delete`
hides degree-proportional work, while lazy `delete` is cheap and says so, and
the O(degree) work runs at a `flush_deletes()` you can see and place. Cost
transparency is preserved at call sites; what becomes invisible is mechanism,
not cost.

**Decided (revised): eager is the default; `@lazy` is a per-relation policy
for high-fan-in hubs.** The first draft made lazy the default to answer
"delete is O(degree), a handle remove is O(1)". Staging (see
[fourth-option-concurrency.md](fourth-option-concurrency.md)) already
answers that: structural ops defer to a visible apply point regardless, so
the cost sits at a line you wrote either way, and lazy's justification
mostly evaporates. What's left of it is narrow — lazy skips fixups for
edges nobody ever reads again — and it is paid for dearly:

| | Eager unlink | Lazy unlink |
|---|---|---|
| Read an edge | pure deref; **nothing to check, ever** | one header-flag load until healed |
| Memory | freed at apply | pinned until every incoming edge heals or flush runs |
| Machinery | none | tombstone flags, heal-on-read, amortized heal-on-insert, flush, heal suppression under shared reads |
| Total fixup work | one write per incoming edge | fewer if edges are never re-read; the same otherwise |

Eager keeps the model's headline claim literally true — a dead pointer does
not exist, so following one needs no check. Lazy makes that claim
"eventually true, with a transient check," which is a real weakening of the
central promise for a benefit most schemas never collect: ordinary in-degree
is 1–5, so eager's per-delete work is a handful of stores.

Lazy survives as an opt-in for the pathological shape it was invented for:
a hub with 100k incoming edges, where walking the list at apply is a genuine
pause and most of those edges will never be read again. `@lazy` on that
relation trades pure reads for a distributed fixup. Two modes, and the
default is the simple one.

(Unrelated to node deletion: ordered edge *containers* still tombstone and
compact entries — A5 — which is local list maintenance, not node
reclamation.)

**Where the lazy check lives: inside `?`.** In eager mode, `e.target? as t`
is exactly the optional unwrap — one none-test, nothing else; unlinked edges
are literally `none`. In lazy mode the unwrap gains a hidden second step:
non-none pointer → load the target's header flag → if dead, self-heal to
`none` and take the none branch. No new syntax, no new concept — the "might
not be there" the programmer already acknowledged by writing `?` is the only
place the runtime needs. (An earlier draft carved out non-optional edges as
eager-only; the adversarial pass then killed non-optional edges entirely —
they can't be constructed under cycles — so every edge has a `?` site and
lazy covers the whole model uniformly. See
[fourth-option-adversarial.md](fourth-option-adversarial.md), A4.)

### Open

The partition pattern — collect refs into a local Vec, then mutate through
them — needs a scope where deletes are locked but field writes are allowed.
`frozen` is too strong (forbids writes). A weaker delete-locked tier is the
one new scope concept this design asks for.

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
3. It does not abolish checked keys; it demotes and renames them. Topology
   (linked lists, trees with parent pointers, graphs, ECS relationships) moves
   to edges: faster reads, no staleness state, no generation ceremony. What
   forces a check is only the reference the delete cannot reach — serialized
   bytes, unsynchronized concurrent holders — and those redeem a `Key<T>`
   through `get(k)?`. Pool folds into Graph as the keys-only case (see census
   above).
4. The box-family bar (`mem.boxes` appendix: "box six needs a scoped access
   discipline the five don't cover") is met — *relational* access, edges fixed
   at delete, is not exclusive/identity/read-heavy/locked/linear. This doc does
   not propose adding it; it establishes that the seat exists and what sitting
   in it would cost. The litmus comparison — three programs both ways, scored
   per METRICS — is in [fourth-option-litmus.md](fourth-option-litmus.md).
