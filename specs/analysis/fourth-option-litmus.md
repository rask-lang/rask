<!-- id: analysis.fourth-option-litmus -->
<!-- status: exploration -->
<!-- summary: Three litmus programs written with handles and with edges, scored per METRICS -->
<!-- depends: analysis/fourth-option.md, memory/pools.md, specs/METRICS.md -->

# Fourth Option Litmus: Edges vs Handles, Scored

Thought experiment, per METRICS.md. Three programs where references die behind
your back, each written twice: today's `Pool` + `Handle`, and the proposed
`Graph` + `Edge` (schema-declared, backlinked, fixed at delete —
[fourth-option.md](fourth-option.md)). Edge syntax is hypothetical throughout.

## L1: Doubly-linked list

The `remove` is where the models differ. Handles:

<!-- test: skip -->
```rask
struct Node { value: i32, next: Handle<Node>?, prev: Handle<Node>? }

func remove(self, h: Handle<Node>) {
    let p: Handle<Node>? = self.nodes[h].prev
    let n: Handle<Node>? = self.nodes[h].next
    if p? as ph { self.nodes[ph].next = n } else { self.head = n }
    if n? as nh { self.nodes[nh].prev = p } else { self.tail = p }
    self.nodes.remove(h)
}
```

Edges, with `prev` declared as `next`'s inverse:

<!-- test: skip -->
```rask
struct Node { value: i32, next: Edge<Node>?, prev: Edge<Node>? inverse(next) }

func remove(self, n: Node) {
    if n.next == none { self.tail = n.prev }
    if n.prev? as p { p.next = n.next } else { self.head = n.next }
    self.graph.delete(n)     // remaining incoming edges unlink themselves
}
```

**The honest finding here:** delete-unlink maintains *referential integrity*,
not *your invariants*. A list wants the neighbors spliced together, and
splicing is domain logic in both versions — edges don't erase it. What the
inverse buys is one direction of every fixup: `p.next = n.next` updates the
target's `prev` automatically, so four branches become two and the prev-side
bookkeeping class of bugs (set one direction, forget the other) is gone.
`head`/`tail` are **root edges** — edge fields on the graph's owning struct,
registered and fixed at delete like any node edge. The sketch needs those
anyway; a structure's entry points can't live inside it.

## L2: Entity targeting (the flagship case)

Entities target entities; targets die every round. Handles — this is the
stale dance, and it is unavoidable, not bad style; the spec's own
recommendation ("follow stored handles with `pool.get(h)`"):

<!-- test: skip -->
```rask
struct Entity { health: i32, damage: i32, target: Handle<Entity>? }

for h in world {
    if world[h].target? as t {
        if world.get(t)? as _alive {          // stale? checked at every follow
            world[t].health -= world[h].damage
        } else {
            world[h].target = none              // lazy cleanup of the dead handle
        }
    }
}
```

Edges:

<!-- test: skip -->
```rask
struct Entity { health: i32, damage: i32, target: Edge<Entity>? }

for e in world {
    if e.target? as t {
        t.health -= e.damage
    }
}
```

The middle two branches don't move somewhere — they stop existing. `target`
became `none` when the target died. Note what the handle version costs beyond
tokens: `get` materializes a throwaway copy just to prove liveness, the
`else` branch is load-bearing for correctness yet the program *runs* without
it (silent rot until a panic or a logic bug), and every function following a
stored handle repeats the dance.

## L3: Scene tree — reparent and subtree delete

Handles:

<!-- test: skip -->
```rask
struct SceneNode { name: string, parent: Handle<SceneNode>?, children: Vec<Handle<SceneNode>> }

func reparent(scene: Scene, n: Handle<SceneNode>, new_parent: Handle<SceneNode>) {
    if scene.nodes[n].parent? as old {
        scene.nodes[old].children.remove_where(|c| c == n)
    }
    scene.nodes[new_parent].children.push(n)
    scene.nodes[n].parent = new_parent
}

func delete_subtree(scene: Scene, n: Handle<SceneNode>) {
    let kids: Vec<Handle<SceneNode>> = scene.nodes[n].children.clone()
    for c in kids { delete_subtree(scene, c) }
    if scene.nodes[n].parent? as p {
        scene.nodes[p].children.remove_where(|c| c == n)
    }
    scene.nodes.remove(n)
}
```

Edges, with the 1:N inverse and a cascade policy in the schema:

<!-- test: skip -->
```rask
struct SceneNode {
    name: string
    children: Edges<SceneNode> on_delete(cascade)
    parent: Edge<SceneNode>? inverse(children)
}

func reparent(n: SceneNode, new_parent: SceneNode) {
    n.parent = new_parent      // 1:N inverse: leaves old children list, joins new
}

func delete_subtree(scene: Scene, n: SceneNode) {
    scene.delete(n)              // cascade follows children
}
```

Reparenting is one assignment because that's what it *is* relationally —
`UPDATE node SET parent = X`. The editor's `selected: Edge<SceneNode>?` (a
root edge) nulls itself when the selection is deleted; the handle version
re-validates the selection at every UI read.

## Memory model

| Stored reference | Handles | Edges |
|---|---|---|
| One direction, inverse declared | 16B (`Handle?`) | 8B (pointer) — the inverse field *is* the backlink |
| Both directions (list, tree) | 32B (two `Handle?`) + 4B generation/slot | 16B (two pointers), no generations |
| Anonymous (no inverse) | 16B | 24B (pointer + intrusive backlink node) |
| Structure bookkeeping | slot table, generations, dead slots linger until reuse | nodes freed at delete; no permanently-dead slots, no generation saturation |

Bidirectional structures — the common case for graphs worth the name — get
*smaller* under edges. Anonymous one-way edges get 1.5× bigger. Pools keep an
edge case edges don't have: a slot whose generation saturates is dead forever.

## Speed (analytic — no implementation exists to measure)

| Operation | Handles | Edges |
|---|---|---|
| Follow a reference | index math + bounds + generation compare + branch (~2 dependent loads, 2 branches; coalescing amortizes repeats) | 1 dependent load — it *is* a pointer chase; nothing to elide |
| Write a reference | 1–2 stores (it's an integer) | unlink old + link new backlink: ~4–8 stores |
| Delete | O(1), bump generation | O(degree), pointer-chasing walk |
| Iterate | O(capacity) slot scan | O(capacity) arena scan — same shape |

The delete asymmetry is smaller than it looks: the handle version's O(1)
remove leaves stale handles that every *future* read must check and every
correct program must eventually clean up (L2's `else` branch — smeared
delete cost, paid at read sites, forever). Edges concentrate the same fixup
work at the delete. For the read-to-write ratios of real graph code (reads
dominate by orders of magnitude), moving cost from reads to writes/deletes
is the favorable direction. The genuinely worse case: high-fan-in nodes
(10k incoming edges) deleted frequently — O(degree) with cache-hostile
chasing, honestly slower than a generation bump.

## Scorecard (per METRICS.md)

| Metric | Handles | Edges | Notes |
|---|---|---|---|
| **MC** stale references | caught at runtime | **impossible by construction** | MC's own text: "impossible by construction, not just caught at runtime. That's the whole point." This is the row the fourth option exists for. Also kills the forget-the-else-branch bug class (L2), which is *silent* under handles |
| **SN** on L2's hot loop | ~1.2 (ceremony exceeds logic; the 0.3 red line is crossed by the mandatory dance) | ~0.3 | On whole programs handles dilute to ~0.5; edges sit near the target everywhere because the dance has no edge equivalent |
| **ED** (ref-management LOC) | L1 remove: 5 stmts / 4 branches. L3 reparent: 5 lines. Subtree delete: recursive helper | 3 / 2; 1 line; 1 line + schema | Roughly half the code on every reference-maintenance path; best-in-class comparison (Go with GC pointers) becomes *reachable* for reparent — one assignment, same as Go |
| **TC** | generation check implicit (blessed by METRICS example) | backlink writes implicit (same small-cost tier); **`delete` hides O(degree)** | The one TC regression. Mitigation: the hidden work is exactly the fixups the handle program writes as visible code; it scales with real obligations, not with a collector's mood. Still — a `delete` whose cost varies 1000× deserves a doc-level answer |
| **PI** | flat, predictable costs | edge write and delete vary with multiplicity/degree | Small loss. Layout prediction improves (no slot table between you and the node) |
| **RS** | `Pool`, `Handle`, the get-dance idiom, `using` contexts, W2 rules | `Edge?`, `inverse`, `on_delete`, delete-locked scopes | Similar count, but edge concepts announce themselves in the schema (RS doesn't charge for type-announced concepts); the get-dance is an unwritten idiom living in heads, the most expensive kind |
| **RO** hot reads | ~1.1–1.5× C (check + indirection; coalescing narrows it) | **1.0× C** — a pointer chase is the C baseline | Edges hit RO ≤ 1.10 with no elimination pass at all; pools need `comp.gen-coalesce` + frozen contexts to approach it |
| **RO** churn-heavy | O(1) removes | O(degree) deletes | Handles win this workload, full stop |
| **CS** | — | schema is per-struct, inverse checking module-local | No whole-program analysis; CS unaffected |
| **IF** | ECS-standard | **no language has relational memory** | The seat is genuinely empty (fourth-option.md prior art) |

## Verdict

Edges dominate handles on the workloads pools were designed for — read-mostly
structures with dying members — winning MC, SN, ED, RS, hot-path RO, and IF.
Handles win churn-heavy deletes and keep two structural duties edges can't
take: durable identity (`Key<T>`, per the census) and escape across the
sync/process boundary. TC's hidden-degree delete and PI's variable costs are
the real regressions to design answers for (a `delete` cost note in the docs;
possibly a lint when a cascade crosses N levels).

Two additions the litmus forced into the sketch: **root edges** (edge fields
on the graph's owning struct — L1's head/tail, L3's selection) and the
restated limit that unlink preserves referential integrity, not domain
invariants — splices stay yours.

If this moves forward, the next artifact is a `mem.graph` spec draft: `Edges<T>`
API, inverse multiplicities (1:1, 1:N, M:N), `on_delete` policies
(`set_none` default, `cascade`, `restrict`), root-edge registration, and the
delete-locked scope tier.
