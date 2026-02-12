<!-- id: analysis.complexity -->
<!-- status: decided -->
<!-- summary: ECS game loop stress test measuring cognitive complexity per phase -->

# Complexity Budget Stress Test: ECS Game Loop

Each Rask mechanism is well-motivated in isolation. The question is whether a developer can hold them all in their head when they collide. This traces an ECS game loop with Vulkan rendering and a C physics library — counting concepts per phase against a 7 ± 2 cognitive chunk budget.

## Scenario

A game with:
- **Pool\<Entity>** — position, velocity, health, plus handles into other pools
- **Pool\<Mesh>** — vertex buffer (VkBuffer), index count
- **Pool\<PhysicsBody>** — `@resource` wrapping a C raw pointer (must be consumed)
- **VulkanDevice** / **PhysicsWorld** — safe wrappers around C FFI

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
    mesh: Handle<Mesh>
    body: Handle<PhysicsBody>
    active: bool
}

struct Mesh {
    vertex_buffer: u64
    index_count: u32
}

@resource
struct PhysicsBody {
    rigid_body: i64
}

struct GameWorld {
    entities: Pool<Entity>
    meshes: Pool<Mesh>
    bodies: Pool<PhysicsBody>
    physics: i64
    vulkan: i64
}
```

**Concept count: 5** — Pool+Handle, @resource, Pool\<@resource>, value semantics, safe wrappers.

## Scorecard

| Phase | Concepts | Budget (7±2) | Verdict |
|-------|----------|-------------|---------|
| Type definitions | 5 | PASS | |
| 1: Physics step | 3 | PASS | |
| 2: Sync physics | 8 | MARGINAL | |
| 3: Game logic/destroy | **10** | **FAIL** | |
| 4: Render | 6 | PASS | |
| 5: Parallel | **12** | **FAIL** | |

**Root cause:** Not any single mechanism. ECS-with-FFI sits at the intersection of ALL mechanisms simultaneously. The 4-step destruction dance (Phase 3) and thread+snapshot+projection pile-up (Phase 5) break the budget.

**Metrics impact:** Game engines carry only 5% weight in UCC, so failing here doesn't sink the language. But ED target (≤ 1.2 vs simplest alternative) is at risk — Odin or Jai would handle Phase 3 in 2 lines.

## Phase 1: Physics Step

<!-- test: skip -->
```rask
world.physics.step(dt)
```

**Concepts: 3** — borrowing, safe wrapper, @resource borrow. **PASS.**

## Phase 2: Sync Physics → Entities

<!-- test: skip -->
```rask
func sync_physics(mutate world: GameWorld) {
    for h in world.entities.cursor() {
        const body_handle = world.entities[h].body
        const body_ptr = world.bodies[body_handle].rigid_body
        const transform = world.physics.get_transform(body_ptr)
        world.entities[h].position = transform.position
    }
}
```

**Concepts: 8** — cursor iteration, instant views, cross-pool handles, two pools active, @resource in pool, raw value extraction, safe FFI wrapper, borrowing modes.

**Friction:** Cross-pool handle chaining — three mechanism boundaries in four lines. **MARGINAL.**

## Phase 3: Game Logic / Entity Destruction

<!-- test: skip -->
```rask
func update_entities(mutate world: GameWorld) -> () or Error {
    let doomed: Vec<Handle<Entity>> = Vec.new()

    for h in world.entities.cursor() {
        world.entities[h].health -= 1
        if world.entities[h].health <= 0 {
            world.entities[h].active = false
            try doomed.push(h)
        }
    }

    for h in doomed {
        const entity = world.entities.remove(h).unwrap()
        const body_handle = entity.body
        const body = world.bodies.remove(body_handle).unwrap()
        body.close(world.physics)
    }

    return Ok(())
}
```

**Concepts: 10** — cursor iteration, instant views, move semantics, cross-pool handles, pool removal, @resource consumption, error handling, handle collection pattern, Pool\<@resource> rules, ownership transfer.

**The 4-step destruction dance is unavoidable:** remove entity → extract handle → remove body → consume body. Miss any step → compile error or runtime panic. **FAIL.**

## Phase 4: Render

<!-- test: skip -->
```rask
func render_frame(world: GameWorld) {
    const frozen_entities = world.entities.freeze_ref()
    const frozen_meshes = world.meshes.freeze_ref()

    for h in frozen_entities.handles() {
        const entity = frozen_entities[h]
        const mesh = frozen_meshes[entity.mesh]
        draw_mesh(mesh.vertex_buffer, mesh.index_count, entity.position)
    }
}
```

**Concepts: 6** — frozen pools, cross-pool handles, zero gen checks, unsafe FFI, read-only borrowing, two frozen pools. **PASS.**

## Phase 5: Parallel Variant

<!-- test: skip -->
```rask
func game_loop_parallel(mutate world: GameWorld, dt: f32) -> () or Error
    using ThreadPool
{
    const (snapshot, _) = world.entities.snapshot()
    const mesh_snap = world.meshes.freeze_ref()

    const render_handle = ThreadPool.spawn({
        for h in snapshot.handles() {
            const entity = snapshot[h]
            const mesh = mesh_snap[entity.mesh]
            draw_mesh(mesh.vertex_buffer, mesh.index_count, entity.position)
        }
    }

    const physics_handle = ThreadPool.spawn({
        world.physics.step(dt)
    }

    try render_handle.join()
    try physics_handle.join()
    sync_physics(world)
    try update_entities(world)

    return Ok(())
}
```

**Concepts: 12** — ThreadPool, spawn thread, affine handles, snapshot isolation, FrozenPool, freeze_ref, cross-pool handles, Send/Sync constraints, disjoint field access, error handling, @resource across threads, copy-on-write. **FAIL.**

## Friction Points

| # | Friction | Severity |
|---|----------|----------|
| 1 | Cross-pool handle chaining is verbose | LOW-MEDIUM |
| 2 | @resource consumption during iteration: 4-step dance | HIGH |
| 3 | `ensure` ordering for multi-resource cleanup can hide UB | HIGH |
| 4 | Context clause explosion in deep call chains | MEDIUM |

## Recommendations

### 1. `pool.remove_with()` for cascading cleanup

<!-- test: skip -->
```rask
// Today: 4-step dance
const entity = world.entities.remove(h).unwrap()
const body = world.bodies.remove(entity.body).unwrap()
body.close(world.physics)

// Proposed: callback collocates cleanup
world.entities.remove_with(h, |entity| {
    const body = world.bodies.remove(entity.body).unwrap()
    body.close(world.physics)
})
```

### 2. Field projections for `spawn thread` closures

<!-- test: skip -->
```rask
const physics_handle = spawn thread(world.{physics}) {
    world.physics.step(dt)
}
```

### 3. `ensure` ordering lint for @resource cleanup

Warn when LIFO ordering might close a dependency before its dependent is drained.

### 4. Style guideline: max 3 context clauses

If a function needs >3, restructure (pass struct, use field projections, split function). Lint, not language rule.

---

## Appendix (non-normative)

### See Also

- `mem.pools` — Pool\<T>, Handle\<T>, frozen pools, snapshots
- `mem.resources` — @resource types, ensure cleanup
- `mem.context` — context clauses
- `conc.async` — spawn, affine handles
- `mem.borrowing` — instant views, field projections
