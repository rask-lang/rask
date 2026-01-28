# Elaboration: Pool Iteration Semantics

## Specification

### Pool Iteration Modes

**Pool<T> supports three iteration modes:**

| Syntax | Yields | Use Case | Semantics |
|--------|--------|----------|-----------|
| `for h in pool` | `Handle<T>` | Modify/remove items | Iteration over handles only |
| `for (h, item) in &pool` | `(Handle<T>, &T)` | Read items without copying | Iteration over handle+ref pairs |
| `for item in pool.drain()` | `T` | Consume all items | Takes ownership, empties pool |

**Desugaring:**

```
// Mode 1: Handles only
for h in pool { body }
// Equivalent to:
for h in pool.handles() { body }

// Mode 2: Handles + refs
for (h, item) in &pool { body }
// Equivalent to:
{
    for h in pool.handles() {
        let item = &pool[h];  // Expression-scoped borrow
        body
    }
}

// Mode 3: Drain
for item in pool.drain() { body }
// Equivalent to:
{
    while let Some(item) = pool._drain_next() {
        body
    }
}
```

### When to Use Each Mode

**Handles only (`for h in pool`):**
- When you need to modify collection during iteration
- When you need to remove items
- When you only need handles for other operations

```
for h in pool {
    if pool[h].expired {
        pool.remove(h);  // OK: no borrow active
    }
}
```

**Handles + refs (`for (h, item) in &pool`):**
- When you need to read items efficiently (no Copy/Clone)
- When you need both handle and data
- Common pattern for debugging/logging

```
for (h, entity) in &pool {
    print("Entity {} at {:?}", h, entity.position);
}
```

**Drain (`for item in pool.drain()`):**
- When consuming all items
- When transferring ownership of all items
- When clearing pool and processing items

```
for entity in entities.drain() {
    entity.save_to_disk()?;
}
// entities is now empty
```

### Unified Collection Iteration Syntax

**General pattern for all collections:**

| Collection | `for x in coll` | `for x in &coll` | `for x in coll.drain()` |
|------------|-----------------|------------------|-------------------------|
| `Vec<T>` | `usize` (index) | Not applicable | `T` (owned) |
| `Pool<T>` | `Handle<T>` | `(Handle<T>, &T)` | `T` (owned) |
| `Map<K,V>` | `K` (requires K: Copy) | `(&K, &V)` | `(K, V)` (owned) |

**Why `for x in &pool` is different:**
- For Vec: Indices are cheap, no need for ref mode
- For Pool: Handles are cheap BUT accessing every item via `pool[h]` would be verbose
- For Map: Keys might not be Copy, need ref mode

**Rationale:** Pool iteration benefits from a ref mode because:
1. Handles are stable identifiers (unlike Vec indices)
2. Common pattern: iterate all items, read fields
3. Without ref mode: `for h in pool { let item = &pool[h]; ... }` is verbose

### Pool Iteration vs Vec Iteration

| Aspect | Vec | Pool |
|--------|-----|------|
| Index/Handle iteration | `for i in vec` → `usize` | `for h in pool` → `Handle<T>` |
| Ref iteration | ❌ Not provided | ✅ `for (h, item) in &pool` |
| Why different? | Indices are ephemeral | Handles are stable IDs |
| Common pattern | Access via `vec[i]` | Often need both handle AND data |

**Example comparison:**

```
// Vec: index iteration is natural
for i in vec {
    process(&vec[i]);  // One access
}

// Pool: handle iteration can be verbose
for h in pool {
    process(&pool[h]);  // One access, but handle is often needed too
}

// Pool: ref mode is ergonomic for read-only
for (h, item) in &pool {
    process(h, item);  // Both handle and data available
}
```

### Map Iteration Modes

**For consistency, Map also supports ref mode:**

| Syntax | Yields | Use Case |
|--------|--------|----------|
| `for k in map` | `K` (requires K: Copy) | Modify map, Copy keys only |
| `for (k, v) in &map` | `(&K, &V)` | Read all entries without cloning |
| `for (k, v) in map.drain()` | `(K, V)` | Consume all entries |

**Example:**
```
// Map with Copy keys:
let counts: Map<u64, u32> = ...;
for id in counts {
    counts[id] += 1;  // Modify in place
}

// Map with non-Copy keys:
let config: Map<String, String> = ...;
for (key, value) in &config {
    print(key, value);  // No cloning needed
}
```

### Borrowing Semantics for Ref Mode

**Question:** Does `for (h, item) in &pool` borrow the pool?

**Answer:** NO. Despite the `&` syntax, ref mode iteration does NOT create a pool-level borrow.

**Rationale:**
- The `&` indicates "iterate in read mode" (like `&pool` parameter mode)
- Each iteration yields expression-scoped refs
- Between iterations, no borrow is active
- Pool remains accessible (for reads)

**Allowed:**
```
for (h, item) in &pool {
    let other = &pool[other_handle];  // OK: expression-scoped borrows
    compare(item, other);
}
```

**Forbidden:**
```
for (h, item) in &pool {
    pool.remove(h);  // COMPILE ERROR: cannot mutate while iterating in ref mode
}
```

**Desugaring shows why:**
```
// Conceptual desugaring:
for h in pool.handles() {
    let item = &pool[h];  // Borrow active
    body
    // Borrow released here
}
```

The compiler must detect that `item` escapes the loop iteration and prevent mutations.

**Precise rule:** Ref mode iteration (`&pool`) creates a **read-mode iteration context**. Mutations are forbidden, but multiple reads are allowed (expression-scoped).

### Handle vs Ref Mode - Mutation Rules

| Mode | Mutation Allowed | Removal Allowed | Rationale |
|------|------------------|-----------------|-----------|
| Handle (`for h in pool`) | ✅ Yes | ✅ Yes | No refs held, same as Gap 1 |
| Ref (`for (h, item) in &pool`) | ❌ No | ❌ No | Ref `item` is live across iterations |
| Drain | ❌ No | N/A | Pool is borrowed mutably |

**Why ref mode forbids mutation:**
The ref `item` from iteration N might be invalidated by removal in iteration N+1. To prevent this, ref mode forbids mutations.

**Example:**
```
// FORBIDDEN:
for (h, item) in &pool {
    if item.expired {
        pool.remove(h);  // ERROR: cannot mutate during ref iteration
    }
}

// Use handle mode instead:
for h in pool {
    if pool[h].expired {
        pool.remove(h);  // OK: no ref held
    }
}
```

### Edge Cases

| Case | Handling |
|------|----------|
| Empty pool | Loop body never executes |
| Pool modified during handle iteration | Allowed (programmer responsibility) |
| Pool modified during ref iteration | COMPILE ERROR |
| Stale handle during iteration | Panic on access or `get()` returns None |
| `for (h, item) in &pool` with linear Pool | ❌ ERROR: cannot create refs to linear items |
| Break during ref iteration | OK, refs dropped |
| `?` during ref iteration | OK, refs dropped before error propagates |

### Integration with Parameter Modes

**Passing pools to functions:**

```
fn process_pool(pool: Pool<Entity>) {
    for h in pool { ... }  // OK: owns pool
}

fn process_pool(read pool: Pool<Entity>) {
    for h in pool { ... }           // OK: iterate handles
    for (h, e) in &pool { ... }     // OK: iterate refs
    pool.remove(h)                   // ERROR: read mode forbids mutation
}

fn process_pool(mutate pool: Pool<Entity>) {
    for h in pool {
        pool[h].health -= 10;        // OK: mutate items
        pool.remove(h);              // OK: mutate pool
    }
}
```

## Self-Validation

### Does it conflict with CORE design?
**NO.**
- ✅ Aligns with expression-scoped borrows (refs released between iterations)
- ✅ No storable references (iteration refs are expression-scoped)
- ✅ Local analysis (read vs mutate modes are explicit in syntax)
- ✅ Transparent costs (iteration modes are clear)

### Is it internally consistent?
**YES.**
- Handle mode allows mutation ✅
- Ref mode forbids mutation ✅
- Clear distinction and reasoning ✅
- Consistent with Vec/Map patterns ✅

### Does it conflict with other specs?
**NO, resolves conflict.**
- `iterators-and-loops.md`: Handle iteration ✅ (now explicit)
- `dynamic-data-structures.md`: Ref iteration ✅ (now integrated)
- Both modes are valid, serve different use cases ✅

### Is it complete enough to implement?
**YES.**
- All three iteration modes specified ✅
- Desugaring provided ✅
- Borrowing semantics clear ✅
- Mutation rules defined ✅
- Edge cases covered ✅

### Is it concise?
**YES.** ~100 lines, tables for comparison, clear use cases.

## Summary

**Key insight:** Pool supports THREE iteration modes:
1. **Handle iteration** (`for h in pool`) — Like Vec index iteration, allows mutation
2. **Ref iteration** (`for (h, item) in &pool`) — Ergonomic read-only, forbids mutation
3. **Drain** (`for item in pool.drain()`) — Consuming iteration

**Rationale:**
- Handle iteration: Consistency with Vec (indices → handles)
- Ref iteration: Ergonomics for read-heavy patterns (common in pools)
- Both modes are justified and serve real use cases

**Resolution:** The "conflict" was incomplete specification. Both modes exist and are complementary.
