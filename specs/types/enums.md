<!-- id: type.enums -->
<!-- status: decided -->
<!-- summary: Tagged unions with positional or named payloads, inferred binding modes, exhaustive matching -->
<!-- depends: types/structs.md -->
<!-- implemented-by: compiler/crates/rask-parser/, compiler/crates/rask-types/ -->

# Enums

Tagged unions with positional or named payloads. Compiler infers binding modes. Exhaustiveness checked at compile time. One `match` keyword, no mode annotations.

## Enum Definition

<!-- test: parse -->
```rask
enum Name { A, B }                    // Simple tag-only
enum Name { A(T), B(U, V) }           // Positional payloads
enum Name { A { x: T, y: U } }       // Named payloads
enum Maybe<T> { Present(T), Missing } // Generic enum
enum Name: u8 { A = 0, B = 1 }       // Explicit discriminants (E14-E18)
```

| Rule | Description |
|------|-------------|
| **E1: Inline storage** | Variant payloads stored inline (no heap except `Owned<T>`) |
| **E2: Discriminant sizing** | Auto-sized: u8 (≤256 variants), u16 (≤65536 variants) |
| **E3: Max variants** | Maximum 65536 variants per enum |

## Positional vs Named Payloads

| Rule | Description |
|------|-------------|
| **E12: Positional variants** | `Variant(T)` or `Variant(T, U)` — fields accessed by position in patterns |
| **E13: Named variants** | `Variant { name: T }` — fields accessed by name in patterns |
| **E14: No mixing** | A single variant is either positional or named, not both |

<!-- test: parse -->
```rask
// Positional: clean for wrappers, single payloads, obvious types
enum Token {
    Number(f64),
    Ident(string),
    Plus,
    Minus,
}

// Named: when fields have distinct roles
enum Shape {
    Circle { center: Point, radius: f32 },
    Rect { origin: Point, width: f32, height: f32 },
}

// Mix within one enum is fine — each variant chooses independently
enum Event {
    Click(Point),                         // positional: one obvious payload
    Resize { width: i32, height: i32 },   // named: two fields with distinct roles
    Quit,                                  // no payload
}
```

**Pattern matching follows the variant form.** Inside `match`, the compiler infers the enum type from the subject, so variant names are unqualified. Qualified names (`Token.Plus`) also work but are unnecessary when the type is known from context.

```rask
match token {
    Number(n) => process(n),         // positional: bind by position
    Ident(s) => lookup(s),           // positional
    Plus => ...,                     // unqualified (idiomatic)
    Token.Minus => ...,              // qualified (also valid)
}

match shape {
    Circle { radius, .. } => area(radius),     // named: bind by name
    Rect { width, height, .. } => width * height,
}

match event {
    Click(pos) => handle_click(pos),
    Resize { width, height } => resize(width, height),
    Quit => break,
}
```

**Guidance:** Use positional for 0-1 payload or when the type makes the meaning obvious. Use named when there are 2+ fields with distinct roles.

## Value Semantics

| Rule | Description |
|------|-------------|
| **E4: Copy eligibility** | Enum is Copy if total size ≤16 bytes AND all variants are Copy |
| **E5: Move semantics** | Non-Copy enums move on assignment; source invalidated |

Cloneable derived automatically if all variants implement Cloneable.

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
match msg {                         // IDE ghost: [borrows]
    Data(value) => println(value),  // value: borrowed (inferred read)
    Fault(error) => log(error)      // error: borrowed (inferred read)
}
// msg still valid
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
match msg {                         // IDE ghost: [takes]
    Data(value) => consume(value),  // value: taken (inferred)
    Fault(error) => handle(error)   // error: taken (inferred)
}
// msg is consumed, invalid here
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
    Loaded(body) if body.len() > 0: process(body),
    Loaded(body) => default(body),  // REQUIRED: catches guard failure
    Failed(e) => handle(e)
}
```

| Guard Condition | Enforcement |
|-----------------|-------------|
| Guard on variant V | Must have unguarded V arm OR wildcard after |
| No catch-all for guarded variant | Error: "pattern `V(_)` may not match when guard fails" |

```rask
// ❌ INVALID: guard may fail with no fallback
match response {
    Loaded(body) if body.len() > 0: process(body),
    Failed(e) => handle(e)
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
match msg {
    Data(0) | Fault(0) => println("zero"),
    Data(n) | Fault(n) => println("value: {n}"),
}
```

## Linear Resources in Enums

Enums may contain linear payloads (File, Socket, etc.).

| Rule | Description |
|------|-------------|
| **PM5: Wildcards forbidden for linear** | `_` on linear payload is compile error |
| **PM6: All arms must bind** | Each arm must name linear values; bound value must be consumed |

User enum `FileOpen { Opened(File), Failed(IoError) }`:

```rask
// ✅ VALID: linear value consumed in each arm
match file_result {                      // IDE ghost: [consumes]
    Opened(file) => try file.close(),    // file transferred to close()
    Failed(e) => return e
}

// ❌ INVALID: wildcard discards linear File
match file_result {
    Opened(_) => {},
    Failed(e) => {}
}
// Error: "wildcard pattern discards linear resource `File`"

// ❌ INVALID: read-only usage of linear resource
match file_result {
    Opened(file) => println(file.path),  // Only reads file
    Failed(e) => log(e)
}
// Error: "linear resource `file` must be consumed, not just read"
```

**Or-patterns with linear payloads** follow the same rules—all alternatives must bind the linear value, and it must be consumed:

```rask
match file_result {
    Opened(file) | Recovered(file) => file.close(),
    Failed(e) => log(e)
}
```

**Guards with linear resources** — catch-all arm must also bind and consume:

```rask
// ✅ VALID: guard with linear resource — both arms consume
match file_result {
    Opened(file) if file.size() > 1000: process(file),  // Large files: process
    Opened(file) => try file.close(),                    // Small files: still must close
    Failed(e) => log(e)
}

// ❌ INVALID: wildcard catch-all discards linear resource
match file_result {
    Opened(file) if file.size() > 1000: process(file),
    Opened(_) => {},                                     // Error: discards linear File
    Failed(e) => log(e)
}
```

## Error Propagation and Linear Resources

`try` extracts the success branch or returns early with the error.

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
}
```

Alternative: use `ensure` for guaranteed cleanup:
```rask
func process(file1: File, file2: File) -> () or Error {
    ensure file1.close()  // Guaranteed at scope exit
    ensure file2.close()  // Runs on any exit
    const data = try file1.read()  // ✅ Safe: ensure registered
}
```

## Enum Methods

Methods in `extend` blocks, separate from definition. Default to non-consuming (borrow `self`).

> Note: builtin `T?` (Option) cannot be pattern-matched and isn't a user enum; use its operator surface (`?`, `!`, `??`). See [optionals.md](optionals.md). The example below defines a user enum with a similar shape.

<!-- test: parse -->
```rask
enum Maybe<T> {
    Present(T),
    Missing,
}

extend Maybe<T> {
    // Default: borrows self (compiler infers read vs mutate)
    func has_value(self) -> bool {
        match self {               // IDE ghost: [reads] (inferred from wildcard-only usage)
            Present(_) => true,
            Missing => false
        }
    }

    // Explicitly consuming
    func force(take self) -> T {
        match self {               // IDE ghost: [consumes] (inferred from returning v)
            Present(v) => v,
            Missing => panic("force on Missing")
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
| **E9: Discriminant access** | `discriminant(e)` returns the variant's discriminant value |
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

For enums with explicit backing types (E14), the return type matches the backing type instead of `u16`.

| Attribute | Behavior |
|-----------|----------|
| (none) | Compiler MAY reorder variants for size optimization |
| `@layout(ordered)` | Discriminant values locked to declaration order |
| `@layout(C)` | C-compatible layout, no niche optimization |

**Null-pointer optimization:**

| Type | Representation |
|------|----------------|
| `Owned<T>?` | null pointer = absent, non-null = present |
| `Handle<T>?` | generation=0 = absent, else present |

## Explicit Discriminants

Fieldless enums can declare a backing type and assign specific values to variants. For wire formats, serialization, C interop.

<!-- test: parse -->
```rask
enum ObjectKind: u8 {
    Reserved = 0,
    String = 1,
    Array = 2,
    Map = 3,
    Struct = 4,
    Enum = 5,
}
```

| Rule | Description |
|------|-------------|
| **E14: Backing type** | `enum Foo: T { ... }` sets the discriminant representation. `T` is any integer type. Omit for default sizing (E2) |
| **E15: Explicit values** | `Variant = N` assigns a discriminant value. Values must be unique and fit the backing type |
| **E16: All or none** | If any variant has `= N`, all must. No mixing explicit and auto-indexed within one enum |
| **E17: No payloads** | Enums with explicit values cannot have payload variants. `Variant(T) = 1` is a compile error |
| **E18: Integer cast** | `e as i64` extracts the discriminant. Works on any fieldless enum, not just explicit ones |

**Integer cast** works on all fieldless enums — auto-indexed or explicit:

<!-- test: parse -->
```rask
// Explicit values
enum ObjectKind: u8 {
    Reserved = 0,
    String = 1,
}
const tag = ObjectKind.String as u8   // 1
const wide = ObjectKind.String as i64 // 1

// Auto-indexed (zero-based declaration order)
enum Color { Red, Green, Blue }
Color.Blue as i64  // 2
```

Enums with payloads cannot use `as` integer cast — compile error.

**Backing type** controls representation size and valid range:

<!-- test: skip -->
```rask
enum Opcode: u8 { Add = 6, Sub = 7 }      // fits in 1 byte
enum Port: u16 { Http = 80, Https = 443 }  // fits in 2 bytes
enum Signal: i32 { Hup = 1, Kill = 9 }     // C-compatible signed
```

When a backing type is specified, variant reordering is disabled (implies `@layout(ordered)`). The compiler stores the enum using exactly the specified type.

**Constructing from integer** for deserialization:

<!-- test: skip -->
```rask
const kind: ObjectKind? = ObjectKind.from_value(1)  // ObjectKind.String
const bad: ObjectKind? = ObjectKind.from_value(99)  // none
```

`from_value` is auto-generated for all fieldless enums. Returns optional — invalid values produce `none`.

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
| `T or Never` | error branch uninhabited, compiler optimizes |

```rask
func infallible() -> i32 or Never { return 42 }

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
| Nested linear | PM6 | `File or FileError(File)` — both arms bind linear, both must consume |
| Enum in Vec | E5 | Allowed if non-linear |
| Enum in Pool | E5 | Allowed; linear payloads require explicit `remove()` + consume |
| `File?` in Pool | PM5 | Error: "Pool cannot contain linear payloads" |
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
            Connecting(addr) => {
                const attempt = try_connect(addr)
                if attempt? as sock { Connected(sock) } else as e { Failed(e) }
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

### Option
```rask
const opt: i32? = 5
if opt? {           // ✅ opt still valid, narrowed to i32 in block
    const val = opt!   // ✅ force-extract
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
match msg {
    Data(discard) => {},  // Explicit acknowledgment
    Fault(e) => handle(e)
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
