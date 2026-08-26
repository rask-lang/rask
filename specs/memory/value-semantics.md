<!-- id: mem.value -->
<!-- status: decided -->
<!-- summary: All types are values; ≤16 bytes copies implicitly, larger types move -->
<!-- depends: memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Value Semantics

All types are values with single ownership. Small types (≤16 bytes) copy implicitly; larger types need explicit `.clone()` or move. `@unique` opts out of implicit copy; `@small` fences a type's size at the threshold.

## Copy vs Move

| Operation | Small types (≤16 bytes, Copy) | Large types |
|-----------|-------------------------------|-------------|
| Assignment `let y = x` | Copies | Moves (x invalid after) |
| Parameter passing | Copies | Borrows by default, moves with `take` |
| Return | Copies | Moves |

| Rule | Description |
|------|-------------|
| **VS1: Copy eligibility** | Copy if all fields are Copy AND total size ≤16 bytes |
| **VS2: Primitives always Copy** | Primitives are always Copy |
| **VS3: Collections never Copy** | Vec, Pool, Map are never Copy (own heap memory, mutable). `string` is not a collection — it's a language primitive with compiler-special refcount semantics. No user-defined type can replicate string's refcounted Copy behavior. This is a deliberate exception, not a pattern |
| **VS3.1: Trait objects never Copy** | `any Trait` is never Copy (owns heap data; copying would create two owners) |
| **VS4: Sync types never Copy** | Shared, Mutex, Atomic are never Copy |
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

**Common type coverage:** `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA{r, g, b, a: u8}`, `string`, small enums.

## Unique Types (Opt-Out)

`@unique` forces move-only semantics even if structurally Copy-eligible.

| Rule | Description |
|------|-------------|
| **U1: No implicit copy** | Unique types MUST be explicitly cloned; assignment/passing moves |
| **U2: Clone still available** | `.clone()` works if all fields implement Cloneable |
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

let user1 = UserId{id: 42}
let user2 = user1              // Moves, user1 invalid
let user3 = user2.clone()      // OK: explicit clone
let user4 = user3              // Moves, user3 invalid
```

| Use Case | Rationale |
|----------|-----------|
| Unique identifiers | Duplication is semantically wrong |
| Capabilities/tokens | Implicit copy would violate access control |
| API contracts | Force callers to explicitly clone |
| Must-use semantics | Small types that should behave like resources |

## Size Fence (Opt-In)

Copy eligibility is automatic (VS1), but it's also fragile at a distance: add a field that pushes a struct past 16 bytes and every assignment flips from copy to move, with the errors landing at use sites far from the field that caused them. `@small` fences the size at the definition, so the growth errors exactly where it happened.

| Rule | Description |
|------|-------------|
| **SM1: Pure size assertion** | `@small` asserts the type's total size stays ≤16 bytes — the copy threshold (VS6). It asserts nothing else and changes no semantics. A `@small` type with all-Copy fields is therefore guaranteed to copy implicitly (VS1) |
| **SM2: Fails at the definition** | A `@small` type over 16 bytes errors at the annotation, naming the field and the sizes — not at use sites |
| **SM3: Generic types check per instantiation** | `@small` on a generic type requires every instantiation to fit — checked once per instantiation the program names, like other generic bounds (`type.generics/G2`). The error names the offending type arguments and lands on the declaration, since that's where both fixes go |
| **SM4: Composes with @unique** | `@small` + `@unique` is legal — a small, move-only ID type is coherent. `@small` is about layout; `@unique` is about copy semantics |

<!-- test: parse -->
```rask
@small
struct Point3D {
    x: f32
    y: f32
    z: f32
}
// 12 bytes — @small records that callers depend on it staying register-sized
```

Use it where staying small is API: math primitives, IDs, anything callers pass around freely and cheaply. Unannotated types keep the automatic behavior — `@small` adds a fence, not a requirement.

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
| `string` | Yes | 16 bytes, immutable refcounted (see `std.strings/S1`) |
| `StringView` | Yes | 16 bytes, refcounted view into a string (see `std.strings/V1`) |
| `Vec<i32>` | No | Collection type, never Copy |
| `any Widget` | No | Trait object, owns heap data |

**Copy vs Clone:**

| Trait | Operation | When available | Cost |
|-------|-----------|----------------|------|
| `Copy` | Implicit copy on assign/pass | Structural: ≤16 bytes, no `@unique` | Bitwise copy (cheap) |
| `Cloneable` | Explicit `.clone()` call | If all fields are Cloneable | May allocate (visible cost) |

All Copy types are also Cloneable. Not all Cloneable types are Copy.

**Generic constraint propagation:**

<!-- test: parse -->
```rask
struct Pair<T> {
    first: T,
    second: T,
}

// Pair<T> is Copy if T is Copy and Pair<T> ≤16 bytes
let p1 = Pair{first: 1, second: 2}      // Pair<i32> is Copy (8 bytes)
let p2 = p1                              // Implicit copy

let p3 = Pair{first: 1i64, second: 2i64} // Pair<i64> is Copy (16 bytes)
let p4 = p3                              // Implicit copy

let p5 = Pair{first: [1i64; 2], second: [2i64; 2]} // Pair<[i64;2]> is NOT Copy (32 bytes > 16)
let p6 = p5                              // ERROR: move, not copy
```

## Error Messages

Move errors name *why* the type moves — the checker tracks the reason (size over threshold, owns heap memory, `@unique`, `@resource`) and the diagnostic states it. This matters most when a struct grows past 16 bytes and every assignment flips from copy to move: the errors land at call sites, so the note is what connects them back to the type.

```
ERROR [E0800]: use of moved value: `x`
   |
 9 |     let y = x
   |     ----------- value moved here
10 |     println("{x.a}")
   |               ^ value used here after move

NOTE: `Big` is 24 bytes (copy threshold is 16) — assignment moves instead of copying
HELP: add `x.clone()` if you need an independent copy
```

**Size fence broken [SM2]:**
```
ERROR [mem.value/SM2]: @small type `Point` outgrew the copy threshold
   |
1  |  @small
   |  ------ size fenced here
2  |  struct Point {
   |
5  |      id: u64
   |      ^^^^^^^ adding this made Point 20 bytes (limit is 16)

WHY: @small asserts this type stays within the 16-byte copy threshold —
     small enough to copy implicitly and pass in registers. The threshold
     is fixed (VS6); the annotation checks it, it never raises it.

FIX: shrink the type (id: u32 fits in 16), or remove @small and let Point
     move — call sites that relied on copying will error with a note
     pointing back to the size change.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Struct with all Copy fields but >16 bytes | VS1 | Move-only (size exceeds threshold); move errors name the size and threshold (see Error Messages) |
| Generic type usage | VS5 | Copy derived when the compiler generates code for a specific type |
| Removing `@unique` from a type | U1 | Non-breaking change (makes type more permissive) |
| Copy type in `take` parameter | — | Value is copied in; `take` is semantically redundant but allowed |
| `@small` on an already-small type | SM1 | No-op assertion — that's the intended use |
| `@small` + `@unique` on one type | SM4 | Legal — size fence on a move-only type |
| Removing `@small` from a type | SM1 | Non-breaking by itself; nothing changes until the type grows |
| `@small` on generic type, one bad instantiation | SM3 | Error at that instantiation, naming the type arguments |

---

## Appendix (non-normative)

### Rationale

**VS1–VS5 (implicit copy):** Without implicit copy, even `let y = x` for integers would invalidate `x`. The 16-byte threshold covers common types while keeping large copies visible. Everything moves or requires `.clone()` above that line — Rask never silently copies anything with meaningful cost.

**VS6 (fixed threshold):** The threshold is a design judgment, not a hardware law. Below 16 bytes, copies are cheap enough that making them visible would add noise. Above it, copies involve real memory traffic, so you must be explicit. Configurable thresholds would mean the same source code has different semantics per build, violating local analysis.

**U1–U4 (unique types):** Default ergonomic — most small types are Copy automatically. `@unique` is opt-in strictness for when semantics require it.

**SM1–SM4 (size fence):** The cliff's real problem was never the threshold — it was error locality. A struct crossing 16 bytes surfaces as "use of moved value" at distant call sites, and until the move-error NOTE (above) landed, nothing connected them to the field that caused it. `@small` finishes the job: for types callers depend on staying cheap, the break errors at the definition, before any call site sees it. I considered the full Rust model — everything moves, annotation opts in — which fixes locality completely, but it fails the litmus test: Go copies structs implicitly at any size, and requiring an annotation on every `Point` and `Vec3` makes Rask noisier than Go on the most basic code in the language. Automatic below the threshold, fenceable where it's API, is both. Naming went two rounds: `@copy` mirrored `@unique` nicely but names a compiler concept — the reader has to know what Copy means in PL jargon to know what they promised. What the author actually controls, and any reader can check with a byte count, is size. `@small` asserts exactly that; copying is the consequence VS1 already explains. Keeping it a pure size assertion also dissolved a fake contradiction — `@small @unique` is a perfectly coherent small move-only ID type, where `@copy @unique` had to be an error.

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
- [Linearity](linear.md) — How `@resource` and `Heap<T>` extend the value model with must-consume rules (`mem.linear`)
- [Resource Types](resource-types.md) — `@resource` annotation (`mem.resources`)
- [Warnings](../tooling/warnings.md) — `@warn(implicit_copy)` (`tool.warnings`)
