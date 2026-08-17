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

1. **The checkless read isn't a property of `Link`.** It's `Link` plus a borrow
   rule for links in locals, and that rule is unwritten. Both obvious ways to
   state it are things Rask chose against — NLL, or `with`-everywhere. There is a
   third way and it works, but it costs you the ability to keep a reference to
   what you just inserted, which brings `Key<T>` back into ordinary code. The
   pattern underneath: **complexity is conserved.** Handles pay at read time with
   a runtime check; links pay at compile time with an aliasing discipline. Rask
   picked its side of that trade once, on purpose.
2. **A link carries write permission, and an edge write mutates its target.**
   There is no read-only link, where a handle gave one for free. And
   `a.target = b` modifies `b` — a hidden write through what reads as a plain
   assignment.
3. **This bets against [#626](https://github.com/rask-lang/rask/issues/626).**
   Links are pointers, so `mem.relocatable`'s founding sentence stops being true
   and tier-A zero-copy persistence dies for anything with edges. That trade is
   made in a one-line representation footnote and belongs to whoever owns the
   reliability direction.

Everything the analysis says about function and day-to-day ergonomics held up.
What it underprices is the bill on the other three counts.

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

So the rule is statable without NLL and without `with`-everywhere. Good — but it
relocates the cost rather than removing it, and where it lands is the finding:

**You cannot keep a reference to what you just inserted.** `push_back` can no
longer return the new node, so acting on a specific node later means finding it
again:

```rask
let n2 = push_back(list, 2)     // ordinary version: O(1) later access
remove(list, n2)

push_back(list, 2)              // no-locals version: no reference handed back
remove_value(list, 2)           // so: walk the list, O(n)
```

That is a third currency for the same conserved complexity — not a read check,
not a borrow rule, but a search. And it lands squarely on the census's claim
that "effectively 100% of handle uses are topology — none would need a `Key`."
Under the locals rule, *any* code that inserts a node and later acts on that
specific node needs a search or a key. L1's `main` is exactly that shape, and it
needed the search. `Key<T>` comes back into ordinary in-process code, not just at
the serialization boundary.

**What the proposal owes:** the locals rule written down in the third form, with
the search-or-key cost admitted, and a judgement on whether that cost is smaller
than the read check it replaces. Until then the model has not met the language's
own founding constraint — it has only moved where the constraint bites.

## Finding 2: links carry write permission, and edge writes mutate their target

Two things, both about a link being more powerful than it looks.

**There is no read-only link.** Hold one and you can write the node. A handle
gives read-only access for free — don't pass the pool mutably and nothing can
write through it. The link version of L2 lost `mutate` from `combat_round`'s
signature, which reads like a win and is half of one: the other half is that
nothing *can* be marked read-only any more. The fix is a `ReadLink<T>`, which
hands back one of the types the census had deleted — so the type count the
proposal claims to shrink goes back up by one.

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

## Finding 3: this bets against #626, in a one-line footnote

`fourth-option.md` decides representation in a sentence: links compile to raw
pointers, and `mem.relocatable` "stays a keys-only feature," declined because the
relocatability story is "narrow in practice."

It is not narrow, and the demotion is bigger than the wording admits.
[mem.relocatable](../memory/relocatable.md) opens on the premise:

> Rask's "no storable references" design means user-visible types contain only
> owned values and integer handles — **never pointers**. This makes pool state
> relocatable.

Links are pointers. So links do not narrow that spec — they falsify its first
sentence. And [#626](https://github.com/rask-lang/rask/issues/626), the durable-state
design, defines its tiers *around handles specifically*: tier-A `Persistable` is
"pointer-free data — primitives, **handles**, enums/structs of those," and its
snapshot semantics are "handles must survive the round-trip and stale handles
must stay stale." Links have no generations and no round-trip; there is no link
analogue of that property to specify.

The consequence, stated plainly: **adopt links and tier-A zero-copy persistence
dies for anything with edges.** Graphs keep tier C — an `Encode` walk — and mmap
survives only for edge-free pools. That may well be the right trade; checkpoint
plus input log is one story and checkless traversal is another, and a language
can prefer the second. But it is a north-star-adjacent trade (#626's own words)
being made in a representation footnote, and it belongs to whoever owns the
reliability direction, not to this document.

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

1. **The locals borrow rule.** Statable without NLL or `with`-everywhere (see
   Finding 1), at the price of search-or-key for post-insert access. Needs
   writing down, and needs a judgement on whether that price beats the read check
   it replaces. If it doesn't, the model is handles with extra steps.
2. **Read-only links.** `ReadLink<T>` or an argued decision to live without
   read-only access to a node.
3. **The #626 trade.** An explicit call on tier-A persistence, made by whoever
   owns the reliability direction, not inherited from a representation footnote.

Two smaller ones the fixup surfaced: required edges need a delete policy the
moment batches admit them (cascade/restrict stops being deferrable), and edge
writes need Transparency of Cost to bless them explicitly.

## Running it

```
rask run --interp specs/analysis/prototype/l2_targeting_links.rk
rask run --interp specs/analysis/prototype/l2_targeting_handles.rk

RASK_STORE_STATS=1 rask run --interp specs/analysis/prototype/fanin_links.rk
```
