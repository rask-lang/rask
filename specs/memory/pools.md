# Solution: Pools and Handles

## The Question
How do we provide stable references for graphs, entity systems, and other dynamic structures without lifetime annotations or garbage collection?

## Decision
`Pool<T>` is handle-based sparse storage. Handles are opaque IDs validated at access via generation counters. Pools are a core memory mechanism, not just a data structure.

## Rationale
Entity systems, graphs with cycles, observers—they need stable references that survive mutations. Rust's borrow checker makes this painful without `Rc`/`RefCell` ceremony.

Pools fix this:
- **Generation counters:** Detect stale handles at O(1)
- **Expression-scoped access:** Interleaved mutation works
- **No lifetime parameters:** Handles are values

## Specification

### Pool Basics

```rask
const pool: Pool<Entity> = Pool.new()
const h: Handle<Entity> = try pool.insert(entity)
pool[h].health -= 10
pool.remove(h)
```

| Operation | Returns | Description |
|-----------|---------|-------------|
| `Pool.new()` | `Pool<T>` | Create unbounded pool |
| `Pool.with_capacity(n)` | `Pool<T>` | Create bounded pool |
| `pool.insert(v)` | `Result<Handle<T>, InsertError>` | Insert, get handle |
| `pool[h]` | `&T` or `&mut T` | Access (panics if invalid) |
| `pool.get(h)` | `Option<T>` | Safe access (T: Copy) |
| `pool.remove(h)` | `Option<T>` | Remove and return |
| `pool.handles()` | `Iterator<Handle<T>>` | Iterate all valid handles |

### Handle Structure

Handles are opaque identifiers with configurable sizes:

```rask
Pool<T, PoolId=u32, Index=u32, Gen=u32>  // Defaults

Handle<T> = {
    pool_id: PoolId,   // Unique per pool instance
    index: Index,      // Slot in internal storage
    generation: Gen,   // Version counter
}
```

**Handle size** = `sizeof(PoolId) + sizeof(Index) + sizeof(Gen)`

Default: `4 + 4 + 4 = 12 bytes` (fits under 16-byte copy threshold with room to spare).

**Common configurations:**

| Config | Size | Pools | Slots | Gens | Use Case |
|--------|------|-------|-------|------|----------|
| `Pool<T>` | 12 bytes | 4B | 4B | 4B | General purpose (default) |
| `Pool<T, Gen=u64>` | 16 bytes | 4B | 4B | ∞ | High-churn scenarios |
| `Pool<T, PoolId=u16, Index=u16, Gen=u32>` | 8 bytes | 64K | 64K | 4B | Memory-constrained |

**Copy rule:** Handle is Copy if total size ≤ 16 bytes.

### Handle Validation

Every access validates the handle:

| Check | Failure mode |
|-------|--------------|
| Pool ID mismatch | Panic: "handle from wrong pool" |
| Generation mismatch | Panic: "stale handle" |
| Index out of bounds | Panic: "invalid handle index" |

**Safe access:**
```rask
pool.get(h)   // Returns Option<T> (T: Copy), no panic
pool.read(h, |v| ...)    // Returns Option<R>
pool.modify(h, |v| ...)  // Returns Option<R>
```

**Generation overflow:**

Saturating semantics. When a slot's generation reaches max:
- Slot becomes permanently unusable (always returns `None`)
- No panic, no runtime check on every removal
- Pool gradually loses capacity (practically never happens: ~4B cycles per slot with default u32)

For extreme high-churn scenarios (billions of remove/reinsert cycles per slot): `Pool<T, Gen=u64>` uses 64-bit generations (16-byte handles, still Copy).

### Handle Identity and Aliasing

Handles are copyable identifiers, not references. Multiple handles can point to the same entity:

| Property | Handle | Rust Reference |
|----------|--------|----------------|
| Nature | Value (like i32, index) | Borrow (temporary access) |
| Copying | Free, creates independent copy | Creates new borrow |
| Aliasing | Allowed - multiple copies to same entity | Subject to borrow checker |
| Access | Each `pool[h]` is independent volatile access | Borrow spans until release |

**Aliased handles are safe:**

```rask
const h1 = try pool.insert(entity)
const h2 = h1  // h2 is a copy - both point to same entity

pool[h1].health -= 10    // Volatile access #1 (released at ;)
pool[h2].health -= 10    // Volatile access #2 (released at ;)
```

This works because:
1. **Handles are values:** `h2 = h1` copies 12 bytes, not creates a borrow
2. **Each access is independent:** `pool[h1]` creates a fresh expression-scoped borrow
3. **No overlapping borrows:** First borrow ends at semicolon, before second begins
4. **Aliasing rule applies to borrows:** The rule is "aliasing XOR mutation of borrows", not "aliasing XOR mutation of handles"

**Mental model:** Handles are database primary keys. You can have 10 copies of the key `42`—they all access the same row. The keys aren't borrowed; only the access is.

**Within a single expression:**

```rask
pool[h1].x + pool[h2].y  // ✅ Two reads - OK (multiple immutable borrows)
pool[h1].x = pool[h2].y  // ✅ Read + write to different entities - OK
pool[h].x = pool[h].y    // ✅ Read + write to same entity - OK (compiler reorders)
```

The compiler ensures aliasing rules are satisfied for the borrows created within each expression, not for handle identity.

### Expression-Scoped Access

Pool access is expression-scoped (borrow released at semicolon):

```rask
pool[h].health -= damage     // Borrow released
if pool[h].health <= 0 {     // New borrow
    pool.remove(h)           // No active borrow - OK
}
```

See [borrowing.md](borrowing.md) for full borrowing rules.

### Multi-Statement Access

For multi-statement operations, use closure-based access:

```rask
try pool.modify(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
})
```

| Method | Signature | Use Case |
|--------|-----------|----------|
| `read(h, f)` | `func(T) -> R → Option<R>` | Multi-statement read |
| `modify(h, f)` | `func(T) -> R → Option<R>` | Multi-statement mutation |

#### Closure Aliasing Prevention (Compile-Time)

Closures cannot mutate the pool that called them. This is caught at compile time:

```rask
// ❌ ERROR: cannot mutate pool while borrowed by modify()
pool.modify(h, |entity| {
    entity.health -= 10
    pool.remove(h)    // Compile error here
})
```

**Why this is an error:**
- `modify()` holds a mutable borrow of `pool` during closure execution
- The closure tries to call `pool.remove()`, which needs its own mutable borrow
- Two overlapping mutable borrows violate safety

**Error message:**
```
error: cannot mutate `pool` while borrowed
  --> example.rask:3:5
   |
 2 | pool.modify(h, |entity| {
   |      ------ `pool` is exclusively borrowed here
 3 |     pool.remove(h)
   |     ^^^^^^^^^^^^^^ cannot mutate while borrowed
```

**Alternative patterns:**
```rask
// Pattern 1: Separate pools for different operations
entities.modify(h, |e| {
    events.insert(Event.Died(h))    // ✅ OK: different pool
})

// Pattern 2: Restructure logic
const should_remove = pool[h].health <= 0
if should_remove {
    pool.remove(h)    // ✅ OK: not inside closure
}

// Pattern 3: Shared borrows are OK
pool.read(h, |e| {
    const other = pool.get(h2)    // ✅ OK: both are reads
})
```

See [aliasing-detection.md](aliasing-detection.md) for the full compile-time analysis algorithm.

### Performance Escape Hatches

For hot paths where generation check overhead matters, two mechanisms provide guaranteed check reduction:

#### Safe: Validated Access (`with_valid`)

Validates once at entry, then provides unchecked access inside the closure:

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.with_valid(h, f)` | `func(Pool<T>, Handle<T>, |T| -> R) -> Option<R>` | One check, then read |
| `pool.with_valid_mut(h, f)` | `func(Pool<T>, Handle<T>, |T| -> R) -> Option<R>` | One check, then write |

```rask
try pool.with_valid_mut(h, |e| {
    e.x = 1   // No check
    e.y = 2   // No check
    e.z = 3   // No check
})
```

**When to use:** Hot loops where profiling shows generation checks are bottleneck; multi-field updates in performance-critical code; safe alternative to frozen pools when mutation is needed.

#### Unsafe: Unchecked Access

For maximum performance where caller has already validated:

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.get_unchecked(h)` | `unsafe func(Pool<T>, Handle<T>) -> T` | Zero-check read |
| `pool.get_mut_unchecked(h)` | `unsafe func(Pool<T>, Handle<T>) -> T` | Zero-check write |

**Safety requirements (caller MUST ensure):**
1. Handle was obtained from this pool
2. Handle has not been removed since obtaining
3. No concurrent mutation (standard borrow rules)

```rask
// Cursor guarantees validity during iteration
for h in pool.cursor() {
    unsafe {
        const e = pool.get_mut_unchecked(h)
        e.velocity += e.acceleration * dt
    }
}
```

**When to use:** After explicit validation (cursor proves validity); FFI callbacks where C has validated; extreme hot paths where even one check matters.

**When NOT to use:** General code (use `with_valid` or coalescing); when handle validity is uncertain; across await points.

---

## Handle Auto-Resolution via Context Clauses

Handles can dereference their fields without explicitly naming the pool. This is enabled by **context clauses** — compile-time requirements that thread pools as hidden parameters.

### How It Works

Functions declare pool requirements using `with` clauses:

```rask
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount    // Auto-resolves via Pool<Player> context
}

const players = Pool.new()
const h = try players.insert(Player { health: 100, ... })

damage(h, 10)    // Compiler passes players as hidden parameter
```

The compiler:
1. Sees `damage` requires `Pool<Player>`
2. Finds `players: Pool<Player>` in caller's scope
3. Passes `players` as a hidden argument
4. `h.health` resolves to `players[h].health`

**Zero overhead:** Context passing is identical to passing a reference parameter. No registry, no runtime lookups.

### Context Clause Syntax

**Unnamed context** (auto-resolution only):
```rask
func update(h: Handle<Entity>) with Pool<Entity> {
    h.velocity += h.acceleration    // Can access fields
    // pool.remove(h)                // ERROR: pool not in scope
}
```

**Named context** (auto-resolution + structural operations):
```rask
func cleanup(h: Handle<Entity>) with entities: Pool<Entity> {
    h.active = false              // Can access fields
    if h.health <= 0 {
        entities.remove(h)        // Can use pool directly
    }
}
```

See [Context Clauses](context-clauses.md) for full specification.

### When You Need Context Clauses

Pass contexts for **any handle usage**:

| Operation | Context Type | Example |
|-----------|--------------|---------|
| Field read/write | Unnamed or Named | `h.health -= 10` |
| Insert | Named | `try pool.insert(x)` |
| Remove | Named | `pool.remove(h)` |
| Iterate | Named | `pool.cursor()` |
| Freeze | Named | `pool.freeze()` |

```rask
// Unnamed context — field access only
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount
    h.last_hit = now()
}

// Named context — field access + structural operations
func kill(h: Handle<Player>) with players: Pool<Player> {
    h.on_death()           // Auto-resolves
    players.remove(h)      // Named pool for structural op
}

func spawn_enemy(pos: Vec3) with enemies: Pool<Enemy> -> Handle<Enemy> {
    try enemies.insert(Enemy { position: pos, health: 100, ... })
}
```

### Function Signature Patterns

**Field access only — unnamed context:**

```rask
func validate_email(user: Handle<User>) with Pool<User> -> bool {
    !user.email.is_empty()
}

func apply_gravity(entity: Handle<Entity>, dt: f32) with Pool<Entity> {
    entity.velocity.y -= 9.8 * dt
    entity.position += entity.velocity * dt
}

func damage_all(targets: Vec<Handle<Enemy>>, amount: i32) with Pool<Enemy> {
    for h in targets {
        h.health -= amount
    }
}
```

**Structural operations — named context:**

```rask
func spawn_wave(count: i32, pos: Vec3)
    with enemies: Pool<Enemy>
    -> Vec<Handle<Enemy>> or PoolFull
{
    let handles = Vec.new()
    for i in 0..count {
        const h = try enemies.insert(Enemy.new(pos))
        handles.push(h)
    }
    handles
}

func cleanup_dead() with entities: Pool<Entity> {
    for h in entities.cursor() {
        if h.health <= 0 {
            entities.remove(h)
        }
    }
}
```

### Multiple Pool Contexts

Functions can require multiple pools:

```rask
func transfer_item(player_h: Handle<Player>, item_h: Handle<Item>)
    with players: Pool<Player>, items: Pool<Item>
{
    player_h.inventory.add(item_h)    // Auto-resolves via players
    item_h.owner = Some(player_h)     // Auto-resolves via items

    if players.len() > 100 {
        players.compact()              // Named pool for structural op
    }
}
```

### Context Resolution at Call Sites

The compiler finds pools in scope and passes them automatically:

```rask
// From local variables
func game_loop() {
    const players = Pool.new()
    const h = try players.insert(Player.new())

    damage(h, 10)    // ✅ Compiler passes players implicitly
}

// From struct fields
struct Game {
    players: Pool<Player>,
    enemies: Pool<Enemy>,
}

extend Game {
    func tick(self, dt: f32) {
        for h in self.players.cursor() {
            update_player(h, dt)    // ✅ self.players provides context
        }

        for h in self.enemies.cursor() {
            update_enemy(h, dt)     // ✅ self.enemies provides context
        }
    }
}

func update_player(h: Handle<Player>, dt: f32) with Pool<Player> {
    h.position += h.velocity * dt
}
```

### Cross-Thread Contexts

Contexts are thread-scoped. Handles sent across threads require their pool to be accessible:

```rask
func worker_thread() with Pool<Player> {
    const h = receive_handle_from_channel()

    // ✅ Works if pool was moved/passed to this thread
    h.health -= 10

    // ❌ Compile error if pool not in scope
    // (runtime panic if pool was moved away after context established)
}
```

For thread communication, prefer sending complete data instead of handles, or architect so pools are owned by dedicated threads.

### Performance Characteristics

| Operation | Cost |
|-----------|------|
| Registry lookup | ~1-2 ns (thread-local HashMap) |
| `with` block access | 0 ns (direct pointer) |
| Pool registration | O(1) on creation |
| Pool deregistration | O(1) on drop |

The registry lookup is comparable to a bounds check — small enough to be implicit per the Transparency principle.

---

## Cursor Iteration

`pool.cursor()` provides zero-allocation iteration with safe removal.

### Basic Usage

```rask
for h in pool.cursor() {
    pool[h].update()
    if pool[h].expired {
        pool.remove(h)      // Safe during iteration
    }
}
```

### With Auto-Resolution

Handles auto-resolve, so cursor iteration is clean:

```rask
for h in players.cursor() {
    h.velocity += gravity * dt      // Auto-resolves via pool_id
    h.position += h.velocity * dt

    if h.health <= 0 {
        players.remove(h)           // Cursor handles this safely
    }
}
```

For hot loops, add `with` as optimization hint:

```rask
with players {
    for h in players.cursor() {
        h.velocity += gravity * dt  // Zero registry lookups
        h.position += h.velocity * dt
    }
}
```

### Cursor Methods

| Method | Returns | Description |
|--------|---------|-------------|
| `cursor.next()` | `Option<Handle<T>>` | Advance to next valid slot |
| `cursor.remove()` | `T` | Remove current element |
| `cursor.remaining()` | `usize` | Approximate remaining elements |

### Safe Removal Rules

| Rule | Description |
|------|-------------|
| **C1: Current removal OK** | Removing the current element is always safe |
| **C2: Other removal OK** | Removing other elements is safe (cursor adjusts) |
| **C3: Insertion deferred** | Insertions during iteration may or may not be visited |
| **C4: No double-visit** | Each existing element visited at most once |

### Drain Cursor

Remove and yield ownership of elements:

```rask
for entity in pool.drain() {
    entity.cleanup()
}
// pool is now empty

// Conditional drain:
for entity in pool.drain_where(|e| e.expired) {
    log_removal(entity)
}
```

---

## Weak Handles

`pool.weak(h)` creates handles that can be checked for validity before use.

### Creation

```rask
let h: Handle<Entity> = try pool.insert(entity)
let weak: WeakHandle<Entity> = pool.weak(h)
```

### Checking Validity

```rask
if weak.valid() {
    if weak.upgrade() is Some(h) {
        pool[h].process()
    }
}
```

| Method | Returns | Description |
|--------|---------|-------------|
| `weak.valid()` | `bool` | True if underlying data still exists |
| `weak.upgrade()` | `Option<Handle<T>>` | Convert to strong handle if valid |

### Invalidation

Weak handles are automatically invalidated when:
- `pool.remove(h)` is called
- `pool.clear()` removes all elements
- Pool is dropped

```rask
const weak = pool.weak(h)
assert!(weak.valid())
pool.remove(h)
assert!(!weak.valid())     // Now invalid
```

### When to Use Weak Handles

| Scenario | Use |
|----------|-----|
| Local function access | Regular `Handle<T>` |
| Stored in struct alongside pool | Regular `Handle<T>` |
| Event queue / callbacks | `WeakHandle<T>` |
| Cross-task communication | `WeakHandle<T>` |
| Cache that may be invalidated | `WeakHandle<T>` |

### Event System Pattern

```rask
struct EventQueue<T> {
    events: Vec<(WeakHandle<Entity>, T)>,
}

extend EventQueue<T> {
    func process(self, pool: Pool<Entity>) {
        for (weak, event) in self.events.drain() {
            if weak.upgrade() is Some(h) {
                pool[h].handle_event(event)
            }
            // Invalid weak handles silently skipped
        }
    }
}
```

---

## Frozen Pools

`pool.freeze()` returns an immutable view where all generation checks are skipped.

### Basic Usage

```rask
const frozen = pool.freeze()
for h in frozen.handles() {
    render(frozen[h])       // Zero generation checks!
}
const pool = frozen.thaw()
```

### Why Zero Checks?

When frozen:
1. No new elements can be inserted (no new generations)
2. No elements can be removed (no generation increments)
3. All handles valid at freeze time remain valid
4. Generation matching is guaranteed

**Performance improvement:** ~10% faster for access-heavy code.

### Methods

**Freezing:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.freeze()` | `Pool<T> -> FrozenPool<T>` | Freeze, consume ownership |
| `pool.freeze_ref()` | `Pool<T> -> FrozenPool<T>` | Freeze reference (scoped) |

**Access (all skip generation checks):**

| Method | Returns | Description |
|--------|---------|-------------|
| `frozen[h]` | `&T` | Direct access, no check |
| `frozen.handles()` | `Iterator<Handle<T>>` | Iterate all valid handles |
| `frozen.iter()` | `Iterator<&T>` | Iterate all values |

**NOT available on FrozenPool:** `insert()`, `remove()`, `modify()`, `clear()`

### Scoped Freezing

```rask
pool.with_frozen(|frozen| {
    for h in frozen.handles() {
        render(frozen[h])
    }
})
// pool is mutable again
```

### Invalid Handle Behavior

| Scenario | Mutable Pool | Frozen Pool |
|----------|--------------|-------------|
| Stale handle | Panic or None | **Undefined behavior** |
| Wrong pool | Panic or None | **Undefined behavior** |
| Valid handle | Access succeeds | Access succeeds |

**Important:** Only use handles valid at freeze time.

---

## Generation Check Coalescing

The compiler automatically eliminates redundant generation checks.

### Basic Coalescing

```rask
// Source code
pool[h].x = 1
pool[h].y = 2
pool[h].z = 3

// After coalescing: ONE generation check
```

### Coalescing Rules

| Rule | Description |
|------|-------------|
| **GC1: Same handle** | Only coalesce accesses to the same handle variable |
| **GC2: No intervening mutation** | No `pool.insert()`, `pool.remove()` between accesses |
| **GC3: No reassignment** | Handle variable not reassigned between accesses |
| **GC4: Local analysis** | Coalescing within function scope only |

### What Breaks Coalescing

```rask
// Coalesced (no mutation)
pool[h].a = 1
pool[h].b = 2    // Same check as above

// NOT coalesced (intervening mutation)
pool[h].a = 1
pool.remove(other_h)  // Mutation invalidates assumption
pool[h].b = 2    // Fresh check required
```

### Ambient Pools

`with` blocks enable more aggressive coalescing:

```rask
with pool {
    h.x = 1         // Check once at first access
    h.y = 2         // Coalesced
    h.z = 3         // Coalesced
}
```

### Expected Performance

| Pattern | Without Coalescing | With Coalescing |
|---------|-------------------|-----------------|
| 3 field updates | 3 checks | 1 check |
| Loop body, 5 accesses | 5 checks/iteration | 1 check/iteration |
| `with` block, 10 accesses | 10 checks | 1 check |

---

## Linear Types in Pools

Linear resource types have special rules in pools:

| Collection | Linear allowed? | Reason |
|------------|-----------------|--------|
| `Vec<Linear>` | ❌ No | Vec drop can't propagate errors |
| `Pool<Linear>` | ✅ Yes | Explicit removal required anyway |

**Pool pattern for linear resources:**
```rask
let files: Pool<File> = Pool.new()
const h = try files.insert(try File.open(path))

// Later: explicit consumption required
for h in files.handles().collect<Vec<_>>() {
    const file = files.remove(h).unwrap()
    try file.close()
}
```

### Drop Behavior

When a `Pool<Linear>` is dropped:
- If empty: normal drop, no additional action
- If non-empty: runtime panic

This ensures linear resources cannot be silently leaked through pool abandonment.

**Safe patterns:**

```rask
// Pattern 1: Explicit take_all loop
for file in files.take_all() {
    try file.close()
}

// Pattern 2: take_all_with (ignore errors)
files.take_all_with(|f| { f.close(); })

// Pattern 3: take_all_with_result (propagate errors)
try files.take_all_with_result(|f| f.close())

// Pattern 4: ensure block (cleanup on any exit)
ensure files.take_all_with(|f| { f.close(); })
```

**Anti-patterns (will panic):**

```rask
// BAD: Dropping non-empty pool
let files: Pool<File> = Pool.new()
try files.insert(try File.open("a.txt"))
// scope exit: PANIC - files not taken

// BAD: Forgetting to take_all in error path
let files: Pool<File> = Pool.new()
try files.insert(try File.open("a.txt"))
try some_operation()  // If this fails, files not taken - PANIC
for file in files.take_all() { try file.close(); }
```

**Correct pattern with ensure:**

```rask
let files: Pool<File> = Pool.new()
ensure files.take_all_with(|f| { f.close(); })

try files.insert(try File.open("a.txt"))
try some_operation()  // If this fails, ensure runs, takes all from pool
```

---

## Thread Safety

| Type | `Send` | `Sync` |
|------|--------|--------|
| `Pool<T>` | if `T: Send` | if `T: Sync` |
| `Handle<T>` | Yes (Copy) | Yes |
| `WeakHandle<T>` | Yes (Copy) | Yes |
| `FrozenPool<T>` | if `T: Send` | Yes (immutable) |

---

## Capacity and Allocation

### Capacity Semantics

- Unbounded: `capacity() == None`, grows indefinitely
- Bounded: `capacity() == Some(n)`, cannot exceed `n` elements

### All Insertions are Fallible

```rask
try pool.insert(x)   // Result<Handle<T>, InsertError<T>>

enum InsertError<T> {
    Full(T),   // Bounded pool at capacity
    Alloc(T),  // Allocation failed
}
```

### Capacity Introspection

| Method | Returns | Semantics |
|--------|---------|-----------|
| `pool.len()` | `usize` | Current element count |
| `pool.capacity()` | `Option<usize>` | `None` = unbounded |
| `pool.remaining()` | `Option<usize>` | Slots available |

---

## Pool Growth & Memory Management

### Handle Stability During Growth

**Key property:** Handles remain valid when pools grow.

Unlike pointers (which become invalid when memory moves), handles store an index:

| Step | What happens |
|------|--------------|
| 1. Pool is full | Internal storage at capacity |
| 2. `insert()` triggers growth | New memory allocated, data copied |
| 3. Handle used | `pool.data[handle.index]` finds data at new location |

```rask
const h = try pool.insert(Entity { ... })   // h = { index: 0, gen: 1 }
// ... pool grows internally ...
pool[h].health -= 10                    // Still works - index 0 is still valid
```

**This is why handles exist** - stable identifiers that don't break when memory moves. Pointers would become dangling; handles never do.

### Bounded vs Unbounded Pools

| Pool type | Growth | Use case |
|-----------|--------|----------|
| `Pool.new()` | Auto-grows (like Vec) | General purpose, prototyping |
| `Pool.with_capacity(n)` | Never grows | Hot paths, real-time systems |

**Unbounded pools:**
- Ergonomic default - just insert, pool handles memory
- Growth is amortized O(1) like Vec
- `insert()` may allocate (implicit cost)

**Bounded pools:**
- Predictable - no allocations after creation
- `insert()` returns `Err(Full)` at capacity
- Use for hot paths where allocation timing matters

```rask
// Game entity pool - pre-allocate for expected count
const entities = Pool.with_capacity(1000)

// Config objects - unbounded is fine, not hot path
const configs = Pool.new()
```

**Guidance:** Use bounded pools for performance-critical code where allocation predictability matters. Use unbounded pools when ergonomics trump predictability.

### Fragmentation

**Observation:** Many types = many pools. If one pool is full but another is empty, memory can't be shared.

**Design decision:** This is a deliberate tradeoff, not a bug.

| Trade-off | Per-type pools | Shared allocator |
|-----------|---------------|------------------|
| Type safety | ✅ No cross-type corruption | ⚠️ Possible type confusion |
| Simplicity | ✅ Each pool independent | ❌ Complex sharing logic |
| Memory efficiency | ⚠️ Can't share across types | ✅ Unified memory pool |
| Predictability | ✅ Each type has its budget | ⚠️ Interference between types |

**Rask chooses:** Type safety and simplicity over memory efficiency.

### Memory Budgeting Patterns

**Pre-allocation:** Know your counts, allocate upfront.
```rask
const players = Pool.with_capacity(64)     // Max 64 players
const bullets = Pool.with_capacity(10000)  // Lots of bullets
const configs = Pool.with_capacity(100)    // Modest config count
```

**Pool reuse:** Clear and reuse instead of drop and recreate.
```rask
func next_level(entities: Pool<Entity>) {
    entities.clear()                    // Free all entities
    try spawn_level_entities(entities)     // Reuse same pool
}
```

**Sizing strategy:**
- Profile your application to find typical counts
- Add headroom for peaks (2x is common)
- Use bounded pools for hard limits, unbounded for soft

---

## Edge Cases

| Case | Handling |
|------|----------|
| Stale handle access | Panic on `pool[h]`, None on `pool.get(h)` |
| Wrong-pool handle | Panic on `pool[h]`, None on `pool.get(h)` |
| `modify_many([h, h], _)` | Panic (duplicate index) |
| Generation overflow | Slot becomes permanently dead |
| Pool ID overflow | Panic (runtime error) |
| Closure panics in `modify` | Pool left in valid state |
| Empty pool cursor | `next()` returns None immediately |
| Nested cursors | Compile error (pool already borrowed) |
| Drop Pool<Linear> while non-empty | Runtime panic |
| take_all() on Pool<Linear> | Returns owned elements for consumption |
| clear() on Pool<Linear> | Compile error (would abandon linear elements) |
| `get_unchecked` with stale handle | **Undefined behavior** |
| `get_unchecked` with wrong-pool handle | **Undefined behavior** |
| `get_mut_unchecked` during active borrow | **Undefined behavior** |

---

## Examples

### Game Entity System

```rask
func update_game(mut entities: Pool<Entity>, dt: f32) {
    for h in entities.cursor() {
        h.velocity += h.acceleration * dt   // Auto-resolves
        h.position += h.velocity * dt

        if h.health <= 0 {
            h.on_death()
            entities.remove(h)              // Needs pool for removal
        }
    }
}

// Optimized version (zero registry lookups in hot loop)
func update_game_optimized(mut entities: Pool<Entity>, dt: f32) {
    with entities {
        for h in entities.cursor() {
            h.velocity += h.acceleration * dt
            h.position += h.velocity * dt

            if h.health <= 0 {
                h.on_death()
                entities.remove(h)
            }
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

### Observer Pattern with Weak Handles

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

---

## Self-Referential Patterns

Handle-based structures have ~10% overhead vs raw pointers, fitting the RO ≤ 1.10 metric. Per METRICS.md, generation checks are acceptable implicit overhead (same category as bounds checks).

**Performance mitigations shown in each pattern:**
- Frozen pools for read-only traversal (0 overhead)
- Generation coalescing for multi-field access
- Compact handle configurations when needed

### Doubly-Linked List

Bidirectional traversal with safe removal.

```rask
struct ListNode<T> {
    data: T,
    prev: Handle<ListNode<T>>?,
    next: Handle<ListNode<T>>?,
}

struct LinkedList<T> {
    nodes: Pool<ListNode<T>>,
    head: Handle<ListNode<T>>?,
    tail: Handle<ListNode<T>>?,
}

extend LinkedList<T> {
    func push_back(self, data: T) -> Handle<ListNode<T>> or Error {
        const h = try self.nodes.insert(ListNode {
            data,
            prev: self.tail,
            next: none,
        })

        if self.tail is Some(old_tail) {
            self.nodes[old_tail].next = Some(h)
        } else {
            self.head = Some(h)
        }
        self.tail = Some(h)
        Ok(h)
    }

    func remove(self, h: Handle<ListNode<T>>) -> Option<T> {
        const node = try self.nodes.remove(h)

        // Update neighbors
        if node.prev is Some(prev) {
            self.nodes[prev].next = node.next
        } else {
            self.head = node.next
        }

        if node.next is Some(next) {
            self.nodes[next].prev = node.prev
        } else {
            self.tail = node.prev
        }

        Some(node.data)
    }

    // Forward traversal - use frozen pool for read-only iteration
    func iter_forward(self) -> any Iterator<Handle<ListNode<T>>> {
        const frozen = self.nodes.freeze_ref()
        const current = self.head
        std.iter.from_fn(move || {
            const h = try current
            current = frozen[h].next  // Zero gen checks with frozen
            Some(h)
        })
    }
}
```

**Performance note:** Use `freeze_ref()` for read-only traversal to eliminate generation checks entirely.

### Tree with Parent Pointers

Bidirectional tree traversal for ASTs, DOM trees, file systems.

```rask
struct TreeNode<T> {
    data: T,
    parent: Handle<TreeNode<T>>?,
    children: Vec<Handle<TreeNode<T>>>,
}

struct Tree<T> {
    nodes: Pool<TreeNode<T>>,
    root: Handle<TreeNode<T>>?,
}

extend Tree<T> {
    // Walk up to root
    func ancestors(self, h: Handle<TreeNode<T>>) -> Vec<Handle<TreeNode<T>>> {
        const frozen = self.nodes.freeze_ref()  // Zero-cost traversal
        const path = Vec.new()
        const current = Some(h)

        while current is Some(node_h) {
            path.push(node_h)
            current = frozen[node_h].parent
        }
        path
    }

    // Reparent a subtree
    func reparent(self, child: Handle<TreeNode<T>>, new_parent: Handle<TreeNode<T>>) {
        // Remove from old parent
        if self.nodes[child].parent is Some(old_parent) {
            self.nodes[old_parent].children.retain(|h| *h != child)
        }

        // Add to new parent
        try self.nodes[new_parent].children.push(child)
        self.nodes[child].parent = Some(new_parent)
    }

    // Delete subtree using cursor for safe iteration
    func delete_subtree(self, root: Handle<TreeNode<T>>) {
        const to_delete = vec![root]

        while to_delete.pop() is Some(h) {
            if self.nodes.remove(h) is Some(node) {
                to_delete.extend(node.children)
            }
        }
    }

    // DFS traversal (frozen for performance)
    func dfs<F>(self, start: Handle<TreeNode<T>>, visit: F)
    where F: FnMut(Handle<TreeNode<T>>, T)
    {
        const frozen = self.nodes.freeze_ref()
        const stack = vec![start]

        while stack.pop() is Some(h) {
            visit(h, frozen[h].data)
            stack.extend(frozen[h].children.iter().rev())
        }
    }
}
```

**Performance note:** Ancestor walks and DFS use `freeze_ref()` for zero-overhead traversal. Mutations (reparent, delete) pay generation check cost.

### Graph with Cycles

Handling cycles requires careful consideration of dangling handles.

```rask
struct GraphNode<T> {
    data: T,
    edges: Vec<Handle<GraphNode<T>>>,
}

struct Graph<T> {
    nodes: Pool<GraphNode<T>>,
}

extend Graph<T> {
    // Add edge (cycles allowed)
    func add_edge(self, from: Handle<GraphNode<T>>, to: Handle<GraphNode<T>>) -> () or Error {
        try self.nodes[from].edges.push(to)
        Ok(())
    }

    // Cycle-aware traversal with visited set
    func reachable_from(self, start: Handle<GraphNode<T>>) -> Vec<Handle<GraphNode<T>>> {
        const frozen = self.nodes.freeze_ref()
        const visited = Set.new()
        const stack = vec![start]
        const result = Vec.new()

        while stack.pop() is Some(h) {
            if visited.insert(h) {
                result.push(h)
                for neighbor in frozen[h].edges.iter() {
                    if !visited.contains(neighbor) {
                        stack.push(neighbor)
                    }
                }
            }
        }
        result
    }

    // Remove node - edges to it become stale (detected on access)
    func remove(self, h: Handle<GraphNode<T>>) -> Option<T> {
        const node = try self.nodes.remove(h)
        Some(node.data)
        // Note: Other nodes may still have edges pointing to h
        // These become stale handles - pool.get() returns None
    }

    // Clean up stale edges after removals
    func gc_edges(self) {
        for h in self.nodes.cursor() {
            self.nodes[h].edges.retain(|edge| {
                self.nodes.get(*edge).is_some()
            })
        }
    }
}
```

**Cycle handling strategies:**

| Strategy | When to use |
|----------|-------------|
| Stale handles (shown above) | Edges to removed nodes detected on access |
| Weak handles | Event systems where you check validity before use |
| Reference counting | When you need automatic cleanup (use external counter) |
| Manual GC pass | Batch cleanup of stale edges periodically |

**Performance note:** The `gc_edges` pass can be expensive. Run it periodically, not after every removal.

### Arena-Allocated AST

Compiler pattern: allocate nodes in a pool, cross-reference freely, deallocate all at once.

```rask
// Single pool for all expression types (enum approach)
enum Expr {
    Literal(i64),
    Binary { op: BinOp, left: Handle<Expr>, right: Handle<Expr> },
    Call { func: Handle<Expr>, args: Vec<Handle<Expr>> },
    Var(string),
}

struct ExprNode {
    expr: Expr,
    parent: Handle<ExprNode>?,  // For error reporting
    span: Span,                  // Source location
}

struct Ast {
    exprs: Pool<ExprNode>,
    root: Handle<ExprNode>?,
}

extend Ast {
    func new_binary(self, op: BinOp, left: Handle<ExprNode>, right: Handle<ExprNode>, span: Span)
        -> Handle<ExprNode> or Error
    {
        const h = try self.exprs.insert(ExprNode {
            expr: Expr.Binary { op, left, right },
            parent: none,
            span,
        })

        // Set parent pointers
        self.exprs[left].parent = Some(h)
        self.exprs[right].parent = Some(h)
        Ok(h)
    }

    // Error reporting: walk up to find context
    func error_context(self, h: Handle<ExprNode>) -> string {
        const frozen = self.exprs.freeze_ref()
        const current = h
        const context = Vec.new()

        while frozen[current].parent is Some(parent) {
            context.push(frozen[parent].span)
            current = parent
        }
        format_error_chain(context)
    }

    // Type checking pass (read-only, zero overhead)
    func type_check(self) -> TypeMap or TypeError {
        const frozen = self.exprs.freeze_ref()
        const types = Map.new()

        for h in frozen.handles() {
            const ty = try infer_type(frozen[h].expr, types)
            try types.insert(h, ty)
        }
        Ok(types)
    }

    // Compilation done - deallocate everything
    func finish(take self) {
        // Pool dropped, all memory freed
        // No need to visit each node
    }
}
```

**Alternative: Separate pools per node type**

```rask
struct TypedAst {
    literals: Pool<LiteralNode>,
    binaries: Pool<BinaryNode>,
    calls: Pool<CallNode>,
    vars: Pool<VarNode>,
}

enum ExprHandle {
    Literal(Handle<LiteralNode>),
    Binary(Handle<BinaryNode>),
    Call(Handle<CallNode>),
    Var(Handle<VarNode>),
}
```

| Approach | Pros | Cons |
|----------|------|------|
| Single pool (enum) | Simpler handle type, one pool to manage | Enum overhead, less type safety |
| Separate pools | Type-safe handles, no enum dispatch | More pools to pass around |

**Performance note:** Compiler passes (type checking, codegen) should use `freeze()` for zero-overhead traversal. The AST is typically built once, then read many times.

---

## Pool Partitioning

Parallel iteration over pools without locks. Two modes: **chunked frozen pools** for data parallelism, and **snapshot isolation** for concurrent read-write.

### Scoped Partitioning (Read-Only)

Split a pool into chunks for parallel read-only processing. Uses scoped API for type-safe automatic reunification.

```rask
let entities: Pool<Entity> = // ... 1000 entities

// Partition into 4 chunks (scoped)
entities.with_partition(4, |chunks| {
    parallel_for(chunks) { |chunk|
        for h in chunk.handles() {
            analyze(chunk[h])  // Zero generation checks (frozen)
        }
    }
})  // Auto-reunifies here
// entities available again
```

**API:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.with_partition(n, f)` | `(Pool<T>, usize, |[FrozenChunk<T>]| -> R) -> R` | Scoped partition (read-only) |
| `chunk.handles()` | `Iterator<Handle<T>>` | Iterate handles in this chunk |
| `chunk[h]` | `&T` | Access (zero generation checks) |
| `chunk.len()` | `usize` | Number of elements in chunk |

**Partitioning Strategy:**

| Strategy | Distribution | Use Case |
|----------|-------------|----------|
| **Round-robin** (default) | Distribute handles evenly | Uniform work per element |
| **Contiguous** | Chunk by index ranges | Better cache locality |

**Scoped semantics:**
- Chunks cannot escape the closure
- Pool automatically reunified on closure exit
- Closure panic leaves pool in valid state
- No explicit reunify needed

**Example with analysis:**
```rask
func parallel_physics(mut entities: Pool<Entity>, dt: f32) {
    // Phase 1: Parallel read (compute forces)
    const forces = entities.with_partition(num_cpus(), |chunks| {
        parallel_map(chunks) { |chunk|
            chunk.handles().map(|h| {
                const e = chunk[h]
                (h, compute_forces(e.position, e.velocity))
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

### Mutable Partitioning (Read-Write)

Split a pool into mutable chunks for parallel read-write processing. Each thread gets exclusive access to its chunk.

```rask
entities.with_partition_mut(4, |chunks| {
    parallel_for(chunks) { |chunk|
        for h in chunk.cursor() {
            chunk[h].position += chunk[h].velocity * dt  // Mutable access
        }
    }
})
```

**API:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.with_partition_mut(n, f)` | `(Pool<T>, usize, |[MutableChunk<T>]| -> R) -> R` | Scoped partition (read-write) |
| `chunk.cursor()` | `Cursor<T>` | Mutable iterator |
| `chunk[h]` | `&T` | Immutable access |
| `chunk[h] = value` | N/A | Mutable access (via cursor or methods) |
| `chunk.modify(h, f)` | `Option<R>` | Mutable closure access |

**MutableChunk properties:**
- Exclusive mutable access within chunk
- `Send` but not `Sync` (each thread owns its chunk)
- Generation checks still apply (not frozen)
- Safe removal during iteration via cursor

**Example: Parallel velocity integration:**
```rask
func integrate_velocities(mut entities: Pool<Entity>, dt: f32) {
    entities.with_partition_mut(num_cpus(), |chunks| {
        parallel_for(chunks) { |chunk|
            for h in chunk.cursor() {
                try chunk.modify(h, |e| {
                    e.velocity += e.acceleration * dt
                    e.position += e.velocity * dt
                })
            }
        }
    })
}
```

### Snapshot Isolation

Create a frozen snapshot for readers while writer continues mutating.

```rask
let (snapshot, mut pool) = entities.snapshot()

// Readers see frozen state
parallel {
    spawn(snapshot.clone()) { |snap|
        for h in snap.handles() {
            render(snap[h])  // Reads old state
        }
    }

    spawn(snapshot.clone()) { |snap|
        for h in snap.handles() {
            audio_update(snap[h])
        }
    }
}

// Writer can mutate concurrently
try pool.insert(new_entity)
pool.remove(dead_entity)

// Snapshot dropped when readers done
drop(snapshot)
```

**API:**

| Method | Signature | Description |
|--------|-----------|-------------|
| `pool.snapshot()` | `Pool<T> -> (FrozenPool<T>, Pool<T>>` | Clone frozen view, keep mutable pool |
| `snapshot.clone()` | `&FrozenPool<T> -> FrozenPool<T>` | Cheap clone for sharing across threads |

**Semantics:**

| Property | Behavior |
|----------|----------|
| **Snapshot is immutable** | `FrozenPool<T>` - zero generation checks |
| **Snapshot is cloneable** | Reference-counted, shared across threads |
| **Writer is independent** | Mutations not visible to snapshot |
| **Memory overhead** | Snapshot shares backing storage (copy-on-write) |
| **Dropping snapshot** | Frees shared data if no mutations occurred |

**Copy-on-Write Details:**

```rask
// Before snapshot
Pool: [A, B, C, D, E]

// After snapshot()
Snapshot: [A, B, C, D, E]  (shared, immutable)
Pool:     [A, B, C, D, E]  (shared until mutation)

// After pool.insert(F)
Snapshot: [A, B, C, D, E]  (still shared)
Pool:     [A, B, C, D, E, F]  (mutation triggers copy)
```

**When copy happens:**
- First insertion after snapshot → copy backing storage
- First removal after snapshot → copy backing storage
- Reads/frozen operations → no copy

**Memory cost:** O(n) copy on first mutation, where n = pool size at snapshot time.

### Filtering Handles (Manual Partitioning)

For conditional processing based on entity properties, use manual handle filtering rather than pool partitioning.

**Pattern: Collect handles by type**

```rask
struct Entity {
    kind: EntityKind,
    // ...
}

enum EntityKind { Player, Enemy, Projectile }

func process_by_type(mut entities: Pool<Entity>) {
    // Collect handles by kind
    letplayer_handles = Vec.new()
    letenemy_handles = Vec.new()
    letprojectile_handles = Vec.new()

    for h in entities.cursor() {
        match entities[h].kind {
            EntityKind.Player => player_handles.push(h),
            EntityKind.Enemy => enemy_handles.push(h),
            EntityKind.Projectile => projectile_handles.push(h),
        }
    }

    // Process each group
    for h in player_handles {
        entities[h].update_player()
    }
    for h in enemy_handles {
        entities[h].update_enemy()
    }
    for h in projectile_handles {
        entities[h].update_projectile()
    }
}
```

**Rationale:** Splitting a pool into multiple pools requires:
- Copying all data (expensive)
- Remapping handles (breaks handle identity)
- Complex reunification (generation conflicts)

**Zero-copy alternative:** Keep pool intact, partition handles only.

**With iterator-based partitioning:**
```rask
// Partition handles (not entities)
let (player_hs, rest): (Vec<_>, Vec<_>) = entities.cursor()
    .partition(|&h| entities[h].kind == EntityKind.Player);

let (enemy_hs, projectile_hs): (Vec<_>, Vec<_>) = rest.into_iter()
    .partition(|&h| entities[h].kind == EntityKind.Enemy);
```

**Note:** Pool cannot be split by predicate because accessing entities requires borrowing the pool, but splitting would consume it. Manual filtering is the correct pattern.

### Why No Pool Merging?

**Question:** Can two pools be merged while preserving handle validity?

**Answer:** No. Merging is not supported because:

1. **Handle identity conflict** - Handles contain pool_id. Merging pool1 (id=1) and pool2 (id=2) creates pool3 (id=3), invalidating all existing handles.

2. **Generation conflicts** - Both pools might use the same indices with different generation counters, causing collisions.

3. **Not ergonomically needed** - Use these patterns instead:
   - **Iterator chaining**: `pool1.cursor().chain(pool2.cursor())`
   - **Multi-pool ambient**: `with (pool1, pool2) { ... }` (already specified)
   - **Sum types**: `enum EntityRef { Player(Handle<Player>), NPC(Handle<NPC>) }`
   - **Explicit multi-pool functions**: Pass multiple pools as parameters

**Design tradeoff:** Per-type pools provide type safety and handle stability at the cost of memory fragmentation. This is an accepted tradeoff - explicit multiple pools are simpler and clearer than complex pool merging.

### Thread Safety Rules

| Type | `Send` | `Sync` | Cloneable |
|------|--------|--------|-----------|
| `Pool<T>` | if `T: Send` | ❌ | ❌ |
| `FrozenPool<T>` | if `T: Send` | if `T: Sync` | ✅ (refcounted) |
| `FrozenChunk<T>` | if `T: Send` | if `T: Sync` | ❌ |
| `MutableChunk<T>` | if `T: Send` | ❌ | ❌ |
| `Handle<T>` | ✅ | ✅ | ✅ (Copy) |

**Rationale:**
- `Send`: Can transfer ownership across threads → requires `T: Send`
- `Sync`: Can share `&Self` across threads → provides `&T`, requires `T: Sync`
- `FrozenPool/FrozenChunk` provide `&T` access → need `T: Sync` for `Sync`
- `MutableChunk` provides `&mut T` access → cannot be `Sync` (exclusive access)

**Safe patterns:**
- ✅ Send `FrozenChunk` to different threads (scoped partition)
- ✅ Send `MutableChunk` to different threads (mutable partition)
- ✅ Clone and share `FrozenPool` across threads (snapshot)
- ✅ Send handles through channels
- ❌ Share mutable `Pool<T>` across threads (compile error)
- ❌ Share `MutableChunk<T>` across threads (compile error)

### Performance Characteristics

| Operation | Cost | Notes |
|-----------|------|-------|
| `with_partition(n, f)` | O(1) setup + O(1) teardown | Scoped partition, auto-reunifies |
| `with_partition_mut(n, f)` | O(1) setup + O(1) teardown | Mutable partition, auto-reunifies |
| Iterate frozen chunk | O(m/n) | m elements, n chunks, zero checks |
| Iterate mutable chunk | O(m/n) | m elements, n chunks, generation checks |
| `snapshot()` | O(1) | Shares backing storage (CoW) |
| First mutation after snapshot | O(n) | Triggers copy-on-write |
| Handle filtering | O(n) | Scan all elements, collect handles |

### Edge Cases

| Case | Handling |
|------|----------|
| `with_partition(0, f)` | Compile error (n must be > 0) |
| `with_partition(n > pool.len(), f)` | Some chunks empty, f still called with n chunks |
| Closure panics in `with_partition` | Pool left in valid state, auto-reunified |
| Attempt to partition while borrowed | Compile error (pool already borrowed) |
| Nested `with_partition` | Compile error (pool already mutably borrowed) |
| Mutation during frozen chunk iteration | Compile error (frozen chunks are immutable) |
| Drop snapshot while readers active | Safe: refcount keeps data alive |
| Empty pool partition | Returns n empty chunks |

### Examples

#### ECS Physics Pipeline

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

**Alternative with mutable partitioning (fully parallel):**
```rask
func physics_tick_parallel(mut entities: Pool<Entity>, dt: f32) {
    // Parallel read-write: each thread mutates its chunk
    entities.with_partition_mut(num_cpus(), |chunks| {
        parallel_for(chunks) { |chunk|
            for h in chunk.cursor() {
                try chunk.modify(h, |e| {
                    const force = compute_forces(e.position, e.mass)
                    e.velocity += force * dt
                    e.position += e.velocity * dt
                })
            }
        }
    })
}
```

#### Render While Simulating

```rask
func game_loop(mut world: World) {
    loop {
        // Snapshot for rendering
        let (render_snapshot, mut sim_pool) = world.entities.snapshot()

        // Render in parallel (old state)
        spawn_daemon {
            render_frame(render_snapshot)
        }

        // Simulate with new state
        for h in sim_pool.cursor() {
            sim_pool[h].update()
            if sim_pool[h].dead {
                sim_pool.remove(h)
            }
        }

        world.entities = sim_pool
    }
}
```

#### Batch Processing by Type

```rask
func process_entities(mut entities: Pool<Entity>) {
    // Collect handles by kind
    letstatic_handles = Vec.new()
    letplayer_handles = Vec.new()
    letnpc_handles = Vec.new()

    for h in entities.cursor() {
        match entities[h].kind {
            EntityKind.Static => static_handles.push(h),
            EntityKind.Player => player_handles.push(h),
            EntityKind.NPC => npc_handles.push(h),
        }
    }

    // Process each group in sequence
    for h in static_handles {
        entities[h].update_static()
    }
    for h in player_handles {
        entities[h].update_player()
    }
    for h in npc_handles {
        entities[h].update_npc()
    }
}
```

**Note:** Pool cannot be split by predicate (would require copying data and remapping handles). Manual handle filtering is the correct zero-copy pattern.

### Comparison with Other Approaches

| Approach | Rask (Partitioned Pools) | Rust (Rayon) | Go (Goroutines) |
|----------|--------------------------|--------------|-----------------|
| **Shared read** | Frozen chunks, zero checks | `par_iter()` with Arc | Mutex/RwLock overhead |
| **Parallel mutation** | Mutable chunks, disjoint access | `par_iter_mut()` + bounds checks | Mutex/channel overhead |
| **Concurrent read-write** | Snapshot isolation (CoW) | Channels or locks | Channels or locks |
| **API style** | Scoped (auto-reunifies) | Iterator adapters | Manual goroutines |
| **Type safety** | Frozen = `Sync`, Pool = not `Sync` | `Send + Sync` bounds | Runtime races |
| **Overhead** | ~0% (frozen), ~10% (mutable) | Lock/Arc cost | Lock/channel cost |

**Rask's advantages:**
- Zero-cost frozen reads (beats Rust's Arc overhead)
- Disjoint mutable partitioning (parallel write without locks)
- Scoped API prevents handle leakage (compile-time safe reunification)
- Hylo can't do parallel mutation easily (stack-bound, single-threaded `inout`)

---

## Shared State Patterns

When multiple parts of a program need to access the same object, Rask's approach differs from both Rust (borrow checker) and Go (GC + pointers): **share handles, not data**.

### Pattern 1: Handles Through Channels (Cross-Task)

Pass lightweight handles (12 bytes) instead of copying or sharing data. The pool owner processes requests.

```rask
struct User { name: string, email: string, ... }
let users: Pool<User> = Pool.new()
const user_h = try users.insert(User { ... })

// Send handles to worker, receive commands back
(cmd_tx, cmd_rx) = Channel<UserCommand>.buffered(100)

nursery { |n|
    // Worker task: sends commands (doesn't access pool directly)
    n.spawn(user_h, cmd_tx) { |h, tx|
        try tx.send(UserCommand.Validate(h))
        try tx.send(UserCommand.Notify(h))
    }

    // Main task: owns pool, processes commands
    while cmd_rx.recv() is Ok(cmd) {
        match cmd {
            UserCommand.Validate(h) => {
                if h.email.is_empty() {   // Auto-resolves (same thread as pool)
                    log("Invalid user")
                }
            }
            UserCommand.Notify(h) => {
                send_email(h.email, "Welcome!")
            }
        }
    }
}
```

**Key insight:** Handles are copyable values that can be sent anywhere. The pool stays in one thread; commands flow back to it.

### Pattern 2: Handle Auto-Resolution (Local Functions)

Multiple functions access the same pool without passing it — handles auto-resolve.

```rask
func process_user(user_h: Handle<User>) -> () or Error {
    try validate_user(user_h)
    try send_notification(user_h)
    try log_activity(user_h)
    Ok(())
}

func validate_user(h: Handle<User>) -> () or Error {
    // h.field auto-resolves via pool_id registry
    if h.email.is_empty() {
        Err(ValidationError)
    } else {
        Ok(())
    }
}

func send_notification(h: Handle<User>) {
    send_email(h.email, "Welcome!")
}

// Just call it — no with block needed
try process_user(user_h)
```

**Key insight:** Handles know which pool they came from. No explicit pool passing or `with` blocks required.

### Pattern 3: Multi-Pool Operations

Access objects from multiple pools — each handle auto-resolves to its own pool.

```rask
struct Player { team_id: Handle<Team>, score: i32, ... }
struct Team { total_score: i32, ... }

let players: Pool<Player> = Pool.new()
let teams: Pool<Team> = Pool.new()

func award_points(player_h: Handle<Player>, points: i32) {
    player_h.score += points                    // Via players pool
    player_h.team_id.total_score += points      // Via teams pool (chained)
}

// For hot paths, use with as optimization:
with (players, teams) {
    for p in players.cursor() {
        p.score += bonus
        p.team_id.total_score += bonus
    }
}
```

**Key insight:** Handles act as foreign keys between pools, enabling relational patterns. Each handle auto-resolves to its own pool.

### Comparison with Other Languages

| Problem | Go | Rust | Rask |
|---------|-----|------|------|
| Share User across 10 functions | Pass `*User` (hidden GC) | `&User` with lifetimes | Pass `Handle<User>` (16 bytes) |
| Ten concurrent readers | Mutex contention | `Arc<RwLock<User>>` | Pool partitioning (future) |
| Cross-task access | Channels (GC overhead) | `Arc<User>` (RC cost) | Channels with handles (zero-copy) |

**Mental model:** The pool is a parking lot. Handles are parking tickets. You can copy a ticket (16 bytes) and hand it to anyone. The car (User) stays parked.

---

## Emergent Capabilities

Because handles are plain integers (not memory addresses), the pool/handle model enables capabilities that pointer-based languages cannot easily replicate.

### Serialization

Handles survive serialization. A `Handle<T>` is `{pool_id, index, generation}` — three integers with no address-space dependency. An entire pool graph can be written to disk and reconstructed with all cross-references intact.

```rask
// Save game state: just serialize the pools
func save_game(world: World, path: string) -> () or Error {
    const file = try File.create(path)
    ensure file.close()
    try serialize(file, world.entities)    // Handles inside entities are just integers
    try serialize(file, world.items)
    Ok(())
}

// Load game state: handles still point to the right slots
func load_game(path: string) -> World or Error {
    const file = try File.open(path)
    ensure file.close()
    const entities: Pool<Entity> = try deserialize(file)
    const items: Pool<Item> = try deserialize(file)
    Ok(World { entities, items })
}
```

No pointer fixup pass needed. No address relocation. Handles that pointed to entity #42 before saving still point to entity #42 after loading.

**Cross-references between pools** also survive, as long as all referenced pools are serialized and deserialized together with the same pool IDs. The serialization format must preserve pool IDs to maintain handle validity.

### Plugin Isolation

Dropping a pool clears its registry entry, instantly invalidating all handles to it. This provides a natural sandboxing boundary:

```rask
func run_plugin(plugin: Plugin) -> () or Error {
    // Plugin gets its own pools
    const plugin_entities = Pool.new()
    const plugin_state = Pool.new()

    try plugin.run(plugin_entities, plugin_state)

    // Unload: drop pools, all plugin handles become invalid
    // Any stale handles in event queues safely fail on access
    drop(plugin_entities)
    drop(plugin_state)
    Ok(())
}
```

| Property | Pointer-based | Handle-based |
|----------|---------------|--------------|
| Unload plugin memory | Must trace all references | Drop the pool |
| Dangling references after unload | Use-after-free (UB) | Panic or `None` (safe) |
| Memory reclamation | GC or manual | Instant (pool owns all data) |

This pattern works because handle validity is checked at access time, not at creation time. A handle to a dropped pool fails safely — no undefined behavior.

---

## Integration Notes

- **Memory Model:** Pools own their data. Handles are values, not references.
- **Type System:** Generic code works uniformly across pool configurations.
- **Borrowing:** Expression-scoped borrowing enables interleaved mutation.
- **Linear Types:** Pool<Linear> requires explicit consumption of each element.
- **Concurrency:** Pools are not `Sync` by default. Use channels for cross-task access (pass handles, not data).
- **Compiler:** Local analysis only. No whole-program analysis needed.
- **C Interop:** Convert Pool to Vec at FFI boundaries. Handles contain runtime IDs.

## See Also

- [Borrowing](borrowing.md) — One rule: views last as long as the source is stable
- [Resource Types](resource-types.md) — Resource consumption requirements
- [Closures](closures.md) — Pool+Handle pattern for shared mutable state
- [Collections](../stdlib/collections.md) — Vec and Map types
