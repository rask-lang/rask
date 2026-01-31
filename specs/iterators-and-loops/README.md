# Iterators and Loops

**Status:** ✅ Complete — All gaps resolved

## Overview

Rask's iteration system provides safe, ergonomic loops without lifetime parameters. The design eliminates iterator invalidation bugs while maintaining performance and simplicity comparable to Go.

**Core Innovation:** Loops yield indices/handles (Copy values), not borrowed references. Collection access uses expression-scoped borrows. This enables mutation during iteration while preventing stored reference bugs.

## Quick Reference

| Pattern | Syntax | Use When |
|---------|--------|----------|
| Index iteration | `for i in vec { vec[i] }` | Need mutation/removal access |
| Handle iteration | `for h in pool { pool[h] }` | Pool operations |
| Ref iteration | `for (h, x) in &pool { ... }` | Read-only, avoid cloning |
| Consume iteration | `for item in vec.consume() { ... }` | Transfer ownership |
| Map iteration (Copy keys) | `for k in map { map[k] }` | Copy keys only |
| Map ref iteration | `for (k, v) in &map { ... }` | All key types |
| Range | `for i in 0..n { ... }` | Counting, indexing |

## Specification Documents

### Core Concepts

1. **[Loop Syntax and Semantics](loop-syntax.md)** (~200 lines)
   - Basic `for-in` syntax
   - Loop borrowing rules (collection NOT borrowed)
   - Desugaring and length capture
   - Why index-based iteration works

2. **[Collection Iteration](collection-iteration.md)** (~350 lines)
   - Vec, Pool, Map iteration modes
   - Value access rules (Copy threshold)
   - Common patterns and examples
   - Map iteration ergonomics (ED: 0.94)

3. **[Consume and Linear Types](consume-and-linear.md)** (~280 lines)
   - Consuming iteration (ownership transfer)
   - Implementation without stored references
   - Linear type handling
   - Early exit and drop semantics

### Advanced Topics

4. **[Iterator Protocol](iterator-protocol.md)** (~450 lines)
   - `Iterator<Item>` trait
   - Adapter type system (filter, map, take, etc.)
   - For-in desugaring rules
   - Custom iterator implementation
   - Compiler requirements

5. **[Mutation and Error Handling](mutation-and-errors.md)** (~350 lines)
   - Mutation during iteration (programmer responsibility)
   - Length capture and bounds checking
   - Safe mutation patterns
   - Error propagation (`?`) semantics
   - `ensure` cleanup integration

6. **[Range Iteration](ranges.md)** (~200 lines)
   - Range types (Range, RangeInclusive, RangeFrom)
   - Infinite ranges
   - Reverse iteration with `.rev()`
   - Overflow behavior
   - Type inference

7. **[Edge Cases](edge-cases.md)** (~150 lines)
   - Zero-sized type iteration rationale
   - Linear type iteration constraints
   - Empty collections
   - Break/continue semantics

## Design Metrics

| Metric | Target | Actual | Status |
|--------|--------|--------|--------|
| **Ergonomic Density (ED)** | ≥0.83 | **0.94** | ✅ Pass |
| **Transparency of Cost (TC)** | ≥0.90 | All costs visible | ✅ Pass |
| **Mechanical Safety (MC)** | ≥0.90 | No iterator invalidation | ✅ Pass |
| **Practical Coverage (UCC)** | ≥0.80 | All common patterns | ✅ Pass |

## Key Design Decisions

1. **No Lifetime Parameters:** Loops yield Copy values (indices/handles), not references
2. **Expression-Scoped Access:** Each `collection[i]` access is independent
3. **Explicit Consume:** Ownership transfer requires `.consume()` method
4. **Local Analysis Only:** No cross-function borrow tracking needed
5. **Mutation Allowed:** Programmer responsibility (like C, Go, Zig)

## Example: Common Patterns

```rask
// Read-only iteration (no cloning)
for i in users {
    print(&users[i].name);  // Borrows for call, releases at ;
}

// In-place mutation
for i in users {
    users[i].login_count += 1;
}

// Filter and collect
let admins = Vec::new();
for i in users {
    if users[i].is_admin {
        admins.push(users[i].clone());
    }
}

// Consume (transfer ownership)
for file in files.consume() {
    file.close()?;
}
```

## Comparison with Other Languages

**vs. Rust:**
- ✅ No lifetime annotations needed
- ✅ Simpler mental model (indices, not borrows)
- ✅ Mutation during iteration allowed
- ⚠️ Explicit `.clone()` for value extraction

**vs. Go:**
- ✅ Identical ergonomics (ED: 0.94)
- ✅ No hidden allocations
- ✅ Iterator invalidation prevented (Go allows unsafe mutation)

**vs. C/Zig:**
- ✅ Same mutation flexibility
- ✅ Type safety (Copy threshold, linear types)
- ✅ No manual memory management

## Implementation Status

All aspects specified and ready for implementation:
- ✅ Loop semantics
- ✅ Collection iteration protocols
- ✅ Iterator trait and adapters
- ✅ For-in desugaring rules
- ✅ Error handling integration
- ✅ Edge cases documented
- ✅ Performance guarantees defined

**Total specification:** ~1950 lines across 8 documents

## See Also

- [Memory Model](../memory-model.md) — Copy threshold, expression-scoped borrows
- [Error Handling](../ensure.md) — `ensure` cleanup, `?` propagation
- [Linear Types](../memory-model.md#linear-types) — Linear resource handling
