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

## The short version

The model does what it claims for topology, and the flagship loop really does
lose its staleness branch. But the prototype turned up one thing the analysis
underrates and one thing it misses entirely:

- **Underrated:** the checkless read isn't a property of `Link`. It's a
  property of `Link` *plus* a rule that forbids links in locals — and that rule
  forbids the first line of every program that uses a store. Nobody has written
  the rule that makes both work.
- **Missed:** a link carries write permission with it. Under handles, writing
  needs `mutate pool` at every frame that writes. Under links, holding a link is
  enough. That's a real loss of control, and I had to add a compiler rule to
  make the flagship loop compile at all.

Everything else the analysis says about function and ergonomics held up.

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
| A scalar edge must be `Link<T>?` (E0327) | enforced |
| `inverse(...)`, `@cascade`, `@lazy`, batches, `Key<T>` | not built |
| Borrow rule forbidding links in locals | **not built — see below** |

**Delete follows backlinks, it doesn't scan.** Each node carries the list of
places pointing at it, so a delete visits exactly those. The measured cost is
in-degree, not store size — deleting a node with in-degree 1 out of 500:

```
$ RASK_STORE_STATS=1 rask run --interp sparse.rk
store stats: deletes=1 edges_fixed=1 holders_visited=1
```

A backlink names a holder — a struct field, an edge list, an index — and never a
position, because positions shift under insertion and rehashing. The fixup
re-checks each candidate before rewriting it, which makes the index safe to
*over*-approximate: a backlink left behind after its edge was overwritten costs
one wasted visit, not a wrong answer. A missing backlink would be unsound, so
registration errs toward recording. That asymmetry is why the write path can
stay cheap without an unlink-on-overwrite step.

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

## The finding that matters: links in locals

The model's headline claim is that a dead link cannot exist. In the prototype it
can, and trivially:

```rask
let a = s.insert(Node { name: "a", next: none })
let b = s.insert(Node { name: "b", next: none })
a.next = b
s.delete(b)

if a.next? as t { ... } else { println("a.next = none (fixed)") }   // fixed
println("but the local link still reads: {b.name}")                 // reads "b"
b.health = 99                                                       // and writes
```

Output:

```
a.next = none (fixed)
but the local link still reads: b health=2
and still writes: 99
```

`a.next` was fixed because it lives in a node. `b` is a local, so the fixup walk
never saw it. In the real design that's a use-after-free, not a stale read —
links compile to raw pointers.

The analysis knows about this. Rule 1 says links live only in node fields, and
locals hold block-scoped borrows. The problem is what that means in practice:

**`store.insert()` returns a link into a local. So does every traversal step.**
Every program starts by putting a link somewhere the rule forbids. The rule
can't mean "no links in locals" literally; it has to mean "a link in a local is
a borrow of the store, and `delete` while one is live is a compile error."

That rule is not written anywhere, and it's the load-bearing one. Two things
make it hard:

1. **Rask's borrows are block-scoped for growable sources, and a `Store` is
   growable.** Under block scoping, `let b = s.insert(…)` borrows until the end
   of the block, so `s.delete(b)` in the same block is an error — which is most
   of the programs anyone would write. Making this usable needs last-use borrow
   ends, which Rask deliberately doesn't have, or `with`-scoped access for every
   node touch, which puts the ceremony straight back.

2. **The cost lands exactly where links looked cheapest.** Handles need no such
   rule: a stale handle is *safe*, just false. Handles trade a read check for
   never needing a borrow rule at all. That trade doesn't show up in the
   litmus scorecard, and it should — it's the same trade in a different
   currency.

So the honest statement of the model is: *the read is checkless because the
borrow checker proved no link outlives its node.* The pointer being self-nulling
handles fields; the borrow checker has to handle locals. Only half of that is
designed.

## The other finding: links carry write permission

The flagship loop doesn't compile without a new rule, and the reason is worth
stating plainly. `if e.target? as t { t.health -= e.damage }` binds `t` from an
optional narrowing, and `as` bindings have no `mut` form — so `t` is immutable
and the write is rejected.

There is precedent to follow: writing through a `Handle` already doesn't count
as mutating the handle binding, because the write lands in pool storage. I
extended that to links, which is the same reasoning and more directly true — a
link *is* the node's address.

But the consequence differs. A handle needs `mutate pool` in scope to write
through; permission comes from the container. A link needs nothing; permission
travels with the reference. In L2 that showed up as `mutate` vanishing from
`combat_round`'s signature — which reads like a win, and partly is, but it also
means **there is no read-only link.** Any function you hand a link to can write
to the node. Handles let you hand out read access by not passing the pool
mutably; links have no such distinction.

That's a real capability the model gives up, and I don't see it discussed. If
it needs answering, the answer is probably a separate read-only link type,
which puts a type back on the "day one" page that the census had removed.

## Delete cost, measured

Fan-in sweep, one delete of a hub with N incoming edges:

| N | holders visited | edges fixed |
|---|---|---|
| 50 | 50 | 50 |
| 100 | 100 | 100 |
| 200 | 200 | 200 |
| 400 | 400 | 400 |
| 800 | 800 | 800 |

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
- **Non-optional edges had to become a compile error** (E0327). The adversarial
  pass killed them on constructibility grounds — a cycle needs one side written
  before its target exists. Implementing the fixup gives the same answer from
  the other end: there is nothing to write into a non-optional field when its
  target dies. A bare link stays legal *inside* a container, where delete drops
  the entry rather than nulling it, and that asymmetry is worth stating in the
  eventual spec because it is not obvious from "every edge is optional".
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
them. What it changes is the ranking of what's left open.

Before: the open questions were `inverse` multiplicities, cascade policies,
`@lazy`, and batches — features. After running it, those are all still
accessories. The one that decides whether the model ships is **the borrow rule
for links in locals**, and it isn't on the list at all. It should be first,
because if it can't be made ergonomic under Rask's block-scoped borrowing, the
checkless read isn't real and the whole advantage collapses back to "handles
with extra steps."

Concretely, the next artifact should answer: what is the scope of a link held in
a local, when does `delete` conflict with one, and what does the error say?
Everything else can wait.

## Running it

```
rask run --interp specs/analysis/prototype/l2_targeting_links.rk
rask run --interp specs/analysis/prototype/l2_targeting_handles.rk

RASK_STORE_STATS=1 rask run --interp specs/analysis/prototype/fanin_links.rk
```
