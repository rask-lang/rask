# Elaboration: Collection Borrowing During Iteration

## Specification

### Loop Syntax and Borrowing Semantics

**Core Rule:** `for i in collection` does NOT borrow the collection. The loop variable receives a Copy value (index or handle), and the collection remains accessible within the loop body.

| Loop Syntax | Borrow Created | Collection Access Inside Loop |
|-------------|----------------|------------------------------|
| `for i in vec` | **NO** | ✅ Allowed: `vec[i]`, `vec.push()`, etc. |
| `for h in pool` | **NO** | ✅ Allowed: `pool[h]`, `pool.remove()`, etc. |
| `for k in map` | **NO** | ✅ Allowed: `map[k]`, `map.insert()`, etc. |
| `for item in vec.drain()` | **YES** | ❌ Forbidden: drain borrows mutably |

### Desugaring

**Index-based iteration** (Vec, Pool, Map with Copy keys):
```
// Written:
for i in vec { body }

// Means:
{
    let _iter_len = vec.len();
    let _iter_pos = 0;
    while _iter_pos < _iter_len {
        let i = _iter_pos;
        body
        _iter_pos += 1;
    }
}
```

**Key points:**
- Collection length captured at loop start
- No borrow of collection created
- Collection can be accessed and even mutated inside loop
- Mutations during iteration are programmer responsibility (may invalidate indices)

**Drain iteration** (consuming):
```
// Written:
for item in vec.drain() { body }

// Means:
{
    let mut _drainer = vec._into_drain();  // Mutable borrow
    while let Some(item) = _drainer._next() {
        body
    }
    // _drainer drops, vec is now empty
}
```

**Key points:**
- `.drain()` creates mutable borrow of collection
- Collection CANNOT be accessed inside drain loop
- Drain consumes elements, leaving collection empty

### Why No Borrow for Index Iteration?

**Design rationale:**
1. **Indices are values (Copy), not references** — No lifetime to track
2. **Expression-scoped access** — Each `vec[i]` is independent, borrow released at `;`
3. **Allows mutation** — Common pattern: `for i in vec { vec[i].field = x }`
4. **Local analysis** — No need to track loop-level borrow state
5. **Simplicity** — Loop is just syntactic sugar over range iteration

**Tradeoff:** Programmer must ensure mutations don't invalidate indices (same as Go, C, Zig).

### Mutation During Iteration

| Pattern | Safety | Notes |
|---------|--------|-------|
| `for i in vec { vec[i].field = x }` | ✅ Safe | In-place mutation doesn't invalidate index |
| `for i in vec { vec.push(x)? }` | ⚠️ Unsafe | New elements not visited; original length captured |
| `for i in vec { vec.swap_remove(i) }` | ⚠️ Unsafe | Later indices refer to wrong elements |
| `for i in vec { vec.clear() }` | ⚠️ Unsafe | All subsequent accesses panic (out of bounds) |

**Compiler MUST NOT error on these patterns.** Runtime behavior:
- Out-of-bounds access → panic
- Wrong element accessed → silent logic bug
- This is programmer responsibility (same as manual index loops in C/Go/Zig)

**Safe patterns for removal:**
```
// Pattern 1: Reverse iteration
for i in (0..vec.len()).rev() {
    if vec[i].expired {
        vec.swap_remove(i);  // Safe: doesn't affect earlier indices
    }
}

// Pattern 2: Drain + filter
let vec = vec.drain().filter(|x| !x.expired).collect();

// Pattern 3: retain
vec.retain(|x| !x.expired);
```

### Collection Access Rules Inside Loops

**General principle:** Same as outside loops — expression-scoped borrows, independent accesses.

```
for i in vec {
    let val = vec[i];         // Expression borrow, released at ;
    process(val);              // val is Copy of vec[i]

    vec[i].field += 1;        // Expression borrow, released at ;

    if vec[i].expired {       // New expression borrow
        vec.push(default)?;    // ✅ OK: no active borrow
    }
}
```

**Forbidden:** Only drain/consuming iteration prevents collection access.
```
for item in vec.drain() {
    vec.push(x)?;  // ❌ ERROR: vec is mutably borrowed by drain
}
```

### Closure Access During Iteration

**Question:** Can closures capture the collection being iterated?

**Answer:** Yes, with expression-scoped semantics.

```
for i in vec {
    vec.read(i, |item| {      // Closure borrows vec for call duration
        process(item);
    })?;
    // Borrow released after closure call
}
```

**Storing closures:** Closures can only be stored if they don't capture expression-scoped borrows.
```
for i in vec {
    let f = || vec[i];        // ❌ ERROR: vec[i] is expression-scoped

    let idx = i;              // Copy index
    let f = || idx;           // ✅ OK: captures Copy index, not borrow
}
```

### Range Iteration

**Ranges are NOT collections** — they generate values without borrowing anything.

```
for i in 0..10 {
    // No collection involved, i is just an integer
}

for i in 0..vec.len() {
    // vec NOT borrowed by range
    vec[i].process();  // ✅ OK
}
```

### Integration with Parameter Modes

**Passing collection to function:**

```
fn process_items(vec: Vec<Item>) {
    for i in vec {
        vec[i].process();  // ✅ OK: vec is owned parameter
    }
}

fn process_items(read vec: Vec<Item>) {
    for i in vec {
        vec[i].process();  // ✅ OK: read mode allows borrows
    }
}

fn process_items(mutate vec: Vec<Item>) {
    for i in vec {
        vec[i].field += 1;  // ✅ OK: mutate mode allows mutable borrows
    }
}
```

**All modes work** because iteration doesn't create a conflicting borrow.

## Self-Validation

### Does it conflict with CORE design?
**NO.**
- Aligns with "no storable references" (indices are values, not references)
- Aligns with "expression-scoped collection borrows" (each access independent)
- Aligns with "local analysis only" (no loop-level borrow tracking needed)
- Aligns with "transparent costs" (mutation dangers are visible)

### Is it internally consistent?
**YES.**
- Index iteration → no borrow → collection accessible
- Drain iteration → mutable borrow → collection inaccessible
- Clear distinction, easy to understand

### Does it conflict with other specs?
**NO.**
- `memory-model.md`: Expression-scoped collection borrows ✅
- `dynamic-data-structures.md`: Collection access methods ✅
- `ensure-cleanup.md`: No conflict ✅

### Is it complete enough to implement?
**YES.**
- Clear desugaring rules
- Clear borrowing semantics
- Clear mutation behavior
- Clear error cases

### Is it concise?
**YES.** ~150 lines, tables for edge cases, one example set.

## Summary

**Key insight:** Index-based iteration creates NO borrow. Collection remains fully accessible using expression-scoped access rules.

**Rationale:** Indices are Copy values. Each collection access is independent and expression-scoped. This enables natural mutation patterns while maintaining safety through runtime bounds checks.

**Tradeoff:** Mutations during iteration are allowed (programmer responsibility). Same as Go, C, Zig — acceptable for systems language.
