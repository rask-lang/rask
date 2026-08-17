<!-- id: analysis.fourth-option-prototype -->
<!-- status: exploration -->
<!-- summary: Store + Link built for real in the interpreter and run against Pool + Handle on the three litmus programs. The model works; the checkless read is bought with a borrow rule nobody has priced -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-litmus.md, memory/pools.md -->

# Fourth Option: What the Prototype Found

`Store<T>` and `Link<T>` now run on the interpreter. The three litmus programs
are written twice, once each way, and both versions produce identical output.
Everything below comes from running them rather than from reasoning about them.

Programs live in [prototype/](prototype/). Native codegen is not implemented —
this is `rask run --interp` only.

Everything claimed below is asserted in CI (`compile_run.rs`, the `store_link_*`
tests): the semantics, the container-churn cases, the delete-cost numbers, the
documented hole, and the litmus pairs' agreement. Two deliberate mutations —
skipping the unlink, and dropping only the first matching list element —
each fail exactly one of those tests, so they check what they claim to.

## What a link is, and what the store does

Everything below depends on these three paragraphs, so they come first.

**A link is a pointer to a node.** At runtime, literally
`Link { store_id, node: <pointer to the node> }`. Reading `l.health` follows the
pointer and reads the field; that is the entire type. It is not a copy of the
node, not an index, not a ticket. Copying a link copies the pointer — the node is
never duplicated:

```rask
let a = s.insert(Node { id: 1, hits: 0 })
let b = a                 // second pointer to the same node
a.hits += 1
b.hits += 1
// hits through a = 2, through b = 2, store len = 1
```

A `Handle` is the opposite kind of thing: a number (slot + generation). It can't
be followed on its own — the pool turns the number into an address, and checks the
generation while doing it. That check is what the whole model is trying to remove.

**The store does three things.** `insert` puts a node in the store and hands back a
pointer. It writes down every place that holds a pointer to each node — "who points
at me?". And `delete(n)` looks up everyone pointing at `n`, sets each of them to
`none`, then frees `n`. That third step is the model: after a delete, no pointer to
a dead node exists *anywhere the store wrote down*, so following a link needs no
check — it is `none`, or it is live.

**The problem is that last phrase.** The store can write down a link held in a
*field* — `entity.target`, `world.player`, an element of `children` — because it
knows where the field is. It cannot write down a link held in a *local variable*,
because a local is on the stack and the store has no way to name it. So:

```rask
let n = store.insert(...)   // pointer now sits where the store can't see it
store.delete(n)             // fixes every field; cannot touch `n`
n.name                      // follows a pointer to freed memory
```

A link in a local is a pointer the store can't find, so delete can't fix it. That
one sentence is Finding 1, and everything in it is a candidate answer.

A note on vocabulary, because the obvious framing is wrong: assigning a link
*always* copies a pointer, in every position, and never copies the node. So the
choice is not between copying and moving data. It is whether **the compiler keeps
trusting the old variable name** — a field-held link stays trustworthy because the
store maintains it, a local one does not because nothing does. Where this document
says a local link "moves", it means the name is revoked, not that anything was
transferred.

### The rule, derived from the type rather than the mechanism

Everything above argues from what the store can reach. There is a shorter route,
and it gives the same answer from the other end.

A link's type states whether it can be absent. `Link<T>?` says "this may be
nothing"; `Link<T>` says "this always points at a live node". Deleting that node
falsifies the second claim, and there is no `none` to fall back to — so
**using a non-optional link after its target is deleted is not merely unsafe, it
contradicts the type.** A contradiction in the type is something a compiler can
reject.

That is already the rule for fields: E0327 rejects a bare `Link<T>` field because
delete would have nothing to put there. The same sentence applied to locals gives
the locals rule, and which resolution you get depends only on whether the fixup can
reach the slot:

| Where the link lives | Store can reach it | Rule |
|---|---|---|
| A field — `entity.target`, `world.player`, a list element | yes | must be `Link<T>?`; delete nulls it at runtime |
| A local variable | no | must be `Link<T>`; the compiler rejects use after delete |

So the `?` is the signal for which discipline is in force: optional means the store
maintains this slot, non-optional means the compiler guarantees liveness. One
principle — *a link never points at a dead node* — with the mechanism chosen by
reachability.

It also sharpens what "checkless" claims, which is narrower than the phrase
suggests:

```rask
for e in world.nodes() {       // e: Link<Entity> — non-optional, guaranteed live
    if e.target? as t {         // one none-test, at the edge traversal
        t.health -= e.damage    // every read and write through `t` is free
    }
}
```

One test per edge *follow*, then unlimited free access through the local. The
handle version tests at every read — three times in that same loop. So the model
does not remove checks; it moves them from every read to one per traversal, and the
local is trustworthy precisely because delete-then-use won't compile.

A corroboration that this is the right shape: under the compile-time experiment,
`store.contains(n)` on a local link became an error. Correct — if the type
guarantees liveness, asking whether it is live is a question with no meaning.

## The short version

The model does what it claims for topology, and the flagship loop really does
lose its staleness branch. Three findings, in the order they matter:

1. **The checkless read isn't a property of `Link`** — it's `Link` plus a rule for
   links in locals, and that rule is unwritten. It follows from the type, though:
   a local link is non-optional, so it asserts its target is alive, and a delete
   would contradict that with no `none` to fall back to — **so use-after-delete is
   a type contradiction the compiler can reject.** Same sentence that already
   justifies E0327 for fields; the field gets nulled at runtime because the store
   can reach it, the local gets rejected at compile time because it can't. Tried
   it: no runtime check, and the flagship loop passes. The one gap is that affine
   links enrol in the aliasing tracker, which treats them as owned aggregates and
   rejects an ordinary list splice — a bounded fix, not a new analysis.
2. **A link carries write permission, and an edge write mutates its target.**
   There is no read-only link, where a handle gave one for free. The fix has to be
   in the type — `mut Link<T>`, default read-only — because a link escapes the
   context that produced it, so no `frozen`-style mechanism can constrain it.
   Separately, `a.target = b` modifies `b`, a hidden write through what reads as a
   plain assignment.
3. **Representation retires `mem.relocatable`'s founding premise** — links are
   pointers, so zero-copy persistence dies for flat graphs and graph `Encode` has
   to assign ids. Smaller than it sounds: tier A's flatness rule already excluded
   any node with a `string` or a `Vec`, which is two of the three litmus node
   types. Worth stating in the decision, not worth escalating.

Everything the analysis says about function and day-to-day ergonomics held up. The
bill is on those three counts, and only the first one is load-bearing.

## What actually got built

| Piece | Status |
|---|---|
| `Store<T>`: insert, delete, len, is_empty, contains, nodes, clear | works |
| `Link<T>`: field read/write, identity `==`, method calls on the node | works |
| Delete sets scalar edges (`target: Link<T>?`) to `none` | works |
| Delete drops entries from edge lists (`children: Vec<Link<T>>`) | works |
| Delete drops entries from indexes (`by_name: Map<K, Link<T>>`) | works |
| Root edges — link fields on the struct that *owns* the store | works |
| `for n in store` | works |
| Backlinks — each node knows who points at it, delete is O(in-degree) | works |
| Unlink on overwrite — a rewritten field drops its old backlink | works |
| Required edges (`Link<T>`, no `?`) | rejected for now (E0327) |
| `inverse(...)`, `@cascade`, `@lazy`, batches, `Key<T>` | not built |
| Borrow rule for links in locals | **not built — the load-bearing gap, see below** |
| `mut Link<T>` — writability in the type | not built; shape argued below |

**Delete follows backlinks, it doesn't scan.** Each node carries the list of
places pointing at it, so a delete visits exactly those. The measured cost is
in-degree, not store size — deleting a node with in-degree 1 out of 500:

```
$ RASK_STORE_STATS=1 rask run --interp sparse.rk
store stats: deletes=1 edges_fixed=1 holders_visited=1
```

A backlink names a holder — a struct field, an edge list, an index — and never a
position, because positions shift under insertion and rehashing.

A struct field names its slot exactly, so overwriting `a.target` unlinks the old
target's backlink precisely. Rewriting one field fifty times leaves one backlink
on its current target and none on the forty-nine it passed through:

```
$ RASK_STORE_STATS=1 rask run --interp unlink_on_overwrite_links.rk
store stats: deletes=2 edges_fixed=1 holders_visited=1
```

A container backlink names the container and no position, so it is one entry per
(container, target) pair however many elements match. Pop the last element
pointing at T and the entry survives until T is deleted, when the visit finds
nothing — one check, once, because the list is discarded by the delete that read
it. Nothing here grows.

That coarseness is the one place the prototype is less precise than an intrusive
list would be, so it is the part with the most tests rather than the most
argument. `store_link_container_churn.rk` covers every way of removing an edge
without telling the store — pop, remove-by-value, clear, a `filter` that builds
a fresh list, the same target twice in one list, one target in two lists, a list
nested two deep, and an index key overwritten — and the fixup gets all of them
right. The asymmetry costs a wasted check; it does not cost correctness.

Registration and unlinking are both O(1): the index is a map keyed by slot, not
a list to scan. Building a hub of in-degree 25,600 is linear.

**A node field and a root field are the same thing to this code.** That fell
out of keying backlinks on the holder rather than on "is it inside the store":
`world.player` and `entity.target` are both a struct field holding a link. Root
edges needed no separate mechanism, which is a small point in the design's
favour — the analysis treats them as an addition, and they aren't one.

## What the programs show

### The staleness branch really does stop existing

L2, the flagship. Handles:

```rask
func combat_round(mutate world: Pool<Entity>) {
    for h in world {
        if world[h].target? as t {
            if world.get(t)? as _alive {
                let dmg = world[h].damage
                world[t].health -= dmg
            } else {
                world[h].target = none      // silent to forget
            }
        }
    }
}
```

Links:

```rask
func combat_round(world: Store<Entity>) {
    for e in world.nodes() {
        if e.target? as t {
            t.health -= e.damage
        }
    }
}
```

Both print the same thing. Three things went away, not one: the liveness check,
the cleanup branch, and `mutate` on the parameter. The last one is worth
noticing — the link version doesn't need the store to be mutable because it
isn't writing to the store, it's writing to a node it already holds.

### Delete-time fixup does the bookkeeping the handle version writes by hand

In L3 the subtree delete never says "remove this node from its parent's
children list" and never says "clear the selection if it pointed here". Both
happen:

```
-- select a2, then delete the subtree at a
   root
     a
       a2
     b
       a1
   selected = a2, nodes = 5
   root                            <- `a` is gone from root.children
     b
       a1
   selected = none, nodes = 3      <- selection cleared itself
```

The handle version writes both by hand, and `report` re-validates the selection
at every read because it might name a removed node.

### But it maintains referential integrity, not your invariants

L1's list splice is the same length both ways. Nothing points at a dead node
after a delete either way; joining the neighbours is domain logic and stays
yours. What links remove from `remove` is the head/tail nulling — those are
root edges, so they fix themselves.

Same for reparenting: without compiler-maintained `inverse`, L3's `reparent` is
three statements in both versions. The one-assignment reparent the litmus
advertises needs `inverse`, which is a separate feature and isn't built here.

### Line counts

Code lines, comments and blanks excluded:

| Program | Handles | Links |
|---|---|---|
| L1 doubly-linked list | 68 | 66 |
| L2 entity targeting | 56 | 49 |
| L3 scene tree | 80 | 66 |

The whole-program gap is smaller than the hot-loop gap, which is what the
litmus predicted: the ceremony concentrates on reference-following paths, and
the rest of a program doesn't care which model it's under.

## Finding 1, load-bearing: the locals rule, and what it costs

The model's headline claim is that a dead link cannot exist. In the prototype it
can, and trivially — `prototype/stale_link_hole.rk`:

```
a.next = none (fixed)               <- an edge inside a node
local link still reads: b health=2  <- a link in a local
and still writes: 99
```

`a.next` was fixed because it lives in a node. `b` is a local, so no backlink
records it. With raw pointers that is a use-after-free.

So the checkless read is not a property of `Link`. It is `Link` **plus** a rule
that makes a local link a live borrow of the store, with delete-while-borrowed a
compile error. That rule is the load-bearing piece, and it is unwritten.

### Why the two obvious statements are both poison

Under block-scoped borrows a local link borrows to end of block, so
`let b = s.insert(…)` then `s.delete(b)` in the same block is an error — the
first program anyone writes. The two ways out are both things Rask chose against:

- **Last-use borrow ends** is NLL: the lifetime-shaped, non-local analysis this
  language was founded to refuse. "Code that looks fine explodes twenty lines
  later" is the argument Rask exists to answer.
- **`with`-scoped access on every node touch** puts back exactly the ceremony
  the model exists to delete, and takes the flagship loop with it.

Underneath is the pattern the litmus scorecard misses: **complexity is
conserved.** Handles pay at read time, with a runtime check. Links pay at compile
time, with an aliasing discipline. Rask already picked its side of that trade
deliberately, so "links are strictly better" can only be true if the aliasing
discipline is free — and it isn't.

### There is a third statement, and it is livable

Neither escape is needed, because traversal already obeys the rule: `if x? as n`
binds a *borrow*, not a link value. The only place a link escaped into a local
was `insert`'s return. So state it there:

> An `insert` result must be stored into a field. Links are fields and borrows,
> never local values.

`prototype/l1_list_links_no_locals.rk` is L1 written that way, and it produces
byte-identical output to the handle version. `push_back` stores the new node's
link straight into a field and reads it back out for the second use:

```rask
func push_back(mutate list: List, value: i32) {
    if list.tail? as t {
        t.next = list.nodes.insert(Node { value: value, next: none, prev: list.tail })
        list.tail = t.next                       // read it back from the field
    } else {
        list.head = list.nodes.insert(Node { value: value, next: none, prev: none })
        list.tail = list.head
    }
}
```

It costs the ability to keep what you inserted, though: `push_back` can no longer
return the new node, so acting on it later means searching for it — `O(n)` where a
stashed handle was `O(1)`. That contradicts the census's "effectively 100% of
handle uses are topology, none would need a `Key`": under this statement, any
insert-then-act-on-that-node code needs a search or a key.

### A fourth statement: let the delete fix the variable too

The three above all assume the fixup can't reach a local. It can — a local lives
on the stack so no backlink can name it, but the delete can walk the live bindings
instead, which is what a precise collector does with its roots.
`null_local_links` implements it behind `RASK_STORE_TRACK_LOCALS=1`:

```
$ rask run --interp stale_link_hole.rk                      # default
local link still reads:  name=b health=2                    <- use-after-free

$ RASK_STORE_TRACK_LOCALS=1 rask run --interp stale_link_hole.rk
store stats: … locals_nulled=1
error[R0005]: cannot access field on enum                    <- `b` became none
```

No borrow rule at all, and stashing keeps working
(`prototype/l1_list_links_tracked_locals.rk`). But a local link can now be
emptied, so it is `Link<T>?` and reading one costs a none-test — and the shape
that produces is odd:

```rask
let n2 = push_back(list, 2)      // just made it
if n2? as n { remove(list, n) }  // …and immediately have to ask if it's there
```

That check is real — a delete elsewhere could have emptied it — but it reads as
ceremony at the point where the answer is obviously yes.

### The fifth, and the one to build: track the deletes at compile time

The check above is only needed because the compiler doesn't know whether a delete
has happened. Within one function body it *does* know — every `delete` is right
there in the source. So make the delete consume the link and let the existing move
checker do the rest. This is not NLL: the invalidation point is a statement you
wrote, not a last use the compiler inferred.

Tried it, and most of the machinery is already in place. Three changes:

1. `delete(mutate self, take link: Link<T>)` — an existing parameter mode.
2. `Link<T>` stops being `Copy`, so it is affine among locals.
3. Assigning a link **into a field** leaves the source name usable, while
   assigning into another local revokes it. Both copy the same pointer; the
   difference is who maintains it afterwards — the store maintains a field, and
   nothing maintains a local.

With those, use-after-delete becomes a compile error and no runtime check remains:

```
error[E0800]: use of moved value: `b`
 29 |     store.delete(b)
    |                  - value moved here
 39 |     println("{b.name}")
    |              ^ value used here after move
```

Two other things fell out, both good: a second `insert` while a link is live is
fine (nodes are individually allocated, so an insert invalidates nothing), and an
ordinary non-`take` parameter borrows rather than consumes, so read-only passing
works unchanged.

**Where it stands, measured.** Across the ten link programs and suite files:

| Result | Count |
|---|---|
| Pass unchanged — including the flagship L2, fan-in, sparse, unlink, churn | 5 |
| Fail on aliasing (E0801) | 18 errors across 3 files |
| Fail on closure capture (E0813) | 1 |
| Correctly rejected use-after-delete | 1 |

The last row is the feature working: `s.delete(b)` followed by `s.contains(b)` is
now an error, and the test asserting the old behaviour is what caught it.

**The one gap.** All 18 aliasing errors are a single shape — the list splice:

```rask
if n.prev? as p { p.next = n.next }
```
```
error[E0801]: cannot write to `n` while it is being read
```

Making a link affine enrols it in the borrow/alias tracker, and that tracker
treats it like an owned aggregate: reading `n.next` borrows `n`, and writing
through `p` (derived from `n.prev`) looks like a conflicting write to the same
value. But a link is a *reference* — projecting through one should not borrow the
link. Until the tracker knows that, ordinary graph manipulation is rejected.

**What the proposal owes, revised.** Not "show the rule can be stated without
either escape" — three statements work, and this is the one to build. It owes one
bounded piece of compiler work: teach the aliasing checker that a link is a
reference. Not a new analysis, and not a language-shaped concession. None of the
three statements appears in the design documents.

The experiment is reverted on this branch, because it breaks five working
programs. It is three edits to reproduce: `take` on `delete` in
`stdlib/memory.rk`, dropping `"Link"` from `is_copy` in
`rask-ownership/src/lib.rs`, and skipping `handle_assignment` when a link is
assigned into a field.

## Finding 2: links carry write permission, and edge writes mutate their target

Two things, both about a link being more powerful than it looks.

**There is no read-only link.** Hold one and you can write the node. A handle
gives read-only access for free — don't pass the pool mutably and nothing can
write through it. The link version of L2 lost `mutate` from `combat_round`'s
signature, which reads like a win and is half of one: the other half is that
nothing *can* be marked read-only any more.

**The fix has to be in the type.** Two weaker answers look tempting and both fail.

*Not a context.* Rask's existing rule is that read-only comes from the source —
the parser says so when it rejects `with c as mut inner`: "bindings are mutable;
read-only access comes from the source (`.read()`, frozen pools)". Applied to
links that would mean a read-only store, which is what `using frozen Pool<T>` was.
But a link outlives the context that produced it, so the context cannot constrain
it:

```rask
// `s` is not `mutate` — this function has read-only access to the store
func find_first(s: Store<Node>) -> Link<Node>? {
    for n in s.nodes() { return n }
    return none
}

if find_first(s)? as n { n.id = 99 }     // writes the node anyway
```

That runs, and prints 99. Whatever permission the store parameter carried is gone
by the time the link is used, which is the difference between a link and a handle:
a handle is inert without its pool, so restricting the pool restricts the handle. A
link needs nothing.

*Not the binding.* Making the write depend on `mut` at the binding doesn't work
either, and the reason exposes an inconsistency that exists today independent of
links: `with c as inner { inner.n += 1 }` is accepted, `if opt? as t { t.n += 1 }`
is rejected as "cannot mutate `t` — declared `let`". Same `as`, opposite
mutability — filed as [#788](https://github.com/rask-lang/rask/issues/788). Even if that were settled, a binding is local — it says nothing about
the link a function hands back.

So writability belongs on the type, which is what "why not a mutable `Link`?"
proposes. Following Rask's own defaults — parameters read-only until `mutate`,
bindings immutable until `mut` — that reads:

```rask
struct Entity {
    target: mut Link<Entity>?        // writes allowed through this edge
    home:   Link<Region>?            // read-only through this one
}

func report(e: Link<Entity>)         // cannot write the node
func damage(e: mut Link<Entity>)     // can
```

The cost is a `mut` on every edge you write through, which is visible at the
declaration and at every signature — the legibility that a context-based answer
can't provide, since nothing at the use site would say the link is restricted. It
is one modifier, not the extra type `ReadLink<T>` would have been.

**`a.target = b` writes to `b`.** Registering the backlink mutates the target, so
an assignment that reads as touching `a` also modifies `b`. In the real design
that is a store into `b`'s intrusive-list header. In this prototype it is a write
to a third object neither name mentions — the store's backlink index — which is
arguably the worse of the two for reading. Either way, Transparency of Cost
should be made to bless this explicitly rather than inherit it: an edge write is
not the integer copy a handle write is, and the litmus already prices it at ~4–8
stores against 1–2.

## Finding 3: the #626 trade is real but smaller than it first looks

`fourth-option.md` decides representation in a sentence: links compile to raw
pointers, and `mem.relocatable` "stays a keys-only feature," declined because the
relocatability story is "narrow in practice."

The premise being retired is load-bearing on its face.
[mem.relocatable](../memory/relocatable.md) opens with it:

> Rask's "no storable references" design means user-visible types contain only
> owned values and integer handles — **never pointers**. This makes pool state
> relocatable.

And [#626](https://github.com/rask-lang/rask/issues/626) defines its top tier
around handles specifically: `Persistable` is "pointer-free data — primitives,
**handles**, enums/structs of those," with snapshot semantics "handles must
survive the round-trip and stale handles must stay stale." Links are pointers with
no generations, so there is no link analogue of that property.

**But how much did that tier actually cover?** Less than "graphs," which is the
correction. `mem.relocatable`'s tier-A is *Flat*, and FL1 excludes `string`, `Vec`,
`Map`, `Cell`, `Shared`, `Mutex`, trait objects and closures — recursively. So of
the litmus programs:

| Node type | Flat? |
|---|---|
| `Node { value: i32, next: Handle?, prev: Handle? }` | yes — tier A |
| `Entity { name: string, health: i32, target: Handle? }` | no, `string` |
| `SceneNode { name: string, children: Vec<Handle> }` | no, `string` + `Vec` |

Two of three realistic graph nodes were already out of tier A before links were
proposed. What links actually cost is the remaining slice: mmap and bitwise copy
for graphs whose nodes are primitives and references only — an intrusive list or
tree of numbers. Real, and narrower than a north-star bet.

Everything else survives with more work rather than less capability. Tier
R2/*Deep* — and #626's tier C — is a traversal that walks heap contents; for links
it has to assign ids during the walk instead of copying integers straight out.
That is an implementation cost in `Encode`, not a lost capability.

So the honest version: **adopting links costs zero-copy persistence for flat
graphs, and makes graph `Encode` do id assignment.** Worth stating in the
representation decision rather than leaving as "narrow in practice", because the
reason it is narrow is FL1, not the feature's importance — but it does not need to
be escalated to the reliability direction the way I first wrote it.

## Delete cost, measured

Fan-in sweep, one delete of a hub with N incoming edges:

| N | holders visited | edges fixed |
|---|---|---|
| 50 | 50 | 50 |
| 100 | 100 | 100 |
| 200 | 200 | 200 |
| 400 | 400 | 400 |
| 800 | 800 | 800 |
| 25600 | 25600 | 25600 |

Exactly linear in in-degree, exactly as predicted, and independent of store
size — a node with in-degree 1 in a 500-node store visits one holder. The interesting half is the handle
comparison. `pool.remove(hub)` is O(1) — and then:

```
fan-in = 200, nodes = 201
after removing the hub, nodes = 200
stale handles found at read time = 200
```

Two hundred units of fixup work either way. Links do it at the delete; handles
do it at the readers, spread out, forever, and only if somebody writes the
cleanup branch. This is the impossibility theorem from the main document,
observed rather than argued: every incoming edge is either written once or
checked at every read.

Nothing here says which is faster in wall-clock terms. A tree-walking
interpreter can't answer that — its per-operation overhead swamps the
difference, and its handle path searches the environment for the pool, which no
real backend does. **The read-path performance claim remains unmeasured.** It
needs native codegen.

## Smaller things worth recording

- **Root edges are needed by all three litmus programs**, not just L1. `head`/
  `tail`, `selected`, and the `by_name` index are all links living outside the
  store. Whatever schema closure answers "who can point at `T`?" has to cover
  every struct that can hold a link, not just node types.
- **The specs disagree about whether required edges exist, and the entry point
  is the stale one.** The adversarial pass killed `Link<T>` without `?` on
  constructibility grounds (A4: a cycle needs one side written before its target
  exists). [concurrency](fourth-option-concurrency.md) then *reversed* that,
  conditional on batches — a staged batch gives the cycle a legal transient
  state, so "`Link<T>` and `Link<T>?` both live". But
  [fourth-option.md](fourth-option.md) still asserted the kill, which is the
  document a reader starts from. Fixed there, with a pointer to the reversal.

  Implementing the fixup adds a second requirement the reversal doesn't mention:
  a required edge needs a **delete policy**, because there is no `none` to set it
  to when its target dies. So cascade/restrict stops being deferrable the moment
  required edges are admitted — the decision table said "set-to-`none` only,
  cascade and restrict deferred", and those two lines can't both hold. Noted in
  the table.

  The prototype rejects required edges (E0327) for exactly those two missing
  pieces, and the diagnostic says so rather than claiming a language rule. A bare
  link stays legal *inside* a container, where delete drops the entry rather than
  nulling it — an asymmetry worth stating in the eventual spec, because it does
  not follow from either position on required edges.
- **Links have to be Copy**, like handles. An edge written into two fields is
  two edges, not a moved one.
- **Identity comparison is pointer equality.** `c == n` on links compares nodes,
  which is what edge-list removal needs.
- **The mutability check had to be deferred** past constraint solving. A link
  bound by optional narrowing is still a type variable during the statement
  walk, so the check couldn't tell a link write from a `let` violation. The same
  gap existed for handles and was invisible because nothing exercised it.

## Bugs found on the way

- [#768](https://github.com/rask-lang/rask/issues/768) — an `own` closure
  capturing a Copy *parameter* inside a branch is wrongly reported as
  maybe-moved. Hit while writing the handle version of L3; unrelated to this
  work.
- [#788](https://github.com/rask-lang/rask/issues/788) — `with x as v` and
  `x? as v` disagree on whether the binding is mutable, and the diagnostic for the
  second suggests a fix that doesn't parse. Found while checking whether a binding
  mode could express read-only links.

## Where this leaves the design

The parts the analysis called decided are decided, and the prototype supports
them: delete-time fixup works, root edges need no separate machinery, and the
flagship loop really does lose its dance.

What changes is the ranking of what's left. Before, the open list was `inverse`
multiplicities, cascade policies, `@lazy` and batches — features. After running
it, those are all accessories. Three things outrank them, and none was on the
list:

1. **The locals rule** — adopt compile-time delete tracking (`delete` consumes the
   link; links affine among locals; links copy into fields), and fund the aliasing
   change it needs. The two runtime alternatives are worse and are described above
   for comparison. None of the three is written down anywhere yet.
2. **Read-only links** — put writability in the type (`mut Link<T>`, default
   read-only, matching `let`/`mut` and parameter modes). Not a context: a link
   escapes the context that made it, demonstrated above. Not a binding mode: local,
   and `with … as` versus `? as` don't even agree on mutability today.
3. **The representation note** — say plainly that flat-graph zero-copy
   persistence is what's being traded away, and that graph `Encode` gains an id
   assignment pass. No escalation needed; the tier was already narrow.

Two smaller ones the fixup surfaced: required edges need a delete policy the
moment batches admit them (cascade/restrict stops being deferrable), and edge
writes need Transparency of Cost to bless them explicitly.

## Running it

```
rask run --interp specs/analysis/prototype/l2_targeting_links.rk
rask run --interp specs/analysis/prototype/l2_targeting_handles.rk

RASK_STORE_STATS=1 rask run --interp specs/analysis/prototype/fanin_links.rk
```
