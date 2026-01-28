# Solution: Iterators and Loops

## The Question
How can collections be iterated when borrows cannot be stored, without lifetime parameters, while maintaining ergonomics comparable to Go?

## Decision
Loops yield indices/handles (never borrowed values). Access uses existing collection borrow rules. Value extraction follows the 16-byte Copy threshold. Ownership transfer uses explicit `drain()`.

## Rationale
Index-based iteration eliminates the need for stored references while preserving Rask's core constraints: no lifetime annotations, local analysis only, transparent costs. The existing expression-scoped collection access rules extend naturally to loop bodies without new concepts. Copy extraction for small types (≤16 bytes) matches assignment semantics, while explicit `clone()` makes large copies visible.

## Specification

### Loop Syntax

```
for <binding> in <collection> { ... }
```

| Collection Type | Binding Type | Semantics |
|----------------|--------------|-----------|
| `Vec<T>` | `usize` | Index into vec |
| `Pool<T>` | `Handle<T>` | Generational handle |
| `Map<K,V>` | `K` (requires K: Copy) | Key (copied) |
| `Range` (`0..n`) | Integer | Range value |

**No mode annotations.** Loop variables are always owned indices/handles (Copy types).

### Value Access

Access follows existing expression-scoped collection rules:

| Expression | Behavior | Constraint |
|------------|----------|------------|
| `vec[i]` where T: Copy (≤16 bytes) | Copies out T | T: Copy |
| `vec[i].field` where field: Copy | Copies out field | field: Copy |
| `vec[i].method()` | Borrows for call, releases at `;` | Expression-scoped |
| `&vec[i]` passed to function | Borrows for call duration | Cannot store in callee |
| `vec[i] = value` | Mutates in place | - |
| `vec[i]` where T: !Copy | **ERROR**: cannot move | Use `.clone()` or `.drain()` |

**Rule:** Each `collection[index]` access is independent. Borrow released at statement end (semicolon).

### Examples: Common Patterns

**Copy types (≤16 bytes):**
```
for i in ids {
    let id = ids[i];  // i32 copies implicitly
    process(id);
}
```

**Non-Copy types (require clone):**
```
for i in users {
    if users[i].is_admin {
        return users[i].clone();  // Explicit clone
    }
}
```

**Read-only processing (no clone needed):**
```
for i in documents {
    print(&documents[i].title);  // Borrows for call
}
```

**In-place mutation:**
```
for i in users {
    users[i].login_count += 1;  // Mutate directly
}
```

**Nested loops:**
```
for i in vec {
    for j in vec {
        compare(&vec[i], &vec[j]);  // Both borrows valid
    }
}
```

**Closures (capture index or cloned value):**
```
for i in events {
    let event_id = events[i].id;  // Copy field
    handlers.push(|| handle(event_id));  // Captures id
}

// Or clone for non-Copy:
for i in events {
    let event = events[i].clone();
    handlers.push(|| handle(event));  // Captures clone
}
```

### Drain: Consuming Iteration

**Syntax:** `collection.drain()`

Yields owned values, consuming the collection:

```
for item in vec.drain() {
    consume(item);  // item is owned T
}
// vec is now empty
```

| Collection | Method | Yields |
|------------|--------|--------|
| `Vec<T>` | `.drain()` | `T` |
| `Pool<T>` | `.drain()` | `T` |
| `Map<K,V>` | `.drain()` | `(K, V)` |

**Early exit:**
```
for file in files.drain() {
    if file.is_locked() {
        break;  // Remaining files DROPPED
    }
    file.close()?;
}
```

Compiler MUST drop remaining items in LIFO order. IDE SHOULD warn: `break /* drops N remaining items */`.

### Linear Types

**Index iteration is forbidden for `Vec<Linear>`:**

```
// COMPILE ERROR:
for i in files {
    files[i].close()?;  // ERROR: cannot move out of index
}

// Required:
for file in files.drain() {
    file.close()?;
}
```

**Error message:** "cannot iterate `Vec<{Linear}>` by index; use `.drain()` to consume"

**Pool iteration works** (handles are Copy):
```
for h in pool {
    pool.remove(h)?.close()?;  // Explicit remove + consume
}
```

### Map Iteration

**Keys MUST be Copy:**

```
// OK:
let counts: Map<u64, u32> = ...;
for id in counts {
    print(counts[id]);
}

// ERROR:
let config: Map<string, string> = ...;
for key in config {  // ERROR: string is not Copy
    print(config[key]);
}

// Required for non-Copy keys:
for (key, value) in config.drain() {
    print(key, value);
}
```

**Alternative:** Explicit clone iterator (library-provided):
```
for key in config.keys_cloned() {
    print(config[key.clone()]);  // Clone twice: once for iter, once for access
}
```

### Mutation During Iteration

**Allowed:** In-place mutation and removal are permitted but **programmer responsibility**.

```
// Safe mutation:
for i in vec {
    vec[i].count += 1;  // OK
}

// Unsafe (semantic bug, not compile error):
for i in vec {
    if vec[i].expired {
        vec.swap_remove(i);  // Invalidates subsequent indices
    }
}
```

Compiler MUST NOT error. Runtime behavior: later indices may be invalid or refer to wrong elements.

**Safe removal pattern:**
```
// Reverse iteration avoids invalidation:
for i in (0..vec.len()).rev() {
    if vec[i].expired {
        vec.swap_remove(i);  // Safe: doesn't affect earlier indices
    }
}
```

Or drain + filter:
```
let vec = vec.drain().filter(|item| !item.expired).collect();
```

### Iterator Adapters

Adapters operate on index/handle streams:

```
for i in vec.indices().filter(|i| vec[*i].active).take(10) {
    process(&vec[i]);
}
```

**Adapter semantics:**
- `filter(pred)` — yields indices where `pred(&index)` returns true
- `take(n)` — yields first n indices
- `skip(n)` — skips first n indices  
- `rev()` — reverses index order

**Closure signature:** `|&Index| -> bool`

Closure receives borrowed index, accesses collection from outer scope. This is **expression-scoped capture**: closure is fully evaluated before next iteration, collection not mutated during filter evaluation.

**Lazy evaluation:** Adapters evaluate on-demand. `take(10)` stops iteration after 10 matches.

### Edge Cases

| Case | Handling |
|------|----------|
| Empty collection | Loop body never executes |
| `Vec<Linear>` index iteration | COMPILE ERROR: use `.drain()` |
| `Map<String, V>` iteration | COMPILE ERROR: keys must be Copy; use `.drain()` |
| Out-of-bounds index | PANIC (same as outside loop) |
| Invalid handle | PANIC (generation mismatch) |
| `break value` for !Copy | Requires `.clone()`: `break vec[i].clone()` |
| Mutation during iteration | ALLOWED (programmer responsibility) |
| Drain + early exit | Drops remaining items (LIFO) |
| Infinite range (`0..`) | Works (lazy, never terminates unless broken) |
| Zero-sized types (`Vec<()>`) | Yields indices 0..len despite no data |

### Error Handling

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

## Integration Notes

- **Copy threshold:** Types ≤16 bytes are Copy-eligible. Loop extraction follows same rules as assignment.
- **Expression-scoped borrows:** `collection[i]` borrow ends at semicolon, consistent with existing collection access.
- **Linear tracking:** Drain satisfies linear consumption requirements. Index access forbidden for linear collections.
- **Pattern matching:** No mode inference in loops (unlike `match`). Loops always yield indices; usage determines clone necessity.
- **Closures:** Can capture indices (Copy) or cloned values. Cannot capture expression-scoped borrows.
- **Ensure cleanup:** Works with drain—`ensure` fires before drop of remaining items.
- **Concurrency:** Index-based iteration is thread-safe if collection is not mutated. Shared access requires synchronization (separate feature).

## Examples

**Find and return:**
```
fn find_admin(users: Vec<User>) -> Option<User> {
    for i in users {
        if users[i].is_admin {
            return Some(users[i].clone());
        }
    }
    None
}
```

**Collect filtered:**
```
let admins = Vec::new();
for i in users {
    if users[i].is_admin {
        admins.push(users[i].clone());
    }
}
```

**Linear resource cleanup:**
```
for file in files.drain() {
    file.close()?;
}
```

**Lazy filter + early exit:**
```
for i in records.indices().filter(|i| records[*i].matches(query)).take(10) {
    results.push(records[i].clone());
}
```