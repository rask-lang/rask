<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Solution: Value Semantics

## The Question
How do values behave on assignment, parameter passing, and return? When are types copied implicitly vs moved?

## Decision
All types are values with single ownership. Small types (≤16 bytes) copy implicitly; larger types need explicit `.clone()` or move. `@unique` opts out of implicit copy when semantics demand it.

## Rationale
Without implicit copy, even `const y = x` for integers would invalidate `x`. That's unusable. The 16-byte threshold covers common types (points, colors, pairs) while keeping large copies visible.

## Specification

### Value Semantics

All types are values. There is no distinction between "value types" and "reference types."

| Operation | Small types (≤16 bytes, Copy) | Large types |
|-----------|-------------------------------|-------------|
| Assignment `const y = x` | Copies | Moves (x invalid after) |
| Parameter passing | Copies | Borrows by default, moves with `take` |
| Return | Copies | Moves |

**Copy eligibility:**
- Primitives: always Copy
- Structs: Copy if all fields are Copy AND total size ≤16 bytes
- Enums: Copy if all variants are Copy AND total size ≤16 bytes
- Collections (Vec, Pool, Map): never Copy (own heap memory)
- Sync types (Shared, Mutex, Atomic*): never Copy (contain synchronization state)

### Why Implicit Copy?

This isn't optional. Without implicit copy, primitives break:

**Broken without copy:**
<!-- test: skip -->
```rask
const x = 5
const y = x              // Without copy: x moved to y
print(x + y)           // ❌ ERROR: x was moved
```

Alternative approaches fail design constraints:

| Approach | Problem |
|----------|---------|
| Everything moves | Violates ES ≥ 0.85 (ergonomics); every int assignment invalidates source |
| Explicit `.clone()` for all | `const y = x.clone()` for every integer violates ED ≤ 1.2 (ceremony) |
| Special-case primitives only | Creates "value types" vs "reference types" distinction, violates Principle 2 (uniform value semantics) |
| Copy-on-write / GC | Violates RO ≤ 1.10 (runtime overhead), TC ≥ 0.90 (hidden costs) |

### The 16-Byte Threshold

**The core argument:** Implicit copies in Rask are cheap enough that annotating them adds ceremony without actionable information.

Types ≤16 bytes fit in register pairs and match the register-passing limit of most calling conventions. Copies are small — at worst a few bytes on the stack, never a heap allocation. Types >16 bytes don't copy at all — they move (ownership transfer) or require explicit `.clone()`. Rask never silently copies anything with meaningful cost:

| Operation | What happens | Cost |
|-----------|-------------|------|
| `const b = a` (≤16 bytes) | Copy | Negligible (≤16 bytes, no allocation) |
| `const b = a` (>16 bytes) | Move | Zero (ownership transfer) |
| `func f(x: T)` | Borrow | Zero (read-only reference) |
| `a.clone()` | Deep duplicate | Explicit, visible |


**Why 16 bytes specifically:**

| Criterion | Justification |
|-----------|---------------|
| **ABI boundary** | Most ABIs pass ≤16 bytes in registers (x86-64 SysV, ARM AAPCS, RISC-V); above this, passing conventions get more expensive |
| **Common type coverage** | Covers `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA{r, g, b, a: u8}`, small enums |
| **Cache line fraction** | 16 bytes = 1/4 cache line; small enough to not pollute cache even when spilled to stack |

The threshold is a design judgment, not a hardware law — but it's a well-grounded one. Below 16 bytes, copies are cheap enough that making them visible would add noise. Above it, copies involve real memory traffic, so you must be explicit. That's the line I'm drawing for transparent cost.

### The goldilock principle

Languages like Hylo (Val) require explicit `.copy()` even for a `Point2D` — an operation that's a couple of register moves at most. I think that's ceremony protecting you from a cost that doesn't warrant annotation. Don't annotate trivially cheap operations.

Swift has the opposite problem: any struct is a value type, regardless of size. A `[String]` with thousands of elements implicitly copies on assignment (hidden behind copy-on-write).

Rask avoids both extremes — small types copy implicitly because the cost is trivial, large types require explicit `.clone()` because the cost is real.

**What about performance-critical code?** Projects that need to audit every copy can enable `@warn(implicit_copy)` — an opt-in warning that flags all implicit copies without changing semantics. The language still copies, you just see where. In practice, when small copies show up in profiles, the fix is usually data layout (SoA, arenas), not copy annotation. See [warnings.md](../tooling/warnings.md#implicit_copy-w0904).

### Threshold Non-Configurability

The 16-byte threshold is fixed. No knobs.

**Why:**

| Reason | Justification |
|--------|---------------|
| **Semantic stability** | Changing threshold changes program semantics (copy vs move); code portability requires fixed behavior |
| **Local analysis** | Per Principle 5, changing a compiler flag should not change whether `const y = x` copies or moves |
| **Mental model simplicity** | Developers learn one rule: ≤16 bytes copies, >16 bytes moves |
| **Library compatibility** | Generic code assumes stable Copy semantics; configurable threshold breaks abstraction boundaries |

**Rejected alternatives:**

| Alternative | Problem |
|-------------|---------|
| Compiler flag `--copy-threshold=N` | Same source code has different semantics per build; violates local analysis |
| Per-project configuration | Libraries compiled with different thresholds have incompatible semantics |
| Per-module configuration | Module boundaries become semantic boundaries; refactoring changes behavior |

### Platform ABI Considerations

The 16-byte threshold is a **semantic** boundary (copy vs move), not necessarily an **ABI** boundary.

**Semantic vs ABI distinction:**

| Concern | Boundary | Platform-specific? |
|---------|----------|-------------------|
| **Semantics** | 16 bytes (copy vs move) | No - fixed by language |
| **ABI** | Register vs stack passing | Yes - varies by platform |

**Platform calling conventions:**

| Platform | Register-passing limit | Rask Copy threshold | Mismatch handling |
|----------|----------------------|---------------------|-------------------|
| x86-64 SysV (Linux, macOS) | ≤16 bytes | 16 bytes | ✅ Perfect match |
| ARM AAPCS (ARM Linux) | ≤16 bytes | 16 bytes | ✅ Perfect match |
| Windows x64 | ≤8 bytes | 16 bytes | Compiler passes 9-16 byte types via stack/reference |
| RISC-V LP64 | ≤16 bytes | 16 bytes | ✅ Perfect match |

**Implementation strategy for ABI mismatches:**

On Windows x64, types in the 9-16 byte range are Copy (semantics) but passed differently than primitives:
- 1-8 bytes: Passed in registers (RCX, RDX, R8, R9)
- 9-16 bytes: Passed by hidden reference (caller allocates stack space, passes pointer)
- Still **semantically Copy** (implicit copy on assignment in source code)
- ABI detail is hidden from programmer

**Why not use 8 bytes as threshold?**

| Approach | Pros | Cons |
|----------|------|------|
| 8-byte threshold | Matches Windows x64 ABI perfectly | Loses ergonomics: `(i64, i64)` would require explicit clone |
| 16-byte threshold | Ergonomic for common types, matches most ABIs | Windows x64 uses stack passing for 9-16 byte types |

**Decision:** Optimize for ergonomics and most platforms. Windows x64 stack-passing is an implementation detail that doesn't affect programmer experience.

**Cost transparency preserved:**

The threshold is about **allocation visibility**, not **register vs stack**:
- Below 16 bytes: No heap allocation, bitwise copy (cheap regardless of ABI)
- Above 16 bytes: Explicit `.clone()` required (heap allocation visible)

Stack vs register passing is a microoptimization detail. The real cost is heap allocation, which the threshold controls.

**Cross-compilation:**

Code written for Linux (SysV ABI) compiles identically for Windows (x64 ABI). Semantics are platform-independent:
- Same types are Copy on all platforms
- Same code means copy vs move on all platforms
- Only low-level calling convention changes (invisible to source code)

### Automatic Copy Derivation

Copy is automatic—structural, not declared:

- Primitives: always Copy
- Structs/enums: Copy if all fields are Copy AND size ≤16 bytes

No `extend Copy` needed. The compiler figures it out.

### Unique Types (Opt-Out)

Types can explicitly opt out of Copy using the `@unique` attribute, even if structurally eligible.

**Syntax:**
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

**Semantics:**

| Rule | Description |
|------|-------------|
| **U1: No implicit copy** | Unique types MUST be explicitly cloned; assignment/passing moves |
| **U2: Clone still available** | `.clone()` works if all fields implement Clone |
| **U3: Size independent** | Works for any size, but most useful for small types |
| **U4: Transitive** | Structs containing unique fields are automatically unique |

**Example:**
<!-- test: skip -->
```rask
@unique
struct UserId { id: u64 }

const user1 = UserId{id: 42}
const user2 = user1              // Moves, user1 invalid
const user3 = user2.clone()      // ✅ OK: explicit clone
const user4 = user3              // Moves, user3 invalid
```

**Use cases:**

| Use Case | Rationale |
|----------|-----------|
| Unique identifiers | User IDs, entity handles where duplication is semantically wrong |
| Capabilities/tokens | Security tokens, permissions where implicit copy would violate access control |
| API contracts | Force callers to explicitly clone, making allocation visible |
| Linear-like semantics | Small types that should behave like resources (even if not true linear resource types) |

**Interaction with generics:**

<!-- test: skip -->
```rask
func process<T>(value: T) { ... }

const id = UserId{id: 1}
process(id)           // Moves id (move-only type)

// For Copy types:
const num = 42
process(num)          // Copies num (i32 is Copy)
```

Move-only types do NOT satisfy `T: Copy` constraints in generics (see Copy trait section below).

**Design rationale:**

- **Default ergonomic:** Most small types are Copy automatically (no annotation needed)
- **Opt-in strictness:** Only use `@unique` when semantics require it
- **Clear intent:** Attribute signals "this type should not be casually duplicated"
- **Backward compatible:** Removing `@unique` from a type is a non-breaking change (makes it more permissive)

**Comparison with resource types:**

| Aspect | Unique types | Resource types |
|--------|--------------|----------------|
| Must consume | No (can drop) | Yes (compiler error if not consumed) |
| Can clone | Yes (if fields are Clone) | No (unique ownership) |
| Use case | Semantic safety | Resource safety |
| Example | `@unique struct UserId` | `@resource struct File` |

### Copy Trait and Generics

The `Copy` trait is a structural, compiler-known property that determines whether a type can be implicitly copied.

**Copy trait satisfaction:**

A type satisfies the `Copy` trait if and only if:
1. All fields are Copy (recursive check)
2. Total size ≤16 bytes
3. NOT marked with `@unique` attribute
4. NOT a collection type (Vec, Pool, Map)

**Generic constraints:**

<!-- test: skip -->
```rask
func duplicate<T: Copy>(value: T) -> (T, T) {
    (value, value)  // ✅ OK: T is Copy, so value can be copied
}

func try_duplicate<T>(value: T) -> (T, T) {
    (value, value)  // ❌ ERROR: cannot use value twice (moved)
}
```

**Type checking:**

| Type | Satisfies `T: Copy`? | Reason |
|------|----------------------|--------|
| `i32` | ✅ Yes | Primitive, always Copy |
| `(i32, i32)` | ✅ Yes | 8 bytes, all fields Copy |
| `Point{x: i32, y: i32}` | ✅ Yes | 8 bytes, all fields Copy |
| `@unique struct UserId{id: u64}` | ❌ No | Explicitly unique (no copy) |
| `string` | ❌ No | >16 bytes, owns heap memory |
| `Vec<i32>` | ❌ No | Collection type, never Copy |

**Monomorphization:**

When a generic function is instantiated with a concrete type, the compiler checks constraints:

<!-- test: skip -->
```rask
const point = Point{x: 1, y: 2}
let (p1, p2) = duplicate(point)  // ✅ OK: Point satisfies Copy

const id = UserId{id: 42}
let (id1, id2) = duplicate(id)   // ❌ ERROR: UserId is move-only (doesn't satisfy Copy)

const name = string.from("Alice")
let (n1, n2) = duplicate(name)   // ❌ ERROR: string doesn't satisfy Copy
```

**Copy vs Clone:**

| Trait | Operation | When available | Cost |
|-------|-----------|----------------|------|
| `Copy` | Implicit copy on assign/pass | Structural: ≤16 bytes, no `@unique` | Bitwise copy (cheap) |
| `Clone` | Explicit `.clone()` call | If all fields are Clone | May allocate (visible cost) |

All Copy types are also Clone (can call `.clone()` explicitly). Not all Clone types are Copy.

<!-- test: skip -->
```rask
// Copy type (implicit):
const p1 = Point{x: 1, y: 2}
const p2 = p1             // Implicit copy
const p3 = p1.clone()     // Explicit clone (same as copy)

// Clone-only type (explicit):
const s1 = string.from("hello")
const s2 = s1             // Move (s1 invalid)
const s3 = s2.clone()     // Explicit clone (allocates)
```

**Relationship with traits system:**

Per CORE_DESIGN Principle 7 (structural traits), Copy is automatically satisfied if the structure matches. No explicit `extend Copy` is required.

However, Copy is special:
- It's a compiler-known trait (affects codegen)
- It changes assignment semantics (copy vs move)
- The `@unique` attribute overrides structural satisfaction

For user-defined traits, structural matching is purely for dispatch. For Copy, it affects language semantics.

**Generic constraints propagation:**

<!-- test: skip -->
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
const p6 = p5                              // ❌ ERROR: move, not copy
```

The compiler automatically derives Copy for generic types when instantiated with Copy type arguments, subject to the size threshold.

## Integration Notes

- **Ownership:** Value semantics integrates with single-owner model (see [ownership.md](ownership.md))
- **Borrowing:** Borrows allow temporary access without copy/move (see [borrowing.md](borrowing.md))
- **Type System:** Copy is a structural trait; no explicit extend required
- **Generics:** Copy constraints work with monomorphization (code generation)
- **Tooling:** IDE shows copy vs move at each use site

## See Also

- [Ownership Rules](ownership.md) — Single-owner model and move semantics
- [Borrowing](borrowing.md) — One rule: views last as long as the source is stable
- [Resource Types](resource-types.md) — Must-consume resources (linear resources)
