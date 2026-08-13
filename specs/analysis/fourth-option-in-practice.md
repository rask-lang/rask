<!-- id: analysis.fourth-option-practice -->
<!-- status: exploration -->
<!-- summary: Day-to-day Rask under edges: a worked example, the consolidated cost model, what it enables, what it closes, what it retires -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-litmus.md -->

# The Fourth Option in Practice

What programs look like if `Graph` + `Edge` lands (lazy default, per
[fourth-option.md](fourth-option.md)). Syntax is placeholder throughout.

## A world, end to end

The data model reads like an ER diagram — because it is one:

<!-- test: skip -->
```rask
struct World {
    entities: Graph<Entity>
    bodies: Graph<Body>
    player: Edge<Entity>?                  // root edge: nulls itself if the player dies
    by_name: Map<string, Edge<Entity>>     // root index: entry drops at delete
}

struct Entity {
    name: string
    health: i32
    damage: i32
    target: Edge<Entity>?                  // nulls when the target dies
    body: Edge<Body> on_delete(cascade)    // owns its physics body: they die together
    squad: Edges<Entity>                   // M:N — deleted members drop out
}

struct Body {
    x: f32
    y: f32
    vx: f32
    vy: f32
}
```

Spawning wires relations by assignment; every backlink is the compiler's
problem:

<!-- test: skip -->
```rask
func spawn_enemy(mutate w: World, name: string, x: f32, y: f32) {
    let body = w.bodies.insert(Body { x: x, y: y, vx: 0.0, vy: 0.0 })
    let e = w.entities.insert(Entity {
        name: name, health: 20, damage: 5,
        target: w.player,          // cross-references the root edge, fine
        body: body,                // cross-graph edge — a foreign key
        squad: Edges.new(),
    })
    w.by_name.insert(name, e)
}
```

Systems are the payoff — this is the whole combat system, and there is no
other version of it hiding in a cleanup pass somewhere:

<!-- test: skip -->
```rask
func combat_round(mutate w: World) {
    for e in w.entities {
        if e.target? as t {
            t.health -= e.damage               // plain deref
        }
    }
    w.entities.delete_where(|e| e.health <= 0)   // tombstones: O(1) each
}

func frame(mutate w: World, dt: f32) {
    for b in w.bodies {
        b.x += b.vx * dt
        b.y += b.vy * dt
    }
    combat_round(mutate w)
    w.entities.flush_deletes()     // the O(degree) fixup work, at a line you can see
}
```

When an entity dies: its `body` cascades (physics entry gone), every other
entity's `target` at it reads `none` from then on, it drops out of every
`squad` list, `by_name` loses its entry, and `player` nulls if it was the
player. All of that is the schema executing, not code someone remembered to
write.

### What's absent, measured against game_loop.rk

Today's game_loop carries: `active: bool` flags (deferred-death workaround),
a `cleanup_system` (sweep the flags), `to_remove` staging Vecs, `world[h]`
access noise on every touch, `using Pool<Entity>` plumbing on every helper,
and the stale-`get` else-branches on every stored-handle follow. None of
those constructs exist in the edge version. They aren't shorter — they're
not there.

## The cost model, consolidated (lazy default)

| Operation | Cost | vs C with raw pointers |
|---|---|---|
| Follow an `Edge<T>?` | the `?` test you wrote; + one header-flag load until the edge heals | 1.0× steady-state; ~1 extra predictable branch transiently |
| Follow a non-optional `Edge<T>` | plain deref, nothing ever | 1.0× |
| Assign an edge | pointer store + backlink relink (~4–8 stores) | ~4× a raw store — but the C/handle program does this relinking by hand as visible code |
| `delete` | O(1) tombstone | cheaper than free() |
| `flush_deletes()` | O(pending fixups), each edge pays once | the same work manual unlinking would do, batched |
| Memory per bidirectional link | 16B (two pointers) | pools: 32B + generations |
| Memory per anonymous edge | 24B (pointer + intrusive backlink) | pools: 16B |
| Memory transient | tombstoned nodes linger until healed/flushed | pools: dead slots linger until reuse — a wash |
| Iteration | arena scan, O(capacity) | same as pools |
| Cross-task | scoped parallel iteration over disjoint chunks, delete-locked | drops the `Arc<Mutex>` pools smuggle in for cross-task `using` today |
| Compile time | schema checks are per-struct, module-local | CS unaffected |

Worst cases, named: high-fan-in nodes deleted at high frequency pay
O(degree) at flush (eager pays it inside `delete`); lazy mode delays memory
reuse until edges heal or flush runs.

## What it enables beyond the replacement

- **Cycle-safe serialization.** The encoder knows the schema, so `Encode` on
  a whole graph can emit stable node ids and reconstruct cycles — the thing
  serde-style tree encoders fundamentally can't. Graphs-with-cycles become
  ordinary data.
- **Topology-aware tooling.** "Who references this?" is a compile-time
  question now. IDE ghosts (principle 5) can show a node's in-edges; a
  visualizer can draw the world's ER diagram from the types alone.
- **Schema invariants for free.** A 1:N inverse means a node *cannot* be in
  two parents' children lists — data-model invariants that are asserts and
  prayer in every engine become unrepresentable states.
- **Structural undo within reach.** A delete's fixup set (which edges got
  nulled) is enumerable — recording it is a transaction log, and replaying it
  backwards is undo of structural mutation. Databases again. Horizon item,
  not a promise.
- **Relational queries as a future.** Declared relations invite
  flecs-style queries/joins (`world.query<Entity, with: target>`...). Not
  designed here; the door is open and the schema is the reason.

## What it closes

- **Edges never leave the structure.** No free-floating persistent
  references; boundaries use domain keys + root indexes (the corpus census
  shows that's what real programs already do — validation's `TaskId`).
- **Graph mmap/relocatability.** Decided: raw pointers, so `mem.relocatable`
  stays keys-only. Graphs serialize through Encode instead.
- **Fully dynamic topology.** Relations are declared per-field. A bag of
  arbitrary runtime-decided cross-references doesn't fit (root
  `Map<K, Edge<T>>` covers the tame cases; reflection-style graphs don't).
- **The high-fan-in churn workload** keeps a real cost floor: each incoming
  edge is written once, whenever it runs.

## What it retires (the consequence cascade)

If Pool folds into Graph and Handle becomes boundary-only `Key<T>`:

- `mem.context` — retires whole. `using` is two features in one keyword: the
  block form (`using Multitasking { }`, `using ThreadPool { }`) is the
  concurrency capability system — sim (S6) and testing (T17–T19) build on
  it — and stays. The signature form (`func f() using X`) had two clients:
  `Pool<T>` dies with handles, and `Allocator`, its last passenger, moves to
  the block form too — `using Arena(64.kb) { ... }` installs a task-ambient
  allocator, the same block-only/no-signature-propagation shape Multitasking
  already has. The hidden-parameter mechanism leaves the language entirely:
  nothing invisible flows through signatures anymore. The arena block *is* a
  region (allocations die at block end, no-escape is the analysis Rask
  already runs everywhere), rhyming with the delete-locked scope. Leftover
  cases have better homes: "this function demands an allocator" is
  principle-5 metadata plus a no-global-alloc build profile; two allocators
  at once is the explicit allocator-as-value form (AL8), visible where it
  should be. Bonus retirement: `frozen` needs no graph equivalent — a plain
  (non-`mutate`) graph parameter is already read-only under ordinary
  parameter modes; only the delete-locked middle tier (writes yes, deletes
  no) needs one new annotation. What remains to teach: `using X { }` opens a
  capability scope. One sentence.
- `comp.gen-coalesce` — the generation-check elimination pass has nothing to
  eliminate. Deleted machinery, not ported machinery.
- Pool's hidden `Arc<Mutex>` threading — replaced by scoped parallel
  iteration, removing a lock the current design hides (a TC violation,
  strictly read).
- `mem.pools`' weak handles, `with_valid`, `get_unchecked` escape hatches —
  the problems they escape from don't exist.
- DAY_ONE.md drops Handle, the get-dance, and `using Pool<T>`; gains `Edge?`
  and two schema clauses that announce themselves.
- boxes.md: the family stays five (`Cell`, `Graph`, `Shared`, `Mutex`,
  `Owned`), with the "identity" discipline upgraded to "relational".

The migration surface is honest: pools.md, boxes.md, context-clauses.md,
relocatable.md, closures.md guidance, every example using `Pool` — plus the
interpreter, the checker, and codegen. This is a big lift; the exploration
docs exist so the decision precedes the lift.
