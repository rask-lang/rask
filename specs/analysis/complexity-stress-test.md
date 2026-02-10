# Complexity Budget Stress Test: ECS Game Loop

## Why This Test

Each Rask mechanism is well-motivated in isolation. The question is whether a developer can hold them all in their head when they collide. This document traces a realistic scenario—an ECS game loop with Vulkan rendering and a C physics library—through every phase and counts the concepts a developer needs simultaneously.

The cognitive chunk limit is roughly 7 ± 2. If a phase exceeds that, the design has a complexity budget problem.

## Scenario

A game with:
- **Pool\<Entity>** — position, velocity, health, plus handles into the other two pools
- **Pool\<Mesh>** — vertex buffer (VkBuffer, just a u64), index count
- **Pool\<PhysicsBody>** — `@resource` wrapping a C raw pointer (must be consumed)
- **VulkanDevice** — safe wrapper around Vulkan C FFI
- **PhysicsWorld** — safe wrapper around C physics library (also `@resource`)

Five phases: physics step → sync → game logic → render → parallel variant.

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

**Concept count just for type definitions: 5**

| # | Mechanism |
|---|-----------|
| 1 | Pool\<T> + Handle\<T> (three pools, cross-referencing handles) |
| 2 | @resource (PhysicsBody must be consumed, not dropped) |
| 3 | Pool\<@resource> (bodies pool panics if dropped non-empty) |
| 4 | Value semantics (all types are values, single owner) |
| 5 | Safe wrappers (C pointers hidden behind Rask types) |

---

## Phase 1: Physics Step

<!-- test: skip -->
```rask
// One line. Clean wrapper hides FFI.
world.physics.step(dt)
```

**Concepts: 3** — borrowing (`world` is mutate param), safe wrapper (hides unsafe C call), @resource (PhysicsWorld is borrowed, not consumed).

**Verdict: PASS.** Developer thinks about physics, not language.

---

## Phase 2: Sync Physics → Entities

<!-- test: skip -->
```rask
func sync_physics(mutate world: GameWorld) {
    for h in world.entities.cursor() {
        // Step 1: Copy out the physics body handle (instant view, released at ;)
        const body_handle = world.entities[h].body

        // Step 2: Copy out the C pointer from the bodies pool (instant view)
        const body_ptr = world.bodies[body_handle].rigid_body

        // Step 3: Cross FFI boundary
        const transform = world.physics.get_transform(body_ptr)

        // Step 4: Write back to entity (instant view)
        world.entities[h].position = transform.position
    }
}
```

**Concepts: 8**

| # | Mechanism | Why |
|---|-----------|-----|
| 1 | Cursor iteration | `world.entities.cursor()` |
| 2 | Instant views | `world.entities[h].body` released at semicolon |
| 3 | Cross-pool handles | body_handle from entities pool indexes bodies pool |
| 4 | Two pools active | entities and bodies accessed simultaneously |
| 5 | @resource in pool | bodies pool contains @resource PhysicsBody |
| 6 | Raw value extraction | `.rigid_body` copies out the C handle |
| 7 | Safe FFI wrapper | `get_transform()` wraps unsafe C call |
| 8 | Borrowing modes | `world` is mutably borrowed, two fields accessed |

**Friction: Cross-pool handle chaining.** The developer must:
1. Index `entities` to get `Handle<PhysicsBody>` (instant view, copy out)
2. Index `bodies` with that handle to get the C pointer (instant view, copy out)
3. Pass to FFI wrapper
4. Write result back to `entities`

Each step involves a different mechanism. Three mechanism boundaries in four lines.

**Could context clauses help?** They could if this were a standalone function:
<!-- test: skip -->
```rask
func sync_entity(h: Handle<Entity>)
    with Pool<Entity>,
         bodies: Pool<PhysicsBody>
{
    const body_ptr = h.body.rigid_body   // chained auto-resolution
    const transform = physics.get_transform(body_ptr)
    h.position = transform.position
}
```

But this needs `physics` too, giving us **3 context clauses + 1 parameter** for a 4-line function. The signature is heavier than the body.

**Verdict: MARGINAL.** At 8, this is learnable but at the edge. The cross-pool chaining is the bottleneck.

---

## Phase 3: Game Logic / Entity Destruction

This is the hardest phase. Removing an entity that holds a handle to an @resource in another pool.

<!-- test: skip -->
```rask
func update_entities(mutate world: GameWorld) -> () or Error {
    // Collect doomed entities (can't destroy during cursor iteration
    // because we need to access a DIFFERENT pool)
    let doomed: Vec<Handle<Entity>> = Vec.new()

    for h in world.entities.cursor() {
        world.entities[h].health -= 1

        if world.entities[h].health <= 0 {
            world.entities[h].active = false
            try doomed.push(h)
        }
    }

    // Destruction: 4-step dance per entity
    for h in doomed {
        // Step 1: Remove entity, get owned value
        const entity = world.entities.remove(h).unwrap()

        // Step 2: Extract body handle from owned entity
        const body_handle = entity.body

        // Step 3: Remove PhysicsBody from bodies pool
        const body = world.bodies.remove(body_handle).unwrap()

        // Step 4: Consume the @resource (C cleanup)
        body.close(world.physics)
    }

    return Ok(())
}
```

**Concepts: 10**

| # | Mechanism | Why |
|---|-----------|-----|
| 1 | Cursor iteration | safe iteration with deferred removal |
| 2 | Instant views | `world.entities[h].health` released at semicolon |
| 3 | Move semantics | `remove(h)` returns owned Entity |
| 4 | Cross-pool handles | `entity.body` is Handle\<PhysicsBody> |
| 5 | Pool removal | `world.bodies.remove(body_handle)` |
| 6 | @resource consumption | `body.close()` — must consume, cannot drop |
| 7 | Error handling | `T or Error` return, `try` on push |
| 8 | Handle collection pattern | collect handles first, mutate second |
| 9 | Pool\<@resource> rules | bodies pool panics if dropped non-empty |
| 10 | Ownership transfer | `.remove()` → owned value → `close(take self)` |

**This is the peak.** The developer is simultaneously reasoning about:
- Iteration safety (defer to post-loop because destruction touches a different pool)
- Cross-pool reference integrity (removing entity makes its body handle orphaned)
- Resource consumption obligation (@resource must be explicitly consumed)
- Error propagation (operations can fail)
- Two-phase destruction (collect then destroy)

**The 4-step destruction dance is the core problem.** It's unavoidable given the design:
1. Remove entity → get owned Entity
2. Extract body handle from Entity
3. Remove PhysicsBody from bodies pool
4. Consume PhysicsBody via `close()`

Miss any step → compile error (@resource not consumed) or runtime panic (Pool\<@resource> dropped non-empty).

**Additional friction: `close(take self)` needs PhysicsWorld.** If `body.close(take self, world: PhysicsWorld)` borrows `world.physics`, but `world` is already mutably borrowed by the outer function, the developer must reason about disjoint field borrowing (borrowing `world.physics` while `world.entities` and `world.bodies` are also in use).

**Verdict: FAIL.** 10 concepts exceed the 7 ± 2 budget by 1-3. The destruction dance is the primary contributor.

---

## Phase 4: Render

<!-- test: skip -->
```rask
func render_frame(world: GameWorld) {
    // Freeze for zero-cost read access
    const frozen_entities = world.entities.freeze_ref()
    const frozen_meshes = world.meshes.freeze_ref()

    for h in frozen_entities.handles() {
        const entity = frozen_entities[h]           // zero generation checks
        const mesh = frozen_meshes[entity.mesh]     // zero generation checks

        // Vulkan draw call (unsafe FFI)
        draw_mesh(mesh.vertex_buffer, mesh.index_count, entity.position)
    }
}
```

**Concepts: 6**

| # | Mechanism | Why |
|---|-----------|-----|
| 1 | Frozen pools | `freeze_ref()` for zero-cost iteration |
| 2 | Cross-pool handles | entity.mesh indexes frozen meshes |
| 3 | Zero gen checks | Why frozen pools exist (performance) |
| 4 | Unsafe FFI | draw call wraps Vulkan C function |
| 5 | Read-only borrowing | world borrowed immutably |
| 6 | Two frozen pools | Track which pool resolves which handle |

~~**Key friction: FrozenPool doesn't satisfy `with Pool<T>`.**~~ **RESOLVED.** Unnamed `with Pool<T>` contexts now accept `FrozenPool<T>` (see [pools.md](../memory/pools.md#frozenpool-context-subsumption)). Helper functions written for the update phase work in the render phase without duplication:

<!-- test: skip -->
```rask
func get_mesh_data(h: Handle<Mesh>) with frozen Pool<Mesh> -> u64 {
    return h.vertex_buffer
}

// ✅ Works during render — FrozenPool satisfies frozen context
const frozen_meshes = world.meshes.freeze_ref()
const vb = get_mesh_data(entity.mesh)
```

**Verdict: PASS.** At 6, comfortably within budget.

---

## Phase 5: Parallel Variant

<!-- test: skip -->
```rask
func game_loop_parallel(mutate world: GameWorld, dt: f32) -> () or Error {
    with threading {
        // Snapshot entities for rendering (reads old state)
        const (snapshot, _) = world.entities.snapshot()
        const mesh_snap = world.meshes.freeze_ref()

        // Render previous frame on thread pool
        const render_handle = spawn thread {
            for h in snapshot.handles() {
                const entity = snapshot[h]
                const mesh = mesh_snap[entity.mesh]
                draw_mesh(mesh.vertex_buffer, mesh.index_count, entity.position)
            }
        }

        // Physics on thread pool
        const physics_handle = spawn thread {
            world.physics.step(dt)
        }

        try render_handle.join()
        try physics_handle.join()

        // Sync + game logic on main thread
        sync_physics(world)
        try update_entities(world)
    }
    return Ok(())
}
```

**Concepts: 12**

| # | Mechanism | Why |
|---|-----------|-----|
| 1 | `with threading` | Thread pool declaration |
| 2 | `spawn thread` | Launch work on pool threads |
| 3 | Affine handles | Must join or detach both ThreadHandles |
| 4 | Snapshot isolation | `snapshot()` for concurrent read/write |
| 5 | FrozenPool semantics | Snapshot returns frozen, zero-cost reads |
| 6 | `freeze_ref()` | Meshes frozen separately |
| 7 | Cross-pool handles | entity.mesh in frozen snapshot |
| 8 | Send/Sync constraints | FrozenPool is Sync, can cross threads |
| 9 | Disjoint field access | physics step needs world.physics, render needs world.entities |
| 10 | Error handling | `try handle.join()` |
| 11 | @resource across threads | PhysicsWorld borrowed across thread boundary |
| 12 | Copy-on-write | First mutation after snapshot triggers O(n) copy |

**Friction: Ownership partitioning across threads.** `spawn thread` with `world.physics.step(dt)` needs mutable access to `world.physics`, while the render thread reads `world.entities`. These are disjoint fields. Field projections are specified for function parameters, but the spec doesn't cover `spawn thread` closure captures. The developer might need to destructure the world struct manually.

**Verdict: FAIL.** 12 concepts, ~5 over budget. Thread spawning + snapshot + frozen pools + field disjointness + affine handles pile up beyond what anyone can hold simultaneously.

---

## Scorecard

| Phase | Concepts | Budget (7±2) | Verdict |
|-------|----------|-------------|---------|
| Type definitions | 5 | ✓ | PASS |
| 1: Physics step | 3 | ✓ | PASS |
| 2: Sync physics | 8 | ~ | MARGINAL |
| 3: Game logic/destroy | **10** | ✗ | **FAIL** |
| 4: Render | 6 | ✓ | PASS |
| 5: Parallel | **12** | ✗ | **FAIL** |

**Root cause:** Not any single mechanism. The problem is that ECS-with-FFI sits at the intersection of ALL mechanisms simultaneously. The 4-step destruction dance (Phase 3) and the thread+snapshot+projection pile-up (Phase 5) are where the budget breaks.

**Metrics impact:** Game engines carry only 5% weight in UCC, so failing here doesn't sink the language. But the ED target (≤ 1.2 vs simplest alternative) is at risk—Odin or Jai would handle Phase 3 in 2 lines of explicit memory management.

---

## Friction Points

### 1. ~~FrozenPool doesn't satisfy `with Pool<T>` context~~ RESOLVED

`with frozen Pool<T>` contexts accept both `Pool<T>` and `FrozenPool<T>`. Default `with Pool<T>` contexts are mutable and only accept `Pool<T>`. See [pools.md](../memory/pools.md#frozenpool-context-subsumption).

**Impact:** Read-only helpers declared with `frozen` work in both update and render phases. Phase 4 (render) drops from 7 to 6 concepts.

### 2. Cross-pool handle chaining is verbose

**Evidence:** `h.body` auto-resolves to `entities[h].body` → `Handle<PhysicsBody>`. Then `body_handle.rigid_body` needs bodies pool. This two-level resolution requires both pools in context.

**Impact:** Works, but signatures accumulate context clauses. A function touching 3 pools needs 3+ context clauses.

**Severity:** LOW-MEDIUM.

### 3. @resource consumption during iteration: 4-step dance

**Evidence:** Removing an entity with a handle to a @resource body requires: remove entity → extract handle → remove body → consume body. Cannot be done inside cursor iteration because it touches a different pool.

**Impact:** Unavoidable boilerplate. Easy to miss a step (compile error for @resource, runtime panic for Pool\<@resource> drop).

**Severity:** HIGH.

### 4. `ensure` ordering for multi-resource cleanup

**Evidence:** GameWorld cleanup must drain Pool\<PhysicsBody> BEFORE closing PhysicsWorld (because `body.close()` calls into the C physics world). If `ensure` blocks execute LIFO:

<!-- test: skip -->
```rask
func run_game() -> () or Error {
    let world = try GameWorld.new()

    // ensure runs LIFO: bodies drained first, then physics, then vulkan
    ensure world.vulkan.close()
    ensure world.physics.close()
    ensure world.bodies.take_all_with(|body| { body.close(world.physics) })

    // ... game loop ...
    return Ok(())
}
```

The developer must understand LIFO ordering AND that the last `ensure` runs first AND that `body.close()` needs the physics world to still be alive. Wrong order → C-level use-after-free hidden behind safe-looking code.

**Severity:** HIGH. UB behind safe syntax is the worst kind of bug.

### 5. Context clause explosion in deep call chains

**Evidence:** A helper touching entities, meshes, and physics needs:
<!-- test: skip -->
```rask
func complex_query(h: Handle<Entity>)
    with entities: Pool<Entity>,
         meshes: Pool<Mesh>,
         bodies: Pool<PhysicsBody>
{ ... }
```

Three context clauses. If it calls another helper with its own requirements, they propagate.

**Impact:** Public API boundaries become signature-heavy. Mitigated for private functions (inference), but public APIs pay the full cost.

**Severity:** MEDIUM.

---

## Recommendations

### 1. FrozenPool satisfies read-only context clauses

`with frozen Pool<T>` contexts accept both `Pool<T>` and `FrozenPool<T>`. The `frozen` keyword marks the context as read-only — writes through handles are a compile error.

<!-- test: skip -->
```rask
func get_health(h: Handle<Entity>) with frozen Pool<Entity> -> i32 {
    return h.health
}

// Works: FrozenPool<Entity> satisfies frozen context
const frozen = pool.freeze_ref()
const hp = get_health(some_handle)
```

**Eliminates:** Code duplication between update and render phases.

### 2. `pool.remove_with()` for cascading cleanup

Standard library addition (not a language change). When removing a pool element that references @resource types in other pools:

<!-- test: skip -->
```rask
// Today: 4-step dance
const entity = world.entities.remove(h).unwrap()
const body = world.bodies.remove(entity.body).unwrap()
body.close(world.physics)

// Proposed: 2-step with callback
world.entities.remove_with(h, |entity| {
    const body = world.bodies.remove(entity.body).unwrap()
    body.close(world.physics)
})
```

**Eliminates:** Scattered destruction logic. Collocates entity removal with its cascading cleanup.

### 3. Field projections for `spawn thread` closures

Extend field projections (borrowing.md, P1-P4) to closure captures:

<!-- test: skip -->
```rask
const physics_handle = spawn thread(world.{physics}) {
    world.physics.step(dt)
}
const render_handle = spawn thread(world.{entities, meshes}) {
    // read-only access
}
```

The projection syntax declares which fields the closure captures. Compiler checks disjointness at spawn site. Cost is visible.

**Eliminates:** The need to destructure world structs for parallelism.

### 4. `ensure` ordering lint for @resource cleanup

Warn when `ensure` LIFO ordering might close a dependency before its dependent is drained. Not a hard error (compiler can't know C semantics), but a lint:

```
warning: `ensure world.physics.close()` runs AFTER `ensure world.bodies.take_all_with(...)`
  but the drain callback uses `world.physics`
  = help: ensure blocks run LIFO (last declared = first to run)
  = help: current order looks correct — bodies drain first, then physics closes
```

**Eliminates:** Silent UB from misordered cleanup.

### 5. Style guideline: max 3 context clauses

If a function needs >3, restructure:
- Pass a struct containing the pools
- Use field projections as a single parameter
- Split the function

This is a lint, not a language rule.
