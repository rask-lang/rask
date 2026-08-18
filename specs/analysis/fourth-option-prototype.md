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

| Where the link lives | Who keeps it honest | Rule |
|---|---|---|
| A field — `entity.target`, `world.player`, a list element | the store nulls it | must be `Link<T>?` |
| A local, parameter or return | the compiler rejects use-after-delete | `Link<T>` is fine |
| A module-level `const` | **nobody** | should be rejected; currently isn't |

So the `?` is the signal for which discipline is in force: optional means the store
maintains this slot, non-optional means the compiler guarantees liveness. One
principle — *a link never points at a dead node* — with the mechanism chosen by
reachability.

Function parameters and return types are locals for this purpose, and already work
unchanged: `func look(n: Link<Node>) -> i32` and
`func first(s: Store<Node>) -> Link<Node>?` both run today, because the caller's
and callee's bodies are exactly where the compiler can see the deletes.

**A third position has neither keeper, and the prototype accepts it.** A
module-level `const` is outside every function body, so no flow analysis reaches
it, and it is not a field, so the store never wrote it down:

```rask
const ROOT: Link<Node> = Store.new().insert(Node { id: 1 })
func main() { println("{ROOT.id}") }        // typechecks, prints 1
```

Worse than dangling-after-delete: the store here is a temporary that is gone by the
time `main` runs, so `ROOT` points into a store that no longer exists. The
prototype survives only because its nodes are refcounted; with the raw pointers the
design specifies, this dangles from program start. The rule the table implies —
*a link may live only where the store or the compiler can reach it* — forbids
const links, and nothing currently enforces that.

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

### What a required edge in a field would need

The table forbids a non-optional link in a field, which is E0327 today. Making one
legal takes two things, and neither is `inverse`.

**To destroy it: a delete policy — cascade or restrict.** When the target dies,
delete has to do *something* with a field that cannot hold `none`. Either works,
and one is enough:

- **restrict** — the delete fails while a required edge points at the target. A
  required edge becomes an ownership claim: you cannot delete the `Body` while an
  `Entity` declares it needs one.
- **cascade** — the delete propagates to the holder, so the field never outlives
  its target.

`inverse` does not help here and is a separate concern. It keeps two edges in sync
(a `parent` and a `children` list naming each other); it says nothing about what
goes in a required field when the target dies. Delete a parent and the child's
`parent: Link<Node>` still has nothing to write — cascade is what saves that, not
the inverse declaration.

**To construct it: batches.** A required cycle cannot be built one field at a time,
which is what killed required edges in the adversarial pass (A4) before batches
reversed it. Both halves are needed, which is why the decision table's
"set-to-`none` only, cascade and restrict deferred" cannot stand alongside
admitting `Link<T>`.

## The short version

The model does what it claims for topology, and the flagship loop really does
lose its staleness branch. Three findings, in the order they matter:

1. **The checkless read isn't a property of `Link`** — it's `Link` plus a rule for
   links in locals. That rule follows from the type: a local link is non-optional,
   so it asserts its target is alive, and a delete would contradict that with no
   `none` to fall back to — **so use-after-delete is a type contradiction the
   compiler can reject.** Same sentence that justifies E0327 for fields, resolved
   the other way because the store can reach a field and can't reach a local.
   **Implemented**: `delete` takes the link, the move checker reports the use, no
   runtime check anywhere, and all thirteen comparison programs pass.
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
| Locals rule — use-after-delete is a compile error | **works** (E0800) |
| Deletes the compiler can't see (a call that deletes inside) | not built |
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

### The rule, implemented

Use-after-delete is now a compile error, and no runtime check was added anywhere:

```
error[E0328]: `b` names a deleted node — this is a use after free
 23 |     store.delete(b)
    |                  - the node `b` names was deleted here
 25 |     println("{b.name}")
    |               ^ `b` points at freed memory from here on
    = fix: move the reads above the `delete`, or store the link in a `Link<T>?` field
```

**The move checker is the mechanism, not the concept.** `delete` takes the link,
so the existing move machinery does the tracking — that reuse is why this was four
small changes instead of a new analysis. But nothing moves: the pointer stays where
it is and the thing it points at is freed, so every name for that node dies at
once. That is a use after free, proven rather than checked.

Reporting it as a move would be wrong, and dangerously so — the generic move
advice is "add `.clone()`", which for a link hands back a second dead pointer. So
`MoveReason::LinkDeleted` splits the diagnostic off (E0328) while sharing all the
detection.

It is not lifetime inference either. The invalidation point is the `delete`
statement in the source, not a last use the compiler worked out, so the analysis
stays inside one function body and needs no annotations.

Four changes, three of them one-liners against machinery that already existed:

1. `delete(mutate self, take link: Link<T>)` — an existing parameter mode.
2. `Link<T>` is no longer `Copy`, so the move checker tracks which names are dead.
3. Assigning a link **into a field** leaves the source name usable; assigning into
   another local revokes it. Both copy the same pointer — the difference is who
   keeps it honest afterwards, and only a field has the store doing that.
4. Reading a link *out of* a field borrows nothing, and capturing one in a closure
   does not scope-limit the closure.

The fourth is the one that took finding. Making a link affine enrolled it in the
borrow tracker, which treated it as owned data: reading `n.next` looked like an
exclusive borrow of `n` (right when the field is owned data being moved out, wrong
when it is a pointer being copied), so the ordinary splice
`if n.prev? as p { p.next = n.next }` collided with the borrow the `if` already
held. The same mistake rejected `children.filter(|c| c != n)`. Both dissolve once
the tracker knows a link read is a pointer copy — which is what `is_copy` already
tells it about integers, for the same reason.

All thirteen comparison programs and both suite files pass under this, including
the flagship L2 and the L1/L3 pairs that still match the handle versions
byte-for-byte.

**Two runtime alternatives were tried first and are worse.** Requiring an
`insert` result to be stored into a field (`prototype/l1_list_links_no_locals.rk`)
works but means you cannot keep a reference to what you inserted, so acting on it
later becomes a search and `Key<T>` returns to ordinary code. Letting the delete
null local variables too — a precise collector's root scan — also works, but makes
every local link optional, so the code reads
`let n2 = push_back(list, 2)` immediately followed by `if n2? as n`, asking whether
something you just made exists. Both are checks at runtime for something the
compiler can prove. Neither is kept.

**What it costs.** `delete(n)` revokes `n`, including a parameter just handed in,
so `func kill(mutate s: Store<Node>, n: Link<Node>) { s.delete(n); n.id }` is
rejected on the second statement. That is the rule working, but it means a function
that deletes has to say so by taking the link, and callers see their name go away.

**`clear` had to be handled separately, and that closed the stdlib.** `delete`
takes its link, so the rule falls out of ordinary argument consumption. `clear`
names no link at all and deletes every node, so nothing consumed anything and
this compiled and printed `a`:

```rask
let n = s.insert(Node { name: "a", peer: none })
s.clear()
println("{n.name}")        // read of a freed node, no diagnostic
```

`clear` now revokes every local link whose element type matches the store's,
which is the same kill the explicit `delete` performs, just for all of them at
once. Conservative in one direction — two stores of the same node type revoke
each other's locals — and worth fixing only if that pattern shows up.

That leaves nothing else in the store's API that deletes: `insert`, `len`,
`is_empty`, `contains` and `nodes` invalidate nothing, and `insert` in particular
must stay exempt, since nodes are allocated individually.

**What it does not cover: a user function that deletes inside.** The store is
passed mutably, the link never is, so the caller's name survives a delete the
compiler cannot see:

```rask
func cleanup(mutate s: Store<Node>) {
    for n in s.nodes() { s.delete(n) }    // re-derives links from the store
}

let n = s.insert(Node { name: "a", peer: none })
cleanup(s)
println("{n.name}")        // still compiles, still reads a freed node
```

The obvious conservative rule — passing a store mutably revokes local links into
it — is not available, and the flagship list is the counterexample. `push_back`
takes `mutate list: List`, and `main` calls it four times while holding `n1..n4`:

```rask
let n1 = push_back(list, 1)
let n2 = push_back(list, 2)    // would revoke n1 under that rule
```

So the rule would have to distinguish a callee that deletes from one that only
inserts — a "may delete this store" fact propagated through the call graph. That
is an effect, and enforcing it is what principle 5 says Rask doesn't do. The gap
is genuine and closing it costs more than it has been shown to be worth.

Two things narrow it in practice without any analysis. A function that deletes
can take the link (`func kill(mutate s: Store<Node>, take n: Link<Node>)`), which
puts the revocation back where the checker can see it. And a link kept in a
`Link<T>?` field instead of a local is nulled by the delete itself, wherever the
delete happens — the runtime fixup has no such gap, because a field is an edge
the store indexes.

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

1. **The locals rule is settled and built** — compile-time delete tracking. What
   remains is the case the compiler can't see: a call taking the store mutably that
   deletes inside. Rask's exclusivity rule is the right shape; `insert` stays
   exempt.
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
