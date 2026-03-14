<!-- id: mem.pools -->
<!-- status: decided -->
<!-- summary: Handle-based sparse storage with generation counters, `with`-based multi-statement access -->
<!-- depends: memory/ownership.md, memory/borrowing.md, memory/resource-types.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Pools and Handles

`Pool<T>` is handle-based sparse storage. Handles are opaque IDs validated at access via generation counters.

## Pool API

| Rule | Operation | Returns | Description |
|------|-----------|---------|-------------|
| **PL1: Create** | `Pool.new()` | `Pool<T>` | Create unbounded pool |
| **PL2: Create bounded** | `Pool.with_capacity(n)` | `Pool<T>` | Create bounded pool |
| **PL3: Insert** | `pool.insert(v)` | `Handle<T>` | Insert, get handle (panics on failure) |
| **PL4: Index access** | `pool[h]` | `&T` or `&mut T` | Access (panics if invalid) |
| **PL5: Safe access** | `pool.get(h)` | `Option<T>` | Safe access (T: Copy) |
| **PL6: Remove** | `pool.remove(h)` | `Option<T>` | Remove and return |
| **PL7: Iterate** | `pool.handles()` | `Iterator<Handle<T>>` | Iterate all valid handles |

```rask
const pool: Pool<Entity> = Pool.new()
const h: Handle<Entity> = pool.insert(entity)
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
```

**Generation overflow:** Saturating semantics. When a slot's generation reaches max, the slot becomes permanently unusable. Practically never happens (~4B cycles per slot with default u32). For extreme high-churn: `Pool<T, Gen=u64>`.

## Inline Expression Access

Pool access uses inline expression access (`mem.borrowing/E1`). Each access is a temporary borrow for the expression.

```rask
pool[h].health -= damage     // Inline access
if pool[h].health <= 0 {     // New inline access
    pool.remove(h)           // No active borrow - OK
}
```

Aliased handles are safe because each `pool[h]` creates an independent temporary access:

```rask
const h1 = pool.insert(entity)
const h2 = h1  // h2 is a copy - both point to same entity

pool[h1].health -= 10    // Access ends after expression
pool[h2].health -= 10    // New access - OK
```

## Multi-Statement Access

Use `with` for multi-statement operations on pool elements (`mem.borrowing/W1`).

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}

// One-liner shorthand
with pool[h] as e: e.health -= damage
```

Pool handles survive reallocation (PL9), so `insert` and `remove(other)` are allowed inside `with` blocks — the compiler re-resolves bindings after each structural mutation (`mem.borrowing/W2a`, `W2b`). Removing the bound handle or clearing the pool remain compile errors (`W2c`, `W2d`):

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.health -= pool[other_h].bonus    // OK: read other element
    const ally = pool.insert(new_ally)  // OK: re-resolves entity  [re-resolved]
    entity.allies.push(ally)                // entity still valid
    pool.remove(expired_h)                  // OK: re-resolves  [re-resolved]
}
```

<!-- test: compile-fail -->
```rask
with pool[h] as entity {
    pool.remove(h)    // ERROR: removing the bound element (W2c)
}
```

`return`, `try`, `break`, and `continue` work naturally inside `with` blocks:
<!-- test: skip -->
```rask
func apply_buff(pool: Pool<Entity>, h: Handle<Entity>) -> () or Error {
    with pool[h] as entity {
        entity.strength += 10
        try log_buff_applied(entity.id)   // propagates to function
    }
}
```

## Iteration

Pools yield handles by default — consistent with their identity-based design. Handles are the primary abstraction; if you just need values, use `.values()`.

**Handle mode (default)** — snapshot iteration, safe for mutation and removal:

```rask
for h in pool {
    pool[h].update()
    if pool[h].expired {
        pool.remove(h)      // Safe: iterating a snapshot
    }
}
```

`for h in pool` copies all active handles into a temporary Vec, then iterates it. The pool is not borrowed during iteration, so mutation and removal are always safe.

| Rule | Description |
|------|-------------|
| **PF1: Current removal OK** | Removing the current element is always safe |
| **PF2: Other removal OK** | Removing other elements is safe |
| **PF3: Insertion ignored** | Elements inserted during iteration are not visited |
| **PF4: No double-visit** | Each existing element visited at most once |

**Value mode** — `pool.values()` provides read-only borrowed iteration:
```rask
for entity in pool.values() {
    print(entity.name)
    entity.render()
}
```

**Mutable mode** — `for mutate` provides in-place element mutation (`std.iteration/I4`):
```rask
for mutate entity in pool {
    entity.health -= 10
    entity.velocity *= 0.9
}
```

Structural mutation (insert/remove) is forbidden during mutable iteration. Use handle mode for that.

**Entries mode** — `pool.entries()` yields both:
```rask
for (h, entity) in pool.entries() {
    if entity.expired {
        pool.remove(h)
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

Automatically invalidated when the element is removed, the pool is cleared, or the pool goes out of scope.

| Scenario | Use |
|----------|-----|
| Local function access | Regular `Handle<T>` |
| Stored alongside pool | Regular `Handle<T>` |
| Event queue / callbacks | `WeakHandle<T>` |
| Cross-task communication | `WeakHandle<T>` |
| Cache that may be invalidated | `WeakHandle<T>` |

## Frozen Context

The `frozen` modifier on context clauses (`mem.context/CC3`) means "no structural mutations." Writing through handles, inserting, removing, and clearing are all compile errors.

| Rule | Description |
|------|-------------|
| **PF5: Frozen guarantees** | No insert/remove/clear/write in `using frozen Pool<T>` context |
| **PF6: Effect inference** | Private functions can have `frozen` inferred; public functions must declare it |

```rask
// Default — mutable
func damage(h: Handle<Entity>, amount: i32) using Pool<Entity> {
    h.health -= amount
}

// Frozen — read-only, no structural mutations
func get_health(h: Handle<Entity>) using frozen Pool<Entity> -> i32 {
    return h.health    // Generation-checked
}
```

In frozen contexts, the compiler may eliminate generation checks during iteration since structural mutations are impossible (see `comp.gen-coalesce`). This is a compiler optimization, not a separate type.

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
const h = players.insert(Player { health: 100 })
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

## Snapshot

`pool.snapshot()` creates a shallow clone for concurrent read access while the original continues to be mutated.

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.snapshot()` | `Pool<T> -> (Pool<T>, Pool<T>)` | Returns `(snapshot, original)` — snapshot is an independent clone |

<!-- test: skip -->
```rask
let (snapshot, mut pool) = entities.snapshot()

// Readers see frozen state
spawn(|| { render_frame(snapshot) })

// Writer can mutate concurrently
pool.insert(new_entity)
pool.remove(dead_entity)
```

First mutation after `snapshot()` triggers an O(n) copy. The snapshot is a regular `Pool<T>` — use it with `using frozen Pool<T>` functions to enforce read-only access at the call site.

## Linear Types in Pools

Resource types (`mem.resources`) have special rules in pools.

| Collection | Resource allowed? | Reason |
|------------|-------------------|--------|
| `Vec<Resource>` | No | Vec cleanup can't propagate errors |
| `Pool<Resource>` | Yes | Explicit removal required anyway |

When `Pool<Resource>` goes out of scope non-empty: runtime panic (`mem.resources/R5`).

```rask
// Required: consume all before pool goes out of scope
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

## Capacity and Allocation

| Rule | Description |
|------|-------------|
| **PL8: Insert panics on failure** | `insert()` returns `Handle<T>`, panics if bounded pool is full. Fallible `try_insert()` returns `Result<Handle<T>, InsertError<T>>` |
| **PL9: Handle stability** | Handles remain valid when pools grow (index-based, not pointer-based) |

```rask
const h = pool.insert(x)           // Handle<T> — panics if full
const h = try pool.try_insert(x)   // Result<Handle<T>, InsertError<T>>

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

**Structural mutation inside with block [W2]:**
```
ERROR [mem.borrowing/W2]: cannot structurally mutate collection inside with block
   |
2  |  with pool[h] as entity {
   |  ---- element borrowed here
3  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ structural mutation not allowed inside with block

WHY: insert, remove, and clear can invalidate the borrowed element.
     Reading and writing other elements is fine.

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

FIX: Remove the frozen annotation if mutation is needed:

  func bad(h: Handle<Entity>) using Pool<Entity> { ... }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Stale handle access | PL4 | Panic on `pool[h]`, None on `pool.get(h)` |
| Wrong-pool handle | PL4 | Panic on `pool[h]`, None on `pool.get(h)` |
| `with pool[h] as e1, pool[h] as e2` | W3 | Panic (duplicate handle) |
| Generation overflow | PH1 | Slot becomes permanently dead |
| Pool ID overflow | PH1 | Panic (runtime error) |
| Panic inside `with` | — | Pool left in valid state |
| Empty pool cursor | PF1 | `next()` returns None immediately |
| Nested cursors | — | Compile error (pool already borrowed) |
| Drop Pool<Resource> while non-empty | R5 | Runtime panic |
| `clear()` on Pool<Resource> | — | Compile error (would abandon resources) |
| Write in frozen context | PF5 | Compile error |
| `get_unchecked` with stale handle | — | **Undefined behavior** |

## Examples

### Game Entity System

```rask
func update_game(mut entities: Pool<Entity>, dt: f32) {
    for h in entities.handles() {
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

    const a = nodes.insert(Node { data: "A", edges: Vec.new() })
    const b = nodes.insert(Node { data: "B", edges: Vec.new() })
    const c = nodes.insert(Node { data: "C", edges: Vec.new() })

    nodes[a].edges.push(b)
    nodes[a].edges.push(c)
    nodes[b].edges.push(c)

    Ok(nodes)
}
```

### Rendering Pipeline

<!-- test: skip -->
```rask
func render_frame(world: World) {
    // Physics update (mutable iteration)
    for mutate entity in world.entities {
        entity.update_physics()
    }

    // Render pass (read-only via frozen context)
    render_entities(world.entities)
}

func render_entities() using frozen entities: Pool<Entity> {
    for entity in entities.values() {
        renderer.draw(entity)
    }
}
```

---

## Appendix (non-normative)

### Rationale

**PL1–PL7 (pool design):** Entity systems, graphs with cycles, observers — they need stable references that survive mutations. Rust's borrow checker makes this painful without `Rc`/`RefCell`. Generation counters detect stale handles at O(1), inline expression access enables interleaved mutation, and handles are values with no lifetime parameters.

**PH3 (handle identity):** Handles are database primary keys. You can have 10 copies of the key `42` — they all access the same row. The keys aren't borrowed; only the access is.

**PF5 (frozen context):** I considered making FrozenPool a separate type with freeze/thaw ownership ceremonies. That's the Rust approach — explicit state transitions. But it adds a whole type to learn, and `using frozen Pool<T>` already provides the compile-time guarantee. The `frozen` modifier is a context property, not a type.

**PL9 (handle stability):** This is why handles exist — stable identifiers that don't break when memory moves. Pointers would become dangling; handles never do. This stability is what allows `insert` and `remove(other)` inside `with` blocks (W2a/W2b) — the compiler re-resolves the binding via the still-valid handle after each structural mutation. Vec/Map can't do this (indices shift, keys rehash), but pools can because handle stability is a structural guarantee.

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

**Aliasing prevention patterns:**
```rask
// Pattern 1: Separate pools for different operations
with entities[h] as e {
    events.insert(Event.Died(h))    // OK: different collection
}

// Pattern 2: Insert/remove other inside with (W2a/W2b)
with pool[h] as entity {
    const ally = pool.insert(new_ally)   // OK: re-resolves
    entity.allies.push(ally)
    pool.remove(expired_h)                   // OK: re-resolves
}

// Pattern 3: Remove bound handle — restructure outside with
const should_remove = pool[h].health <= 0
if should_remove {
    pool.remove(h)    // OK: not inside with block
}

// Pattern 4: Multi-element access
with pool[h1] as e1, pool[h2] as e2 {
    e1.health -= e2.attack
}
```

**Self-referential patterns:** Doubly-linked lists, trees with parent pointers, graphs with cycles, and arena-allocated ASTs all use the pool+handle pattern. Key guidance:
- Use `pool.values()` or `pool.entries()` for read-only traversal
- Follow stored handles with `pool.get(h)` (checked, returns Option)
- Use `using frozen Pool<T>` functions for analysis passes that shouldn't mutate

**Shared state:** Share handles, not data. Handles are 12-byte copyable values that can be sent anywhere. The pool stays in one thread; commands flow back to it.

**Emergent capabilities:**
- **Relocatable state:** Handles are integers — no pointer fixup. Pools can be serialized to bytes (`pool.to_bytes()`), memory-mapped for flat types (`pool.to_mmap()`), and restored with handles still valid. See `mem.relocatable` for the full specification.
- **Plugin isolation:** Dropping a pool invalidates all its handles. Stale handles fail safely (panic or `None`), never undefined behavior.

**Handle filtering (manual partitioning):**

Pool cannot be split by predicate because accessing entities requires borrowing the pool. Instead, partition handles:

```rask
func process_by_type(mut entities: Pool<Entity>) {
    let player_handles = Vec.new()
    let enemy_handles = Vec.new()

    for h in entities.handles() {
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
| `insert()` | Amortized O(1) | May allocate (unbounded) |
| `remove()` | O(1) | Bumps generation |
| `cursor()` iteration | O(capacity) | Scans all slots |
| `snapshot()` | O(n) | Shallow clone |
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

- [Borrowing](borrowing.md) — Value-based access, `with` blocks (`mem.borrowing`)
- [Resource Types](resource-types.md) — Resource consumption in pools (`mem.resources`)
- [Context Clauses](context-clauses.md) — Handle auto-resolution (`mem.context`)
- [Closures](closures.md) — Pool+Handle pattern for shared mutable state (`mem.closures`)
- [Collections](../stdlib/collections.md) — Vec and Map types (`std.collections`)
- [Aliasing Detection](aliasing-detection.md) — Compile-time closure aliasing analysis (`mem.aliasing`)
- [Generation Coalescing](../compiler/generation-coalescing.md) — Check elimination algorithm (`comp.gen-coalesce`)
