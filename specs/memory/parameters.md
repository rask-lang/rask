# Solution: Parameter Modes

## The Question
How are values passed to functions? When does the caller keep the value vs give it up?

## Decision
Three parameter modes: **borrow** (default, mutability inferred), **read** (explicit read-only), and **take** (ownership transfer).

## Rationale
Borrowing is far more common than ownership transfer (~85% of parameters). The default should be the common case with mutability inferred from usage.

However, explicit `read` provides three benefits:
1. **API contracts** — The read-only guarantee is visible in the signature
2. **Enforcement** — Compiler rejects any mutation, even accidental
3. **Concurrency** — Multiple `read` borrows can coexist (no aliasing XOR mutability conflict)

The `take` keyword marks the exceptional case of ownership transfer.

## Specification

### Parameter Modes

| Mode | Syntax | Meaning | Caller After |
|------|--------|---------|--------------|
| **Borrow** | `param: T` | Temporary access (mutability inferred) | Value still valid |
| **Read** | `read param: T` | Read-only access (enforced) | Value still valid |
| **Take** | `take param: T` | Ownership transfer | Value invalid |

### Borrow Mode (Default)

The default. Function temporarily accesses the value; caller keeps ownership.

```rask
func process(data: Data) -> Report {
    // Can read data.field
    // Can call data.method()
    // Cannot give data away (store, send, return)
    Report.from(data)
}

const d = Data.new()
process(d)      // d borrowed
print(d.name)   // ✅ OK: d still valid
```

**Mutability is inferred:**

```rask
func read_only(data: Data) {
    print(data.name)       // Only reads → inferred immutable
}

func mutates(data: Data) {
    data.count += 1        // Mutates → inferred mutable
}
```

The compiler analyzes the function body and infers:
- **Immutable borrow** if parameter is only read
- **Mutable borrow** if parameter is mutated

IDE shows inferred mutability as ghost annotation:
```rask
func mutates(data: Data) {   // ghost: [mutates data]
    data.count += 1
}
```

### Read Mode

Explicit read-only guarantee. Compiler enforces no mutation.

```rask
func validate(read user: User) -> bool {
    user.email.contains("@")  // ✅ OK: reading
    user.count += 1           // ❌ ERROR: cannot mutate read parameter
}
```

**Why use `read` over bare borrow?**

1. **Explicit contract** — Signature documents the guarantee
2. **Compiler enforcement** — Cannot accidentally add mutation
3. **Concurrent access** — Multiple `read` borrows can coexist

```rask
func process(data: Data) {
    // Both calls can happen concurrently because they only read
    validate(data)     // read borrow
    checksum(data)     // read borrow - OK, disjoint from validate
}

func validate(read data: Data) -> bool { ... }
func checksum(read data: Data) -> u32 { ... }
```

**Use `read` when:**
- The function should never mutate the parameter
- You want the guarantee visible in the API
- You want to enable concurrent read access

### Take Mode

Explicit ownership transfer. Caller gives up the value.

```rask
func consume(take data: Data) {
    // Can do anything with data
    // Including store it, send it, or drop it
    storage.store(data)
}

const d = Data.new()
consume(d)      // d taken
print(d.name)   // ❌ ERROR: d was taken
```

**Use `take` when:**
- Storing value in a struct field
- Inserting into a collection
- Sending through a channel
- Consuming a linear resource
- Returning a transformed version

### Self Parameter

Same rules apply to `self`:

| Syntax | Meaning |
|--------|---------|
| `self` | Borrow self (mutability inferred) |
| `read self` | Read-only self (enforced) |
| `take self` | Take ownership (consuming method) |

```rask
extend File {
    // Read-only: guarantees no mutation
    func size(read self) -> usize {
        self.metadata.size
    }

    // Inferred: may or may not mutate
    func read(self, buf: [u8]) -> Result<usize, Error> {
        // reads from self (mutates internal position)
    }

    // Consuming: can only call once
    func close(take self) -> Result<(), Error> {
        // closes and invalidates self
    }
}

const file = File.open("data.txt")?
file.read(buf)?    // borrows file
file.read(buf)?    // ✅ OK: can borrow again
file.close()?      // takes file
file.read(buf)?    // ❌ ERROR: file was taken
```

### Interaction with Copy Types

For Copy types (≤16 bytes, all fields Copy), the distinction is less visible because values are copied:

```rask
func process(x: i32) {
    // x is copied in, caller keeps original
}

func process(take x: i32) {
    // Also copied, but semantically "taken"
    // Useful for move-only small types
}
```

For non-Copy types, the distinction matters:
- Borrow: caller keeps value
- Take: caller loses value

### Interaction with Linear Types

Linear types must be consumed exactly once. Only `take` parameters can consume them:

```rask
@linear
struct File { ... }

func process(file: File) {        // Borrow
    file.read()?                // ✅ OK: reading
    // file must NOT be consumed here
}   // file returned to caller

func finish(take file: File) {    // Take
    file.close()?               // ✅ OK: consuming
}   // file consumed

const f = File.open(path)?
process(f)     // borrows f
finish(f)      // takes f, f now invalid
```

### Mutability Inference Rules

For bare borrows (no `read` keyword), mutability is inferred:

| Usage in Body | Inferred Mode |
|---------------|---------------|
| Only field reads | Immutable borrow |
| Only method calls with `read self` | Immutable borrow |
| Any field assignment | Mutable borrow |
| Any method call with mutating `self` | Mutable borrow |
| Passed to `take` parameter | Ownership transfer (requires `take`) |

For `read` parameters, the compiler enforces read-only access regardless of what the code tries to do.

**Inference is local:** The compiler only looks at the function body, not transitive calls.

```rask
func example(data: Data) {
    data.x = 5              // Assignment → mutable
    data.validate()         // If validate borrows → OK
}
```

### Error Messages

**Mutating a read parameter:**
```
ERROR: cannot mutate read-only parameter
   |
5  |  func update(read data: Data) {
   |              ^^^^ 'data' is read-only
6  |      data.count += 1
   |      ^^^^^^^^^^^^^^^ cannot assign to field of read parameter

HELP: Remove 'read' to allow mutation:
   |
5  |  func update(data: Data) {
```

**Taking a borrowed parameter:**
```
ERROR: cannot take ownership of borrowed parameter
   |
5  |  func process(data: Data) {
   |             ^^^^ 'data' is borrowed, not taken
6  |      storage.store(data)
   |                    ^^^^ 'store' takes ownership

HELP: Add 'take' to receive ownership:
   |
5  |  func process(take data: Data) {
```

**Using after taken:**
```
ERROR: value used after being taken
   |
5  |  consume(data)
   |          ^^^^ 'data' taken here
6  |  print(data.name)
   |        ^^^^ cannot use 'data' after it was taken
```

## IDE Integration

### Ghost Annotations

| Context | Annotation |
|---------|------------|
| Inferred mutable borrow | `[mutates param]` |
| Inferred immutable borrow | `[reads param]` (on hover) |
| Explicit `read` parameter | No ghost (already explicit in source) |
| Take parameter | `[takes param]` (explicit in source) |

### Hover Information

```rask
func process(data: Data)
           ^^^^
Parameter: Data (borrowed)
Inferred: mutable (assigned on line 8)

Caller's value remains valid after call.
```

## Examples

### Common Patterns

```rask
// Explicit read-only (guaranteed, visible in API)
func validate(read user: User) -> bool {
    user.email.contains("@")
}

// Inferred read-only (same effect, but not explicit)
func check(user: User) -> bool {
    user.active
}

// Mutating (borrow, mutable inferred)
func increment(counter: Counter) {
    counter.value += 1
}

// Storing (take required)
func register(take user: User) {
    users.insert(user.id, user)
}

// Consuming linear (take required)
func finish(take file: File) -> Result<(), Error> {
    file.close()
}
```

### Method Chains

```rask
extend Builder {
    // Borrow self: can chain
    func name(self, n: String) -> Self {
        self.name = n
        self
    }

    // Take self: ends chain
    func build(take self) -> Widget {
        Widget.new(self.name)
    }
}

Builder.new()
    .name("foo")      // borrows, returns self
    .name("bar")      // borrows, returns self
    .build()          // takes, returns Widget
```

### Projections (Partial Borrows)

Projections allow borrowing only specific fields of a struct, enabling disjoint borrows across function calls.

**Syntax:**
```rask
func heal(p: Player.{health}) {
    p.health += 10       // ✅ OK: health is projected
    p.inventory          // ❌ ERROR: not in projection
}

func loot(p: Player.{inventory}) {
    p.inventory.push(item)
}
```

**Disjoint borrows:**
```rask
func update(player: Player) {
    heal(player)         // Borrows player.health
    loot(player)         // ✅ OK: borrows player.inventory (disjoint)
}
```

**Multiple fields:**
```rask
func combat(p: Player.{health, mana}) {
    p.health -= damage
    p.mana -= spell_cost
}
```

| Rule | Description |
|------|-------------|
| Syntax | `Type.{field1, field2, ...}` |
| Disjoint | Non-overlapping projections can coexist |
| Mutability | Inferred from usage (same as regular borrows) |
| Scope | Only projected fields are accessible |
| Nesting | Nested fields: `Player.{stats.health}` (TBD) |

**Compiler tracking:**
The compiler tracks which fields each projection borrows. Two calls with disjoint projections can proceed independently, just like borrowing two different variables.

## Edge Cases

| Case | Handling |
|------|----------|
| Generic parameters | Mode applies to concrete type at instantiation |
| Closure captures | Captured borrows follow closure lifetime rules |
| Pattern matching | Each arm infers mode from usage; highest wins |
| Conditional mutation | If any branch mutates, inferred mutable |

## Integration Notes

- **Value Semantics:** Borrow/take builds on copy/move rules (see [value-semantics.md](value-semantics.md))
- **Linear Types:** Linear values require `take` for consumption (see [linear-types.md](linear-types.md))
- **Borrowing:** Parameter borrows follow block-scoped rules (see [borrowing.md](borrowing.md))
- **Closures:** Closure parameters use same modes (see [closures.md](closures.md))

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Linear Types](linear-types.md) — Must-consume resources
- [Borrowing](borrowing.md) — Borrow scope rules
