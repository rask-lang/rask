<!-- id: analysis.forest -->
<!-- status: exploration -->
<!-- summary: Ownership is a forest and racks are the only cross-edges. Five things fall out: contracts get their frame from parameter modes, required edges have a delete-policy menu, a rack is a dependency graph for derived values, every owned value is sendable, and "no leaks" is a theorem now that a box can't hold its own holder -->
<!-- depends: memory/racks.md, memory/parameters.md, memory/linear.md, memory/shared-rack-heap.md, concurrency/sync.md, concurrency/async.md, compiler/effects.md, stdlib/encoding.md, memory/relocatable.md -->

# What the forest buys

Rask's rules were chosen one at a time for ergonomics. Put together they pin
down the shape of the heap, and that shape is a theorem other fields have
built on for decades. This note states the theorem and follows five of its
consequences far enough to see what each would cost to build.

## The invariant

At every point in a running Rask program:

1. **Live values form a forest.** Every value has exactly one owner
   (`mem.ownership/O1`), a view ends with its block (`mem.borrowing/B1`), and
   no field holds a reference (`CORE_DESIGN` principle 3). So the only path to
   a value is down from its owner.
2. **The only cross-edges are links, and they only point into racks.**
   `Link<T>` is the one storable reference (`mem.racks/RK2`), and a rack owns
   every node a link can name (RK1).
3. **Every cross-edge is recorded at its target.** The rack keeps the incoming
   edges of each node, on the node for edges from node fields and on a
   foreign-holder list for everything else (RK3, RK10). That is what lets
   `delete` null them all before it returns.

The honest exceptions are the closed set of sharing primitives. `string` is
refcounted but immutable, so two names for one buffer can't observe each
other. `Shared<T, S>` is refcounted and mutable, and is the one way to reach
a value from two places; it is a leaf of the forest with several roots above
it, not a cross-edge, and section 5 says where that costs us.

This is the strongest form of what the ownership-types literature called
*owners as dominators* (Clarke, Potter and Noble, 1998): every path from a
root to an object goes through its owner. Those systems carried owner
parameters on every type and never got light enough to use. Rask has the
property with no annotations, because it never let a reference be stored in
the first place. I'd state the invariant in `CORE_DESIGN` under principle 3,
so the rest of this note reads as consequences rather than proposals.

## 1. Contracts, and the frame for free

The hard part of specifying imperative code is never the postcondition. It is
the *frame*: saying what a call does not touch, so the caller can keep what it
knew about everything else. Dafny makes you write `modifies` clauses on every
method and spends most of its proof effort discharging them. Separation logic
exists for the same reason; its separating conjunction `P * Q` says "P and Q
hold on disjoint heaps", and the frame rule lets you forget the rest.

In Rask the frame is already in the signature. A parameter is a view, a
`mutate`, a `deleting` or a `take` (`mem.parameters/PM1`–`PM2b`), and the
forest says that is *all* a call can reach: there is no path from a parameter
to anything but the subtree under it. So:

- A view parameter leaves its subtree bit-for-bit unchanged.
- `mutate r` may change values under `r`, and for a rack may insert, but
  cannot delete: the node set after the call includes the node set before
  (PM8/PM9). Every link the caller holds into `r` is still valid after. That
  is a theorem of the mode, and the checker already relies on it (E0329 is
  its contrapositive).
- `deleting r` may shrink the node set; which nodes survive is not knowable
  from the signature, which is exactly why the mode exists.
- `take x` ends the caller's knowledge of `x` altogether.

Two values with different roots are disjoint by clause 1, so the separating
conjunction between two parameters is automatic. Nothing has to be proven
about aliasing because there is nothing to alias. A contract language for
Rask would therefore need only ordinary pre- and postconditions over
parameters and the result, plus one theory for racks.

**The rack theory.** A rack is a finite map from node identity to node value,
plus the edge index. Its invariants: every link field is `none` or names a
live node of the same rack (referential integrity, RK3), and the index at a
node is exactly the set of fields and containers holding a link to it. The
three operations have one-line rules. `insert(v)` adds a fresh identity with
no incoming edges. A field write that retargets a link moves one index entry.
`delete(n)` removes `n` from the map and sets every link in its index to
`none`. The only quantified clause a verifier would ever meet is the last one,
"for every holder recorded at `n`, that holder's field now reads `none`", and
it ranges over a set the runtime literally keeps, so it is a finite-map theory
any SMT solver takes.

**What it would look like.** The shape that fits principle 9 (information
without enforcement) is a contract the compiler never uses to reject a
program: checked at the call boundary under `rask test` and in debug builds,
shown as ghost text by `rask annotate`, and available to a verifier later if
one is ever worth writing. Sketch only, syntax undecided:

<!-- test: skip -->
```rask
func pop(mutate self) -> T?
    requires !self.is_empty()
    ensures self.len() == old(self.len()) - 1
{
    ...
}
```

`old(...)` is the one piece of machinery a postcondition needs, and for a view
or `mutate` parameter it is cheap: the forest means a snapshot of a parameter
is a `clone()` of its subtree, nothing else can have changed it, and a test
build can afford the clone. For a `deleting` rack parameter `old(r)` is
`r.snapshot()` (RK13), which is already defined.

**What stays hard.** Loop invariants, if anyone wants full verification.
Contracts that mention a `Shared` box, because the box is the one place two
names reach one value and the frame argument stops there. And contracts on
closures, which carry captures the signature doesn't show. None of those is
Rask-specific; the point is that the Rask-specific part, the frame, is free.

## 2. Required edges: the delete-policy menu

RK7 admits only optional edges and defers required ones, because a required
`Link<T>` has no `none` to fall back on when its target dies. SQL-92 settled
this menu for foreign keys thirty-four years ago, under the name referential
actions, and three of the four transfer directly.

| SQL action | Rask reading | Status |
|---|---|---|
| `ON DELETE SET NULL` | `Link<T>?` field goes to `none` | Built. Today's only behaviour (RK3) |
| `ON DELETE CASCADE` | the holder dies with its target | A required edge on a rack node: deleting the target deletes the holder too. A `Vec<Link<T>>` entry already behaves this way (RK3 removes the entry), which is cascade at the element |
| `ON DELETE RESTRICT` | the delete is refused while a holder exists | A required edge where the holder must outlive the target: `delete` returns an error naming the holder |
| `ON DELETE SET DEFAULT` | the field is repointed at a designated node | Rejected. A "default node" is a stable identity the program would have to keep alive by hand, which is the weak-reference bookkeeping the rack exists to remove |

The spelling would sit on the field, since that is where SQL puts it and
where a reader looks:

<!-- test: skip -->
```rask
struct Edge {
    from: Link<Node> on delete cascade    // an edge can't outlive an endpoint
    to:   Link<Node> on delete cascade
}

struct Task {
    owner:  Link<User> on delete restrict   // a user with tasks can't be deleted
    parent: Link<Task>?                     // set null, the default for optionals
}
```

Three consequences the rack rules already decide:

- **Cascade is an unnamed delete.** `rack.delete(n)` on a node type that is
  the target of any cascade edge kills nodes the caller never named. By PM9
  that is what `deleting` is for, so such a delete needs the `deleting` mode
  on the rack it goes through, and the compiler knows which node types are
  cascade targets because every field type is in the program.
- **Restrict makes `delete` fallible.** For a node type that is the target
  of a restrict edge, `delete` answers `void or DeleteBlocked`, carrying the
  holder, and the call site writes `try`. For every other node type it stays
  infallible. Which one applies is a property of the node type, decided at
  compile time, so the fallible signature only appears where a restrict edge
  exists.
- **Construction wants a batch.** A required edge must point at a live node
  at insert time, so two nodes that require each other can't be inserted one
  at a time. SQL's answer is a deferrable constraint checked at commit. The
  Rask-shaped version is a scoped block, `with rack.batch() as b { ... }`,
  inside which required edges may read as unset and are checked when the
  block ends. This is the "batch" RK7 says both policies wait on.

The orphan-row cost is the same as in a database: with restrict in play,
delete order becomes something the program has to get right, and the error
should name the holder that blocked it.

## 3. Derived values over a rack

A derived value is a field whose contents are a function of other fields and
of the nodes a link reaches: a directory's total size, a scene node's world
transform, an editor's line table. Recomputing it on every read is the
simple answer; recomputing it only when an input changed is what the
self-adjusting computation line of work (Acar 2005, Adapton 2014) does, and
it needs one thing: a dependence graph with *reverse* edges, so that a change
at a node can find the derived values that read it.

A rack keeps those reverse edges already. If a parent's derived field reads
its children through links, then the children's incoming-edge records name
the parent. Dirtying on write is a walk of exactly the records RK3 maintains
for delete, and reading a dirty value recomputes it along outgoing links.
Sketch:

<!-- test: skip -->
```rask
struct Dir {
    name: string
    size: i64
    children: Vec<Link<Dir>>
    derived total: i64 = self.size + self.children.sum(|c| c.total)
}
```

What the rack gives and what it doesn't:

- **Cost lands only on types that declare a derived field.** A write to a
  plain node does today's work. A write to a node some derived field reads
  follows its incoming records and marks them dirty; a read of a dirty field
  recomputes. Demand-driven, so an untouched subtree costs nothing.
- **Cycles need a rule.** Adapton handles DAGs. A derived field over a cyclic
  rack either computes a fixpoint or is an error. I'd make it an error at
  the read, "derived `total` depends on itself through `b`", since the graph
  is there to name the cycle.
- **The database shelf has the general version.** A derived field is a
  materialized view, and incremental view maintenance is the research area.
  DBSP (Budiu et al. 2022) proves that any relational query has an
  incremental form computed from deltas; a rack is a relation, so the whole
  of that result applies to queries over racks, not only to per-node fields.
  That is the long road. The per-node field is the short one and covers the
  three examples above.

This can start as a library over two rack hooks, "a field of `n` was written"
and "walk the holders of `n`", with no syntax at all. The `derived` keyword is
worth adding only once the library has shown which shape programs actually
reach for.

## 4. Every owned value is iso

Pony's type system has six reference capabilities, and the reason it has six
is to carve out `iso`: a value reachable from exactly one place, which can
therefore be sent to another actor without copying and without a lock. In
Rask every owned value is `iso` by clause 1. The others map too:

| Pony | Rask |
|---|---|
| `iso` (unique, mutable, sendable) | every owned value |
| `val` (immutable, shared, sendable) | `string`, `bytes` |
| `ref` (mutable, not sendable) | a `with` binding, a `mutate` parameter |
| `box` (readable, not sendable) | a view parameter |
| `tag` (identity only) | no equivalent; a link carries read access |
| actor-mediated access | `Shared<T, Mutex>`, `Shared<T, Readers>` |

The consequence `conc.async/S5` already states for tasks generalises: a send
is a move. Between two tasks in one process the move is a pointer; across a
machine boundary it is the same move followed by serialisation, and the
forest makes the serialisation canonical, because a subtree has no edges out
of it to fix up (`std.encoding/E12` auto-derives `Encode` for exactly that
reason). Erlang gets mobility by copying everything; Kilim's isolation types
(2008) got it for message types only; Rask has it for every value.

What can't move is the complement of what the checker already computes. The
`task_bound` walk (links and `Local` boxes, `conc.sync/SH7`,
`mem.ownership/T2`) is the first half of a *mobility* predicate; the second
half is "no OS resource and `Encode` derivable". A value is sendable to a
task when the first half is clear and to a machine when both are. Graphs move
as racks: `snapshot()` and `mem.relocatable` write links as slot numbers
(RK13), so the unit of graph mobility is the rack, and a link never crosses
alone. That is the rule Leden wants for object references and it falls out
of RK13 rather than needing its own.

## 5. The release theorem

**Claim.** When a scope ends, every value owned by a binding of that scope is
released, transitively and in reverse declaration order, and a linear value
has been consumed exactly once on every path including unwinding. Therefore a
Rask program allocates nothing it does not release. Two cases need naming:
one was a hole and is closed, the other only looks like one.

**Why it holds.** Release is a walk down from the binding. By clause 1 the
walk is over a tree, so it terminates and reaches every value the binding
owns, and no value is reached twice. `mem.linear/L1`–`L7` make a linear
value's consumption a checked obligation on every path, and
`ctrl.ensure/EN1`–`EN2` with `ctrl.panic/U6` make the unwinding path one of
those paths. Containers release their elements as part of the walk, and
`mem.resource-types/RC1`–`RC5` make a container of linear values linear
itself, and keep them out of racks, so the walk never has an obligation it
cannot discharge. `tests/leak_gate.sh` is this claim run over
the suite under `RASK_LEAK_CHECK=1`.

**Exception one, now closed: `Shared` cycles.** A struct may hold a `Shared`,
so a box could hold a clone of a box that holds a clone of it, and a refcount
can't see a cycle. That was the one place outside a rack where the language
could build a cycle, and it leaked (rask-lang/rask#1672). It is now a type
error, `conc.sync/SH10`: a `Shared` field whose payload can hold a box of
the owning type again, by value or through further boxes, is rejected the
way `mem.resource-types/RC2` rejects a linear node in a `Rack`. The rule
is on the shape, so a tree of boxes is rejected along with the cycle it
could become, and the answer in both cases is the one the design already
gives for values that point at each other: put them in a rack and share the
rack.

**Exception two: orphans inside a rack, which are not leaks.** A node that no
link names and no local binding holds is still owned by its rack and freed
with it. Call that what it is. A leak is memory that can be neither reached
nor released. An orphan can be reached (`rack.nodes()` walks it) and released
(`delete` frees it), so by the definition every other language uses it is not
a leak; it is unintended retention, the same thing as an element you forgot
to remove from a `Vec`. With SH10 in place the theorem therefore reads **a
Rask program allocates nothing it does not release**, full stop, and
`tests/leak_gate.sh` is its proof over the suite.

What is left is ergonomic: a program that thinks of a rack as a graph, where
a node unreachable from the rest *should* die, has to delete it by hand. The
rack can do that itself, and it already holds everything the job needs.

**Anchored racks.** Today a rack is a set: a node lives until deleted, and
iteration is one of its access paths. The other reading is that a node lives
while something points at it. I'd offer that as a rack strategy, the way
`Shared` takes its locking strategy as a type argument, spelled here as
`Rack<T, Anchored>` with the name undecided:

- A node is held by its recorded edges, the ones the rack already indexes
  for RK3: a `Link` field in another live node, or a foreign holder outside
  the rack. When the last recorded edge to a node goes, the node is deleted,
  which nulls its own outgoing edges and may free its targets in turn. The
  in-degree is the length of the index the rack keeps anyway, so this costs
  nothing a plain rack doesn't pay.
- A local link is a temporary, as now. When its binding ends and the
  compiler can see the link was never stored, the node it names has no holder
  left, and the compiler emits the delete there: one in-degree test per local
  link that dies, only in an anchored rack. The ownership pass already knows
  which local links were stored, since RK6 tracks the same thing.
- Cycles among unheld nodes are the one case in-degree can't see. They are
  collected by a mark from the foreign holders, and the moment to run it is
  the end of any `deleting` operation on the rack. PM9 revokes every local
  link into the rack at such a call, so at that point the recorded edges are
  the whole root set, with no stack to scan. A rack with no edge removed
  since the last collection runs nothing.

That is reference counting where the stored half is paid for by an index the
rack keeps for other reasons, the local half is the compiler's scope
analysis, and the cycle half is a bounded, rack-local mark whose roots are
exact by construction. A plain `Rack<T>` keeps set semantics and no new cost.
Compile-time detection of orphans beyond the "inserted and never stored"
case is not on offer, since whether a node is still held is decided at run
time, which is why the mechanism is dynamic and the choice of strategy is in
the type.

## What I'd do first

1. State the invariant in `CORE_DESIGN` principle 3, three clauses, one
   paragraph.
2. Write the mode theorems into `mem.parameters` as one row: a view preserves
   its subtree, `mutate` preserves a rack's node set, only `deleting` shrinks
   it.
3. Required edges with cascade and restrict, with the batch block, replacing
   the RK7 deferral.
4. Anchored racks, as a strategy on `Rack`, once required edges exist: a
   cascade edge and an anchored node are the same mechanism from two sides.
5. Derived values as a library over two rack hooks, keyword later or never.
6. Contracts last, as a `rask test` boundary check, once the four above have
   shown what the postconditions need to say.
