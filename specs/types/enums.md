<!-- id: type.enums -->
<!-- status: decided -->
<!-- summary: Tagged unions with inline payloads, inferred binding modes, exhaustive matching -->
<!-- depends: types/structs.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Enums

Tagged unions with inline payloads. Compiler infers binding modes. Exhaustiveness checked at compile time. One `match` keyword, no mode annotations.

## Enum Definition

<!-- test: parse -->
```rask
enum Name { A, B }                    // Simple tag-only
enum Name { A(T), B(U, V) }           // Variants with payloads
enum Name<T> { Some(T), None }        // Generic enum
```

| Rule | Description |
|------|-------------|
| **E1: Inline storage** | Variant payloads stored inline (no heap except `Owned<T>`) |
| **E2: Discriminant sizing** | Auto-sized: u8 (≤256 variants), u16 (≤65536 variants) |
| **E3: Max variants** | Maximum 65536 variants per enum |

## Value Semantics

| Rule | Description |
|------|-------------|
| **E4: Copy eligibility** | Enum is Copy if total size ≤16 bytes AND all variants are Copy |
| **E5: Move semantics** | Non-Copy enums move on assignment; source invalidated |

Clone derived automatically if all variants implement Clone.

**Copy vs Move in patterns:**

| Enum Type | `match x` Behavior | After Match |
|-----------|-------------------|-------------|
| Copy | Copies x into binding | x still valid |
| Non-Copy | Moves x into binding | x consumed |

## Pattern Matching

One keyword: `match`. The compiler infers binding modes from usage.

| Rule | Description |
|------|-------------|
| **PM1: Mode inference** | Compiler infers binding mode from usage; highest mode wins across all arms |
| **PM2: Exhaustiveness** | All variants must be handled; compiler reports specific unhandled variants |
| **PM3: Pattern guards** | Guarded variant must have unguarded fallback arm or wildcard after |
| **PM4: Or-patterns** | Multiple patterns share an arm with `\|`; all must bind same names with compatible types |

**Binding mode summary:**

| Binding Usage | Inferred Mode | Source After |
|---------------|---------------|--------------|
| Only reads | Borrow (immutable) | Valid |
| Any mutation | Borrow (mutable) | Valid, may be modified |
| Any `take` parameter | Taken (moved) | Consumed |

No mode annotations in source. IDE shows inferred mode as ghost text.

**Borrow (inferred when bindings only borrowed):**
```rask
match result {                      // IDE ghost: [borrows]
    Ok(value) => println(value),    // value: borrowed (inferred read)
    Err(error) => log(error)        // error: borrowed (inferred read)
}
// result still valid
```

**Borrow + mutate (inferred when any binding mutated):**
```rask
match connection {                          // IDE ghost: [mutates]
    Connected(sock) => sock.set_timeout(30),  // sock: borrowed (inferred mutate)
    _ => {}
}
// connection still valid, possibly modified
```

**Take (inferred when any binding passed to `take` parameter):**
```rask
match result {                      // IDE ghost: [takes]
    Ok(value) => consume(value),    // value: taken (inferred)
    Err(error) => handle(error)     // error: taken (inferred)
}
// result is consumed, invalid here
```

## Exhaustiveness Checking

| Condition | Compiler Behavior |
|-----------|-------------------|
| All variants matched | Valid |
| Missing variant, no wildcard | Error: "non-exhaustive match, missing `VariantName`" |
| Wildcard `_` present | Valid |
| Unreachable pattern | Warning: "unreachable pattern" |

Compiler reports which specific variants are unhandled.

## Pattern Guards

Conditional matching requires explicit catch-all. No hidden gaps.

```rask
match response {
    Ok(body) if body.len() > 0: process(body),
    Ok(body) => default(body),  // REQUIRED: catches guard failure
    Err(e) => handle(e)
}
```

| Guard Condition | Enforcement |
|-----------------|-------------|
| Guard on variant V | Must have unguarded V arm OR wildcard after |
| No catch-all for guarded variant | Error: "pattern `V(_)` may not match when guard fails" |

```rask
// ❌ INVALID: guard may fail with no fallback
match response {
    Ok(body) if body.len() > 0: process(body),
    Err(e) => handle(e)
}
```

## Or-Patterns

Multiple patterns share a single arm using `|` (or).

```rask
match input {
    "quit" | "exit" | "q" => break,
    "help" | "h" | "?" => print_help(),
    _ => process(input)
}

match token {
    Plus | Minus => parse_additive(),
    Star | Slash | Percent => parse_multiplicative(),
    _ => parse_primary()
}
```

| Pattern | Valid | Notes |
|---------|-------|-------|
| `A \| B => ...` | Yes | Simple alternatives |
| `A(x) \| B(x) => use(x)` | Yes | Both bind `x` with same type |
| `A(x) \| B(y) => ...` | No | Different binding names |
| `A(x: i32) \| B(x: string) => ...` | No | Incompatible types for `x` |
| `(A \| B, C \| D) => ...` | Yes | Nested or-patterns |

Or-patterns can be nested and work with guards: `A(x) | B(x) if x > 0 => ...`

**With payloads:**
```rask
match result {
    Ok(0) | Err(0) => println("zero"),
    Ok(n) | Err(n) => println("value: {n}"),
}
```

## Linear Resources in Enums

Enums may contain linear payloads (File, Socket, etc.).

| Rule | Description |
|------|-------------|
| **PM5: Wildcards forbidden for linear** | `_` on linear payload is compile error |
| **PM6: All arms must bind** | Each arm must name linear values; bound value must be consumed |

```rask
// ✅ VALID: linear value consumed in each arm
match file_result {                 // IDE ghost: [consumes]
    Ok(file) => try file.close(),    // file transferred to close()
    Err(e) => return Err(e)
}

// ❌ INVALID: wildcard discards linear File
match file_result {
    Ok(_) => {},
    Err(e) => {}
}
// Error: "wildcard pattern discards linear resource `File`"

// ❌ INVALID: read-only usage of linear resource
match file_result {
    Ok(file) => println(file.path),  // Only reads file
    Err(e) => log(e)
}
// Error: "linear resource `file` must be consumed, not just read"
```

**Or-patterns with linear payloads** follow the same rules—all alternatives must bind the linear value, and it must be consumed:

```rask
match file_result {
    Ok(file) | Recovered(file) => file.close(),
    Err(e) => log(e)
}
```

**Guards with linear resources** — catch-all arm must also bind and consume:

```rask
// ✅ VALID: guard with linear resource — both arms consume
match file_result {
    Ok(file) if file.size() > 1000: process(file),  // Large files: process
    Ok(file) => try file.close(),                     // Small files: still must close
    Err(e) => log(e)
}

// ❌ INVALID: wildcard catch-all discards linear resource
match file_result {
    Ok(file) if file.size() > 1000: process(file),
    Ok(_) => {},                                     // Error: discards linear File
    Err(e) => log(e)
}
```

## Error Propagation and Linear Resources

`try` extracts Ok or returns early with Err.

**Rule:** All linear resources in scope must be resolved before `try`.

```rask
// ❌ INVALID: file2 may leak on early return
func process(file1: File, file2: File) -> () or Error {
    const data = try file1.read()  // file2 not consumed!
    try file2.close()
}
// Error: "linear resource `file2` may leak on early return at `try`"

// ✅ VALID: all linear resources resolved before `try`
func process(file1: File, file2: File) -> () or Error {
    const result1 = file1.read()
    const result2 = file2.close()
    const data = try result1
    try result2
    Ok(())
}
```

Alternative: use `ensure` for guaranteed cleanup:
```rask
func process(file1: File, file2: File) -> () or Error {
    ensure file1.close()  // Guaranteed at scope exit
    ensure file2.close()  // Runs on any exit
    const data = try file1.read()  // ✅ Safe: ensure registered
    Ok(())
}
```

## Enum Methods

Methods in `extend` blocks, separate from definition. Default to non-consuming (borrow `self`).

<!-- test: parse -->
```rask
enum Option<T> {
    Some(T),
    None,
}

extend Option<T> {
    // Default: borrows self (compiler infers read vs mutate)
    func is_some(self) -> bool {
        match self {               // IDE ghost: [reads] (inferred from wildcard-only usage)
            Some(_) => true,
            None => false
        }
    }

    // Explicitly consuming — prefer x! operator over calling this directly
    func force(take self) -> T {
        match self {               // IDE ghost: [consumes] (inferred from returning v)
            Some(v) => v,
            None => panic("force on None")
        }
    }
}
```

| Self Mode | Behavior |
|-----------|----------|
| `self` (default) | Borrow (compiler infers read vs mutate) |
| `take self` | Consumes enum |

## Recursive Enums

Self-referential enums need explicit `Owned<T>` indirection. See [owned.md](../memory/owned.md).

<!-- test: parse -->
```rask
enum Tree<T> {
    Leaf(T),
    Node(Owned<Tree<T>>, Owned<Tree<T>>)
}

const tree = Node(own Leaf(1), own Leaf(2))  // `own` = visible allocation
```

| Rule | Description |
|------|-------------|
| **E6: Indirection required** | Recursive enum without `Owned<T>` indirection is rejected |

| Syntax | Meaning |
|--------|---------|
| `own expr` | Heap-allocate expr, return `Owned<T>` |
| `Owned<T>` | Owning heap pointer (linear) |

`Owned<T>` is linear—must consume exactly once. Drop deallocates automatically.

## Variant Iteration

Fieldless enums (all variants have zero fields) support `.variants()`, which returns a `Vec` of all variant values in declaration order.

| Rule | Description |
|------|-------------|
| **E7: Variant iteration** | `EnumName.variants()` returns `Vec<EnumName>` with all variants in declaration order |
| **E8: Fieldless constraint** | `.variants()` is a compile error if any variant has fields |

```rask
enum Color { Red, Green, Blue }

const all = Color.variants()       // [Color.Red, Color.Green, Color.Blue]

for color in Color.variants() {
    println(color)                 // Red, Green, Blue
}
```

Enums with payloads cannot use `.variants()` — there's no way to construct values without data:

```rask
enum Shape { Circle(f64), Rect { w: f64, h: f64 } }
Shape.variants()  // ❌ Compile error: variants() requires fieldless enum
```

## Discriminant Access

| Rule | Description |
|------|-------------|
| **E9: Discriminant access** | `discriminant(e)` returns zero-indexed variant index as `u16` |
| **E10: Null-pointer optimization** | Compiler applies niche optimization automatically; `@layout(C)` disables it |

```rask
enum Status { Pending, Done, Failed }

const s = Done
const d = discriminant(s)  // 1 (zero-indexed)
```

**Function signature:**
```rask
func discriminant(e: T) -> u16 where T: Enum
```

| Attribute | Behavior |
|-----------|----------|
| None | Compiler MAY reorder variants for size optimization |
| `@layout(ordered)` | Discriminant values locked to declaration order |
| `@layout(C)` | C-compatible layout, no niche optimization |

**Null-pointer optimization:**

| Type | Representation |
|------|----------------|
| `Option<Owned<T>>` | Null = None, non-null = Some |
| `Option<Handle<T>>` | generation=0 = None, else Some |

## Empty Enum (Never Type)

<!-- test: parse -->
```rask
enum Never {}  // Cannot be constructed
```

| Rule | Description |
|------|-------------|
| **E11: Empty enum** | Zero-size, uninhabited; cannot be constructed; match needs no arms |

| Property | Behavior |
|----------|----------|
| Size | 0 bytes |
| Construction | Impossible (no variants) |
| Pattern match | No arms needed |
| `Result<T, Never>` | Err arm unreachable, compiler optimizes |

```rask
func infallible() -> i32 or Never { Ok(42) }

const value = infallible()!  // Cannot panic (compiler knows)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Empty enum | E11 | Zero-size, uninhabited, match needs no arms |
| `.variants()` on fieldless enum | E7 | Returns `Vec` of all variant values |
| `.variants()` on enum with payloads | E8 | Compile error |
| `.variants()` on empty enum | E7,E11 | Returns empty `Vec` |
| Single-variant enum | E2 | Valid, discriminant may be optimized away |
| Zero-sized payload | E1 | `enum Foo { A(()), B }` — unit optimized to `{ A, B }` |
| >65536 variants | E3 | Compile error: "enum exceeds variant limit" |
| Nested linear | PM6 | `Result<File, Error(File)>` — both arms bind linear, both must consume |
| Enum in Vec | E5 | Allowed if non-linear |
| Enum in Pool | E5 | Allowed; linear payloads require explicit `remove()` + consume |
| Option\<File\> in Pool | PM5 | Error: "Pool cannot contain linear payloads" |
| Generic constraints | E4 | `enum Foo<T: Clone>` — constraint applies to ALL variants uniformly |
| Match on Copy type | E4 | Copies enum, mutations in arm affect copy not original |

## Examples

### State Machine
<!-- test: skip -->
```rask
enum Connection {
    Idle,
    Connecting(Address),
    Connected(Socket),
    Failed(Error)
}

extend Connection {
    func step(take self) -> Connection {
        match self {                    // IDE ghost: [consumes]
            Idle => Connecting(resolve_address()),
            Connecting(addr) => match try_connect(addr) {
                Ok(sock) => Connected(sock),
                Err(e) => Failed(e)
            },
            Connected(sock) => {
                sock.send(heartbeat())
                Connected(sock)
            },
            Failed(e) => Failed(e)
        }
    }

    func is_connected(self) -> bool {
        match self {                    // IDE ghost: [reads]
            Connected(_) => true,
            _ => false
        }
    }
}
```

### Option Methods
```rask
const opt = Some(5)
if opt.is_some() {          // ✅ opt still valid (borrows self)
    const val = opt!  // ✅ opt consumed (force unwrap)
}
```

---

## Appendix (non-normative)

### Rationale

Enums follow the same ownership rules as structs. Compiler infers whether bindings are borrowed or taken based on usage in each arm. For borrows, compiler infers read vs mutate. IDE shows these as ghost annotations. Exhaustiveness checked locally—all variants known from definition. Linear resources tracked through match arms, no silent drops.

**PM1 (mode inference):** Highest mode wins across all arms. If any arm takes, the whole match consumes. If any arm mutates (and none take), the match borrows mutably. Otherwise, immutable borrow. No mode annotations needed—the compiler figures it out.

**PM5 (wildcards forbidden for linear):** Wildcards on non-linear large payloads get a warning instead of an error:

| Payload Type | Wildcard Behavior |
|--------------|-------------------|
| Copy type | Allowed silently |
| Non-Copy, ≤64 bytes | Allowed silently |
| Non-Copy, >64 bytes | Warning: "wildcard discards large value (N bytes)" |

Use explicit `discard` to silence:
```rask
match result {
    Ok(discard) => {},  // Explicit acknowledgment
    Err(e) => handle(e)
}
```

### Patterns & Guidance

**Integration notes:**
- **Type system:** Enum variants participate in structural trait matching; explicit `extend` optional
- **Generics:** Bounds on `enum Foo<T: Bound>` checked at instantiation, applied to all variants
- **Collections:** Vec\<Enum\> and Map\<K, Enum\> allowed if enum is non-linear; Pool\<Enum\> allowed with manual linear cleanup
- **Concurrency:** Enums sent across channels transfer ownership; linear payloads remain tracked
- **C interop:** `@layout(C)` disables optimizations; discriminant size stable
- **Compiler:** Exhaustiveness checking and binding mode inference are local analysis only (no whole-program tracing)
- **Error handling:** `try` keyword requires linear resources resolved first; `ensure` provides cleanup guarantee (see [ensure.md](../control/ensure.md))
- **Tooling:** IDE displays inferred match modes, binding modes, discriminant values, enum sizes, and move/copy decisions as ghost annotations

### Verified Examples

<!-- test: run | 0\n1\n2 -->
```rask
for i in 0..3 {
    println("{i}")
}
```

### See Also

- `type.structs` — product types, `extend` blocks, value semantics
- `mem.ownership` — Copy/move rules, single-owner model
- `mem.resource-types` — linear resources, `ensure` cleanup
- `type.error-types` — `T or E`, `try` propagation
