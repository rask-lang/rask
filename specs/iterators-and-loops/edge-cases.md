# Edge Cases

See also: [README.md](README.md)

## Edge Cases Summary

| Case | Handling |
|------|----------|
| Empty collection | Loop body never executes |
| `Vec<Linear>` index iteration | COMPILE ERROR: use `.consume()` |
| `Map<String, V>` iteration | COMPILE ERROR: keys must be Copy; use `.consume()` |
| Out-of-bounds index | PANIC (same as outside loop) |
| Invalid handle | PANIC (generation mismatch) |
| `break value` for !Copy | Requires `.clone()`: `break vec[i].clone()` |
| Mutation during iteration | ALLOWED (programmer responsibility) |
| Consume + early exit | Drops remaining items (LIFO) |
| Infinite range (`0..`) | Works (lazy, never terminates unless broken) |
| Zero-sized types (`Vec<()>`) | Yields indices 0..len despite no data |

## Zero-Sized Type Iteration

**Question:** Why allow iteration over `Vec<()>` when there's no actual data to access?

**Answer:** Zero-sized types (ZSTs) like `()`, empty structs, and zero-field enums are valid types in Rask. Iteration over ZST collections is allowed for three reasons:

**1. Counting and Cardinality**

ZST collections represent counts or cardinalities without data:

```
// Representing "5 events occurred" without storing event data
let events: Vec<()> = Vec::new();
for _ in 0..5 {
    events.push(())?;
}

// Iterate count:
for i in events {
    print("Event #", i);  // Prints Event #0, Event #1, ..., Event #4
}
```

**Use case:** Lightweight counters, semaphores, occurrence tracking.

**2. Generic Code Uniformity**

Generic code over `Vec<T>` should work regardless of whether T is zero-sized:

```
fn process_batch<T>(items: Vec<T>, f: |usize, &T| -> ()) {
    for i in items {
        f(i, &items[i]);  // Works for T=() just like T=i32
    }
}

// Works with ZST:
let signals: Vec<()> = ...;
process_batch(signals, |i, _| print("Signal ", i));

// Works with regular types:
let users: Vec<User> = ...;
process_batch(users, |i, u| print("User ", i, u.name));
```

**Benefit:** No special-casing needed. Generic algorithms work uniformly.

**3. Iterator Adapter Composition**

ZST iteration enables adapter patterns without data:

```
// Generate 100 indices without storing data:
let indices = Vec::<()>::with_capacity(100);
for _ in 0..100 { indices.push(()); }

for i in indices.indices().filter(|i| *i % 2 == 0) {
    process_even_index(i);
}
```

**Alternative (clearer):** Use ranges instead: `for i in (0..100).filter(|i| *i % 2 == 0)`

**Implementation:**

ZST collections have special handling:

| Aspect | ZST Behavior | Regular Type Behavior |
|--------|-------------|----------------------|
| Storage | No heap allocation (len tracked, no buffer) | Heap buffer allocated |
| `push(())` | Increments len only | Writes to buffer, may realloc |
| `vec[i]` | Returns `()` (no read) | Reads from buffer |
| Memory | O(1) space (just len counter) | O(n) space |

**Desugaring:**

```
// ZST iteration:
for i in zst_vec {  // where zst_vec: Vec<()>
    body
}

// Desugars identically to regular Vec:
{
    let mut _iter = zst_vec.into_iter();  // Returns RangeIterator(0..len)
    loop {
        let i = match _iter.next() { Some(v) => v, None => break };
        body
    }
}
```

**Key Point:** Iteration yields **indices**, not values. For ZSTs, `zst_vec[i]` yields `()` but the index `i` is meaningful.

**Design Decision:**

**Why not forbid ZST iteration?**

- Would require special-casing generic code
- Adds complexity: "collections are iterable, except ZSTs"
- ZST iteration is cheap (no data movement) and occasionally useful
- Consistent with Rust, Zig (which also allow ZST collections)

**Why not make ZST collections iterate values instead of indices?**

- Inconsistent with non-ZST collections (which iterate indices for Vec)
- Breaks generic code expecting consistent iteration protocol
- `for _ in vec` would work for ZSTs but fail for other types

**Recommendation:** Prefer ranges over ZST collections for counting:

| Pattern | Status | Notes |
|---------|--------|-------|
| `for i in 0..count { ... }` | ✅ Preferred | Clear intent, no allocation |
| `let vec = Vec::<()>::new(); ... for i in vec` | ⚠️ Allowed | Valid but unusual |
| `for _ in 0..count { ... }` | ✅ Preferred | Idiomatic for discarding index |

**Error Handling:**

ZST iteration can fail for the same reasons as regular iteration (e.g., allocation failure during `push`), but accessing elements never fails (always returns `()`).

```
let mut signals = Vec::<()>::new();
for _ in 0..1_000_000 {
    signals.push(())?;  // May fail if len counter overflows (unlikely)
}

for i in signals {
    let _ = signals[i];  // Always succeeds, returns ()
}
```

**Conclusion:** ZST iteration is allowed for simplicity and generic code uniformity. It's rarely used explicitly but enables cleaner generic implementations. Prefer ranges for explicit counting.

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Ranges](ranges.md) - Range iteration details
- [Iterator Protocol](iterator-protocol.md) - Iterator trait and adapter details
