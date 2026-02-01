# Solution: Value Semantics

## The Question
How do values behave on assignment, parameter passing, and return? When are types copied implicitly vs moved?

## Decision
All types are values with single ownership. Small types (≤16 bytes) that contain only copyable data are implicitly copied; larger types require explicit `.clone()` or move. The `move` keyword allows opt-out of implicit copy for semantic reasons.

## Rationale
Implicit copy is fundamental for ergonomic value semantics—without it, even integer assignments would invalidate the source. The 16-byte threshold balances ergonomics (covers common types like points, colors, pairs) with cost transparency (larger types require visible `.clone()`).

## Specification

### Value Semantics

All types are values. There is no distinction between "value types" and "reference types."

| Operation | Small types (≤16 bytes, Copy) | Large types |
|-----------|-------------------------------|-------------|
| Assignment `let y = x` | Copies | Moves (x invalid after) |
| Parameter passing | Copies | Moves (unless `read`/`mutate` mode) |
| Return | Copies | Moves |

**Copy eligibility:**
- Primitives: always Copy
- Structs: Copy if all fields are Copy AND total size ≤16 bytes
- Enums: Copy if all variants are Copy AND total size ≤16 bytes
- Collections (Vec, Pool, Map): never Copy (own heap memory)

### Why Implicit Copy?

Implicit copy is a fundamental requirement for ergonomic value semantics, not an optional optimization.

**Without implicit copy, primitives would have move semantics:**
```
let x = 5
let y = x              // Without copy: x moved to y
print(x + y)           // ❌ ERROR: x was moved
```

Alternative approaches fail design constraints:

| Approach | Problem |
|----------|---------|
| Everything moves | Violates ES ≥ 0.85 (ergonomics); every int assignment invalidates source |
| Explicit `.clone()` for all | `let y = x.clone()` for every integer violates ED ≤ 1.2 (ceremony) |
| Special-case primitives only | Creates "value types" vs "reference types" distinction, violates Principle 2 (uniform value semantics) |
| Copy-on-write / GC | Violates RO ≤ 1.10 (runtime overhead), TC ≥ 0.90 (hidden costs) |

### The 16-Byte Threshold

Value semantics (Principle 2) requires uniform behavior: if `i32` copies, then `Point{x: i32, y: i32}` should also copy. But blind copying of large types violates cost transparency (TC ≥ 0.90).

The threshold balances ergonomics with visibility:
- **Below threshold:** Types behave like mathematical values (copy naturally)
- **Above threshold:** Explicit `.clone()` required (cost visible)

**Threshold criteria:**

| Criterion | Justification |
|-----------|---------------|
| **Platform ABI alignment** | Most ABIs pass ≤16 bytes in registers (x86-64 SysV, ARM AAPCS); copies are zero-cost |
| **Common type coverage** | Covers primitives, pairs, RGBA colors, 2D/3D points, small enums |
| **Cache efficiency** | 16 bytes = 1/4 cache line; small enough to not pollute cache |
| **Visibility boundary** | Large enough for natural types, small enough that copies stay obvious |

**Chosen threshold: 16 bytes**

Rationale:
- Matches x86-64 and ARM register-passing conventions (zero-cost copy)
- Covers `(i64, i64)`, `Point3D{x, y, z: f32}`, `RGBA{r, g, b, a: u8}`
- Small enough that silent copies don't violate cost transparency
- Consistent with Rust's typical Copy threshold (though Rust leaves it to type authors)

Types above 16 bytes MUST use explicit `.clone()` or move semantics, making allocation/copy cost visible.

### Threshold Non-Configurability

The 16-byte threshold is **fixed by the language specification** and is NOT configurable.

**Rationale for fixed threshold:**

| Reason | Justification |
|--------|---------------|
| **Semantic stability** | Changing threshold changes program semantics (copy vs move); code portability requires fixed behavior |
| **Local analysis** | Per Principle 5, changing a compiler flag should not change whether `let y = x` copies or moves |
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

**Copy is automatic (structural):**

The compiler automatically determines whether a type is Copy based on structure:
- Primitives: always Copy (language-defined)
- Structs/enums: Copy if all fields are Copy AND size ≤16 bytes

No explicit `impl Copy` declaration is required—Copy is a structural property.

### Move-Only Types (Opt-Out)

Types can explicitly opt out of Copy using the `move` keyword, even if structurally eligible.

**Syntax:**
```
move struct UserId {
    id: u64  // 8 bytes, Copy-eligible, but forced move-only
}

move enum Token {
    Access(u64),
    Refresh(u64),
}
```

**Semantics:**

| Rule | Description |
|------|-------------|
| **MO1: No implicit copy** | Move-only types MUST be explicitly cloned; assignment/passing moves |
| **MO2: Clone still available** | `.clone()` works if all fields implement Clone |
| **MO3: Size independent** | Works for any size, but most useful for small types |
| **MO4: Transitive** | Structs containing move-only fields are automatically move-only |

**Example:**
```
move struct UserId { id: u64 }

let user1 = UserId{id: 42}
let user2 = user1              // Moves, user1 invalid
let user3 = user2.clone()      // ✅ OK: explicit clone
let user4 = user3              // Moves, user3 invalid
```

**Use cases:**

| Use Case | Rationale |
|----------|-----------|
| Unique identifiers | User IDs, entity handles where duplication is semantically wrong |
| Capabilities/tokens | Security tokens, permissions where implicit copy would violate access control |
| API contracts | Force callers to explicitly clone, making allocation visible |
| Linear-like semantics | Small types that should behave like resources (even if not true linear types) |

**Interaction with generics:**

```
fn process<T>(value: T) { ... }

let id = UserId{id: 1}
process(id)           // Moves id (move-only type)

// For Copy types:
let num = 42
process(num)          // Copies num (i32 is Copy)
```

Move-only types do NOT satisfy `T: Copy` bounds in generics (see Copy trait section below).

**Design rationale:**

- **Default ergonomic:** Most small types are Copy automatically (no annotation needed)
- **Opt-in strictness:** Only use `move` when semantics require it
- **Clear intent:** Keyword signals "this type should not be casually duplicated"
- **Backward compatible:** Removing `move` from a type is a non-breaking change (makes it more permissive)

**Comparison with linear types:**

| Aspect | Move-only types | Linear types |
|--------|-----------------|--------------|
| Must consume | No (can drop) | Yes (compiler error if not consumed) |
| Can clone | Yes (if fields are Clone) | No (unique ownership) |
| Use case | Semantic safety | Resource safety |
| Example | `move struct UserId` | `linear struct File` |

### Copy Trait and Generics

The `Copy` trait is a structural, compiler-known property that determines whether a type can be implicitly copied.

**Copy trait satisfaction:**

A type satisfies the `Copy` trait if and only if:
1. All fields are Copy (recursive check)
2. Total size ≤16 bytes
3. NOT marked with `move` keyword
4. NOT a collection type (Vec, Pool, Map)

**Generic bounds:**

```
fn duplicate<T: Copy>(value: T) -> (T, T) {
    (value, value)  // ✅ OK: T is Copy, so value can be copied
}

fn try_duplicate<T>(value: T) -> (T, T) {
    (value, value)  // ❌ ERROR: cannot use value twice (moved)
}
```

**Type checking:**

| Type | Satisfies `T: Copy`? | Reason |
|------|----------------------|--------|
| `i32` | ✅ Yes | Primitive, always Copy |
| `(i32, i32)` | ✅ Yes | 8 bytes, all fields Copy |
| `Point{x: i32, y: i32}` | ✅ Yes | 8 bytes, all fields Copy |
| `move struct UserId{id: u64}` | ❌ No | Explicitly move-only |
| `String` | ❌ No | >16 bytes, owns heap memory |
| `Vec<i32>` | ❌ No | Collection type, never Copy |

**Monomorphization:**

When a generic function is instantiated with a concrete type, the compiler checks bounds:

```
let point = Point{x: 1, y: 2}
let (p1, p2) = duplicate(point)  // ✅ OK: Point satisfies Copy

let id = UserId{id: 42}
let (id1, id2) = duplicate(id)   // ❌ ERROR: UserId is move-only (doesn't satisfy Copy)

let name = String::from("Alice")
let (n1, n2) = duplicate(name)   // ❌ ERROR: String doesn't satisfy Copy
```

**Copy vs Clone:**

| Trait | Operation | When available | Cost |
|-------|-----------|----------------|------|
| `Copy` | Implicit copy on assign/pass | Structural: ≤16 bytes, no `move` | Bitwise copy (cheap) |
| `Clone` | Explicit `.clone()` call | If all fields are Clone | May allocate (visible cost) |

All Copy types are also Clone (can call `.clone()` explicitly). Not all Clone types are Copy.

```
// Copy type (implicit):
let p1 = Point{x: 1, y: 2}
let p2 = p1             // Implicit copy
let p3 = p1.clone()     // Explicit clone (same as copy)

// Clone-only type (explicit):
let s1 = String::from("hello")
let s2 = s1             // Move (s1 invalid)
let s3 = s2.clone()     // Explicit clone (allocates)
```

**Relationship with traits system:**

Per CORE_DESIGN Principle 7 (structural traits), Copy is automatically satisfied if the structure matches. No explicit `impl Copy` is required.

However, Copy is special:
- It's a compiler-known trait (affects codegen)
- It changes assignment semantics (copy vs move)
- The `move` keyword overrides structural satisfaction

For user-defined traits, structural matching is purely for dispatch. For Copy, it affects language semantics.

**Generic constraints propagation:**

```
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
let p6 = p5                              // ❌ ERROR: move, not copy
```

The compiler automatically derives Copy for generic types when instantiated with Copy type arguments, subject to the size threshold.

## Integration Notes

- **Ownership:** Value semantics integrates with single-owner model (see [ownership.md](ownership.md))
- **Borrowing:** Borrows allow temporary access without copy/move (see [borrowing.md](borrowing.md))
- **Type System:** Copy is a structural trait; no explicit impl required
- **Generics:** Copy bounds work with monomorphization
- **Tooling:** IDE shows copy vs move at each use site

## See Also

- [Ownership Rules](ownership.md) — Single-owner model and move semantics
- [Borrowing](borrowing.md) — Block-scoped and expression-scoped borrows
- [Linear Types](linear-types.md) — Must-consume resources
