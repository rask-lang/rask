<!-- id: mem.pools -->
<!-- status: decided -->
<!-- summary: Handle-based sparse storage with generation counters for stable references -->
<!-- depends: memory/ownership.md, memory/borrowing.md, memory/resource-types.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Pools and Handles

`Pool<T>` is handle-based sparse storage. Handles are opaque IDs validated at access via generation counters.

## Pool API

| Rule | Operation | Returns | Description |
|------|-----------|---------|-------------|
| **PL1: Create** | `Pool.new()` | `Pool<T>` | Create unbounded pool |
| **PL2: Create bounded** | `Pool.with_capacity(n)` | `Pool<T>` | Create bounded pool |
| **PL3: Insert** | `pool.insert(v)` | `Result<Handle<T>, InsertError>` | Insert, get handle |
| **PL4: Index access** | `pool[h]` | `&T` or `&mut T` | Access (panics if invalid) |
| **PL5: Safe access** | `pool.get(h)` | `Option<T>` | Safe access (T: Copy) |
| **PL6: Remove** | `pool.remove(h)` | `Option<T>` | Remove and return |
| **PL7: Iterate** | `pool.handles()` | `Iterator<Handle<T>>` | Iterate all valid handles |

```rask
const pool: Pool<Entity> = Pool.new()
const h: Handle<Entity> = try pool.insert(entity)
pool[h].health -= 10
pool.remove(h)
```

## Handle Structure

Handles are opaque identifiers with configurable sizes.

| Rule | Description |
|------|-------------|
| **PH1: Opaque** | Handles contain pool_id, index, and generation — all opaque to users |
| **PH2: Copy** | Handle is Copy if total size ≤16 bytes (`mem.value/VS1`) |
| **PH3: Value identity** | Handles are values (like integers), not references — aliased handles are safe |

```rask
Pool<T, PoolId=u32, Index=u32, Gen=u32>  // Defaults

Handle<T> = {
    pool_id: PoolId,   // Unique per pool instance
    index: Index,      // Slot in internal storage
    generation: Gen,   // Version counter
}
```

Default: `4 + 4 + 4 = 12 bytes` (under 16-byte copy threshold).

| Config | Size | Pools | Slots | Gens | Use Case |
|--------|------|-------|-------|------|----------|
| `Pool<T>` | 12 bytes | 4B | 4B | 4B | General purpose (default) |
| `Pool<T, Gen=u64>` | 16 bytes | 4B | 4B | ∞ | High-churn scenarios |
| `Pool<T, PoolId=u16, Index=u16, Gen=u32>` | 8 bytes | 64K | 64K | 4B | Memory-constrained |

## Handle Validation

Every access validates the handle.

| Check | Failure mode |
|-------|--------------|
| Pool ID mismatch | Panic: "handle from wrong pool" |
| Generation mismatch | Panic: "stale handle" |
| Index out of bounds | Panic: "invalid handle index" |

**Safe access (no panic):**
```rask
pool.get(h)   // Returns Option<T> (T: Copy)
pool.read(h, |v| ...)    // Returns Option<R>
pool.modify(h, |v| ...)  // Returns Option<R>
```

**Generation overflow:** Saturating semantics. When a slot's generation reaches max, the slot becomes permanently unusable. Practically never happens (~4B cycles per slot with default u32). For extreme high-churn: `Pool<T, Gen=u64>`.

## Expression-Scoped Access

Pool access follows instant-view borrowing rules (`mem.borrowing/B1`). Borrow released at semicolon.

```rask
pool[h].health -= damage     // Borrow released
if pool[h].health <= 0 {     // New borrow
    pool.remove(h)           // No active borrow - OK
}
```

Aliased handles are safe because each `pool[h]` creates an independent expression-scoped borrow:

```rask
const h1 = try pool.insert(entity)
const h2 = h1  // h2 is a copy - both point to same entity

pool[h1].health -= 10    // Borrow released at semicolon
pool[h2].health -= 10    // New borrow - OK
```

## Multi-Statement Access

Closure-based access for multi-statement operations.

| Method | Signature | Use Case |
|--------|-----------|----------|
| `read(h, f)` | `func(T) -> R -> Option<R>` | Multi-statement read |
| `modify(h, f)` | `func(T) -> R -> Option<R>` | Multi-statement mutation |

```rask
try pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
})
```

The closure borrows the collection exclusively — no other collection access inside it:
<!-- test: compile-fail -->
```rask
pool.modify(h, |entity| {
    entity.health -= 10
    pool.remove(h)    // ERROR: pool borrowed by closure
})
```

## Cursor Iteration

`pool.cursor()` provides zero-allocation iteration with safe removal.

| Rule | Description |
|------|-------------|
| **PF1: Current removal OK** | Removing the current element is always safe |
| **PF2: Other removal OK** | Removing other elements is safe (cursor adjusts) |
| **PF3: Insertion deferred** | Insertions during iteration may or may not be visited |
| **PF4: No double-visit** | Each existing element visited at most once |

```rask
for h in pool.cursor() {
    pool[h].update()
    if pool[h].expired {
        pool.remove(h)      // Safe during iteration
    }
}
```

**Drain cursor** — remove and yield ownership:
```rask
for entity in pool.drain() {
    entity.cleanup()
}
// pool is now empty
```

## Weak Handles

`pool.weak(h)` creates handles that can be checked for validity before use.

| Method | Returns | Description |
|--------|---------|-------------|
| `weak.valid()` | `bool` | True if underlying data still exists |
| `weak.upgrade()` | `Option<Handle<T>>` | Convert to strong handle if valid |

Automatically invalidated when the element is removed, the pool is cleared, or the pool is dropped.

| Scenario | Use |
|----------|-----|
| Local function access | Regular `Handle<T>` |
| Stored alongside pool | Regular `Handle<T>` |
| Event queue / callbacks | `WeakHandle<T>` |
| Cross-task communication | `WeakHandle<T>` |
| Cache that may be invalidated | `WeakHandle<T>` |

## Frozen Pools

`pool.freeze()` returns an immutable view where all generation checks are skipped.

| Rule | Description |
|------|-------------|
| **PF5: Freeze guarantees** | No insert/remove while frozen — all handles valid at freeze time remain valid |
| **PF6: Zero checks** | Generation matching is guaranteed, so checks are skipped |
| **PF7: Stale handle UB** | Using a handle that was invalid at freeze time is undefined behavior |

```rask
const frozen = pool.freeze()
for h in frozen.handles() {
    render(frozen[h])       // Zero generation checks
}
const pool = frozen.thaw()
```

**Freezing API:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.freeze()` | `Pool<T> -> FrozenPool<T>` | Freeze, consume ownership |
| `pool.freeze_ref()` | `Pool<T> -> FrozenPool<T>` | Freeze reference (scoped) |
| `pool.with_frozen(f)` | `(|FrozenPool<T>| -> R) -> R` | Scoped freeze |

**NOT available on FrozenPool:** `insert()`, `remove()`, `modify()`, `clear()`

### FrozenPool Context Subsumption

Context clauses (`mem.context`) default to mutable. Add `frozen` for read-only — both `Pool<T>` and `FrozenPool<T>` satisfy it.

```rask
// Default — mutable, only Pool<T>
func damage(h: Handle<Entity>, amount: i32) using Pool<Entity> {
    h.health -= amount
}

// Frozen — read-only, accepts both Pool<T> and FrozenPool<T>
func get_health(h: Handle<Entity>) using frozen Pool<Entity> -> i32 {
    return h.health
}
```

| Context | `Pool<T>` | `FrozenPool<T>` |
|---------|-----------|-----------------|
| `using Pool<T>` | Accepted | Rejected |
| `using frozen Pool<T>` | Accepted | Accepted |
| `using name: Pool<T>` | Accepted | Rejected |
| `using frozen name: Pool<T>` | Accepted | Accepted |

Writing through handles in a `frozen` context is a compile error. Private functions can have `frozen` inferred; public functions must declare it.

## Generation Check Coalescing

The compiler eliminates redundant generation checks. See `comp.gen-coalesce` for the full algorithm.

| Rule | Description |
|------|-------------|
| **PF8: Same handle** | Only coalesce accesses to the same handle variable |
| **PF9: No intervening mutation** | No `pool.insert()`, `pool.remove()` between accesses |
| **PF10: No reassignment** | Handle variable not reassigned between accesses |
| **PF11: Local analysis** | Coalescing within function scope only |

```rask
pool[h].x = 1
pool[h].y = 2    // Same check as above — coalesced
pool[h].z = 3    // Same check as above — coalesced
// Total: ONE generation check
```

## Context Clauses (Auto-Resolution)

Handles auto-resolve fields without explicitly naming the pool. See `mem.context` for full specification.

```rask
func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount    // Auto-resolves via Pool<Player> context
}

const players = Pool.new()
const h = try players.insert(Player { health: 100 })
damage(h, 10)    // Compiler passes players as hidden parameter
```

**Unnamed context** — field access only. **Named context** — field access + structural operations:

```rask
func cleanup(h: Handle<Entity>) using entities: Pool<Entity> {
    h.active = false              // Field access via auto-resolution
    if h.health <= 0 {
        entities.remove(h)        // Structural op via named pool
    }
}
```

## Pool Partitioning

Split a pool for parallel processing.

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.with_partition(n, f)` | `(usize, \|[FrozenChunk<T>]\| -> R) -> R` | Scoped read-only partition |
| `pool.with_partition_mut(n, f)` | `(usize, \|[MutableChunk<T>]\| -> R) -> R` | Scoped read-write partition |
| `pool.snapshot()` | `Pool<T> -> (FrozenPool<T>, Pool<T>)` | Copy-on-write snapshot isolation |

```rask
entities.with_partition(4, |chunks| {
    parallel_for(chunks) { |chunk|
        for h in chunk.handles() {
            analyze(chunk[h])  // Zero generation checks (frozen)
        }
    }
})  // Auto-reunifies here
```

**Snapshot isolation** — frozen snapshot for readers while writer continues:

```rask
let (snapshot, mut pool) = entities.snapshot()

// Readers see frozen state (zero checks)
spawn(|| { render_frame(snapshot) }

// Writer can mutate concurrently
try pool.insert(new_entity)
pool.remove(dead_entity)
```

First mutation after snapshot triggers O(n) copy-on-write.

## Linear Types in Pools

Resource types (`mem.resources`) have special rules in pools.

| Collection | Resource allowed? | Reason |
|------------|-------------------|--------|
| `Vec<Resource>` | No | Vec drop can't propagate errors |
| `Pool<Resource>` | Yes | Explicit removal required anyway |

When `Pool<Resource>` is dropped non-empty: runtime panic (`mem.resources/R5`).

```rask
// Required: consume all before pool drops
for file in files.take_all() {
    try file.close()
}

// Or with ensure:
ensure files.take_all_with(|f| { f.close(); })
```

## Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Pool<T>` | if `T: Send` | if `T: Sync` |
| `Handle<T>` | Yes (Copy) | Yes |
| `WeakHandle<T>` | Yes (Copy) | Yes |
| `FrozenPool<T>` | if `T: Send` | Yes (immutable) |
| `FrozenChunk<T>` | if `T: Send` | if `T: Sync` |
| `MutableChunk<T>` | if `T: Send` | No |

## Capacity and Allocation

| Rule | Description |
|------|-------------|
| **PL8: All inserts fallible** | `insert()` returns `Result<Handle<T>, InsertError<T>>` |
| **PL9: Handle stability** | Handles remain valid when pools grow (index-based, not pointer-based) |

```rask
try pool.insert(x)   // Result<Handle<T>, InsertError<T>>

enum InsertError<T> {
    Full(T),   // Bounded pool at capacity
    Alloc(T),  // Allocation failed
}
```

| Method | Returns | Semantics |
|--------|---------|-----------|
| `pool.len()` | `usize` | Current element count |
| `pool.capacity()` | `Option<usize>` | `None` = unbounded |
| `pool.remaining()` | `Option<usize>` | Slots available |

| Pool type | Growth | Use case |
|-----------|--------|----------|
| `Pool.new()` | Auto-grows (like Vec) | General purpose |
| `Pool.with_capacity(n)` | Never grows | Hot paths, real-time |

## Performance Escape Hatches

### Safe: Validated Access (`with_valid`)

Validates once at entry, then provides unchecked access inside the closure.

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.with_valid(h, f)` | `(Handle<T>, \|T\| -> R) -> Option<R>` | One check, then read |
| `pool.with_valid_mut(h, f)` | `(Handle<T>, \|T\| -> R) -> Option<R>` | One check, then write |

### Unsafe: Unchecked Access

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.get_unchecked(h)` | `unsafe (Handle<T>) -> T` | Zero-check read |
| `pool.get_mut_unchecked(h)` | `unsafe (Handle<T>) -> T` | Zero-check write |

**Safety requirements:** Handle was obtained from this pool, has not been removed, and no concurrent mutation.

## Error Messages

**Stale handle access [PL4]:**
```
ERROR [mem.pools/PL4]: stale handle access
   |
5  |  pool.remove(h)
   |  ^^^^^^^^^^^^^^ handle invalidated here
8  |  pool[h].update()
   |  ^^^^^^^ handle is stale (generation mismatch)

WHY: Handles are validated by generation counter. After removal, the slot's
     generation is incremented, so old handles no longer match.

FIX: Check validity before access:

  if pool.get(h) is Some(val) {
      // handle is still valid
  }
```

**Pool borrowed by closure [PF8]:**
```
ERROR [mem.pools/PF8]: cannot mutate pool while borrowed by closure
   |
2  |  pool.modify(h, |entity| {
   |       ------ pool is exclusively borrowed here
3  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ cannot mutate while borrowed

FIX: Separate the check from the mutation:

  const should_remove = pool[h].health <= 0
  if should_remove {
      pool.remove(h)
  }
```

**Frozen context write [PF5]:**
```
ERROR [mem.pools/PF5]: cannot write through handle in frozen context
   |
1  |  func bad(h: Handle<Entity>) using frozen Pool<Entity> {
   |                                     ------ context is frozen
2  |      h.health -= 10
   |      ^^^^^^^^^^^^^^ cannot write in frozen context
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Stale handle access | PL4 | Panic on `pool[h]`, None on `pool.get(h)` |
| Wrong-pool handle | PL4 | Panic on `pool[h]`, None on `pool.get(h)` |
| `modify_many([h, h], _)` | — | Panic (duplicate index) |
| Generation overflow | PH1 | Slot becomes permanently dead |
| Pool ID overflow | PH1 | Panic (runtime error) |
| Closure panics in `modify` | — | Pool left in valid state |
| Empty pool cursor | PF1 | `next()` returns None immediately |
| Nested cursors | — | Compile error (pool already borrowed) |
| Drop Pool<Resource> while non-empty | R5 | Runtime panic |
| `clear()` on Pool<Resource> | — | Compile error (would abandon resources) |
| `get_unchecked` with stale handle | — | **Undefined behavior** |
| `with_partition(0, f)` | — | Compile error (n must be > 0) |
| Partition while borrowed | — | Compile error |
| Drop snapshot while readers active | — | Safe (refcount keeps data alive) |

## Examples

### Game Entity System

```rask
func update_game(mut entities: Pool<Entity>, dt: f32) {
    for h in entities.cursor() {
        h.velocity += h.acceleration * dt
        h.position += h.velocity * dt

        if h.health <= 0 {
            h.on_death()
            entities.remove(h)
        }
    }
}
```

### Graph with Handles

```rask
struct Node {
    data: string,
    edges: Vec<Handle<Node>>,
}

func build_graph() -> Pool<Node> or Error {
    const nodes = Pool.new()

    const a = try nodes.insert(Node { data: "A", edges: Vec.new() })
    const b = try nodes.insert(Node { data: "B", edges: Vec.new() })
    const c = try nodes.insert(Node { data: "C", edges: Vec.new() })

    try nodes[a].edges.push(b)
    try nodes[a].edges.push(c)
    try nodes[b].edges.push(c)

    Ok(nodes)
}
```

### Rendering Pipeline

```rask
func render_frame(world: World) {
    // Physics update (needs mutation)
    for h in world.entities.cursor() {
        world.entities[h].update_physics()
    }

    // Render pass (read-only, zero-cost)
    const frozen = world.entities.freeze()
    for h in frozen.handles() {
        renderer.draw(frozen[h])
    }
    world.entities = frozen.thaw()
}
```

---

## Appendix (non-normative)

### Rationale

**PL1–PL7 (pool design):** Entity systems, graphs with cycles, observers — they need stable references that survive mutations. Rust's borrow checker makes this painful without `Rc`/`RefCell`. Generation counters detect stale handles at O(1), expression-scoped access enables interleaved mutation, and handles are values with no lifetime parameters.

**PH3 (handle identity):** Handles are database primary keys. You can have 10 copies of the key `42` — they all access the same row. The keys aren't borrowed; only the access is.

**PF5–PF7 (frozen pools):** When frozen, no structural changes can happen, so generation checks are redundant. ~10% faster for access-heavy code.

**PL9 (handle stability):** This is why handles exist — stable identifiers that don't break when memory moves. Pointers would become dangling; handles never do.

### Patterns & Guidance

**Choosing bounded vs unbounded:**

| Pool type | Use when |
|-----------|----------|
| `Pool.new()` | General purpose, prototyping, non-hot paths |
| `Pool.with_capacity(n)` | Hot paths, real-time, allocation predictability matters |

**Memory budgeting:**
```rask
const players = Pool.with_capacity(64)     // Max 64 players
const bullets = Pool.with_capacity(10000)  // Lots of bullets
const configs = Pool.with_capacity(100)    // Modest config count
```

Pool reuse: clear and reuse instead of drop and recreate.

**Closure aliasing prevention patterns:**
```rask
// Pattern 1: Separate pools for different operations
entities.modify(h, |e| {
    events.insert(Event.Died(h))    // OK: different pool
})

// Pattern 2: Restructure logic
const should_remove = pool[h].health <= 0
if should_remove {
    pool.remove(h)    // OK: not inside closure
}

// Pattern 3: Shared borrows are OK
pool.read(h, |e| {
    const other = pool.get(h2)    // OK: both are reads
})
```

**Pool partitioning — parallel physics:**
```rask
func physics_tick(mut entities: Pool<Entity>, dt: f32) {
    // Phase 1: Parallel read (compute forces)
    const forces = entities.with_partition(num_cpus(), |chunks| {
        parallel_map(chunks) { |chunk|
            chunk.handles().map(|h| {
                const e = chunk[h]
                (h, compute_forces(e.position, e.mass))
            }).collect<Vec<_>>()
        }
    });

    // Phase 2: Serial write (apply forces)
    for (h, force) in forces.into_iter().flatten() {
        entities[h].velocity += force * dt
        entities[h].position += entities[h].velocity * dt
    }
}
```

**Self-referential patterns:** Doubly-linked lists, trees with parent pointers, graphs with cycles, and arena-allocated ASTs all use the pool+handle pattern. Key guidance:
- Use `freeze_ref()` for read-only traversal (zero generation checks)
- Stale handles in graphs are detected on access — run `gc_edges()` periodically
- ASTs: build once, then freeze for analysis passes

**Shared state:** Share handles, not data. Handles are 12-byte copyable values that can be sent anywhere. The pool stays in one thread; commands flow back to it.

**Emergent capabilities:**
- **Serialization:** Handles are integers (pool_id, index, generation) — no pointer fixup needed. An entire pool graph survives serialization and deserialization.
- **Plugin isolation:** Dropping a pool invalidates all its handles. Stale handles fail safely (panic or `None`), never undefined behavior.

**Handle filtering (manual partitioning):**

Pool cannot be split by predicate because accessing entities requires borrowing the pool. Instead, partition handles:

```rask
func process_by_type(mut entities: Pool<Entity>) {
    let player_handles = Vec.new()
    let enemy_handles = Vec.new()

    for h in entities.cursor() {
        match entities[h].kind {
            EntityKind.Player => player_handles.push(h),
            EntityKind.Enemy => enemy_handles.push(h),
        }
    }

    for h in player_handles { entities[h].update_player() }
    for h in enemy_handles { entities[h].update_enemy() }
}
```

**Fragmentation tradeoff:** Many types = many pools. Memory can't be shared across pools. I chose type safety and simplicity over memory efficiency.

### Performance Characteristics

| Operation | Cost | Notes |
|-----------|------|-------|
| `pool[h]` | ~1 ns | Generation check + index |
| `pool.get(h)` | ~1 ns | Same + Option wrap |
| Frozen `frozen[h]` | ~0 ns | No generation check |
| `insert()` | Amortized O(1) | May allocate (unbounded) |
| `remove()` | O(1) | Bumps generation |
| `cursor()` iteration | O(capacity) | Scans all slots |
| `with_partition(n, f)` | O(1) setup/teardown | Auto-reunifies |
| `snapshot()` | O(1) | CoW on first mutation |
| First mutation after snapshot | O(n) | Triggers copy-on-write |
| Registry lookup | ~1-2 ns | Thread-local HashMap |

### IDE Integration

**Observer pattern with weak handles:**
```rask
struct Observable<T> {
    value: T,
    observers: Vec<WeakHandle<Observer>>,
}

extend Observable<T> {
    func set(self, value: T, pool: Pool<Observer>) {
        self.value = value
        self.observers.retain(|weak| {
            if weak.upgrade() is Some(h) {
                pool[h].notify(self.value);
                true
            } else {
                false
            }
        })
    }
}
```

### See Also

- [Borrowing](borrowing.md) — Expression-scoped views for growable sources (`mem.borrowing`)
- [Resource Types](resource-types.md) — Resource consumption in pools (`mem.resources`)
- [Context Clauses](context-clauses.md) — Handle auto-resolution (`mem.context`)
- [Closures](closures.md) — Pool+Handle pattern for shared mutable state (`mem.closures`)
- [Collections](../stdlib/collections.md) — Vec and Map types (`std.collections`)
- [Aliasing Detection](aliasing-detection.md) — Compile-time closure aliasing analysis (`mem.aliasing`)
- [Generation Coalescing](../compiler/generation-coalescing.md) — Check elimination algorithm (`comp.gen-coalesce`)
