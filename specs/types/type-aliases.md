<!-- id: type.aliases -->
<!-- status: decided -->
<!-- summary: Transparent type aliases for readability and refactoring -->
<!-- depends: types/primitives.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Type Aliases

`type Name = Existing` creates a transparent alias. The alias and the underlying type are identical — no conversion, no wrapper, no runtime cost.

## Alias Declaration

| Rule | Description |
|------|-------------|
| **TA1: Syntax** | `type Name = ExistingType` at module scope |
| **TA2: Transparent** | Alias and target are the same type everywhere |
| **TA3: Generic aliases** | `type Pair<T> = (T, T)` — type parameters allowed |
| **TA4: Visibility** | `public type Name = ...` exports the alias |
| **TA5: No cycles** | `type A = A` or `type A = B` / `type B = A` is a compile error |
| **TA6: Error messages** | Compiler shows alias name; expanded type on hover/detail |

<!-- test: parse -->
```rask
type UserId = u64
type Pair<T> = (T, T)
type Handler = func(i32) -> string

const id: UserId = 42
const coords: Pair<f64> = (1.0, 2.0)
```

Result alias with `or` syntax: `type Result<T> = T or AppError` — parses when `AppError` is defined.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Alias to alias | TA2 | Chains resolve: `type A = B`, `type B = i32` → A is i32 |
| Alias in generic position | TA3 | `Vec<UserId>` works, same as `Vec<u64>` |
| Cyclic alias | TA5 | Compile error with cycle path |
| Shadowing builtin | — | `type string = i32` — error: cannot shadow builtin type |

## Error Messages

**Cyclic alias [TA5]:**
```
ERROR [type.aliases/TA5]: cyclic type alias
   |
1  |  type A = B
   |       ^ alias A
2  |  type B = A
   |           ^ resolves back to A

FIX: Break the cycle by using the underlying type directly.
```

---

## Appendix (non-normative)

### Rationale

**TA2 (transparent):** I chose transparent over nominal (newtype) because aliases are about readability, not type safety. `UserId` and `u64` should mix freely — the alias says "this u64 means a user ID" without adding conversion boilerplate. If you want compile-time enforcement, use `newtype` (see `type.distinct`).

**TA3 (generic):** Generic aliases reduce repetition. `type NodeMap<V> = Map<NodeId, V>` is clearer than writing the full type everywhere. Parameterized aliases resolve at use site — no runtime impact.

**TA5 (no cycles):** Cycles would require infinite expansion. Detected at alias registration time, not lazily.

### Patterns & Guidance

#### Domain types

<!-- test: parse -->
```rask
type UserId = u64
type Email = string
type Timestamp = i64

func find_user(id: UserId) -> User? {
    return db.get(id)
}
```

These don't prevent misuse (`find_user(42)` still works), but they document intent and make refactoring safer — change the alias and `rask check` shows every site.

#### Shortening generic types

<!-- test: parse -->
```rask
type Matrix = Vec<Vec<f64>>
type Callback<T> = func(T) -> bool
```

### See Also

- `type.distinct` — for nominal wrappers with compile-time enforcement (`newtype`)
- `type.structs` — for nominal types (distinct types with named fields)
- `type.generics` — generic type parameters
