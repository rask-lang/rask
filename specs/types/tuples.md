<!-- id: type.tuples -->
<!-- status: decided -->
<!-- summary: Anonymous product types with positional access, destructuring, and structural equality -->
<!-- depends: memory/value-semantics.md, types/structs.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Tuple Types

Anonymous product types. Use when naming fields adds nothing — function returns, table-driven tests, temporary grouping. For anything with meaning, use a struct.

## Tuple Syntax

| Rule | Description |
|------|-------------|
| **TU1: Type syntax** | `(T1, T2, ..., Tn)` denotes a tuple type (arity ≥ 2) |
| **TU2: Literal syntax** | `(v1, v2, ..., vn)` constructs a tuple value (arity ≥ 2) |
| **TU3: No empty or 1-tuples** | `()` is not a tuple — it's call syntax. For the zero-sized type, use `void` (`type.primitives/P6`). `(T)` is a parenthesized expression, not a 1-tuple |

<!-- test: parse -->
```rask
const pair: (i32, string) = (42, "hello")
const nested: ((i32, i32), string) = ((1, 2), "point")
```

## Element Access

| Rule | Description |
|------|-------------|
| **TU5: Positional access** | `.0`, `.1`, `.2`, etc. access tuple elements by position |
| **TU6: Bounds checked** | Accessing beyond tuple length is a compile error |

<!-- test: parse -->
```rask
const point = (10, 20)
const x = point.0
const y = point.1
```

Destructuring also works:

<!-- test: parse -->
```rask
const point = (10, 20)
const (x, y) = point
```

## Value Semantics

| Rule | Description |
|------|-------------|
| **TU7: Copy** | Tuples are Copy when all elements are Copy |
| **TU8: Cloneable** | Tuples are Cloneable when all elements are Cloneable |
| **TU9: Equality** | `==` and `!=` work when all elements support equality |
| **TU10: Layout** | Struct layout rules apply: elements in order, padded for alignment (see `comp.mem-layout`) |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `()` in type position | TU3 | Error: use `void` for the zero-sized type |
| Single without comma `(x)` | TU3 | Parenthesized expression, not a tuple |
| Named field access on tuple `.name` | TU5 | Error: tuples use positional access |
| Index out of range `.3` on 2-tuple | TU6 | Compile error |

## Error Messages

**Positional access out of bounds [TU6]:**
```
ERROR [type.tuples/TU6]: tuple index out of bounds
   |
3  |  const z = pair.2
   |                 ^ index 2 out of range for (i32, i32) (length 2)

FIX: Valid indices are .0 and .1.
```

---

## Appendix (non-normative)

### Rationale

**TU1–TU3 (syntax):** Parenthesized, comma-separated — same convention as Python, Rust, Swift. Tuples are arity ≥ 2 only: 0-tuples and 1-tuples are degenerate and confuse `()` (call syntax) and `(x)` (grouping). The zero-sized type is `void`, a keyword (`type.primitives/P6`), not an empty tuple — I don't need the ML-style empty-product unification to cover a case users never write.

**TU5 (positional access):** `.0`, `.1` instead of named fields. If you need names, use a struct — `type.structs/S1` requires named fields precisely because tuples fill the anonymous gap. Two tools, clear boundary.

**TU7 (Copy):** Follows from `mem.value-semantics` — tuples under 16 bytes with Copy elements are automatically Copy. Structural, no annotation needed.

### Patterns & Guidance

#### When to use tuples vs structs

| Use case | Choice | Why |
|----------|--------|-----|
| Function returning two values | Tuple | Short-lived, callers destructure immediately |
| Table-driven test data | Tuple | `for (input, expected) in [...]` is clean |
| Map iteration | Tuple | `for (key, value) in map` |
| Anything stored in a struct field | Struct | Named fields document intent |
| More than 3 elements | Struct | Positional access gets confusing |

#### Table-driven tests (see `std.testing/T5`)

<!-- test: parse -->
```rask
test "add cases" {
    for (a, b, expected) in [(1, 2, 3), (0, 0, 0), (-1, 1, 0)] {
        check a + b == expected
    }
}
```

### See Also

- `type.structs/S1` — structs require named fields (tuples are the anonymous counterpart)
- `comp.mem-layout` — tuple memory layout details
- `std.testing/T5` — tuple iteration in tests
- `ctrl.flow` — destructuring syntax for tuples
