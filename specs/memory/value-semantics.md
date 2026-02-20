<!-- id: mem.value -->
<!-- status: decided -->
<!-- summary: All types are values; ≤16 bytes copies implicitly, larger types move -->
<!-- depends: memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Value Semantics

All types are values with single ownership. Small types (≤16 bytes) copy implicitly; larger types need explicit `.clone()` or move. `@unique` opts out of implicit copy.

## Copy vs Move

| Operation | Small types (≤16 bytes, Copy) | Large types |
|-----------|-------------------------------|-------------|
| Assignment `const y = x` | Copies | Moves (x invalid after) |
| Parameter passing | Copies | Borrows by default, moves with `take` |
| Return | Copies | Moves |

| Rule | Description |
|------|-------------|
| **VS1: Copy eligibility** | Copy if all fields are Copy AND total size ≤16 bytes |
| **VS2: Primitives always Copy** | Primitives are always Copy |
| **VS3: Collections never Copy** | Vec, Pool, Map are never Copy (own heap memory) |
| **VS3.1: Trait objects never Copy** | `any Trait` is never Copy (owns heap data; copying would create two owners) |
| **VS4: Sync types never Copy** | Shared, Mutex, Atomic* are never Copy |
| **VS5: Automatic derivation** | Copy is structural — no `extend Copy` needed |

## The 16-Byte Threshold

| Rule | Description |
|------|-------------|
| **VS6: Fixed threshold** | 16 bytes. Not configurable. Changing it would change program semantics |
| **VS7: Semantic boundary** | Determines copy vs move — platform ABI differences are hidden |

| Size | What happens | Cost |
|------|-------------|------|
| ≤16 bytes | Implicit copy | Negligible (no allocation) |
| >16 bytes | Move (ownership transfer) | Zero |
| `.clone()` | Deep duplicate | Explicit, visible |

**Common type coverage:** `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA{r, g, b, a: u8}`, small enums.

## Unique Types (Opt-Out)

`@unique` forces move-only semantics even if structurally Copy-eligible.

| Rule | Description |
|------|-------------|
| **U1: No implicit copy** | Unique types MUST be explicitly cloned; assignment/passing moves |
| **U2: Clone still available** | `.clone()` works if all fields implement Clone |
| **U3: Size independent** | Works for any size, but most useful for small types |
| **U4: Transitive** | Structs containing unique fields are automatically unique |

<!-- test: parse -->
```rask
@unique
struct UserId {
    id: u64  // 8 bytes, Copy-eligible, but forced move-only
}

@unique
enum Token {
    Access(u64),
    Refresh(u64),
}
```

<!-- test: skip -->
```rask
@unique
struct UserId { id: u64 }

const user1 = UserId{id: 42}
const user2 = user1              // Moves, user1 invalid
const user3 = user2.clone()      // OK: explicit clone
const user4 = user3              // Moves, user3 invalid
```

| Use Case | Rationale |
|----------|-----------|
| Unique identifiers | Duplication is semantically wrong |
| Capabilities/tokens | Implicit copy would violate access control |
| API contracts | Force callers to explicitly clone |
| Must-use semantics | Small types that should behave like resources |

## Copy Trait and Generics

| Rule | Description |
|------|-------------|
| **VS8: Copy is structural** | Satisfied automatically if structure matches — no explicit `extend Copy` |
| **VS9: Copy is special** | Compiler-known trait that affects codegen and assignment semantics |
| **VS10: Unique overrides** | `@unique` overrides structural satisfaction |

<!-- test: skip -->
```rask
func duplicate<T: Copy>(value: T) -> (T, T) {
    (value, value)  // OK: T is Copy, so value can be copied
}

func try_duplicate<T>(value: T) -> (T, T) {
    (value, value)  // ERROR: cannot use value twice (moved)
}
```

| Type | Satisfies `T: Copy`? | Reason |
|------|----------------------|--------|
| `i32` | Yes | Primitive, always Copy |
| `(i32, i32)` | Yes | 8 bytes, all fields Copy |
| `Point{x: i32, y: i32}` | Yes | 8 bytes, all fields Copy |
| `@unique struct UserId{id: u64}` | No | Explicitly unique |
| `string` | No | >16 bytes, owns heap memory |
| `Vec<i32>` | No | Collection type, never Copy |
| `any Widget` | No | Trait object, owns heap data |

**Copy vs Clone:**

| Trait | Operation | When available | Cost |
|-------|-----------|----------------|------|
| `Copy` | Implicit copy on assign/pass | Structural: ≤16 bytes, no `@unique` | Bitwise copy (cheap) |
| `Clone` | Explicit `.clone()` call | If all fields are Clone | May allocate (visible cost) |

All Copy types are also Clone. Not all Clone types are Copy.

**Generic constraint propagation:**

<!-- test: parse -->
```rask
struct Pair<T> {
    first: T,
    second: T,
}

// Pair<T> is Copy if T is Copy and Pair<T> ≤16 bytes
const p1 = Pair{first: 1, second: 2}      // Pair<i32> is Copy (8 bytes)
const p2 = p1                              // Implicit copy

const p3 = Pair{first: 1i64, second: 2i64} // Pair<i64> is Copy (16 bytes)
const p4 = p3                              // Implicit copy

const p5 = Pair{first: [1i64; 2], second: [2i64; 2]} // Pair<[i64;2]> is NOT Copy (32 bytes > 16)
const p6 = p5                              // ERROR: move, not copy
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Struct with all Copy fields but >16 bytes | VS1 | Move-only (size exceeds threshold) |
| Generic type usage | VS5 | Copy derived when the compiler generates code for a specific type |
| Removing `@unique` from a type | U1 | Non-breaking change (makes type more permissive) |
| Copy type in `take` parameter | — | Value is copied in; `take` is semantically redundant but allowed |

---

## Appendix (non-normative)

### Rationale

**VS1–VS5 (implicit copy):** Without implicit copy, even `const y = x` for integers would invalidate `x`. The 16-byte threshold covers common types while keeping large copies visible. Everything moves or requires `.clone()` above that line — Rask never silently copies anything with meaningful cost.

**VS6 (fixed threshold):** The threshold is a design judgment, not a hardware law. Below 16 bytes, copies are cheap enough that making them visible would add noise. Above it, copies involve real memory traffic, so you must be explicit. Configurable thresholds would mean the same source code has different semantics per build, violating local analysis.

**U1–U4 (unique types):** Default ergonomic — most small types are Copy automatically. `@unique` is opt-in strictness for when semantics require it.

**Why 16 bytes:**

| Criterion | Justification |
|-----------|---------------|
| ABI boundary | Most ABIs pass ≤16 bytes in registers (x86-64 SysV, ARM AAPCS, RISC-V) |
| Common type coverage | Covers `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA`, small enums |
| Cache line fraction | 16 bytes = 1/4 cache line; small enough to not pollute cache |

**The Goldilocks principle:** Languages like Hylo require explicit `.copy()` even for a `Point2D` — that's ceremony protecting from a trivial cost. Swift has the opposite problem: any struct is a value type regardless of size. Rask avoids both extremes.

**Platform ABI considerations:** The 16-byte threshold is a *semantic* boundary, not an ABI boundary. On Windows x64 (8-byte register limit), 9-16 byte types are still semantically Copy but passed by hidden reference. The ABI detail is invisible to the programmer.

**What about performance-critical code?** Projects that need to audit every copy can enable `@warn(implicit_copy)` — an opt-in warning that flags all implicit copies without changing semantics. See `tool.warnings`.

### Patterns & Guidance

**Unique vs resource types:**

| Aspect | Unique (`@unique`) | Resource (`@resource`) |
|--------|--------------------|--------------------|
| Implicit copy | Disabled | Disabled |
| Can drop | Yes | No (must consume) |
| Explicit clone | Allowed | Not allowed |
| Use case | Semantic safety | Resource safety |
| Example | `@unique struct UserId` | `@resource struct File` |

### See Also

- [Ownership Rules](ownership.md) — Single-owner model and move semantics (`mem.ownership`)
- [Borrowing](borrowing.md) — Scoped borrowing rules (`mem.borrowing`)
- [Resource Types](resource-types.md) — Must-consume resources (`mem.resources`)
- [Warnings](../tooling/warnings.md) — `@warn(implicit_copy)` (`tool.warnings`)
