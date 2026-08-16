<!-- id: analysis.fourth-option-adversarial -->
<!-- status: exploration -->
<!-- summary: Adversarial pass on the edge design — fifteen attacks, six wounds fixed, two regressions recorded, one feature killed, no fatal hit -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-litmus.md, analysis/fourth-option-in-practice.md -->

# Fourth Option: Adversarial Pass

Fifteen attacks on the edge design, strongest first. Verdicts: **kills** (design
dies), **wounds** (real flaw, fix found and adopted), **survives** (attack
fails). Score: 0 kills, 6 wounds fixed, 2 regressions recorded honestly,
1 feature killed as a simplification, 6 survivals.

## A1 — Aliasing: a traversal can reach the node you hold exclusively

`with e.target as t { ... t.next?.prev ... }` — in a cyclic structure,
`t.next?.prev` can *be* `t`, an aliased path to a node currently exclusively
borrowed. Pools have the same corner (`pool[other_h]` inside a `with` where
`other_h == h` at runtime) and the spec only covers the multi-binding case
(W3 panics on duplicate handles). Cycles make this corner *common* for edges,
not exotic.

**Wound → fix adopted:** inline node accesses inside a `with` compare the
resolved address against the open bindings — lexically nested, so the set is
tiny (nesting depth, typically 1–2 pointer compares) — and panic on a match,
extending W3's duplicate rule to inline paths. Cost exists only inside `with`
blocks. Pools should adopt the same rule regardless; the corner is unspecced
there today.

## A2 — Sync domains: a cross-graph edge can span two locks

The killer shape: `Mutex<GraphA>` and `Mutex<GraphB>` as separate values, an
edge from a node in A to a node in B. Deleting in B under B's lock must write
backlink fixups into A's memory — **without holding A's lock**. Unsound, full
stop.

**Wound → fix adopted, and it's load-bearing:** edges may only connect graphs
with a common ownership root (fields of one `World`-style struct, or one
graph), and any sync boundary must enclose that whole root — `Mutex<World>`,
never `Mutex` per graph with edges across. This is checkable at edge-field
declaration (the schema names the co-owned graphs) and it is exactly how
databases already work: foreign keys live *inside* one database, not across
two. The rule pays twice — see A9.

**Superseded as a concurrency answer.** The ownership rule stands for
soundness (edges connect co-owned graphs), but "wrap the root in a lock" was
a patch, and one global lock is the design Go beats. The real answer is
staged structural mutation — no lock on the hot path at all — worked out in
[fourth-option-concurrency.md](fourth-option-concurrency.md). What follows is
the original reasoning, kept because its two adopted rules survive.

**Thought through against the concurrency model** (channels, green tasks,
threads): the rule costs almost nothing real, because pools.md already
blesses exactly one cross-task architecture — "share handles, not data; the
pool stays in one thread; commands flow back to it." The world was already
single-owner with channels feeding it; messages carry `Key<T>` or domain IDs
instead of handles. Independent subsystems under separate Mutexes stay legal
so long as no edges span them; separate locks on *edge-connected* graphs are
what's forbidden — and that case was never safely lockable under handles
either (two pools with cross-handles under two locks tear the same way,
silently). The rule doesn't restrict concurrency; it names which concurrency
was fictional.

Two findings out of that think-through, both adopted:

- **Parallel iteration is a three-tier contract.** (1) Per-node parallel:
  chunked, mutate your chunk, follow no edges — movement systems. (2) Frozen
  parallel: follow anything, write nothing — render, queries. (3) Everything
  else sequential — combat follows edges *and* writes, and is sequential
  today anyway. Tier 3 racing is not an edge problem (handles race across
  chunks identically); the contract makes the phase discipline every ECS
  relies on compiler-checkable.
- **Healing is suppressed in shared-read contexts.** A self-healing read
  *writes*. Under `Shared`'s reader lock or any frozen context, readers race
  on the heal — so there, a read of a tombstoned edge returns `none` without
  the write-back. Healing is an optimization, skippable exactly where it's
  unsafe.

## A3 — Node literals in flight: an edge into a stack value

`w.entities.insert(Entity { target: w.player, ... })` — the literal exists on
the *stack* before insert. Register the backlink at literal creation and it
points at a stack address that's about to move into the arena; don't register
it and a delete between literal creation and insert leaves a dangling raw
pointer in the literal.

**Wound → fix adopted:** edge fields in a not-yet-inserted node value are
*borrows* of their targets — block-scoped, tracked by the machinery that
tracks every other borrow, excluding deletes while the literal lives (the
usual W2c-shaped error). Backlink registration is emitted at `insert`, when
the node reaches its final address. Moves of the literal before insert are
plain memcpys because no backlink exists yet.

## A4 — Non-optional edges can't be constructed under cycles

`a.body = b` and `b.owner = a`, both non-optional: neither literal can be
completed first. Databases need deferred constraints inside transactions to
pull this off; Rask has no transaction to defer into. Any two-phase
construction form is ceremony bolted onto the flagship path.

**Attack lands → feature killed:** **all edges are optional.** `Link<T>`
without `?` is gone. This deletes the construction problem, and it
retroactively simplifies the lazy model: the earlier carve-out ("lazy applies
to optional edges only; non-optional resolve eagerly") vanishes because the
non-optional case no longer exists — every edge has a `?` site, every edge
can heal lazily. `cascade`/`restrict` policies attach to optional edges
unchanged. Cost owned honestly: "an Entity always has a Body" leaves the type
system (it was unenforceable at construction anyway); a future transactional
multi-insert could win it back.

**Reversed by batches.** See
[fourth-option-concurrency.md](fourth-option-concurrency.md): a staged batch
gives a required cycle a legal transient state, with constraints checked at
apply — the database's deferred-constraint mechanism. `Link<T>` and
`Link<T>?` both live, and the distinction is meaningful (required edges never
need a `?` at use sites). The word-form syntax briefly proposed here
(`one Entity` / `many Entity` / `inverse of children`) is **withdrawn** — it
put English prose in a type position and invented keywords that don't look
like types, against every existing Rask convention. Relations use type-shaped
names and the annotation style the language already has:

<!-- test: skip -->
```rask
struct SceneNode {
    name: string
    children: Vec<Link<SceneNode>>

    @inverse(children)
    parent: Link<SceneNode>?

    @cascade
    body: Link<Body>
}
```

## A5 — Ordered edge containers: removal is O(n) backlink fixups

If an edge list is a Vec of pointers with backlinks carrying indexes,
swap-remove keeps deletes O(1) — but *ordered* containers (text_editor's
`line_order`) can't swap-remove, and a shift invalidates every subsequent
entry's backlink index: O(n) fixups per removal.

**Wound → fix adopted:** the lazy trick again. Removal tombstones the
container entry in place (O(1)); iteration skips tombstones; compaction runs
at flush. Same policy knob as node deletion, same flush call.

## A6 — Restrict mid-cascade: a delete that fails halfway

`delete` cascades into a subtree; three levels down, a node has an incoming
`restrict` edge. Abort now and half the subtree is already gone — a failed
operation that mutated state.

**Wound → fix adopted:** two-phase delete. Phase one walks the cascade
read-only and collects restrict violations; phase two mutates only if phase
one is clean. Delete becomes atomic: it either happens or reports why not,
touching nothing. O(2×degree), and no user code runs in either phase (no
destructors — the property that makes atomicity *possible*).

## A7 — Lazy reclamation: a tombstone can be pinned forever

Self-healing reads require the tombstone's header to stay readable until
every incoming edge has healed. A never-read edge pins the memory
indefinitely; "flush is optional" was oversold.

**Wound → fix adopted:** reclamation policy specced as: heal-on-read (as
designed), plus every `insert` incrementally heals K pending edges
(amortized, Lua-GC-style, cost attached to a visible allocating call), plus
`flush_deletes()` for full reclamation at chosen points. Worst case named in
the spec: no reads, no inserts, no flush → tombstones hold memory, exactly as
dead pool slots hold theirs today. Games flush per frame; servers flush per
request; the failure mode is a leak-shaped curve, never unsoundness.

## A8 — The snapshot pattern (reframed: a handle-shaped habit, not a need)

`pool.snapshot()` is a shallow O(n) memcpy because handles are integers; a
graph snapshot must translate pointers, O(n + edges). First recorded as a
regression — reframed on review: copy-the-world was never the *need*, it was
the pattern handles made cheap, so it leaves with them. The underlying need
(render while simulating) has native answers: frozen parallel phases (A2's
tier 2) and extracting POD render data — which is what engines do at scale
anyway. Honest footnote: a program that insists on copy-the-world pays the
O(n + edges) translation.

## A9 — Schema closure: who is allowed to point at Entity?

Per-node header layout (backlink list heads) needs to know every edge field
targeting the type — a whole-program property in the worst case, threatening
CS (no whole-program analysis).

**Survives via A2's rule:** edges only connect co-owned graphs, so the set of
node types that can target `Entity` is closed by the owning `World` struct's
module. Layout is computed where the world type is declared —
monomorphization-scale work the compiler already does for generics. The
sync-domain rule pays twice.

## A10–A15 — Attacks that fail

- **`?` is no longer "just a branch"** (hidden heal write): same cost tier as
  the generation check METRICS already blesses as implicit; the write is
  once-per-edge-death. Survives.
- **ABA/identity confusion:** a re-inserted "replacement" node doesn't
  resurrect old edges — they read `none`, which is correct (the identity
  died). No generation-reuse equivalent exists at all. Survives, better than
  pools (whose generations saturate).
- **Iteration during mutation:** tombstones make it *cleaner* than pools —
  no O(n) handle-snapshot allocation per loop; skip-dead iteration with
  PF1–PF4-equivalent guarantees. Needs precise spec text, not a new
  mechanism. Survives.
- **Panic mid-unlink:** no destructors means no user code interleaves with
  fixups (A6's two-phase covers the policy case). Survives.
- **`Heap<T>` overlap:** it keeps single-owner recursive values that
  never need incoming references (ASTs); graphs take shared topology.
  Guidance line, not a conflict. Survives.
- **Implementation lift** given the current compiler can't keep `Handle<T>?`
  intact through a struct literal (#733, #734): fair, recorded — but the
  edge machinery *replaces* generation coalescing, pool threading, and
  context-clause resolution rather than adding to them. Net compiler
  complexity is plausibly flat. Survives as a schedule risk, not a design
  flaw.

## A16 — The on-delete policies are the worst-designed part

Asked directly whether `cascade`/`restrict` are constraining or unintuitive.
They are, in three separate ways, and the default is innocent of all of them.

**The direction is ambiguous, and this document already got it wrong.** Given:

<!-- test: skip -->
```rask
@cascade
body: Link<Body>
```

does deleting the *entity* delete the body, or does deleting the *body* delete
the entity? SQL is unambiguous — `ON DELETE CASCADE` on a foreign key means
"when the referenced row dies, delete this row," i.e. body → entity. But the
in-practice doc wrote exactly this field and commented it "dies with the
entity," which is the **opposite** direction. If the person designing the
feature reverses it inside his own worked example, the notation is wrong, not
the reader.

The two intents are genuinely different and both are wanted:

| Intent | Meaning | SQL analogue |
|---|---|---|
| **Ownership** | deleting *me* deletes the target | none on this column — SQL models it from the other side |
| **Existence dependency** | deleting the *target* deletes me | `ON DELETE CASCADE` |

Ownership is the far more common intent in the game/ECS code this design
targets, and it's the one the foreign-key mechanism expresses *least*
naturally.

**Ownership mostly shouldn't be an edge at all.** A node can hold a plain
value: `Entity { body: Body }` composes by value, and deleting the entity
deletes the body with zero policy, zero annotation, and zero fixups. The only
reason to make an owned thing a separate node is that others must reference
it, or it lives in its own graph for layout reasons (the ECS motivation).
That shrinks the ownership-cascade case to something much rarer than it first
appears.

**Cascade hides unbounded cost — a worse transparency violation than the
fixup walk.** `store.delete(n)` looks like one deletion; with a cascade
declared three types away it can remove an arbitrarily large reachable set.
The O(in-degree) fixup this design has been careful to make visible is
bounded and local by comparison. Anyone who has run a `DELETE` in SQL and
watched ten thousand rows disappear knows the failure mode. The call site
should say it: `delete_cascading(n)` as a distinct operation, so the reader
of the *code* — not the reader of the schema — sees that a subtree may go.

**Restrict makes an ordinary delete fallible at a distance.** Some other
type's annotation turns `store.delete(n)` into an operation that returns an
error you must handle. Action at a distance, and it adds an error path to
code whose author never opted into one.

**Decided: ship the default alone.** Set-to-`none` is intuitive to the
point of invisibility — the thing died, so the reference is empty — and it
carries every litmus program and the flagship. Composition-by-value covers
most ownership. Cascade and restrict should wait for a real program that
demands them, and when they arrive they need direction-explicit names, not
SQL's, plus a visible call site — `delete_cascade(n)` as its own operation, so
the code says a subtree may go, not just the schema. Same discipline already
applied to `NodeId` and `@lazy`: don't ship the speculative half.

## Design deltas adopted from this pass

1. All edges are optional (`Link<T>?`); non-optional edges
   deleted from the sketch. Lazy healing now covers every edge uniformly.
2. Sync-domain rule: edges connect only co-owned graphs; locks wrap the
   ownership root or nothing.
3. Inline aliasing inside `with` extends W3: address compare against open
   bindings, panic on match (pools should adopt the same).
4. In-flight node literals borrow their edge targets until `insert`.
5. Two-phase (validate, then mutate) atomic delete.
6. Ordered edge containers tombstone + compact at flush.
7. Reclamation contract: heal-on-read + amortized heal-on-insert + explicit
   flush; worst case documented.

## Verdict

The design survives its own adversary, at the price of one dead feature
(non-optional edges — and killing it simplified the model), one honest
regression (snapshots), and five rules it needed anyway. The sync-domain rule
(A2) is the finding that matters most: it wasn't optional polish, it was a
soundness hole, and its fix doubles as the answer to schema closure (A9).
That's the usual signature of a load-bearing constraint — the design is
better-shaped with it than it was before the attack.
