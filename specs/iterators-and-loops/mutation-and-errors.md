# Mutation During Iteration and Error Handling

See also: [README.md](README.md)

## Mutation During Iteration

**Allowed:** Because index iteration does NOT borrow the collection, mutations are permitted but are **programmer responsibility**.

| Pattern | Safety | Notes |
|---------|--------|-------|
| `for i in vec { vec[i].field = x }` | ✅ Safe | In-place mutation doesn't invalidate index |
| `for i in vec { vec.push(x)? }` | ⚠️ Unsafe | New elements not visited; original length captured |
| `for i in vec { vec.swap_remove(i) }` | ⚠️ Unsafe | Later indices refer to wrong elements |
| `for i in vec { vec.clear() }` | ⚠️ Unsafe | All subsequent accesses panic (out of bounds) |

Compiler MUST NOT error on these patterns. Runtime behavior:
- Out-of-bounds access → panic
- Wrong element accessed → silent logic bug
- This is programmer responsibility (same as C, Go, Zig)

### Detailed Runtime Semantics

**Length Capture:**

When a loop begins, the collection length is captured once:

```
for i in vec {  // len = 5 captured here
    vec.push(x)?;  // len now 6, but loop still iterates 0..5
}
```

**Desugaring shows the behavior:**

```
{
    let _len = vec.len();  // Captures 5
    let _pos = 0;
    while _pos < _len {     // Iterates 0..5
        let i = _pos;
        vec.push(x)?;       // Doesn't affect _len
        _pos += 1;
    }
}
```

**Implication:** Elements added during iteration are NOT visited. Elements removed may cause panics.

**Bounds Checking Behavior:**

| Mutation | Loop Length | Access Result |
|----------|-------------|---------------|
| `vec.push(x)` while looping | 5 (unchanged) | Later `vec[4]` succeeds (element exists) |
| `vec.pop()` at i=2 | 5 (unchanged) | Later `vec[4]` **PANICS** (out of bounds, len=4) |
| `vec.swap_remove(i)` at i=2 | 5 (unchanged) | Later `vec[4]` **PANICS** if vec.len ≤ 4 |
| `vec.clear()` at i=2 | 5 (unchanged) | Later `vec[3]` **PANICS** (len=0) |
| `vec[i].field = x` | 5 (unchanged) | All accesses succeed (no length change) |

**Panic Message Requirements:**

Runtime MUST provide actionable panic messages:

```
"index 4 out of bounds for Vec of length 3 (modified during iteration?)"
```

**Safe Mutation Patterns:**

**Pattern 1: Reverse iteration for removal**

```
for i in (0..vec.len()).rev() {
    if vec[i].expired {
        vec.swap_remove(i);  // ✅ Safe: only affects indices > i
    }
}
```

**Why safe:** Reverse iteration (4, 3, 2, 1, 0) means removing at index i doesn't affect indices we haven't visited yet (i-1, i-2, ..., 0).

**Pattern 2: Fallible access**

```
for i in 0..original_len {
    if let Some(item) = vec.get(i) {
        process(item);  // ✅ Safe: handles out-of-bounds gracefully
    }
}
```

**Method signature:**
```
fn Vec<T>.get(&self, index: usize) -> Option<&T>
```

Returns `None` if index ≥ len, never panics.

**Pattern 3: Collect indices, then mutate**

```
let to_remove = Vec::new();
for i in vec {
    if vec[i].expired {
        to_remove.push(i);
    }
}
for i in to_remove.rev() {  // Reverse to avoid invalidation
    vec.swap_remove(i);
}
```

**Why safe:** Two-pass approach separates read from write. Reverse removal prevents invalidation.

**Pattern 4: Filter via consume**

```
let vec = vec.consume()
    .filter(|item| !item.expired)
    .collect();  // Rebuilds vec without expired items
```

**Why safe:** No indexing during mutation. Consume yields owned values, filter creates new collection.

**Pattern 5: Mutation with length check**

```
for i in 0..vec.len() {
    if i >= vec.len() { break; }  // Dynamic length check
    if vec[i].should_duplicate {
        vec.push(vec[i].clone())?;  // Duplicates not visited
    }
}
```

**Why acceptable:** Explicit length check makes intent clear. Duplicates are added at end (not visited in current iteration).

**Unsafe Patterns (Will Panic):**

| Pattern | Problem | Panic Occurs |
|---------|---------|--------------|
| `for i in vec { vec.clear(); vec[i] }` | Access after clear | Iteration i ≥ 1 |
| `for i in vec { vec.pop(); }` | Shrinking while forward iterating | When i ≥ new_len |
| `for i in vec { vec.swap_remove(0); }` | Removing earlier indices | When i ≥ new_len |
| `for i in vec { if i % 2 == 0 { vec.remove(i); } }` | Forward removal | Later iterations |

**Compiler Warnings (Optional):**

Compilers SHOULD warn on obvious mutation-during-iteration patterns:

```
for i in vec {
    vec.push(x);  // Warning: mutation during iteration may cause unexpected behavior
}
```

**Warning level:** Optional — compiler implementation defined. MUST NOT be an error.

**Pool and Map Mutation:**

Pool and Map have different invalidation characteristics:

**Pool:**
```
for h in pool {
    pool.remove(h);  // ⚠️ Invalidates handle h, but loop continues with other handles
}
```

Behavior: `pool.remove(h)` invalidates handle h. Later `pool[h]` panics with generation mismatch.

**Map:**
```
for k in map {
    map.remove(k);  // ⚠️ Removes entry, but loop continues with other keys
}
```

Behavior: `map.remove(k)` removes entry. Later `map[k]` panics with "key not found."

**Recommendation:** Use consume for exhaustive removal:
```
for item in pool.consume() { item.cleanup(); }
for (k, v) in map.consume() { process(k, v); }
```

## Error Handling and `?` Propagation

**Fallible operations use `?`:**

```
for i in lines {
    let parsed = parse(&lines[i])?;  // Exits loop on error
    results.push(parsed);
}
```

**Fallible access:**

```
for i in 0..items.len() {
    if let Some(item) = items.get(i) {
        process(item);  // Safe for potentially invalid indices
    }
}
```

### Error Propagation (`?`) Exit Semantics

**When `?` exits a loop, cleanup happens in this order:**

1. Current iteration's variables drop (normal scope exit)
2. `ensure` blocks registered in current iteration run (LIFO)
3. Loop iterator drops (for consume: remaining items dropped LIFO)
4. Function returns with error

**Index/Handle Iteration + `?`:**

```
for i in vec {
    vec[i].validate()?;  // Error exits loop
    vec[i].process();
}
// On error: loop exits, vec remains intact (was never moved)
```

**Consume Iteration + `?`:**

```
for file in files.consume() {  // files consumed here
    file.write(data)?;          // Error on file 3 of 10
    file.close()?;
}
// On error:
// - Current file (file 3) drops normally
// - Consumer iterator drops
// - Remaining files (4-10) dropped in LIFO order (10, 9, 8, ...)
// - Original files collection already consumed (can't access)
```

**Key Behavior:** Remaining items in consume iterator are DROPPED when `?` exits. They are NOT accessible in error handling code.

**Consume + `ensure` Interaction:**

```
for file in files.consume() {
    ensure file.close();        // Registers cleanup for THIS file
    file.write(data)?;          // Error here
}

// Execution on error (file 3 of 10):
// 1. ensure runs: file.close() called on file 3
// 2. file (variable) drops
// 3. Consumer iterator drops, dropping files 4-10 (LIFO)
// 4. Function returns with error
```

**Ensure Scope:** `ensure` registered inside a loop body is scoped to that iteration. It runs when the iteration completes (normally or via `?`), not when the entire loop ends.

**Multiple Ensures in One Iteration:**

```
for file in files.consume() {
    ensure file.close();           // Cleanup 1
    let lock = acquire_lock()?;
    ensure lock.release();         // Cleanup 2

    file.write(data)?;             // Error here
}

// On error:
// - ensure lock.release() runs
// - ensure file.close() runs  (LIFO order)
// - Remaining files in iterator dropped
```

**Error Value Access:**

Remaining items in a consume iterator are NOT accessible after `?` exits:

```
// This is not possible:
let result = (|| {
    for file in files.consume() {
        file.process()?;
    }
    Ok(())
})();

match result {
    Err(e) => {
        // Cannot access remaining files here
        // They were dropped when ? exited the loop
    }
}
```

**If you need to preserve remaining items on error, don't use consume:**

```
// Option 1: Index iteration (collection not consumed)
for i in files {
    files[i].process()?;  // Error keeps files intact
}
// On error: files still valid, can retry or cleanup selectively

// Option 2: Manual iteration with early return
let mut pos = 0;
for file in files.consume() {
    if let Err(e) = file.process() {
        // Return remaining items with error somehow?
        // This is complex - consume isn't the right tool
    }
    pos += 1;
}
```

**Design Rationale:**

- Consume iterator takes ownership—original collection is gone
- On early exit, remaining items must be dropped (can't leak linear resources)
- LIFO drop order ensures proper cleanup (reverse of allocation order)
- This matches Rust's behavior and is predictable

**Summary Table:**

| Loop Type | `?` Exits | Original Collection | Remaining Items |
|-----------|-----------|---------------------|-----------------|
| Index mode | Yes | ✅ Intact | N/A (not consumed) |
| Handle mode | Yes | ✅ Intact | N/A (not consumed) |
| Ref mode | Yes | ✅ Intact | N/A (not consumed) |
| Consume mode | Yes | ❌ Already consumed | Dropped (LIFO) |

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Collection Iteration](collection-iteration.md) - Iteration modes for Vec, Pool, Map
- [Consume and Linear](consume-and-linear.md) - Consume iteration details
