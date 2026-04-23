<!-- id: std.iteration -->
<!-- status: decided -->
<!-- summary: Iteration modes for Vec, Pool, and Map collections -->
<!-- depends: stdlib/collections.md, memory/pools.md, control/loops.md, types/sequence-protocol.md -->

# Collection Iteration Patterns

Four iteration modes per collection: value (default, read-only), mutable (read-write), index (explicit), and take-all (consuming).

## Iteration Modes

| Rule | Description |
|------|-------------|
| **I1: Value mode** | Default `for x in collection` yields borrowed elements (read-only). Natural, matches all major languages |
| **I2: Index mode** | Use range `for i in 0..collection.len()` for index-based mutation |
| **I3: Take-all mode** | `.take_all()` consumes the collection, yields owned values |
| **I4: Mutable mode** | `for mutate x in collection` yields mutable access to each element. Structural mutation still forbidden |

| Collection | Value Mode (Default) | Mutable Mode | Index Mode | Take All Mode |
|------------|---------------------|--------------|------------|--------------|
| `Vec<T>` | `for item in vec` -> borrowed `T` | `for mutate item in vec` -> mutable `T` | `for i in 0..vec.len()` -> `usize` | `for item in vec.take_all()` -> `T` |
| `Pool<T>` | `for item in pool` -> borrowed `T` | `for mutate item in pool` -> mutable `T` | `for h in pool.handles()` -> `Handle<T>` | `for item in pool.take_all()` -> `T` |
| `Map<K,V>` | `for (k, v) in map` -> `(K, borrowed V)` | `for mutate (k, v) in map` -> `(K, mutable V)` | `for k in map.keys()` -> `K` | `for (k,v) in map.take_all()` -> `(K, V)` |

## Value Access

| Rule | Description |
|------|-------------|
| **A1: Copy out** | `collection[i]` copies T when T: Copy (≤16 bytes) |
| **A2: Field copy** | `collection[i].field` copies field when field: Copy |
| **A3: Expression borrow** | `collection[i].method()` borrows for call, released at `;` |
| **A4: No move** | `collection[i]` where T: !Copy is a compile error in user code. Use `.clone()` or `.take_all()`. Loop bindings bypass this — see `ctrl.loops/LP17` |

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

## Mutable Iteration

`for mutate` provides mutable access to each element without switching to index mode. The binding acts as a mutable alias for the current element — each use desugars to inline access on the underlying collection.

| Rule | Description |
|------|-------------|
| **MI1: No structural mutation** | Cannot insert, remove, or clear during mutable iteration (same as R1) |
| **MI2: In-place mutation** | `item.field = x` is in-place mutation. `item = x` replaces the entire element |
| **MI3: Collection readable** | Other elements accessible via inline expression access during iteration |
| **MI4: No take parameters** | Cannot pass `item` to `take` parameters (same as R3) |

<!-- test: skip -->
```rask
// Mutable iteration (clean)
for mutate entity in entities {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Status.Dead
    }
}

// Equivalent index mode (verbose)
for i in 0..entities.len() {
    entities[i].health -= damage
    entities[i].last_hit = now()
    if entities[i].health <= 0 {
        entities[i].status = Status.Dead
    }
}
```

Reading other elements is allowed (MI3):

<!-- test: skip -->
```rask
for mutate entity in entities {
    entity.health -= damage
    const max = entities[0].max_health    // OK: inline read of another element
    if entity.health > max {
        entity.health = max
    }
}
```

**Map mutable iteration:** Keys are always copies. `mutate` applies to the value binding:

<!-- test: skip -->
```rask
for mutate (key, value) in config {
    // key is a copy (immutable), value is mutable
    value.count += 1
    value.last_access = now()
}
```

**Pool mutable iteration:**

<!-- test: skip -->
```rask
for mutate entity in pool {
    entity.health -= 10
    entity.velocity *= 0.9
}
```

### When to use mutable vs index mode

| Need | Use |
|------|-----|
| Mutate element fields | `for mutate item in vec` — clean, intent clear |
| Mutate + access other elements | `for mutate item in vec` — MI3 allows reads |
| Structural mutation (insert/remove) | Index mode — `for i in 0..` or `for h in pool.handles()` |
| Swap or reorder elements | Index mode — need indices for `vec.swap(i, j)` |

## Take-All Iteration

| Rule | Description |
|------|-------------|
| **T1: Consumes collection** | `.take_all()` takes ownership (`take self`). Collection left empty |
| **T2: Buffer transfer** | Collection's internal buffer transferred to iterator |
| **T3: Early exit cleanup** | On `break`/`return`/`try`, remaining items cleaned up in LIFO order |

| Collection | Method | Yields |
|------------|--------|--------|
| `Vec<T>` | `.take_all()` | `T` |
| `Pool<T>` | `.take_all()` | `T` |
| `Map<K,V>` | `.take_all()` | `(K, V)` |

<!-- test: skip -->
```rask
for file in files.take_all() {
    if file.is_locked() {
        break  // Remaining files cleaned up (LIFO order)
    }
    try file.close()
}
```

## Mutation During Iteration

In-place mutation uses mutable mode (`for mutate`). Structural mutation requires index mode (explicit range or `.handles()`). Programmer responsibility for structural changes.

| Rule | Description |
|------|-------------|
| **M1: In-place safe** | `vec[i].field = x` doesn't invalidate indices |
| **M2: Growth unsafe** | `vec.push(x)` inside loop: new elements not visited, length captured at start |
| **M3: Removal unsafe** | `vec.swap_remove(i)` inside loop: later indices refer to wrong elements |

<!-- test: skip -->
```rask
// Mutable mode for in-place mutation
for mutate entity in entities {
    entity.health -= 10
}

// Index mode for structural mutation
for i in (0..entities.len()).rev() {
    if entities[i].health <= 0 {
        entities.swap_remove(i)
    }
}

// Value mode is read-only
for entity in entities {
    print(entity.health)
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
| **K2: Float key warning** | `Map<f32, V>` and `Map<f64, V>` produce a compile-time warning. NaN != NaN breaks map lookup invariants |

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
ERROR [std.iteration/R2]: cannot mutate items during value iteration
   |
3  |  for entity in entities {
   |                ^^^^^^^^ value iteration is read-only
4  |      entity.health -= 10
   |      ^^^^^^^^^^^^^^^^^^ cannot mutate borrowed value

FIX: Use mutable iteration:

  for mutate entity in entities {
      entity.health -= 10
  }
```

```
ERROR [std.iteration/MI1]: cannot modify collection structure during mutable iteration
   |
3  |  for mutate entity in entities {
   |                       ^^^^^^^^ mutable iteration active
4  |      entities.push(new_entity)
   |      ^^^^^^^^^^^^^^^^^^^^^^^^^ structural mutation forbidden

WHY: Mutable iteration allows in-place element mutation, not structural
     changes (insert/remove/clear). Structural changes could invalidate
     the iteration position.

FIX: Use index mode for structural mutation, or collect changes first:

  // Option 1: index mode
  for i in 0..entities.len() {
      if entities[i].should_split {
          entities.push(entities[i].split())
      }
  }

  // Option 2: collect first
  let to_add = Vec.new()
  for mutate entity in entities {
      if entity.should_split {
          to_add.push(entity.split())
      }
  }
  for item in to_add.take_all() { entities.push(item) }
```

```
WARNING [std.iteration/K2]: float type used as map key
   |
3  |  const cache: Map<f64, Result> = Map.new()
   |                   ^^^ f64 keys break map lookups when NaN is present

WHY: NaN != NaN by IEEE 754. A NaN key can be inserted but never
     found by lookup, silently breaking map semantics.

FIX: Use an integer key or newtype wrapper with defined equality:

  const cache: Map<u64, Result> = Map.new()
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty collection | — | Loop body never executes |
| `Vec<Linear>` value iteration | L1 | Compile error |
| `Vec<Linear>` mutable iteration | L1 | Compile error (same reason — use `.take_all()`) |
| Out-of-bounds index | — | Panic |
| Invalid handle | — | Panic (generation mismatch) |
| Mutate in value loop | R2 | Compile error |
| Structural mutation in mutable loop | MI1 | Compile error |
| `break value` for !Copy | A4 | Requires `.clone()` |
| Infinite range (`0..`) | — | Works (lazy) |
| Zero-sized types (`Vec<void>`) | — | Yields values (all identical) |
| `Map<f32, V>` or `Map<f64, V>` | K2 | Compile-time warning |

---

## Appendix (non-normative)

### Patterns & Guidance

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Value (default) | Read-only iteration — the common case |
| Mutable | In-place element mutation without structural changes |
| Index (explicit) | Structural mutation (insert/remove), swaps, or need indices |
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
for i in to_remove.rev() { vec.swap_remove(i) }

// 3. Filter via take_all
const vec = vec.take_all().filter(|item| !item.expired).collect()
```

### See Also

- `std.collections` — Vec, Map APIs
- `mem.pools` — Pool and Handle types
- `ctrl.loops` — Loop syntax and desugaring
- `type.sequence` — Sequence protocol, adapters, terminals
