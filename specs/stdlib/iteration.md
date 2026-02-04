# Collection Iteration Patterns

This spec covers how to iterate over standard library collections (Vec, Pool, Map).

For loop syntax and desugaring, see [control/loops.md](../control/loops.md).
For the Iterator trait, see [types/iterator-protocol.md](../types/iterator-protocol.md).

---

## Iteration Modes

Collections support multiple iteration modes depending on access needs:

| Collection | Index/Handle Mode | Ref Mode | Take All Mode |
|------------|-------------------|----------|--------------|
| `Vec<T>` | `for i in vec` → `usize` | `for item in vec.iter()` → borrowed `T` | `for item in vec.take_all()` → `T` |
| `Pool<T>` | `for h in pool` → `Handle<T>` | `for (h, item) in pool.iter()` → `(Handle<T>, borrowed T)` | `for item in pool.take_all()` → `T` |
| `Map<K,V>` | `for k in map` → `K` (K: Copy) | `for (k, v) in map.iter()` → `(borrowed K, borrowed V)` | `for (k,v) in map.take_all()` → `(K, V)` |

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Index/Handle | Need to mutate or remove during iteration |
| Ref | Read-only access, avoid cloning large items |
| Take All | Consuming all items, transferring ownership |

---

## Value Access

Access follows expression-scoped collection rules:

| Expression | Behavior | Constraint |
|------------|----------|------------|
| `vec[i]` where T: Copy (≤16 bytes) | Copies out T | T: Copy |
| `vec[i].field` where field: Copy | Copies out field | field: Copy |
| `vec[i].method()` | Borrows for call, releases at `;` | Expression-scoped |
| `vec[i]` passed to function | Borrows for call duration | Cannot store in callee |
| `vec[i] = value` | Mutates in place | - |
| `vec[i]` where T: !Copy | **ERROR**: cannot move | Use `.clone()` or `.take_all()` |

**Rule:** Each `collection[index]` access is independent. Borrow released at statement end (semicolon).

---

## Ref Mode

Ref mode (`for (h, item) in collection.iter()`) provides ergonomic read-only access.

**Enforcement:** The compiler forbids all mutation operations within ref loop blocks:

| Operation | In Ref Mode Loop | Error |
|-----------|------------------|-------|
| `pool.remove(h)` | Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.insert(item)` | Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool[h].field = x` | Allowed | Mutates item in place, doesn't invalidate iteration |

**Function calls:**

| Parameter Mode | In Ref Loop | Reason |
|----------------|-------------|--------|
| borrow (read-only) | Allowed | Cannot mutate by definition |
| borrow (mutable) | Forbidden | Would allow mutation |
| `take` | Forbidden | Ownership transfer impossible |

---

## Take All Iteration

**Syntax:** `collection.take_all()`

Yields owned values, consuming the collection:

<!-- test: skip -->
```rask
for item in vec.take_all() {
    process(item)  // item is owned T
}
// vec is now empty
```

| Collection | Method | Yields | Returns |
|------------|--------|--------|---------|
| `Vec<T>` | `.take_all()` | `T` | `VecTakeAll<T>` |
| `Pool<T>` | `.take_all()` | `T` | `PoolTakeAll<T>` |
| `Map<K,V>` | `.take_all()` | `(K, V)` | `MapTakeAll<K,V>` |

**Key Properties:**
1. `.take_all()` takes ownership of collection (`take self`)
2. Collection's internal buffer transferred to take_all iterator
3. Original collection left in valid empty state
4. When take_all iterator drops, remaining items dropped in LIFO order

**Early Exit:**

<!-- test: skip -->
```rask
for file in files.take_all() {
    if file.is_locked() {
        break  // Remaining files DROPPED (LIFO order)
    }
    try file.close()
}
```

---

## Mutation During Iteration

**Allowed but programmer responsibility** (index/handle mode only):

| Pattern | Safety | Notes |
|---------|--------|-------|
| `for i in vec { vec[i].field = x }` | Safe | In-place mutation doesn't invalidate index |
| `for i in vec { try vec.push(x) }` | Unsafe | New elements not visited; length captured at start |
| `for i in vec { vec.swap_remove(i) }` | Unsafe | Later indices refer to wrong elements |

**Safe Patterns:**

1. **Reverse iteration for removal:**
   <!-- test: skip -->
   ```rask
   for i in (0..vec.len()).rev() {
       if vec[i].expired { vec.swap_remove(i) }
   }
   ```

2. **Collect indices, then mutate:**
   <!-- test: skip -->
   ```rask
   const to_remove = Vec.new()
   for i in vec { if vec[i].expired { to_remove.push(i) } }
   for i in to_remove.rev() { vec.swap_remove(i) }
   ```

3. **Filter via take_all:**
   <!-- test: skip -->
   ```rask
   const vec = vec.take_all().filter(|item| !item.expired).collect()
   ```

---

## Error Propagation (`try`)

When `try` exits a loop:

| Loop Type | Original Collection | Remaining Items |
|-----------|---------------------|-----------------|
| Index mode | Intact | N/A |
| Handle mode | Intact | N/A |
| Ref mode | Intact | N/A |
| Take all mode | Already taken | Dropped (LIFO) |

**Take all + ensure:**
<!-- test: skip -->
```rask
for file in files.take_all() {
    ensure file.close()   // Runs if try exits
    try file.write(data)
}
```

---

## Linear Types

**Index iteration forbidden for `Vec<Linear>`:**

<!-- test: skip -->
```rask
// COMPILE ERROR:
for i in files { try files[i].close() }

// Required:
for file in files.take_all() { try file.close() }
```

**Pool iteration works** (handles are Copy):
<!-- test: skip -->
```rask
for h in pool {
    const removed = try pool.remove(h)
    try removed.close()
}
```

---

## Map Iteration

**Key mode requires Copy keys:**
<!-- test: skip -->
```rask
// OK: u64 is Copy
for id in counts { print(counts[id]) }

// ERROR: string is not Copy
for key in config { ... }  // Use .iter() or .take_all()
```

**Ref mode for all key types:**
<!-- test: skip -->
```rask
for (key, value) in config.iter() {
    print(key, value)
}
```

---

## Edge Cases

| Case | Handling |
|------|----------|
| Empty collection | Loop body never executes |
| `Vec<Linear>` index iteration | COMPILE ERROR: use `.take_all()` |
| `Map<string, V>` key iteration | COMPILE ERROR: use `.iter()` or `.take_all()` |
| Out-of-bounds index | PANIC |
| Invalid handle | PANIC (generation mismatch) |
| `deliver value` for !Copy | Requires `.clone()` |
| Infinite range (`0..`) | Works (lazy) |
| Zero-sized types (`Vec<()>`) | Yields indices 0..len |

**ZST Iteration:** Allowed for generic code uniformity. ZST collections iterate indices, not values.
