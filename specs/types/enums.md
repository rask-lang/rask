# Solution: Sum Types and Pattern Matching

## The Question
How do algebraic data types (sum types/enums) work in Rask? Covers enum syntax, pattern matching exhaustiveness, and interaction with ownership.

## Decision
Tagged unions with inline payloads, compiler-inferred binding modes, compile-time exhaustiveness checking, and a single `match` keyword (no mode annotations).

## Rationale
Enums are values like structs, following the same ownership rules. The compiler infers whether pattern bindings are borrowed or taken based on how they're used in each arm—no explicit mode annotations needed. For borrowed bindings, the compiler further infers read vs mutate. This aligns with Principle 7 (Compiler Knowledge is Visible) => the compiler infers, the IDE displays inferred modes as ghost annotations. Exhaustiveness is checked locally (all variants known from definition). Linear resources tracked through match arms (no silent drops).

## Specification

### Enum Definition

Enums are tagged unions with inline variant storage.

<!-- test: parse -->
```rask
enum Name { A, B }                    // Simple tag-only
enum Name { A(T), B(U, V) }           // Variants with payloads  
enum Name<T> { Some(T), None }        // Generic enum
```

**Rules:**
- Compiler MUST allocate inline storage for largest variant
- Compiler MUST choose discriminant size automatically: u8 (≤256 variants), u16 (≤65536 variants)
- Compiler MUST reject enums with >65536 variants
- IDE SHOULD display discriminant size and total enum size as ghost annotation

### Value Semantics

| Property | Rule |
|----------|------|
| Inline storage | Variant payloads stored inline (no heap except `Owned<T>`) |
| Copy eligibility | Enum is Copy if total size ≤16 bytes AND all variants are Copy |
| Move semantics | Non-Copy enums move on assignment; source invalidated |
| Clone | Derived automatically if all variants implement Clone |

**Copy vs Move in Patterns:**

| Enum Type | `match x` Behavior | After Match |
|-----------|-------------------|-------------|
| Copy | Copies x into binding | x still valid |
| Non-Copy | Moves x into binding | x consumed |

IDE SHOULD show move vs copy at each match site.

### Pattern Matching

One keyword: `match`. The compiler infers binding modes from usage.

**Borrow (inferred when bindings only borrowed) =>**
```rask
match result {                      // IDE ghost: [borrows]
    Ok(value) => println(value),    // value: borrowed (inferred read)
    Err(error) => log(error)        // error: borrowed (inferred read)
}
// result still valid
```

**Borrow + mutate (inferred when any binding mutated) =>**
```rask
match connection {                          // IDE ghost: [mutates]
    Connected(sock) => sock.set_timeout(30),  // sock: borrowed (inferred mutate)
    _ => {}
}
// connection still valid, possibly modified
```

**Take (inferred when any binding passed to `take` parameter) =>**
```rask
match result {                      // IDE ghost: [takes]
    Ok(value) => consume(value),    // value: taken (inferred)
    Err(error) => handle(error)     // error: taken (inferred)
}
// result is consumed, invalid here
```

| Binding Usage | Inferred Mode | Source After |
|---------------|---------------|--------------|
| Only reads | Borrow (immutable) | Valid |
| Any mutation | Borrow (mutable) | Valid, may be modified |
| Any `take` parameter | Taken (moved) | Consumed |

**Mode inference rule:** Highest mode wins across all arms. If any arm takes, the whole match consumes. If any arm mutates (and none take), the match borrows mutably. Otherwise, immutable borrow.

**Rules:**
- Compiler MUST infer binding mode from usage in arm body
- Compiler MUST apply highest mode across all arms to the matched value
- IDE SHOULD display inferred mode as ghost annotation at match site
- IDE SHOULD display inferred binding modes as ghost annotations in patterns
- No mode annotations appear in source code

### Exhaustiveness Checking

Compiler verifies all variants handled using only local analysis (enum definition provides complete variant list).

| Condition | Compiler Behavior |
|-----------|-------------------|
| All variants matched | ✅ Valid |
| Missing variant, no wildcard | ❌ Error: "non-exhaustive match, missing `VariantName`" |
| Wildcard `_` present | ✅ Valid |
| Unreachable pattern | ⚠️ Warning: "unreachable pattern" |

Compiler MUST report which specific variants are unhandled.

### Pattern Guards

Conditional matching requires explicit catch-all to prevent hidden gaps.

```rask
match response {
    Ok(body) if body.len() > 0: process(body),
    Ok(body) => default(body),  // REQUIRED: catches guard failure
    Err(e) => handle(e)
}
```

| Rule | Enforcement |
|------|-------------|
| Guard on variant V | MUST have unguarded V arm OR wildcard after |
| No catch-all for guarded variant | ❌ Error: "pattern `V(_)` may not match when guard fails" |

```rask
// ❌ INVALID: guard may fail with no fallback
match response {
    Ok(body) if body.len() > 0: process(body),
    Err(e) => handle(e)
}
```

### Or-Patterns

Multiple patterns can share a single arm using `|` (or).

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

**Rules:**
- All alternatives MUST have the same type
- All alternatives MUST bind the same names with compatible types
- Or-patterns can be nested within other patterns
- Or-patterns work with guards: `A(x) | B(x) if x > 0 => ...`

| Pattern | Valid | Notes |
|---------|-------|-------|
| `A \| B => ...` | ✅ | Simple alternatives |
| `A(x) \| B(x) => use(x)` | ✅ | Both bind `x` with same type |
| `A(x) \| B(y) => ...` | ❌ | Different binding names |
| `A(x: i32) \| B(x: String) => ...` | ❌ | Incompatible types for `x` |
| `(A \| B, C \| D) => ...` | ✅ | Nested or-patterns |

**With Payloads:**
```rask
match result {
    Ok(0) | Err(0) => println("zero"),
    Ok(n) | Err(n) => println("value: {n}"),
}
```

**Linear Resources:**
Or-patterns with linear payloads follow the same rules—all alternatives must bind the linear value, and it must be consumed:

```rask
match file_result {
    Ok(file) | Recovered(file) => file.close(),
    Err(e) => log(e)
}
```

### Linear Resources in Enums

Enums may contain linear payloads (File, Socket, etc.).

| Rule | Enforcement |
|------|-------------|
| Wildcards forbidden | `_` on linear payload is compile error |
| All arms must bind | Each arm must name linear values |
| Bound value must be consumed | Standard linear tracking applies per arm |
| Must transfer | Compiler enforces consuming usage; read-only usage is compile error |

```rask
// ✅ VALID: linear value consumed in each arm
match file_result {                 // IDE ghost: [consumes]
    Ok(file) => file.close()?,      // file transferred to close()
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

### Guards with Linear Resources

When using pattern guards on variants containing linear resources, the catch-all arm must also bind and consume the resource:

```rask
// ✅ VALID: guard with linear resource — both arms consume
match file_result {
    Ok(file) if file.size() > 1000: process(file),  // Large files: process
    Ok(file) => file.close()?,                       // Small files: still must close
    Err(e) => log(e)
}

// ❌ INVALID: wildcard catch-all discards linear resource
match file_result {
    Ok(file) if file.size() > 1000: process(file),
    Ok(_) => {},                                     // Error: discards linear File
    Err(e) => log(e)
}
```

### Wildcard Warnings for Large Payloads

| Payload Type | Wildcard Behavior |
|--------------|-------------------|
| Copy type | Allowed silently |
| Non-Copy, ≤64 bytes | Allowed silently |
| Non-Copy, >64 bytes | ⚠️ Warning: "wildcard discards large value (N bytes)" |

Use explicit `discard` to silence:
```rask
match result {
    Ok(discard) => {},  // Explicit acknowledgment
    Err(e) => handle(e)
}
```

### Enum Methods

Methods are defined in `extend` blocks, separate from the enum definition. Methods default to non-consuming (borrow `self`).

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

    // Explicitly consuming
    func unwrap(take self) -> T {
        match self {               // IDE ghost: [consumes] (inferred from returning v)
            Some(v) => v,
            None => panic("unwrap on None")
        }
    }
}
```

| Self Mode | Behavior |
|-----------|----------|
| `self` (default) | Borrow (compiler infers read vs mutate) |
| `take self` | Consumes enum |

### Recursive Enums

Self-referential enums require explicit `Owned<T>` indirection. See [owned.md](../memory/owned.md) for full specification.

<!-- test: parse -->
```rask
enum Tree<T> {
    Leaf(T),
    Node(Owned<Tree<T>>, Owned<Tree<T>>)
}

const tree = Node(own Leaf(1), own Leaf(2))  // `own` = visible allocation
```

| Syntax | Meaning |
|--------|---------|
| `own expr` | Heap-allocate expr, return `Owned<T>` |
| `Owned<T>` | Owning heap pointer (linear) |

**Rules:**
- `own` keyword makes allocation visible at construction site
- `Owned<T>` is linear: must be consumed exactly once
- Drop deallocates automatically when consumed
- Compiler MUST reject recursive enum without indirection

### Discriminant Access

```rask
enum Status { Pending, Done, Failed }

const s = Done
const d = discriminant(s)  // 1 (zero-indexed)
```

| Attribute | Behavior |
|-----------|----------|
| None | Compiler MAY reorder variants for size optimization |
| `@layout(ordered)` | Discriminant values locked to declaration order |
| `@layout(C)` | C-compatible layout, no niche optimization |

Discriminant values assigned 0, 1, 2, ... in declaration order unless reordered.

**Function signature:**
```rask
func discriminant(e: T) -> u16 where T: Enum
```

### Null-Pointer Optimization

Compiler MUST apply niche optimization automatically when possible.

| Type | Representation |
|------|----------------|
| `Option<Owned<T>>` | Null = None, non-null = Some |
| `Option<Handle<T>>` | generation=0 = None, else Some |

`@layout(C)` disables niche optimization for ABI stability.

### Empty Enum (Never Type)

<!-- test: parse -->
```rask
enum Never {}  // Cannot be constructed
```

| Property | Behavior |
|----------|----------|
| Size | 0 bytes |
| Construction | Impossible (no variants) |
| Pattern match | No arms needed |
| `Result<T, Never>` | Err arm unreachable, compiler optimizes |

```rask
func infallible() -> Result<i32, Never> { Ok(42) }

const value = infallible().unwrap()  // Cannot panic (compiler knows)
```

### Error Propagation and Linear Resources

The `?` operator extracts Ok or returns early with Err.

**Rule:** All linear resources in scope MUST be resolved before `?`.

```rask
// ❌ INVALID: file2 may leak on early return
func process(file1: File, file2: File) -> Result<(), Error> {
    const data = file1.read()?  // file2 not consumed!
    file2.close()?
}
// Error: "linear resource `file2` may leak on early return at `?`"

// ✅ VALID: all linear resources resolved before `?`
func process(file1: File, file2: File) -> Result<(), Error> {
    const result1 = file1.read()
    const result2 = file2.close()
    const data = result1?
    result2?
    Ok(())
}
```

Alternative: use `ensure` for guaranteed cleanup:
```rask
func process(file1: File, file2: File) -> Result<(), Error> {
    ensure file1.close()  // Guaranteed at scope exit
    ensure file2.close()  // Runs on any exit
    const data = file1.read()?  // ✅ Safe: ensure registered
    Ok(())
}
```

### Edge Cases

| Case | Handling |
|------|----------|
| Empty enum | Zero-size, uninhabited, match needs no arms |
| Single-variant enum | Valid, discriminant may be optimized away |
| Zero-sized payload | `enum Foo { A(()), B }` — unit optimized to `{ A, B }` |
| >65536 variants | ❌ Compile error: "enum exceeds variant limit" |
| Nested linear | `Result<File, Error(File)>` — both arms bind linear, both must consume |
| Enum in Vec | Allowed if non-linear |
| Enum in Pool | Allowed; linear payloads require explicit `remove()` + consume |
| Option<File> in Pool | ❌ Error: "Pool cannot contain linear payloads" |
| Generic constraints | `enum Foo<T: Clone>` — constraint applies to ALL variants uniformly |
| Match on Copy type | Copies enum, mutations in arm affect copy not original |

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
    const val = opt.unwrap()  // ✅ opt consumed (take self)
}
```

## Integration Notes

- **Type system:** Enum variants participate in structural trait matching; explicit `extend` optional
- **Generics:** Bounds on `enum Foo<T: Bound>` checked at instantiation, applied to all variants
- **Collections:** Vec<Enum> and Map<K, Enum> allowed if enum is non-linear; Pool<Enum> allowed with manual linear cleanup
- **Concurrency:** Enums sent across channels transfer ownership; linear payloads remain tracked
- **C interop:** `@layout(C)` disables optimizations; discriminant size stable
- **Compiler:** Exhaustiveness checking and binding mode inference are local analysis only (no whole-program tracing)
- **Error handling:** `?` operator requires linear resources resolved first; `ensure` provides cleanup guarantee (see [ensure.md](../control/ensure.md))
- **Tooling:** IDE displays inferred match modes, binding modes, discriminant values, enum sizes, and move/copy decisions as ghost annotations