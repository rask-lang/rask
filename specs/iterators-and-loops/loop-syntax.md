# Loop Syntax and Borrowing Semantics

See also: [README.md](README.md)

## The Question
How can collections be iterated when borrows cannot be stored, without lifetime parameters, while maintaining ergonomics comparable to Go?

## Decision
Loops yield indices/handles (never borrowed values). Access uses existing collection borrow rules. Value extraction follows the 16-byte Copy threshold. Ownership transfer uses explicit `consume()`.

## Rationale
Index-based iteration eliminates the need for stored references while preserving Rask's core constraints: no lifetime annotations, local analysis only, transparent costs. The existing expression-scoped collection access rules extend naturally to loop bodies without new concepts. Copy extraction for small types (≤16 bytes) matches assignment semantics, while explicit `clone()` makes large copies visible.

## Loop Syntax

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

## Loop Borrowing Semantics

**Core Rule:** `for i in collection` does NOT borrow the collection. The loop variable receives a Copy value (index or handle), and the collection remains accessible within the loop body.

| Loop Syntax | Ownership Transfer | Collection Access Inside Loop |
|-------------|-------------------|------------------------------|
| `for i in vec` | **NO** | ✅ Allowed: `vec[i]`, `vec.push()`, etc. |
| `for h in pool` | **NO** | ✅ Allowed: `pool[h]`, `pool.remove()`, etc. |
| `for k in map` | **NO** | ✅ Allowed: `map[k]`, `map.insert()`, etc. |
| `for item in vec.consume()` | **YES** | ❌ Forbidden: consume consumed vec |

**Desugaring:**
```
// Index iteration (Vec, Pool, Map):
for i in vec { body }
// Equivalent to:
{
    let _len = vec.len();
    let _pos = 0;
    while _pos < _len {
        let i = _pos;
        body
        _pos += 1;
    }
}

// Consume iteration (consuming):
for item in vec.consume() { body }
// Equivalent to:
{
    let mut _consumer = vec.consume();  // Takes ownership, vec now empty
    while let Some(item) = _consumer.next() {
        body
    }
    // _consumer drops here, dropping any remaining items
}
```

**Why no borrow?**
- Indices are Copy values, not references
- Each `vec[i]` access is independent (expression-scoped)
- Enables mutation patterns: `for i in vec { vec[i].field = x }`
- Local analysis only — no loop-level borrow tracking
- Same semantics as Go, C, Zig

**Implication:** Collection length captured at loop start. Mutations during iteration may invalidate indices (programmer responsibility).

---

## See Also
- [Collection Iteration](collection-iteration.md) - Iteration modes for Vec, Pool, Map
- [Iterator Protocol](iterator-protocol.md) - Iterator trait and adapter details
- [Mutation and Errors](mutation-and-errors.md) - Mutation during iteration, error handling
