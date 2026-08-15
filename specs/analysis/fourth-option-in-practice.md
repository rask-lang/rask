<!-- id: analysis.fourth-option-practice -->
<!-- status: exploration -->
<!-- summary: Day-to-day Rask under edges: a worked example, the consolidated cost model, what it enables, what it closes, what it retires -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-litmus.md -->

# The Fourth Option in Practice

What programs look like if `Store` + `Link` lands (eager default, per
[fourth-option.md](fourth-option.md)). Syntax is placeholder throughout.

## The two types

Originally three: the plural type (`Edges<T>`, alongside `Edge<T>`) differed
from the singular by one character — unreadable, and a typo would compile.
**The plural is deleted.** It never needed its own type: put links in the
collections that already exist.

| Today | Proposed | What it is |
|---|---|---|
| `Pool<Task>` | `Store<Task>` | **where the things live.** Owns the memory. You insert, delete, and iterate it |
| `Handle<Task>` in a field | `Link<Task>?` | **one reference** to a thing that lives in a graph |
| `Vec<Handle<Task>>` | `Vec<Link<Task>>` | **many references** — an ordinary Vec |

Read as a database, which is where the model comes from: `Store<Task>` is the
tasks table, `Link<User>?` is a foreign key column pointing at one row, and a
`Vec<Link<Task>>` is the many side.

<!-- test: skip -->
```rask
// Today
struct Task {
    assignee: Handle<User>?          // one, maybe
    deps: Vec<Handle<Task>>          // several
}
struct Store { tasks: Pool<Task>, users: Pool<User> }

// Proposed — same shape, different guarantee
struct Task {
    assignee: Link<User>?            // one, maybe
    deps: Vec<Link<Task>>            // several
}
struct Store { tasks: Store<Task>, users: Store<User> }
```

`Vec<Link<T>>` and `Map<K, Link<T>>` work because the compiler knows the
element type is a link and uses the tombstone-and-compact representation
A5 already specified — the container is edge-aware underneath, ordinary at
the source level. One reference concept, and collections stay collections.

The difference is entirely in what happens when a target dies:

- `Link<User>?` becomes `none` — automatically, at the delete.
- an entry in `Vec<Link<Task>>` disappears — the list gets shorter.
- With handles, both keep pointing at a dead thing, and every reader has to
  check. (Missing that check is [#740](https://github.com/rask-lang/rask/issues/740).)

### Do you have to thread the graph through everything?

Less than today, and visibly instead of invisibly.

- **Reading and traversing needs nothing.** A function takes the node:
  `func damage(e: Entity, amount: i32)`. The edge is reachable from `e`, and
  dereferencing it is a pointer hop — no container required.
- **Structural operations need the graph**, as an ordinary parameter:
  `func kill(mutate w: World, e: Entity)`. Insert and delete have to name
  where they're inserting into and deleting from.

Compare today, where even a *read-only helper* needs the pool, because a
handle is a detached integer that means nothing without it. In the flagship
store, five helpers carry `using frozen Pool<Task>` for exactly that reason —
`rank_of`, `id_of`, `matches_filter`, `task_is_blocked`, `to_view`. All five
take a plain `Task` under edges and need no context at all. None of the
mutating functions gain a parameter, because they're already methods on the
store.

So the count goes down: hidden context clauses disappear, and the graph shows
up as a normal argument exactly where the code actually mutates structure.

### How it maps to languages you know

| Coming from | An `Link<T>?` is… |
|---|---|
| SQL | a foreign key with `ON DELETE SET NULL`. Precisely this, and where the model came from |
| Java / C# / Python | a normal object reference — except it goes null when someone deletes the object, instead of keeping it alive |
| Go | a pointer that becomes nil when the object is deleted, with no GC involved |
| Rust | a `Weak<T>` that's already upgraded — no check, because dangling can't happen. (The real Rust options are `slotmap`, which is handles, or `Rc`/`Weak`, which is refcounting) |
| C++ / Qt | `QPointer<T>` — auto-nulls when the target is destroyed. Closest existing thing in a systems language, but library-level and QObject-only |
| Swift / Obj-C | a zeroing weak reference — same observable behaviour, but implemented on top of ARC, so death is refcount-driven |
| ECS (flecs, Bevy) | an entity relationship with an `OnDelete` policy |

The one-sentence version for each: **the reference doesn't keep the thing
alive, and it doesn't dangle — it empties.**

**Why "graph" and not "pool"?** A pool is a bag of things you hold tickets
for; it knows nothing about how its contents relate. This container has to
know the relationships — that's what lets it fix them when something dies.
Things plus the connections between them is a graph. (Working name; the
concept is settled, the spelling isn't.)

Everything else in this document is consequences of those two types.

## A world, end to end

The data model reads like an ER diagram — because it is one:

<!-- test: skip -->
```rask
struct World {
    entities: Store<Entity>
    player: Link<Entity>?                  // root edge: nulls itself if the player dies
    by_name: Map<string, Link<Entity>>     // root index: entry drops at delete
}

struct Entity {
    name: string
    health: i32
    damage: i32
    target: Link<Entity>?         // nulls when the target dies
    squad: Vec<Link<Entity>>      // M:N — deleted members drop out
    body: Body                    // owned by value — dies with the entity, no policy needed
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
    let e = w.entities.insert(Entity {
        name: name, health: 20, damage: 5,
        target: w.player,          // cross-references the root edge, fine
        body: Body { x: x, y: y, vx: 0.0, vy: 0.0 },
        squad: Vec.new(),
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
    for e in w.entities {
        e.body.x += e.body.vx * dt
        e.body.y += e.body.vy * dt
    }
    combat_round(mutate w)
    w.entities.flush_deletes()     // the O(degree) fixup work, at a line you can see
}
```

When an entity dies: its `body` goes with it (owned by value), every other
entity's `target` at it reads `none` from then on, it drops out of every
`squad` list, `by_name` loses its entry, and `player` nulls if it was the
player. All of that is the schema executing, not code someone remembered to
write. Note that no delete *policy* appears anywhere — the set-to-`none`
default and ordinary composition carry the whole example (see A16).

(Splitting `Body` into its own `Store<Body>` for cache locality — the usual
ECS reason — is the one shape that *would* need an ownership policy, since
the entity would then reference its body rather than contain it. That policy
is deliberately deferred; see A16.)

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
| Follow an `Link<T>?` | the `?` test you wrote; + one header-flag load until the edge heals | 1.0× steady-state; ~1 extra predictable branch transiently |
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

### What an edge write actually costs

`a.target = b` is not one store. Counting an intrusive doubly-linked
back-list:

| | Stores |
|---|---|
| The pointer itself (`a.target = b`) | 1 |
| Unlink `a` from the **old** target's incoming list | ~2–3 (plus loads) |
| Link `a` into `b`'s incoming list | ~3 |

So ~4 when setting a `none` edge, ~7 when rewiring a live one, all to hot
nearby memory — the node's own inline link fields, the target's list head,
and one neighbor. Call it 2–5ns.

When the relation declares an inverse, those stores **are** the data
structure — a tree's sibling links, a list's `prev`/`next`. The hand-written
version writes exactly the same stores (and is where the classic
forgot-one-direction bug lives). No separate backlink storage exists at all.

Non-edge writes have no barrier whatsoever. Scalars, values, and everything
that isn't a declared relation compile to a plain store, which is most of a
Rask program.

**The crossover, stated plainly.** Handles make the write cheap (one integer
store) and the read expensive (a check, forever). Links make the write cost
~4–7 stores and the read free. Set a target once and read it 60×/second for
ten seconds: handles pay ~600 checks (~1.2µs), edges pay one write (~5ns)
and 600 free reads. Reads dominate by orders of magnitude in every real
graph workload, which is why this trade is favorable — but it is a trade. A
workload that rewires references constantly and reads them rarely is the
shape where handles genuinely win, and the honest guidance is to say so
rather than pretend the write is free.

## What it enables beyond the replacement

- **The layout is yours to optimize.** A live node can move: walk its incoming
  list and write the new address instead of `none` — the delete walk with a
  different value. So a graph can defragment its arena, or sort nodes into
  traversal order so a hot loop walks contiguous memory. Pools cannot do this
  at all: a handle's index *is* its identity, and `mem.pools/PL9`'s guarantee
  that handles survive growth is the very property that forbids moving a live
  element. This is a structural capability gain, not a tradeoff, and it lands
  in exactly the workloads pools were designed for. **Explicit only** —
  `store.compact()` is a call you write. A runtime that relocates on its own
  schedule is a moving collector, which is the thing this design exists to
  avoid.
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

- **Links never leave the structure.** No free-floating persistent
  references; boundaries use domain keys + root indexes (the corpus census
  shows that's what real programs already do — validation's `TaskId`).
- **Graph mmap/relocatability.** Decided: raw pointers, so `mem.relocatable`
  stays keys-only. Graphs serialize through Encode instead.
- **Fully dynamic topology.** Relations are declared per-field. A bag of
  arbitrary runtime-decided cross-references doesn't fit (root
  `Map<K, Link<T>>` covers the tame cases; reflection-style graphs don't).
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
- DAY_ONE.md drops Handle, the get-dance, and `using Pool<T>`; gains `Link?`
  and two schema clauses that announce themselves.
- boxes.md: the family stays five (`Cell`, `Graph`, `Shared`, `Mutex`,
  `Heap`), with the "identity" discipline upgraded to "relational".

The migration surface is honest: pools.md, boxes.md, context-clauses.md,
relocatable.md, closures.md guidance, every example using `Pool` — plus the
interpreter, the checker, and codegen. This is a big lift; the exploration
docs exist so the decision precedes the lift.
