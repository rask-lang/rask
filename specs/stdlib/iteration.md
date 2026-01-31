# Collection Iteration Patterns

This spec covers how to iterate over standard library collections (Vec, Pool, Map).

For loop syntax and desugaring, see [control/loops.md](../control/loops.md).
For the Iterator trait, see [types/iterator-protocol.md](../types/iterator-protocol.md).

---

## Iteration Modes

Collections support multiple iteration modes depending on access needs:

| Collection | Index/Handle Mode | Ref Mode | Consume Mode |
|------------|-------------------|----------|--------------|
| `Vec<T>` | `for i in vec` → `usize` | N/A | `for item in vec.consume()` → `T` |
| `Pool<T>` | `for h in pool` → `Handle<T>` | `for (h, item) in &pool` → `(Handle<T>, &T)` | `for item in pool.consume()` → `T` |
| `Map<K,V>` | `for k in map` → `K` (K: Copy) | `for (k, v) in &map` → `(&K, &V)` | `for (k,v) in map.consume()` → `(K, V)` |

**When to use each mode:**

| Mode | Use When |
|------|----------|
| Index/Handle | Need to mutate or remove during iteration |
| Ref | Read-only access, avoid cloning large items |
| Consume | Consuming all items, transferring ownership |

---

## Value Access

Access follows expression-scoped collection rules:

| Expression | Behavior | Constraint |
|------------|----------|------------|
| `vec[i]` where T: Copy (≤16 bytes) | Copies out T | T: Copy |
| `vec[i].field` where field: Copy | Copies out field | field: Copy |
| `vec[i].method()` | Borrows for call, releases at `;` | Expression-scoped |
| `&vec[i]` passed to function | Borrows for call duration | Cannot store in callee |
| `vec[i] = value` | Mutates in place | - |
| `vec[i]` where T: !Copy | **ERROR**: cannot move | Use `.clone()` or `.consume()` |

**Rule:** Each `collection[index]` access is independent. Borrow released at statement end (semicolon).

---

## Ref Mode

Ref mode (`for (h, item) in &collection`) provides ergonomic read-only access.

**Enforcement:** The compiler forbids all mutation operations within ref loop blocks:

| Operation | In Ref Mode Loop | Error |
|-----------|------------------|-------|
| `pool.remove(h)` | Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool.insert(item)` | Forbidden | "cannot mutate `pool` during ref iteration" |
| `pool[h].field = x` | Allowed | Mutates item in place, doesn't invalidate iteration |

**Function calls:**

| Parameter Mode | In Ref Loop | Reason |
|----------------|-------------|--------|
| `read` | Allowed | Cannot mutate by definition |
| `mutate` | Forbidden | Would allow mutation |
| `transfer` | Forbidden | Ownership transfer impossible |

---

## Consume Iteration

**Syntax:** `collection.consume()`

Yields owned values, consuming the collection:

```
for item in vec.consume() {
    process(item);  // item is owned T
}
// vec is now empty
```

| Collection | Method | Yields | Returns |
|------------|--------|--------|---------|
| `Vec<T>` | `.consume()` | `T` | `VecConsume<T>` |
| `Pool<T>` | `.consume()` | `T` | `PoolConsume<T>` |
| `Map<K,V>` | `.consume()` | `(K, V)` | `MapConsume<K,V>` |

**Key Properties:**
1. `.consume()` takes ownership of collection (`self`, not `&mut self`)
2. Collection's internal buffer transferred to consume iterator
3. Original collection left in valid empty state
4. When consume iterator drops, remaining items dropped in LIFO order

**Early Exit:**

```
for file in files.consume() {
    if file.is_locked() {
        break;  // Remaining files DROPPED (LIFO order)
    }
    file.close()?;
}
```

---

## Mutation During Iteration

**Allowed but programmer responsibility** (index/handle mode only):

| Pattern | Safety | Notes |
|---------|--------|-------|
| `for i in vec { vec[i].field = x }` | Safe | In-place mutation doesn't invalidate index |
| `for i in vec { vec.push(x)? }` | Unsafe | New elements not visited; length captured at start |
| `for i in vec { vec.swap_remove(i) }` | Unsafe | Later indices refer to wrong elements |

**Safe Patterns:**

1. **Reverse iteration for removal:**
   ```
   for i in (0..vec.len()).rev() {
       if vec[i].expired { vec.swap_remove(i); }
   }
   ```

2. **Collect indices, then mutate:**
   ```
   let to_remove = Vec::new();
   for i in vec { if vec[i].expired { to_remove.push(i); } }
   for i in to_remove.rev() { vec.swap_remove(i); }
   ```

3. **Filter via consume:**
   ```
   let vec = vec.consume().filter(|item| !item.expired).collect();
   ```

---

## Error Propagation (`?`)

When `?` exits a loop:

| Loop Type | Original Collection | Remaining Items |
|-----------|---------------------|-----------------|
| Index mode | Intact | N/A |
| Handle mode | Intact | N/A |
| Ref mode | Intact | N/A |
| Consume mode | Already consumed | Dropped (LIFO) |

**Consume + ensure:**
```
for file in files.consume() {
    ensure file.close();   // Runs if ? exits
    file.write(data)?;
}
```

---

## Linear Types

**Index iteration forbidden for `Vec<Linear>`:**

```
// COMPILE ERROR:
for i in files { files[i].close()?; }

// Required:
for file in files.consume() { file.close()?; }
```

**Pool iteration works** (handles are Copy):
```
for h in pool {
    pool.remove(h)?.close()?;
}
```

---

## Map Iteration

**Key mode requires Copy keys:**
```
// OK: u64 is Copy
for id in counts { print(counts[id]); }

// ERROR: string is not Copy
for key in config { ... }  // Use &config or .consume()
```

**Ref mode for all key types:**
```
for (key, value) in &config {
    print(key, value);
}
```

---

## Edge Cases

| Case | Handling |
|------|----------|
| Empty collection | Loop body never executes |
| `Vec<Linear>` index iteration | COMPILE ERROR: use `.consume()` |
| `Map<String, V>` key iteration | COMPILE ERROR: use ref or consume |
| Out-of-bounds index | PANIC |
| Invalid handle | PANIC (generation mismatch) |
| `break value` for !Copy | Requires `.clone()` |
| Infinite range (`0..`) | Works (lazy) |
| Zero-sized types (`Vec<()>`) | Yields indices 0..len |

**ZST Iteration:** Allowed for generic code uniformity. ZST collections iterate indices, not values.
