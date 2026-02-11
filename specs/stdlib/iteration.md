<!-- id: std.iteration -->
<!-- status: decided -->
<!-- summary: Iteration modes for Vec, Pool, and Map collections -->
<!-- depends: stdlib/collections.md, memory/pools.md, control/loops.md, types/iterator-protocol.md -->

# Collection Iteration Patterns

Three iteration modes per collection: index/handle, ref, and take-all. Mode determines ownership and mutation rights.

## Iteration Modes

| Rule | Description |
|------|-------------|
| **I1: Index mode** | Default `for x in collection` yields index/handle/key. Allows mutation via indexing |
| **I2: Ref mode** | `.iter()` yields borrowed elements. Collection mutation forbidden (except in-place field writes) |
| **I3: Take-all mode** | `.take_all()` consumes the collection, yields owned values |

| Collection | Index/Handle Mode | Ref Mode | Take All Mode |
|------------|-------------------|----------|--------------|
| `Vec<T>` | `for i in vec` -> `usize` | `for item in vec.iter()` -> borrowed `T` | `for item in vec.take_all()` -> `T` |
| `Pool<T>` | `for h in pool` -> `Handle<T>` | `for (h, item) in pool.iter()` -> `(Handle<T>, borrowed T)` | `for item in pool.take_all()` -> `T` |
| `Map<K,V>` | `for k in map` -> `K` (K: Copy) | `for (k, v) in map.iter()` -> `(borrowed K, borrowed V)` | `for (k,v) in map.take_all()` -> `(K, V)` |

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

## Ref Mode Constraints

| Rule | Description |
|------|-------------|
| **R1: No structural mutation** | `pool.remove(h)`, `pool.insert(x)` forbidden inside ref loops |
| **R2: In-place mutation OK** | `pool[h].field = x` allowed (doesn't invalidate iteration) |
| **R3: No take parameters** | Cannot pass borrowed items to `take` parameters |

| Operation | In Ref Loop | Reason |
|-----------|-------------|--------|
| `pool.remove(h)` | Forbidden | Structural mutation |
| `pool.insert(item)` | Forbidden | Structural mutation |
| `pool[h].field = x` | Allowed | In-place, no invalidation |
| Borrow (read-only) param | Allowed | Cannot mutate |
| Borrow (mutable) param | Forbidden | Would allow mutation |
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

Index/handle mode only. Programmer responsibility.

| Rule | Description |
|------|-------------|
| **M1: In-place safe** | `vec[i].field = x` doesn't invalidate indices |
| **M2: Growth unsafe** | `vec.push(x)` inside loop: new elements not visited, length captured at start |
| **M3: Removal unsafe** | `vec.swap_remove(i)` inside loop: later indices refer to wrong elements |

## Linear Types

| Rule | Description |
|------|-------------|
| **L1: No index iteration** | `Vec<Linear>` forbids index iteration. Use `.take_all()` |
| **L2: Pool handles OK** | Pool handles are Copy, so handle iteration works for linear pool elements |

<!-- test: skip -->
```rask
// COMPILE ERROR: index iteration on Vec<Linear>
for i in files { try files[i].close() }

// Required: take_all consumes each element
for file in files.take_all() { try file.close() }
```

## Map Key Constraints

| Rule | Description |
|------|-------------|
| **K1: Copy keys required** | `for k in map` requires K: Copy. Non-Copy keys: use `.iter()` or `.take_all()` |

<!-- test: skip -->
```rask
// OK: u64 is Copy
for id in counts { print(counts[id]) }

// ERROR: string is not Copy — use .iter()
for (key, value) in config.iter() {
    print(key, value)
}
```

## Error Propagation (`try`)

| Loop Type | Original Collection | Remaining Items |
|-----------|---------------------|-----------------|
| Index/Handle/Ref mode | Intact | N/A |
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
ERROR [std.iteration/L1]: cannot use index iteration on Vec<Linear>
   |
3  |  for i in files { try files[i].close() }
   |           ^^^^^ Linear types must be consumed via .take_all()

WHY: Index access cannot transfer ownership of linear resources.

FIX: Use take_all:

  for file in files.take_all() { try file.close() }
```

```
ERROR [std.iteration/K1]: map key iteration requires Copy keys
   |
3  |  for key in config { ... }
   |             ^^^^^^ string is not Copy

FIX: Use .iter() or .take_all():

  for (key, value) in config.iter() { ... }
```

```
ERROR [std.iteration/R1]: cannot mutate collection during ref iteration
   |
3  |  for (h, item) in pool.iter() {
   |                   ^^^^^^^^^^^ ref iteration borrows pool
4  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ cannot mutate

FIX: Collect handles first, then mutate:

  const to_remove: Vec<Handle<T>> = Vec.new()
  for (h, item) in pool.iter() { if item.expired { to_remove.push(h) } }
  for h in to_remove { pool.remove(h) }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty collection | — | Loop body never executes |
| `Vec<Linear>` index iteration | L1 | Compile error |
| `Map<string, V>` key iteration | K1 | Compile error |
| Out-of-bounds index | — | Panic |
| Invalid handle | — | Panic (generation mismatch) |
| `break value` for !Copy | A4 | Requires `.clone()` |
| Infinite range (`0..`) | — | Works (lazy) |
| Zero-sized types (`Vec<()>`) | — | Yields indices 0..len |

---

## Appendix (non-normative)

### Patterns & Guidance

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Index/Handle | Need to mutate or remove during iteration |
| Ref | Read-only access, avoid cloning large items |
| Take All | Consuming all items, transferring ownership |

**Safe removal patterns:**

<!-- test: skip -->
```rask
// 1. Reverse iteration for removal
for i in (0..vec.len()).rev() {
    if vec[i].expired { vec.swap_remove(i) }
}

// 2. Collect indices, then mutate
const to_remove = Vec.new()
for i in vec { if vec[i].expired { to_remove.push(i) } }
for i in to_remove.rev() { vec.swap_remove(i) }

// 3. Filter via take_all
const vec = vec.take_all().filter(|item| !item.expired).collect()
```

### See Also

- `std.collections` — Vec, Map APIs
- `mem.pools` — Pool and Handle types
- `ctrl.loops` — Loop syntax and desugaring
- `type.iterator-protocol` — Iterator trait
