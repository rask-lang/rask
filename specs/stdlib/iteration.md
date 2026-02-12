<!-- id: std.iteration -->
<!-- status: decided -->
<!-- summary: Iteration modes for Vec, Pool, and Map collections -->
<!-- depends: stdlib/collections.md, memory/pools.md, control/loops.md, types/iterator-protocol.md -->

# Collection Iteration Patterns

Three iteration modes per collection: value (default), index (explicit), and take-all. Default iteration yields borrowed values (read-only), matching all major languages.

## Iteration Modes

| Rule | Description |
|------|-------------|
| **I1: Value mode** | Default `for x in collection` yields borrowed elements (read-only). Natural, matches all major languages |
| **I2: Index mode** | Use range `for i in 0..collection.len()` for index-based mutation |
| **I3: Take-all mode** | `.take_all()` consumes the collection, yields owned values |

| Collection | Value Mode (Default) | Index Mode | Take All Mode |
|------------|---------------------|------------|--------------|
| `Vec<T>` | `for item in vec` -> borrowed `T` | `for i in 0..vec.len()` -> `usize` | `for item in vec.take_all()` -> `T` |
| `Pool<T>` | `for item in pool` -> borrowed `T` | `for h in pool.handles()` -> `Handle<T>` | `for item in pool.take_all()` -> `T` |
| `Map<K,V>` | `for (k, v) in map` -> `(K, borrowed V)` | `for k in map.keys()` -> `K` | `for (k,v) in map.take_all()` -> `(K, V)` |

## Value Access

| Rule | Description |
|------|-------------|
| **A1: Copy out** | `collection[i]` copies T when T: Copy (≤16 bytes) |
| **A2: Field copy** | `collection[i].field` copies field when field: Copy |
| **A3: Expression borrow** | `collection[i].method()` borrows for call, released at `;` |
| **A4: No move** | `collection[i]` where T: !Copy is a compile error. Use `.clone()` or `.take_all()` |

<!-- test: skip -->
```rask
vec[i].field              // Copy out field (A2)
vec[i].method()           // Expression-scoped borrow (A3)
vec[i] = value            // Mutate in place
```

## Value Mode Constraints

| Rule | Description |
|------|-------------|
| **R1: No structural mutation** | `pool.remove(h)`, `pool.insert(x)` forbidden inside value loops |
| **R2: Read-only** | Value mode is read-only. Use index mode for mutation |
| **R3: No take parameters** | Cannot pass borrowed items to `take` parameters |

| Operation | In Value Loop | Reason |
|-----------|---------------|--------|
| `pool.remove(h)` | Forbidden | Structural mutation |
| `pool.insert(item)` | Forbidden | Structural mutation |
| Field mutation | Forbidden | Value mode is read-only |
| Read-only access | Allowed | Natural use case |
| `take` param | Forbidden | Ownership transfer impossible |

## Take-All Iteration

| Rule | Description |
|------|-------------|
| **T1: Consumes collection** | `.take_all()` takes ownership (`take self`). Collection left empty |
| **T2: Buffer transfer** | Collection's internal buffer transferred to iterator |
| **T3: Early exit drops** | On `break`/`return`/`try`, remaining items dropped in LIFO order |

| Collection | Method | Yields |
|------------|--------|--------|
| `Vec<T>` | `.take_all()` | `T` |
| `Pool<T>` | `.take_all()` | `T` |
| `Map<K,V>` | `.take_all()` | `(K, V)` |

<!-- test: skip -->
```rask
for file in files.take_all() {
    if file.is_locked() {
        break  // Remaining files DROPPED (LIFO order)
    }
    try file.close()
}
```

## Mutation During Iteration

Mutation requires index mode (explicit range or `.handles()`). Programmer responsibility.

| Rule | Description |
|------|-------------|
| **M1: In-place safe** | `vec[i].field = x` doesn't invalidate indices |
| **M2: Growth unsafe** | `vec.push(x)` inside loop: new elements not visited, length captured at start |
| **M3: Removal unsafe** | `vec.swap_remove(i)` inside loop: later indices refer to wrong elements |

<!-- test: skip -->
```rask
// Mutation requires explicit index mode
for i in 0..entities.len() {
    entities[i].health -= 10    // Clear you're mutating via index
}

// Value mode is read-only
for entity in entities {
    print(entity.health)        // Natural iteration
}
```

## Linear Types

| Rule | Description |
|------|-------------|
| **L1: No value iteration** | `Vec<Linear>` forbids value iteration (would alias). Use `.take_all()` |
| **L2: Pool iteration OK** | Pool value iteration works - items borrowed one at a time, no aliasing |

<!-- test: skip -->
```rask
// COMPILE ERROR: value iteration on Vec<Linear> would alias
for file in files { try file.close() }

// Required: take_all consumes each element
for file in files.take_all() { try file.close() }
```

## Map Key Constraints

| Rule | Description |
|------|-------------|
| **K1: Keys copied** | `for (k, v) in map` copies keys (required K: Copy) to allow lookup. Non-Copy keys: use `.take_all()` |

<!-- test: skip -->
```rask
// OK: u64 is Copy
for (id, count) in counts {
    print(id, count)
}

// OK: string keys (Copy), borrowed values
for (key, value) in config {
    print(key, value)
}
```

## Error Propagation (`try`)

| Loop Type | Original Collection | Remaining Items |
|-----------|---------------------|-----------------|
| Value/Index mode | Intact | N/A |
| Take-all mode | Already taken | Dropped (LIFO) |

<!-- test: skip -->
```rask
for file in files.take_all() {
    ensure file.close()   // Runs if try exits
    try file.write(data)
}
```

## Error Messages

```
ERROR [std.iteration/L1]: cannot iterate over Vec<Linear> by value
   |
3  |  for file in files { try file.close() }
   |              ^^^^^ Linear types cannot be borrowed in loops

WHY: Value iteration borrows each item, but linear types must be consumed.

FIX: Use take_all to consume:

  for file in files.take_all() { try file.close() }
```

```
ERROR [std.iteration/R1]: cannot mutate collection during value iteration
   |
3  |  for item in pool {
   |              ^^^^ value iteration borrows pool
4  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ cannot mutate

FIX: Use index mode or collect first:

  // Option 1: Index mode
  for h in pool.handles() {
      if pool[h].expired { pool.remove(h) }
  }

  // Option 2: Collect first
  const to_remove = Vec.new()
  for item in pool {
      if item.expired { to_remove.push(h) }
  }
  for h in to_remove { pool.remove(h) }
```

```
ERROR [std.iteration/M2]: cannot mutate items during value iteration
   |
3  |  for entity in entities {
   |                ^^^^^^^^ value iteration is read-only
4  |      entity.health -= 10
   |      ^^^^^^^^^^^^^^^^^^ cannot mutate borrowed value

FIX: Use index mode for mutation:

  for i in 0..entities.len() {
      entities[i].health -= 10
  }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty collection | — | Loop body never executes |
| `Vec<Linear>` value iteration | L1 | Compile error |
| Out-of-bounds index | — | Panic |
| Invalid handle | — | Panic (generation mismatch) |
| Mutate in value loop | R2 | Compile error |
| `break value` for !Copy | A4 | Requires `.clone()` |
| Infinite range (`0..`) | — | Works (lazy) |
| Zero-sized types (`Vec<()>`) | — | Yields values (all identical) |

---

## Appendix (non-normative)

### Patterns & Guidance

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Value (default) | Read-only iteration - the common case |
| Index (explicit) | Need to mutate or remove during iteration |
| Take All | Consuming all items, transferring ownership |

**Safe removal patterns:**

<!-- test: skip -->
```rask
// 1. Reverse iteration for removal
for i in (0..vec.len()).rev() {
    if vec[i].expired { vec.swap_remove(i) }
}

// 2. Collect during value iteration, then mutate
const to_remove = Vec.new()
for (i, item) in vec.enumerate() {
    if item.expired { to_remove.push(i) }
}
for i in to_remove.iter().rev() { vec.swap_remove(i) }

// 3. Filter via take_all
const vec = vec.take_all().filter(|item| !item.expired).collect()
```

### See Also

- `std.collections` — Vec, Map APIs
- `mem.pools` — Pool and Handle types
- `ctrl.loops` — Loop syntax and desugaring
- `type.iterator-protocol` — Iterator trait
