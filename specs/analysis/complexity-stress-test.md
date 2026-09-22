<!-- id: analysis.complexity -->
<!-- status: decided -->
<!-- summary: ECS game loop stress test measuring cognitive complexity per phase -->

# Complexity Budget Stress Test: ECS Game Loop

Each Rask mechanism is well-motivated in isolation. The question is whether a developer can hold them all in their head when they collide. This traces an ECS game loop with Vulkan rendering and a C physics library — counting concepts per phase against a 7 ± 2 cognitive chunk budget.

## Scenario

A game with:
- **Rack\<Entity>** — position, velocity, health, plus links to other nodes
- **Rack\<Mesh>** — vertex buffer (VkBuffer), index count
- **PhysicsBody** — a `@resource` wrapping a C raw pointer (must be consumed)
- **VulkanDevice** / **PhysicsWorld** — safe wrappers around C FFI

The physics bodies are the interesting part: a `@resource` can't live in *any*
container (`mem.resources/RC1`–RC3), so the C side keeps them and an entity node
stores the raw id. That constraint is not a workaround, it's the finding — see
Phase 3.

## Type Definitions

<!-- test: parse -->
```rask
struct Vec3 {
    x: f32
    y: f32
    z: f32
}

struct Transform {
    position: Vec3
    rotation: Vec3
}

struct Entity {
    position: Vec3
    velocity: Vec3
    health: i32
    mesh: Link<Mesh>?
    body: i64            // the physics library's id for this entity's body
    active: bool
}

struct Mesh {
    vertex_buffer: u64
    index_count: u32
}

struct GameWorld {
    entities: Rack<Entity>
    meshes: Rack<Mesh>
    physics: i64
    vulkan: i64
}
```

**Concept count: 4** — Rack+Link, `Link<T>?` edges, value semantics, safe wrappers.

## Scorecard

| Phase | Concepts | Budget (7±2) | Verdict |
|-------|----------|-------------|---------|
| Type definitions | 4 | PASS | |
| 1: Physics step | 3 | PASS | |
| 2: Sync physics | 5 | PASS | |
| 3: Game logic/destroy | 6 | PASS | |
| 4: Render | 4 | PASS | |
| 5: Parallel | 8 | MARGINAL | |

**What changed.** This page used to fail two phases and blame "ECS-with-FFI sits
at the intersection of all mechanisms". The intersection was smaller than it
looked: three of the five phases were over budget because of the pool, not
because of the domain. Cross-pool handle chaining, the four-step destruction
dance, frozen contexts and `using` clauses are all gone with it
(rask-lang/rask#908), and Phases 2–4 come in under budget with no new mechanism
added. Phase 5 is the one that stayed hard, and it stayed hard for a reason that
has nothing to do with storage: a graph can't cross a task boundary by reference.

## Phase 1: Physics Step

<!-- test: skip -->
```rask
world.physics.step(dt)
```

**Concepts: 3** — borrowing, safe wrapper, FFI. **PASS.**

## Phase 2: Sync Physics → Entities

<!-- test: skip -->
```rask
func sync_physics(mutate world: GameWorld) {
    for e in world.entities.nodes() {
        let transform = world.physics.get_transform(e.body)
        e.position = transform.position
    }
}
```

**Concepts: 5** — rack walk, writing through a link, safe FFI wrapper, borrowing
modes, raw value extraction.

The handle version needed two lookups per entity and a second pool in scope. A
link is the node, so `e.position = …` is a field write. **PASS.**

## Phase 3: Game Logic / Entity Destruction

<!-- test: skip -->
```rask
func update_entities(mutate world: GameWorld) -> void or Error {
    for e in world.entities.nodes() {
        e.health -= 1
        if e.health <= 0 {
            world.physics.destroy_body(e.body)
            world.entities.delete(e)
        }
    }
    return
}
```

**Concepts: 6** — rack walk, writing through a link, delete, FFI cleanup, error
handling, `nodes()` hands back its own Vec so deleting mid-walk is fine.

**The four-step dance is gone**, and so is the collect-then-remove pass: `nodes()`
answers a `Vec<Link<Entity>>` the loop owns, so a delete inside the walk touches
nothing the walk is reading. Anything else pointing at the dead entity —
another entity's `target`, an index `Map` — is nulled by `delete` before it
returns (`mem.racks/RK3`), which is the sweep that used to be written by hand.

The `@resource` didn't survive the move into a container, and that is the
honest cost: `Rack<PhysicsBody>` is rejected (RC2), because `delete` frees a
node rather than handing it back, so nothing could consume it. The body's
lifetime therefore lives on the C side and the Rask side carries an id. That's
one concept the type system isn't checking for you — the thing the old
`Pool<@resource>` rules were buying, at the price of four extra rules and a
runtime guard. **PASS**, with that caveat recorded.

## Phase 4: Render

<!-- test: skip -->
```rask
func render_frame(world: GameWorld) {
    for e in world.entities.nodes() {
        if e.mesh? as mesh {
            draw_mesh(mesh.vertex_buffer, mesh.index_count, e.position)
        }
    }
}
```

**Concepts: 4** — rack walk, optional edge test, following a link, FFI call.

No `frozen`, no context clause, no checked random access: the edge is a
`Link<Mesh>?`, so "is there a mesh" and "here it is" are the same test. **PASS.**

## Phase 5: Parallel Variant

<!-- test: skip -->
```rask
func game_loop_parallel(mutate world: GameWorld, dt: f32) -> void or Error {
    let frame = world.entities.snapshot()

    let render = ThreadPool.spawn(own || {
        for e in frame.nodes() {
            if e.mesh? as mesh {
                draw_mesh(mesh.vertex_buffer, mesh.index_count, e.position)
            }
        }
    })

    let physics = ThreadPool.spawn(|| {
        world.physics.step(dt)
    })

    try render.join()
    try physics.join()
    sync_physics(mutate world)
    try update_entities(mutate world)

    return
}
```

**Concepts: 8** — ThreadPool, spawn, must-use handles, `own` capture, snapshot
(a deep copy), disjoint field capture, join semantics, error handling.
**MARGINAL.**

This is the phase that didn't get easier. A link is an address, so it means
nothing in another task, and `snapshot()` is a full copy of the graph — O(nodes
+ edges) per frame. The pool's handles were plain integers and crossed for free;
that was a real advantage and it's gone. Whether the copy is acceptable depends
on how big the graph is, and nothing in the language will tell you.

## Friction Points

| # | Friction | Severity |
|---|----------|----------|
| 1 | A `@resource` can't live in a rack, so FFI handles are raw ids the compiler doesn't track | MEDIUM |
| 2 | `ensure` ordering for multi-resource cleanup can hide UB | HIGH |
| 3 | Sharing a graph across tasks means copying it | MEDIUM |

## Recommendations

### 1. `ensure` ordering lint for @resource cleanup ([#584](https://github.com/rask-lang/rask/issues/584))

Warn when LIFO ordering might close a dependency before its dependent is drained.

### 2. Measure the snapshot

Phase 5 copies the whole graph per frame. Before designing anything around it,
measure: a few thousand nodes is nothing, a few hundred thousand is a frame
budget. `RASK_RACK_STATS=1` reports what the copy walked.

**Retired with pools:** `pool.remove_with()` for cascading cleanup
([#582](https://github.com/rask-lang/rask/issues/582)) and the max-3-context-clauses
style rule ([#585](https://github.com/rask-lang/rask/issues/585)). The first
existed to shorten the four-step destruction dance, which `delete` does in one
step; the second counted a clause that no longer exists.

Disjoint field borrows in thread closures
([#583](https://github.com/rask-lang/rask/issues/583)) is unaffected — Phase 5
still wants the compiler to see that the physics closure captures only
`world.physics`.

---

## Appendix (non-normative)

### See Also

- `mem.racks` — Rack\<T>, Link\<T>, delete-time edge fixup, snapshots
- `mem.resources` — @resource types, ensure cleanup, the no-container rules
- `conc.async` — spawn, must-use handles
- `mem.borrowing` — inline access, `with` blocks, disjoint field borrowing
