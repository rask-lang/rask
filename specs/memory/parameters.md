<!-- id: mem.parameters -->
<!-- status: decided -->
<!-- summary: Three parameter modes — borrow (default), mutate, take -->
<!-- depends: memory/ownership.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-parser/ -->

# Parameter Modes

Three modes: **borrow** (default, read-only), **mutate** (explicit mutable borrow), **take** (ownership transfer).

## Modes

| Rule | Mode | Syntax | Meaning | Caller After |
|------|------|--------|---------|--------------|
| **PM1: Borrow** | Borrow | `param: T` | Read-only access (enforced) | Value still valid |
| **PM2: Mutate** | Mutate | `mutate param: T` | Mutable access | Value still valid |
| **PM3: Take** | Take | `take param: T` | Ownership transfer | Value invalid |

### Borrow Mode (Default)

Function gets read-only access; caller keeps ownership. Compiler enforces immutability.

<!-- test: skip -->
```rask
func process(data: Data) -> Report {
    // Can read data.field, call data.method()
    // Cannot mutate data or give it away
    Report.from(data)
}

const d = Data.new()
process(d)      // d borrowed (read-only)
print(d.name)   // OK: d still valid
```

### Mutate Mode

Explicit mutable borrow. Function can modify the value; caller keeps ownership.

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

Ownership transfer. Caller gives up the value.

<!-- test: skip -->
```rask
func consume(take data: Data) {
    // Can do anything: store, send, drop
    storage.store(data)
}

const d = Data.new()
consume(d)      // d taken
print(d.name)   // ERROR: d was taken
```

## Self Parameter

| Syntax | Meaning |
|--------|---------|
| `self` | Read-only self (enforced) |
| `mutate self` | Mutable self |
| `take self` | Take ownership (consuming method) |

<!-- test: skip -->
```rask
extend File {
    func size(self) -> usize {
        self.metadata.size
    }

    func read(mutate self, buf: [u8]) -> usize or Error {
        // reads from self (mutates internal position)
    }

    func close(take self) -> () or Error {
        // closes and invalidates self
    }
}

const file = try File.open("data.txt")
try file.read(buf)    // mutably borrows file
try file.read(buf)    // OK: can borrow again
try file.close()      // takes file
try file.read(buf)    // ERROR: file was taken
```

## Projections (Partial Borrows)

Projections borrow only specific fields, enabling disjoint borrows across function calls.

| Rule | Description |
|------|-------------|
| **PM4: Projection syntax** | `Type.{field1, field2}` in function params accepts a projection |
| **PM5: Disjoint** | Non-overlapping projections can coexist |
| **PM6: Scope** | Only projected fields are accessible |

<!-- test: skip -->
```rask
func heal(mutate p: Player.{health}) {
    p.health += 10       // OK: health is projected
    p.inventory          // ERROR: not in projection
}

func loot(mutate p: Player.{inventory}) {
    p.inventory.push(item)
}

func update(mutate player: Player) {
    heal(player)         // Borrows player.health
    loot(player)         // OK: borrows player.inventory (disjoint)
}
```

## Interaction with Copy Types

For Copy types (≤16 bytes), values are copied in. The mode distinction matters for non-Copy types.

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

## Interaction with Resource Types

Resource types (`mem.resources/R1`) must be consumed exactly once. Only `take` parameters can consume them.

<!-- test: skip -->
```rask
@resource
struct File { ... }

func process(file: File) {         // Read-only borrow
    try file.read()              // OK: reading
}   // file returned to caller

func finish(take file: File) {     // Take
    try file.close()             // OK: consuming
}   // file consumed

const f = try File.open(path)
process(f)     // borrows f (read-only)
finish(f)      // takes f, f now invalid
```

## Error Messages

**Mutating a read-only parameter [PM1]:**
```
ERROR [mem.parameters/PM1]: cannot mutate parameter 'data'
   |
5  |  func update(data: Data) {
   |              ^^^^ 'data' is read-only (default)
6  |      data.count += 1
   |      ^^^^^^^^^^^^^^^ cannot assign to field of read-only parameter

FIX: Add 'mutate' to allow mutation:
   |
5  |  func update(mutate data: Data) {
```

**Taking a borrowed parameter [PM3]:**
```
ERROR [mem.parameters/PM3]: cannot take ownership of borrowed parameter
   |
5  |  func process(data: Data) {
   |             ^^^^ 'data' is borrowed, not taken
6  |      storage.store(data)
   |                    ^^^^ 'store' takes ownership

FIX: Add 'take' to receive ownership:
   |
5  |  func process(take data: Data) {
```

**Using after taken [PM3]:**
```
ERROR [mem.parameters/PM3]: value used after being taken
   |
5  |  consume(data)
   |          ^^^^ 'data' taken here
6  |  print(data.name)
   |        ^^^^ cannot use 'data' after it was taken
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Generic parameters | PM1–PM3 | Mode applies to concrete type at instantiation |
| Closure captures | — | Captured borrows follow closure lifetime rules (`mem.closures`) |
| Pattern matching | PM2 | Mutation only allowed if parameter is `mutate` |
| Copy type + mutate | PM2 | Value is copied in; mutations affect the copy |
| Nested fields in projection | PM4 | `Player.{stats.health}` (TBD) |

---

## Appendix (non-normative)

### Rationale

**PM1 (borrow default):** Borrowing is ~85% of parameters, and most borrows are read-only. I made the default read-only because mutation should be visible — if a function changes your data, you should see that in the signature.

**PM2 (mutate):** `mutate` marks intent: "I will change this parameter." Like `const`/`let` for bindings, it's about communicating intent, not frequency.

**PM3 (take):** The rare case. Ownership transfer only when you need to store, send, or consume.

### Patterns & Guidance

**Method chains:**

<!-- test: skip -->
```rask
extend Builder {
    func name(mutate self, n: string) -> Self {
        self.name = n
        self
    }

    func build(take self) -> Widget {
        Widget.new(self.name)
    }
}

Builder.new()
    .name("foo")      // mutably borrows, returns self
    .name("bar")      // mutably borrows, returns self
    .build()          // takes, returns Widget
```

### IDE Integration

| Context | Annotation |
|---------|------------|
| Default parameter | No ghost (read-only is explicit in source) |
| `mutate` parameter | No ghost (explicit in source) |
| Take parameter | No ghost (explicit in source) |

All parameter modes are visible in source text. Ghost annotations are not needed.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value`)
- [Resource Types](resource-types.md) — Must-consume resources (`mem.resources`)
- [Borrowing](borrowing.md) — Borrow scope rules (`mem.borrowing`)
- [Closures](closures.md) — Closure parameter modes (`mem.closures`)
- [Structs](../types/structs.md) — Projection type syntax (`type.structs`)
