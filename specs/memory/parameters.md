<!-- depends: memory/ownership.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-parser/ -->

# Solution: Parameter Modes

## The Question
How are values passed to functions? When does the caller keep the value vs give it up?

## Decision
Three modes: **borrow** (default, read-only), **mutate** (explicit mutable borrow), **take** (ownership transfer).

## Rationale
Borrowing is 85% of parameters, and most borrows are read-only. I made the default read-only because mutation should be visible—if a function changes your data, you should see that in the signature, not buried in the body.

`mutate` marks the intent: "I will change this parameter." Like `const`/`let` for bindings, it's about communicating intent, not frequency.

`take` marks the rare case: ownership transfer.

## Specification

### Parameter Modes

| Mode | Syntax | Meaning | Caller After |
|------|--------|---------|--------------|
| **Borrow** | `param: T` | Read-only access (enforced) | Value still valid |
| **Mutate** | `mutate param: T` | Mutable access | Value still valid |
| **Take** | `take param: T` | Ownership transfer | Value invalid |

### Borrow Mode (Default)

The default. Function gets read-only access; caller keeps ownership. The compiler enforces that the function cannot mutate the parameter.

<!-- test: skip -->
```rask
func process(data: Data) -> Report {
    // Can read data.field
    // Can call data.method()
    // Cannot mutate data
    // Cannot give data away (store, send, return)
    Report.from(data)
}

const d = Data.new()
process(d)      // d borrowed (read-only)
print(d.name)   // ✅ OK: d still valid
```

**Read-only is enforced:**

<!-- test: skip -->
```rask
func validate(user: User) -> bool {
    user.email.contains("@")  // ✅ OK: reading
    user.count += 1           // ❌ ERROR: cannot mutate parameter
}
```

**Concurrent access:** Multiple read-only borrows can coexist:

<!-- test: skip -->
```rask
func process(data: Data) {
    // Both calls can happen concurrently because they only read
    validate(data)     // read-only borrow
    checksum(data)     // read-only borrow - OK, concurrent
}

func validate(data: Data) -> bool { ... }
func checksum(data: Data) -> u32 { ... }
```

**Connection to borrowing rules:** Parameter borrows are persistent for the function call duration. When accessing elements of a growable parameter (Vec, Pool, Map), those element views follow instant-view rules. See [borrowing.md](borrowing.md) for the "can it grow?" rule.

### Mutate Mode

Explicit mutable borrow. Function can modify the value; caller keeps ownership.

<!-- test: skip -->
```rask
func increment(mutate counter: Counter) {
    counter.value += 1     // ✅ OK: mutate allows modification
}

const c = Counter.new()
increment(c)        // c mutably borrowed
print(c.value)      // ✅ OK: c still valid (and modified)
```

**Use `mutate` when:**
- The function needs to change the parameter's fields
- The function calls methods that modify the parameter
- You want to make the mutation visible in the signature

<!-- test: skip -->
```rask
func apply_damage(mutate player: Player, amount: i32) {
    player.health -= amount
    player.last_hit = now()
    if player.health <= 0 {
        player.status = Status.Dead
    }
}
```

### Take Mode

Explicit ownership transfer. Caller gives up the value.

<!-- test: skip -->
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

Same modes apply to `self`:

| Syntax | Meaning |
|--------|---------|
| `self` | Read-only self (enforced) |
| `mutate self` | Mutable self |
| `take self` | Take ownership (consuming method) |

<!-- test: skip -->
```rask
extend File {
    // Read-only: guaranteed no mutation
    func size(self) -> usize {
        self.metadata.size
    }

    // Mutable: explicitly modifies self
    func read(mutate self, buf: [u8]) -> usize or Error {
        // reads from self (mutates internal position)
    }

    // Consuming: can only call once
    func close(take self) -> () or Error {
        // closes and invalidates self
    }
}

const file = try File.open("data.txt")
try file.read(buf)    // mutably borrows file
try file.read(buf)    // ✅ OK: can borrow again
try file.close()      // takes file
try file.read(buf)    // ❌ ERROR: file was taken
```

### Interaction with Copy Types

For Copy types (≤16 bytes, all fields Copy), the distinction is less visible because values are copied:

<!-- test: parse -->
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
- Borrow: caller keeps value (read-only)
- Mutate: caller keeps value (modified)
- Take: caller loses value

### Interaction with Linear Resource Types

Linear resource types must be consumed exactly once. Only `take` parameters can consume them:

<!-- test: skip -->
```rask
@resource
struct File { ... }

func process(file: File) {         // Read-only borrow
    try file.read()              // ✅ OK: reading
    // file must NOT be consumed here
}   // file returned to caller

func finish(take file: File) {     // Take
    try file.close()             // ✅ OK: consuming
}   // file consumed

const f = try File.open(path)
process(f)     // borrows f (read-only)
finish(f)      // takes f, f now invalid
```

### Error Messages

**Mutating a default (read-only) parameter:**
```
ERROR: cannot mutate parameter 'data'
   |
5  |  func update(data: Data) {
   |              ^^^^ 'data' is read-only (default)
6  |      data.count += 1
   |      ^^^^^^^^^^^^^^^ cannot assign to field of read-only parameter

HELP: Add 'mutate' to allow mutation:
   |
5  |  func update(mutate data: Data) {
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
| Default parameter | No ghost (read-only is explicit in source) |
| `mutate` parameter | No ghost (mutation is explicit in source) |
| Take parameter | No ghost (explicit in source) |

All parameter modes are now visible in source text. Ghost annotations are no longer needed for parameter mutability.

### Hover Information

<!-- test: skip -->
```rask
func process(data: Data)
           ^^^^
Parameter: Data (borrowed, read-only)

Caller's value remains valid after call.
```

## Examples

### Common Patterns

<!-- test: skip -->
```rask
// Read-only (default — most common)
func validate(user: User) -> bool {
    user.email.contains("@")
}

// Mutating (explicit intent)
func increment(mutate counter: Counter) {
    counter.value += 1
}

// Storing (take required)
func register(take user: User) {
    users.insert(user.id, user)
}

// Consuming linear (take required)
func finish(take file: File) -> () or Error {
    file.close()
}
```

### Method Chains

<!-- test: skip -->
```rask
extend Builder {
    // Mutate self: can chain
    func name(mutate self, n: string) -> Self {
        self.name = n
        self
    }

    // Take self: ends chain
    func build(take self) -> Widget {
        Widget.new(self.name)
    }
}

Builder.new()
    .name("foo")      // mutably borrows, returns self
    .name("bar")      // mutably borrows, returns self
    .build()          // takes, returns Widget
```

### Projections (Partial Borrows)

Projections allow borrowing only specific fields of a struct, enabling disjoint borrows across function calls.

**Syntax:**
<!-- test: skip -->
```rask
func heal(mutate p: Player.{health}) {
    p.health += 10       // ✅ OK: health is projected
    p.inventory          // ❌ ERROR: not in projection
}

func loot(mutate p: Player.{inventory}) {
    p.inventory.push(item)
}
```

**Disjoint borrows:**
<!-- test: skip -->
```rask
func update(mutate player: Player) {
    heal(player)         // Borrows player.health
    loot(player)         // ✅ OK: borrows player.inventory (disjoint)
}
```

**Multiple fields:**
<!-- test: skip -->
```rask
func combat(mutate p: Player.{health, mana}) {
    p.health -= damage
    p.mana -= spell_cost
}
```

| Rule | Description |
|------|-------------|
| Syntax | `Type.{field1, field2, ...}` |
| Disjoint | Non-overlapping projections can coexist |
| Mutability | Determined by `mutate` keyword (same as regular params) |
| Scope | Only projected fields are accessible |
| Nesting | Nested fields: `Player.{stats.health}` (TBD) |

**Compiler tracking:**
The compiler tracks which fields each projection borrows. Two calls with disjoint projections can proceed independently, just like borrowing two different variables.

## Edge Cases

| Case | Handling |
|------|----------|
| Generic parameters | Mode applies to concrete type at instantiation |
| Closure captures | Captured borrows follow closure lifetime rules |
| Pattern matching | Mutation only allowed if parameter is `mutate` |
| Copy type + mutate | Value is copied in; mutations affect the copy |

## Integration Notes

- **Value Semantics:** Borrow/take builds on copy/move rules (see [value-semantics.md](value-semantics.md))
- **Resource Types:** Resource values require `take` for consumption (see [resource-types.md](resource-types.md))
- **Borrowing:** Parameter borrows follow block-scoped rules (see [borrowing.md](borrowing.md))
- **Closures:** Closure parameters use same modes (see [closures.md](closures.md))

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Resource Types](resource-types.md) — Must-consume resources
- [Borrowing](borrowing.md) — Borrow scope rules
