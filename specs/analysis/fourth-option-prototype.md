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

## The short version

The model does what it claims for topology, and the flagship loop really does
lose its staleness branch. Three findings, in the order they matter:

1. **The checkless read isn't a property of `Link`** — it's `Link` plus a rule for
   links in locals, and that rule is unwritten. Two obvious statements are things
   Rask chose against (NLL; `with`-everywhere). Two work. The better one is not in
   any design document: **a variable holding a link is a link, so let the delete
   fix it too.** Implemented and demonstrated here. It needs no borrow rule, keeps
   stashing, and costs one none-test per *local* read while edges stay checkless —
   complexity still conserved, but landing on the cold path instead of the hot one.
2. **A link carries write permission, and an edge write mutates its target.**
   There is no read-only link, where a handle gave one for free. The fix isn't a
   new `ReadLink<T>` type: Rask already rules that read-only comes from the
   *source*, not the reference — and the proposal deletes `frozen`, which was that
   mechanism. Separately, `a.target = b` modifies `b`, a hidden write through what
   reads as a plain assignment.
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
| `ReadLink<T>` — a link that can't write its node | not built; not designed |

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

### And a fourth statement, which is better than all three

The three above all assume the fixup *can't* reach a local. But a variable holding
a link is a link. Nothing stops the delete from fixing it too — a local lives on
the stack, so no backlink can name it, but the delete can walk the live bindings
instead. That is what a precise collector does with its roots.

`null_local_links` in `store.rs` implements it, behind `RASK_STORE_TRACK_LOCALS=1`
so the default keeps demonstrating the gap. It closes the hole with **no borrow
rule at all**:

```
$ rask run --interp stale_link_hole.rk                      # default
local link still reads:  name=b health=2                    <- use-after-free

$ RASK_STORE_TRACK_LOCALS=1 rask run --interp stale_link_hole.rk
store stats: … locals_nulled=1
error[R0005]: cannot access field on enum                    <- `b` became none
```

`prototype/l1_list_links_tracked_locals.rk` is L1 under it: stashing works again,
and using a stashed link takes one unwrap.

```rask
let n2 = push_back(list, 2)      // keeps working — returns Link<Node>?
if n2? as n { remove(list, n) }  // one unwrap, and that is the whole ceremony
```

**What it costs, stated exactly.** A local link can be emptied by a delete, so it
is `Link<T>?` and every read of one is a none-test. That is the check handles
pay — but handles pay it on *every* reference, and this pays it only on locals.
Edges inside nodes stay checkless, and traversal goes through edges, so the
flagship loop is untouched. Complexity is still conserved, but the split is
favourable: the check lands on the cold path (you just inserted something) instead
of the hot one (walking the graph).

Two consequences worth naming before anyone adopts this:

- **`delete(n)` nulls `n`.** Including a parameter you were just handed —
  `func kill(mutate s: Store<Node>, n: Link<Node>) { s.delete(n); n.id }` fails on
  the second statement. Safe, and not alien to Rask (it is what `take` does), but
  it is use-after-*null* rather than use-after-free: still a logic bug you can
  write, just not a memory-safety one.
- **The scan is the prototype's shortcut, not the model's cost.** Walking every
  live binding per delete is O(bindings). A real implementation registers stack
  slots as they are created — O(1) per local link, O(locals pointing here) at
  delete, the same shape as backlinks.

**What the proposal owes, revised.** Not "show the rule can be stated without
either escape" — it can, twice over. It owes a choice between the two workable
statements: insert-results-go-in-fields (no check anywhere, but no stashing and
`Key<T>` returns), or track-the-locals (stashing works, one none-test per local
read, `delete` empties its argument). The second looks better on every axis I can
measure, and it is the one the design documents don't mention.

## Finding 2: links carry write permission, and edge writes mutate their target

Two things, both about a link being more powerful than it looks.

**There is no read-only link.** Hold one and you can write the node. A handle
gives read-only access for free — don't pass the pool mutably and nothing can
write through it. The link version of L2 lost `mutate` from `combat_round`'s
signature, which reads like a win and is half of one: the other half is that
nothing *can* be marked read-only any more.

The fix is **not** a `ReadLink<T>` — Rask has already ruled on where read-only
comes from, and the parser says so out loud. Writing `with c as mut inner` is
rejected with:

```
error: with-bindings take a bare name
  = fix: bindings are mutable; read-only access comes from the source
         (`.read()`, frozen pools) — write `as name`
```

So read-only is a property of *how you obtained the thing*, never of the reference
type. Applied to links, that means a read-only **store** — which is exactly what
`using frozen Pool<T>` was, and which
[fourth-option.md](fourth-option.md) deletes: "Also gone, though they aren't
types: `using Pool<T>` context clauses, **`frozen`**, and the generation-coalescing
compiler pass."

The proposal removes the mechanism that answers this question and then has no
answer. Keeping a frozen-equivalent on `Store<T>` costs no new type and no new
concept; adding `ReadLink<T>` costs both, and contradicts the rule the error
message states.

I had to add a compiler rule for this to work at all. `if e.target? as t { … }`
binds `t` immutably and `as` has no `mut` form, so the flagship loop doesn't
compile unless writing through a link is defined as not-mutating-the-binding.
There is precedent — a handle write already lands in pool storage, not the
handle — but the handle case borrows its permission from `mutate pool`, and the
link case has nothing to borrow it from.

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

## Where this leaves the design

The parts the analysis called decided are decided, and the prototype supports
them: delete-time fixup works, root edges need no separate machinery, and the
flagship loop really does lose its dance.

What changes is the ranking of what's left. Before, the open list was `inverse`
multiplicities, cascade policies, `@lazy` and batches — features. After running
it, those are all accessories. Three things outrank them, and none was on the
list:

1. **The locals rule** — pick between the two statements that work.
   Insert-results-go-in-fields costs stashing and brings `Key<T>` back;
   track-the-locals keeps stashing, costs a none-test per local read, and makes
   `delete(n)` empty `n`. Measured, the second wins on every axis. Neither is
   written down anywhere yet.
2. **Read-only links** — keep a `frozen`-equivalent on `Store<T>` (the mechanism
   the proposal deletes), rather than adding the `ReadLink<T>` type Rask's own
   read-only rule argues against.
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
