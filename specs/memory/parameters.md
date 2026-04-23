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

Explicit mutable borrow. Function can modify the value; caller keeps ownership. This includes field mutation, method calls, **and full reassignment** of the parameter.

<!-- test: skip -->
```rask
func apply_damage(mutate player: Player, amount: i32) {
    player.health -= amount              // field mutation
    player.last_hit = now()              // field mutation
    if player.health <= 0 {
        player.status = Status.Dead      // field mutation
    }
}

func reset(mutate player: Player) {
    player = Player.new()                // full reassignment — allowed
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

## Disjoint Field Borrows

When passing `value.field` to a `mutate` parameter, the borrow checker tracks the borrow at field granularity. Functions take the field's concrete type — no special projection syntax needed.

<!-- test: skip -->
```rask
func heal(mutate health: Health) {
    health.current += 10
}

func loot(mutate inventory: Inventory) {
    inventory.push(item)
}

func update(mutate player: Player) {
    heal(player.health)         // Borrows player.health
    loot(player.inventory)      // OK: borrows player.inventory (disjoint)
}
```

See `mem.borrowing/F1`–`F4` for the full disjoint field borrowing rules.

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
| Disjoint field borrows | — | Passing `value.field` to `mutate` borrows only that field (`mem.borrowing/F1`) |

---

## Appendix (non-normative)

### Rationale

**PM1 (borrow default):** Borrowing is ~85% of parameters, and most borrows are read-only. I made the default read-only because mutation should be visible — if a function changes your data, you should see that in the signature.

**PM2 (mutate):** `mutate` marks intent: "I will change this parameter." This includes field-level modification AND full reassignment — there's no half-mutable state. A `mutate` parameter gives you unrestricted write access to the value; the constraint is that the caller keeps ownership after the call.

Note the interaction with `const` bindings: `const` is deep — you cannot pass a `const` binding as a `mutate` argument, call a `mutate self` method on it, or assign through an index/field. Rebinding and all forms of mutation through the binding name are rejected. If you need to pass a value to a `mutate` parameter, bind it with `mut`. Moving a const-bound value to a `take` parameter is allowed — ownership transfer is not mutation.

**PM3 (take):** The rare case. Ownership transfer only when you need to store, send, or consume.

**Why no call-site markers for `mutate`?** Swift (`&x`), C# (`ref x`), and Rue (`&x`) all require markers at call sites for mutable parameters. Three languages converging on the same choice deserves an answer for why Rask diverges.

The argument *for* call-site markers: mutation is a major effect. When reading `apply_damage(player, 10)` in a diff, you can't tell if `player` gets mutated without checking the signature. Diffs, terminal output, and code review tools don't have ghost annotations.

I chose against it for three reasons:

1. **Ceremony cost is per-call, not per-definition.** A `mutate` parameter is declared once but called many times. Adding markers to every call site trades one line of signature clarity for N lines of call-site noise. The signature is the contract — calls are uses of the contract.

2. **`own` already marks the destructive case.** Ownership transfer (`take`) is the dangerous one — after the call, your value is gone. That gets a call-site marker (`own`). Mutation is temporary — your value comes back, possibly changed. The asymmetry is intentional: mark the irreversible action, not the reversible one.

3. **IDE ghost annotations cover the readability gap.** The compiler knows which arguments are mutated. IDEs show `mutate` as ghost text at call sites. This gives you the information without the ceremony. The cost: code review outside IDEs loses this. I think that's acceptable — the signature is one jump away, and `mutate` in the signature is loud enough to notice.

This is a deliberate tradeoff, not an oversight. If real-world usage shows that hidden mutation at call sites causes bugs or confusion, call-site markers can be added without breaking existing code (they'd be optional annotations on existing syntax).

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

**Signatures:** All modes are visible in source — no ghost annotations needed.

**Call sites:** The IDE shows parameter modes as ghost text at each argument:

<!-- test: skip -->
```rask
apply_damage(player, 10)        // IDE shows: apply_damage(mutate player, 10)
consume(user)                   // IDE shows: consume(own user)  [already in source if take]
process(data)                   // IDE shows nothing (borrow is default, no annotation)
```

| Context | Ghost annotation |
|---------|-----------------|
| Borrow argument | None (default, no noise) |
| `mutate` argument | `mutate` ghost before argument |
| `take` argument | `own` ghost before argument (redundant if `own` already written) |

This bridges the gap between source-level simplicity and full visibility. In an IDE, mutation is always visible. In plain text (diffs, terminal), check the function signature.

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value`)
- [Linearity](linear.md) — Only `take` parameters can consume linear values (`mem.linear`)
- [Resource Types](resource-types.md) — `@resource` annotation (`mem.resources`)
- [Borrowing](borrowing.md) — Borrow scope rules (`mem.borrowing`)
- [Closures](closures.md) — Closure parameter modes (`mem.closures`)
- [Boxes](boxes.md) — Box parameters move ownership like any other value (`mem.boxes`)
- [Structs](../types/structs.md) — Struct definition, methods (`type.structs`)
