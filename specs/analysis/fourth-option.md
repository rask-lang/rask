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
| Types | `Rack<T>` (where nodes live), `Link<T>` (one reference). No plural type — `Vec<Link<T>>` and `Map<K, Link<T>>` are edge-aware underneath |
| Reference semantics | An edge goes `none` when its target dies. That's the whole model |
| Representation | Raw pointers. `mem.relocatable` stays keys-only |
| Where edges may live | Anywhere the graph transitively owns — nodes, values inside nodes, graph-owned containers, root fields. **Locals hold links too**, kept honest by the compiler rather than the rack: `delete` takes the link, so use-after-delete is a move error (not a borrow rule, not lifetime inference). A `const` can hold neither |
| Unlink timing | **Eager** at the apply point. `@lazy` deferred |
| Delete policy | **Set-to-`none` only.** Cascade and restrict deferred; if cascade ships it needs a direction-explicit name and a `delete_cascade(n)` call site. Note this is only complete while every edge is optional — a required edge has no `none` to be set to, so admitting `Link<T>` (see below) makes one of cascade/restrict mandatory rather than deferred |
| Ownership | Composition by value (`Entity { body: Body }`), not a policy |
| Concurrency | Deferred deletes, no lock on the hot path. Three parallel tiers: per-node, frozen, staged. Parallel inserts claim slots by atomic bump (B10) |
| Atomicity | Batches — a region where **deletes** defer to the end. No validation step (required links are a compile-time check), no rollback needed. Also the delete-locked scope, and how required-link cycles get built. See [batches](fourth-option-batches.md) |
| Compaction | Possible (relocation rewrites incoming edges) and **explicit only** — never automatic |
| Escapes | Domain ids at process/sync boundaries. `NodeId` deferred |
| Pool / Handle | Pool folds into `Rack<T>`; `Handle` becomes boundary-only, if it's needed at all |
| `Heap<T>` | **Kept.** Different rung of the ownership ladder — exclusively-owned heap data that nothing else references, and unlike a node it can be returned and moved. An AST wants it: movable, half the memory, free delete |

**Deferred on purpose:** `@lazy`, cascade/restrict, `NodeId`. Each failed the
"does a real program demand this yet?" test. That the core keeps surviving
scrutiny while the accessories keep failing it is itself a signal.

### Does this grow the type zoo?

No — it shrinks it by one, and stratifies what's left.

| Type | Status after this change |
|---|---|
| `Vec`, `Map` | untouched |
| `Pool` → `Rack` | renamed, not added |
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
- **When things reference each other and can be deleted:** `Rack` + `Link`.
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

Each was checked with three questions — *is it legal, is it needed, can an
existing mechanism do it?* Three of the four dissolved.

**1. Index backlinks across a `Map` rehash — dissolved, by a rule that
already exists.**

The worry: a `Map<K, Link<T>>` moves its entries when it grows, so a backlink
pointing at a map slot breaks.

But a rehash **already** touches every entry — that's what a rehash is. While
it's moving each entry it can re-point that link's backlink to the new
location. No asymptotic cost on an operation that was already O(n), and no
new mechanism, because `Vec<Link<T>>` compaction (A5) needs exactly the same
thing.

**One rule covers both:** *a container that stores links and moves them must
re-point their backlinks as it moves them.* The compiler knows the element
type is a link, so it emits the fixup in `Vec`'s compaction and `Map`'s
rehash alike.

(An earlier draft "solved" this by inventing a rack-owned index —
`Rack<Task> @key(id)` with a generated `by_id` lookup. That was a feature
answering a question the rule above answers for free, and it introduced
magic method names derived from field names. Withdrawn.)

**2. The delete-locked scope — subsumed by batches; no implicit version.**
The need: collect node references into a local `Vec`, then work through them,
without a delete invalidating one mid-loop. Inside a staged batch, deletes
are *enqueued and applied at the end* — so no node dies while the batch runs
and references stay valid by construction. The batch already is the scope.

**Should an ordinary `for` over a rack imply one?** No — weighed below.

### Should `for` over a rack be implicitly delete-locked?

Three shapes, and the middle one is the tempting mistake.

**(a) `for` forbids deletes inside it.** Simple to state, and it makes
collected references trivially safe. But it *removes a capability pools have
today* — `mem.pools/PF1` guarantees "removing the current element is always
safe", and iterate-and-delete is one of the commonest loops there is
(`cleanup_system`, `evict_one`, `invalidate`). Trading that away to avoid one
line of ceremony is a bad deal.

**(b) `for` silently stages deletes and applies them at loop end.** Gets both
properties — delete-during-iteration works *and* references stay valid. It's
also the tempting mistake: `rack.delete(x)` would then mean something
different inside a loop than outside it, with no syntax marking the
difference. A reader can't tell when the delete takes effect without knowing
which construct encloses them. That's the kind of action-at-a-distance this
whole design has been removing (`frozen`, hidden context clauses, Pool's
covert `Arc<Mutex>`).

**(c) No magic. Deleting during iteration takes a batch.** One extra line,
and the mechanism is visible:

<!-- test: skip -->
```rask
world.stage(|w| {
    for e in w.entities {
        if e.health <= 0 { w.delete(e) }
    }
})
```

**Recommendation: (c).** The cost is one line at the loops that delete; the
benefit is that `for` means the same thing everywhere and `delete` takes
effect at a point you can see. The compile error carries the fix — "deleting
during iteration needs a batch; wrap the loop in `stage`" — so nobody has to
learn it twice.

Worth noting the shape of the argument: (b) is more convenient and is exactly
what a language with a garbage collector would do, because there the timing
doesn't matter. Here it does, so it has to be written down.

**3. Batch semantics — designed and settled, in
[batches](fourth-option-batches.md).**
The pass simplified it twice over: only *deletes* defer (inserts and link
writes are immediate, since neither can invalidate a reference), so a batch
is precisely "a region where deletes are deferred" rather than a general
command buffer; and required links are checked by definite assignment at
compile time, which removes the validation step and the rejection path
entirely. The one hole it left — parallel inserts allocating concurrently — is
resolved in B10: allocation is a single atomic bump, which is lock-free and
not a lock, with chunked growth and `compact()` to defragment.

**4. Root link registration — dissolved, it's static.**
"Root link" means a link stored on the struct that *owns* the rack rather
than inside a node — a list's `head`/`tail`, an editor's `selected`, a
world's `player`:

<!-- test: skip -->
```rask
struct World {
    entities: Rack<Entity>
    player: Link<Entity>?          // beside the rack, not inside a node
}
```

Deleting the player has to null that field, so the fixup walk must reach it.
No runtime registration is needed: the compiler knows at `World`'s module
that this field targets that rack — the same schema closure that answers
"who can point at `Entity`?" (A9) — so the fixup for root fields is emitted
statically, like any other known link.

**Specified nowhere yet, but no obvious difficulty:** the diagnostics for
every new error path, and what `mem.relocatable` becomes when it's keys-only.
(Iteration guarantees are now stated as B11 — they fell out of B2 and B9
rather than needing a decision.)

**Not a conflict after all:** `mem.boxes`' closed-family rule is about the set
*growing* — it bars users from adding boxes. Merging `Cell` and `Mutex` into
`Shared` shrinks it, which the rule doesn't speak to.

**Untested, not unsolved:**

- **The read-path claim is still unmeasured.** An interpreter prototype now
  exists ([prototype](fourth-option-prototype.md)) and confirms the *semantics*
  — the litmus programs run both ways with identical output — but a
  tree-walking interpreter can't price a deref against a checked deref.
  That needs native codegen. Delete cost *was* measured and is linear in
  in-degree, exactly as predicted.
- **The locals rule is settled: use-after-delete is a compile error.** `delete`
  takes the link, so the existing move checker reports the use — no runtime check,
  and not lifetime inference either, since the invalidation point is the `delete`
  statement rather than an inferred last use. Built and passing on every comparison
  program. The reasoning below is why it is the right rule; what remains open is
  the delete the compiler can't see (a call taking the rack mutably that deletes
  inside), for which Rask's existing exclusivity rule is the right shape. A
  local link is non-optional, so it asserts its target is alive; a delete
  contradicts that and there is no `none` to fall back to, which makes
  use-after-delete a type contradiction rather than only a memory hazard. That is
  the same sentence that forces a *field* edge to be `Link<T>?`, resolved the other
  way: the rack can reach a field so it nulls it at runtime, and cannot reach a
  local so the compiler must reject the use. The `?` is therefore the signal for
  which discipline applies. Demonstrated in the prototype; not written down here. `rack.insert()` hands a link into a local,
  which rule 1 forbids; without a rule reconciling those, a local link outlives
  its node and the checkless read isn't sound. Both obvious statements of the rule
  are things Rask chose against — last-use borrow ends is NLL, and
  `with`-everywhere restores the ceremony the model removes. A third statement
  works ("an `insert` result must be stored into a field"), demonstrated in
  `prototype/l1_list_links_no_locals.rk`, but it costs the ability to keep a
  reference to what you just inserted — so `Key<T>` returns to ordinary code, not
  just the serialization boundary, against the census's claim that no in-process
  use needs one.
- **There is no read-only link.** A handle gives read-only access by not passing
  the pool mutably; a link carries write permission wherever it goes. Answering
  it means `ReadLink<T>`, which puts back a type the census deleted.
- **Representation bets against #626.** Declining pointer-freedom is one line
  here, but `mem.relocatable` opens by asserting user-visible types hold "owned
  values and integer handles — never pointers", and #626's tier-A `Persistable`
  is defined around handles surviving a round-trip. Links are pointers, so
  tier-A zero-copy persistence dies for anything with edges. Possibly the right
  trade; not one a representation footnote should make.

  These three outrank `inverse`, cascade and `@lazy` on the open list. See the
  prototype document.
- **The `Local` default has no corpus example.** Not one program shares a
  mutable value between closures in a single task, so the case `Shared<T>`
  defaults to is unrepresented in the evidence.
- **Migration cost is unsized.** Ten specs, two backends, the whole example
  corpus. Nobody has counted it.

**Companion documents:** [prototype](fourth-option-prototype.md) (**built and
run** — the litmus programs both ways on the interpreter, and what that
changed) · [batches](fourth-option-batches.md) ·
[litmus](fourth-option-litmus.md) (three programs
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

A graph-shaped box (working name `Rack<T>`; naming comes last). Nodes live in
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

3. **Delete unlinks.** `rack.delete(n)` walks n's incoming edges (enumerable,
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
   `using frozen Rack<T>` no deletes can happen, so raw node refs in a local
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

| | `Pool` + `Handle` | `Rack` + `Link` |
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
| Secondary indexes (`by_name: Map<string, Handle<Pkg>>`, package_manager; `by_id: Map<TaskId, Handle<Task>>`, validation rack) | Root `Map<K, Link<T>>` — delete removes the entry, the database's index-maintenance move. Needs spec: the backlink must carry the key (or survive rehash) |
| Chunked parallel iteration (game_loop's aspirational `spawn` over handle chunks) | Scoped parallel iteration under a delete-locked scope — disjoint node sets, no keys, and none of the `Arc<Mutex>` pools currently smuggle in for cross-task `using` |
| References serialized out (save files, network) | Keys — though the validation flagship's actual escaping identity is `TaskId`, a user-level ID redeemed through the `by_id` index, not a `Handle`. Even the web-service case prefers domain keys + a maintained index |
| References held by unsynchronized concurrent holders | Keys |

What's left of `Pool` after edges take topology is small: a registry that hands
out checked keys. That doesn't earn a separate box. **Direction (decided): Pool
folds into Graph.** One box, two reference kinds — `Link<T>` inside (checkless,
fixed at delete), `Key<T>` escaping (a Copy value, Send, storable anywhere,
redeemed via `rack.get(k)?`). `Key` is today's `Handle` with its honest name;
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
`rack.flush_deletes()`; memory is reused when the backlink list drains.
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
they can't be constructed under cycles — so every edge had a `?` site and lazy
covered the whole model uniformly. See
[fourth-option-adversarial.md](fourth-option-adversarial.md), A4. **That kill
was later reversed**: batches give a required cycle a legal transient state, so
`Link<T>` and `Link<T>?` both live and lazy has a non-optional case to answer
for again — see [concurrency](fourth-option-concurrency.md), "Delta to the
earlier docs".)

### Open

The partition pattern — collect refs into a local Vec, then mutate through
them — needs a scope where deletes are locked but field writes are allowed.
`frozen` is too strong (forbids writes). A weaker delete-locked tier is the
one new scope concept this design asks for.

## The Rack is a slab

Worth writing down, because it changes what's available and because two people
have now asked why the container exists at all.

### Why there's a container when the interest is in nodes

Measured on the corpus: 256 node operations (field reads and writes through a
link) against 183 rack operations, and of the rack ones 74 are `insert` and 39
are `len`. Birth and reporting. Field access — the thing programs actually spend
their time on — never mentions the rack:

<!-- test: skip -->
```rask
if e.target? as t { t.health -= e.damage }
```

So the container is nearly invisible in bodies and unavoidably visible in
declarations. It isn't there because anyone wanted a collection. It's the object
that **owns the nodes' lifetime**, and five rules hang off exactly that with
nowhere else to live:

1. **Delete-time edge fixup.** `delete` nulls every `Link<T>?` pointing at the
   node. That needs the reverse-edge index, and the index needs one home per
   graph.
2. **The lifetime rule.** A link may not outlive its rack (E0379). Remove the
   rack and there is nothing for a link's validity to attach to.
3. **`snapshot()`.** Crossing a task boundary means copying the graph, which
   needs a thing to copy.
4. **The frozen graph.** `let g = build()` freezes structure and contents; that
   needs a handle to freeze.
5. **Ownership.** Two racks of one node type are independent — a delete in one
   doesn't touch the other, so two tasks can own two graphs.

The implicit alternative — an ambient arena per node type, so you never name a
rack — breaks all five, and (5) fatally: a per-type arena is a global, so every
graph of a type becomes one ownership unit.

**And a one-node rack is not the smaller version of this.** A one-node graph has
no edges: a single node can only point at itself, so the backlink index has
nothing to maintain. For one value the answer is a plain field, `Owned<T>` if it
needs the heap, or `Shared<T>` if several accessors share it — steps 1 and 4 of
the choice order in `analysis.storage-type-consolidation`. The rack starts at
"they reference each other", which is plural by construction.

### What the backing store already is

<!-- test: skip -->
```
slots: Vec<Option<T>>     // None marks a freed slot
free_list: Vec<u32>
slot_of: HashMap<node, u32>
```

A flat array of slots, a free list, elements that never move. That is a slab, and
it means the objection recorded against merging `Rack` and `Vec` — "contiguity
versus stability" — named the wrong pair. A slab has both. What it gives up is
**density and order**: iteration walks the high-water mark rather than the live
count, and slot reuse doesn't preserve insertion order.

Density largely self-heals: freed slots are reused, so 1000 live nodes still
occupy 1000 slots after 500 deletes and 500 inserts. Order does not, and that is
the honest reason to keep `Vec` separate — a program indexing by position depends
on order, and a slab cannot promise it.

### What that buys, and what it doesn't

`Rack`/`Link` closed the graph cases. What's left of "no stored references" is
narrower than it was:

| case | answer |
|---|---|
| many of one type, contiguous iteration, point at individual ones | a slab-backed rack — this is it |
| a stored iterator (a position in a collection) | hold a `Link` to the element |
| `&v[3]` where `v` stays dense *and* ordered under removal | impossible: density under removal means moving elements |
| a `string_view` into a buffer | out of scope — a sub-range of one value, not an element |

So the residue is sub-ranges of a single buffer. Everything else that used to
want a stored pointer has a name now.

### The open question, pending native

The prototype's slots hold *pointers* to heap-allocated nodes, so today the
contiguity is in the index array only — the nodes themselves are scattered. A
native lowering would want nodes inline in the array, which is where the cache
win actually is. Until that exists, "the rack is a slab" is true in shape and
unproven in payoff.

Two affordances to weigh once it is:

- **Contiguous iteration as a stated guarantee**, so an aggregate reader gets the
  scan a `Vec` promises and a rack currently doesn't.
- **An explicit `compact()`** that regains density by moving nodes and
  re-pointing their backlinks. The fixup rule already exists — *a container that
  stores links and moves them must re-point their backlinks as it moves them* —
  and the cost stays visible because you typed the call.

And one to note rather than propose: a slot index *is* a handle. A slab is the
one structure that can hand out both — a link for checkless traversal, a slot
index as a compact stable name for serialization or an external reference —
without choosing between them. That is the #626 trade ("a handle is a name, a
link is an address") stated as a both-and rather than an either-or.

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
