<!-- id: ctrl.ensure -->
<!-- status: decided -->
<!-- summary: Block-scoped scope-exit cleanup with LIFO ordering -->
<!-- depends: memory/resource-types.md, memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Ensure

Block-scoped `ensure` statement schedules an expression to run when the enclosing block exits, regardless of how it exits (normal flow, early return, `try` propagation). Multiple `ensure` statements run in LIFO order.

| Rule | Description |
|------|-------------|
| **EN1: Block-scoped** | Executes when enclosing block exits, not function |
| **EN2: LIFO ordering** | Multiple ensure statements run in reverse order (last scheduled runs first) |
| **EN3: Linear consumption commitment** | `ensure` on linear resource counts as consumption guarantee |
| **EN4: Errors ignored by default** | If ensure body returns `Result` and fails, error silently ignored |
| **EN5: try forbidden in ensure** | Cannot use `try` inside ensure body or else handler |
| **EN6: Explicit consumption cancels ensure** | If value is consumed before scope exit, ensure is void |

## Basic Usage

<!-- test: parse -->
```rask
func read_file(){
    const file = try open("data.txt")
    ensure file.close()           // Scheduled, not executed yet

    const data = try file.read()       // If this fails...
    try process(data)                // ...or this...
}                                 // file.close() runs HERE
```

## Exit Triggers

| Trigger | Behavior |
|---------|----------|
| **EX1: Normal flow** | Block completes normally |
| **EX2: Early return** | `return` statement exits function |
| **EX3: Error propagation** | `try` propagates error up |
| **EX4: Loop control** | `break` or `continue` exits loop block |

## LIFO Ordering

Multiple `ensure` statements run in reverse order—resources acquired last are released first.

<!-- test: parse -->
```rask
func read_two_files(){
    const a = try open("a.txt")
    ensure a.close()              // Runs second (EN2)

    const b = try open("b.txt")
    ensure b.close()              // Runs first (EN2)

    // use a and b
}
// Order: b.close(), then a.close()
```

## Linear Resource Integration

`ensure` satisfies linear consumption requirements—once scheduled, the value is guaranteed to be consumed at scope exit.

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try open("data.txt")   // file is linear
    ensure file.close()              // EN3: file WILL be consumed

    const data = try file.read()        // Safe to use try now
    try transform(data)
    Ok(())
}
// Compiler accepts: file's consumption is guaranteed
```

| Rule | Description |
|------|-------------|
| **L1: After ensure, try is safe** | Linear resource with `ensure` allows `try` in same scope |
| **L2: Without ensure, standard rules apply** | Must consume before `try` or scope exit |
| **L3: Partial ensure** | Only ensured linears are safe; others require manual handling |

<!-- test: skip -->
```rask
func process(a: File, b: File) -> () or Error {
    ensure a.close()

    const data = try some_op()     // ✅ Safe: a is ensured (L1)
                              // ❌ Error: b may leak on early return (L2)
}
```

## Error Handling in Ensure

Cleanup actions may fail. Errors are ignored by default; opt-in handling with `else` clause.

| Rule | Description |
|------|-------------|
| **ER1: Default ignore** | If ensure body returns `Err(e)`, error silently ignored |
| **ER2: Opt-in else clause** | `ensure expr else \|e\| handler` passes error to handler |
| **ER3: Infallible handler** | `else` handler must not use `try`—nowhere to propagate |
| **ER4: try forbidden** | Cannot use `try` inside ensure body |

<!-- test: skip -->
```rask
ensure file.close()                        // ER1: errors silently ignored

ensure file.close() else |e| log(e)       // ER2: handle the error

ensure file.close() else |_| panic("!")   // ER2: panic on error

ensure { try file.close() }                     // ❌ Error: ER4
ensure file.close() else |e| { try fallible() }   // ❌ Error: ER3
```

**When cleanup errors matter, use explicit handling instead:**

<!-- test: parse -->
```rask
// When cleanup errors actually matter (rare), don't use ensure:
func write_important(data: Data) -> () or Error {
    const file = try create("important.txt")
    try file.write(data)
    try file.close()                 // Explicit: propagate close error
    Ok(())
}
```

## Nested Scopes

`ensure` is block-scoped, enabling precise lifetime control.

<!-- test: parse -->
```rask
func process() -> () or Error {
    const config = try load_config()

    {
        const file = try open(config.path)
        ensure file.close()

        try process_file(file)
    }  // file.close() runs here (EN1)

    // file is already closed, config still available
    log(config.summary)
    Ok(())
}
```

## Explicit Consumption Cancellation

If a value is consumed explicitly before scope exit, the ensure is cancelled.

| Rule | Description |
|------|-------------|
| **C1: Explicit consumption wins** | If value consumed before scope exit, ensure doesn't run |
| **C2: Cancellation tracked** | Compiler tracks consumption and voids the ensure |

<!-- test: skip -->
```rask
const tx = try db.begin()
ensure tx.rollback()    // Scheduled

// ... operations ...

tx.commit()             // Consumes tx, cancels ensure (C1)
Ok(())
```

**Transaction pattern:** Ensure the unhappy path (rollback), explicitly handle the happy path (commit).

<!-- test: parse -->
```rask
func transfer(db: Database, from: AccountId, to: AccountId, amount: i64) -> () or Error {
    const tx = try db.begin()
    ensure tx.rollback()      // Rollback if we don't commit

    const from_balance = try tx.get_balance(from)
    if from_balance < amount {
        return Err(InsufficientFunds)  // ensure runs: rollback
    }

    try tx.set_balance(from, from_balance - amount)
    const to_balance = try tx.get_balance(to)
    try tx.set_balance(to, to_balance + amount)

    tx.commit()               // Consumes tx, cancels ensure (C1)
    Ok(())
}
```

## Pool Cleanup

Cleaning up pools of linear resources:

<!-- test: skip -->
```rask
func process_many_files(paths: Vec<string>) -> () or Error {
    let files: Pool<File> = Pool.new()
    ensure files.take_all_with(|f| { f.close(); })

    for path in paths {
        const file = try File.open(path)
        const h = try files.insert(file)
        // ... use files[h] ...
    }

    // Normal exit: ensure takes and closes all files
    // Early return (error): ensure still takes and closes all files
    Ok(())
}
```

Errors during cleanup (e.g., `close()` fails) are ignored by default (ER1). If cleanup errors matter, explicitly `take_all` before returning:

<!-- test: skip -->
```rask
func process_many_files_careful(paths: Vec<string>) -> () or Error {
    let files: Pool<File> = Pool.new()

    for path in paths {
        const file = try File.open(path)
        const h = try files.insert(file)
        // ... use files[h] ...
    }

    // Explicit take_all - propagate close errors
    for file in files.take_all() {
        try file.close()
    }
    Ok(())
}
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Multiple ensures | EN2 | Run in LIFO order (last scheduled runs first) |
| Ensure on already-consumed value | — | Compile error: value not available |
| Ensure in nested blocks | EN1 | Each ensure runs when its enclosing block exits |
| Ensure + explicit consumption | C1 | Explicit consumption cancels ensure |
| Ensure body panics | — | Panic propagates, subsequent ensures don't run |
| Ensure body returns value | — | Value discarded (ensure is statement, not expression) |
| Nested ensures (ensure inside ensure) | — | Forbidden—ensure is statement, not block |

## Error Messages

**Using `try` inside ensure body [ER4]:**
```
ERROR [ctrl.ensure/ER4]: cannot use try inside ensure
   |
5  |  ensure { try file.close() }
   |           ^^^ try forbidden in ensure body

WHY: Ensure handlers run during scope exit—there's nowhere to propagate errors.

FIX: Remove try and optionally handle errors with else clause:

  // Ignore errors (default)
  ensure file.close()

  // Handle errors
  ensure file.close() else |e| log(e)
```

**Using `try` in else handler [ER3]:**
```
ERROR [ctrl.ensure/ER3]: cannot use try in else handler
   |
5  |  ensure file.close() else |e| { try log(e) }
   |                                  ^^^ try forbidden in else handler

WHY: Else handlers run during scope exit—there's nowhere to propagate errors.

FIX: Use infallible operations in else handler:

  ensure file.close() else |e| println("Failed: {}", e)
```

**Linear resource not consumed [L2]:**
```
ERROR [ctrl.ensure/L2]: linear resource may leak on error propagation
   |
3  |  const file = try open("data.txt")
   |        ^^^^ linear resource created
4  |  const data = try file.read()
   |               ^^^ try may exit early, leaving file unconsumed

WHY: Linear resources must be consumed on all paths. try can exit early,
     skipping normal consumption.

FIX: Use ensure to guarantee cleanup:

  const file = try open("data.txt")
  ensure file.close()         // Now safe
  const data = try file.read()
```

**Ensure on non-linear without else [ER1]:**
```
NOTE [ctrl.ensure/ER1]: ensure result ignored
   |
5  |  ensure file.close()
   |         ^^^^^^^^^^^^ returns Result<(), Error>, error will be ignored

WHY: Ensure errors are silently ignored by default.

FIX: Add else clause to handle errors:

  ensure file.close() else |e| log("Close failed: {}", e)

  // Or explicitly handle errors without ensure
  try file.close()
```

---

## Appendix (non-normative)

### Rationale

**EN1 (block-scoped):** Block-scoped gives precise control over resource lifetime. Function-scoped (Go's `defer`) is less precise—you might want to release a resource mid-function. Block scoping matches Rask's lexical scoping model.

**EN2 (LIFO ordering):** Resources acquired last should be released first—matches acquisition order. If you open file A, then file B, you should close B before A (B might depend on A). LIFO is the natural order.

**EN3 (linear consumption commitment):** Linear resources must be consumed exactly once. `ensure` commits to consumption at scope exit, allowing the compiler to accept `try` after the ensure. Without this, every `try` after acquiring a linear resource would be an error.

**EN4 (errors ignored by default):** Most cleanup errors are unrecoverable. What do you do when `close()` fails? The OS has already released the resource. Silent ignore keeps simple cases simple (no ceremony). The `else` clause provides opt-in visibility for the rare cases where cleanup errors matter.

**EN5 (try forbidden in ensure):** Ensure runs during scope exit—there's nowhere to propagate errors. Forbidding `try` makes this explicit. If you need fallible cleanup, use explicit handling (not ensure).

**EN6 (explicit consumption cancels ensure):** Transaction pattern: ensure rollback, explicitly commit. If commit succeeds, rollback shouldn't run. Explicit consumption cancels the ensure—compiler tracks this.

Name choice: "ensure" reads naturally—"ensure this happens before we leave this scope." Considered `defer` (Go), `finally` (Java), `scope(exit)` (D). "Ensure" emphasizes the guarantee, not the timing.

### Patterns & Guidance

**Pattern 1: File I/O**

<!-- test: parse -->
```rask
func copy_file(src: string, dst: string) -> () or Error {
    const input = try open(src)
    ensure input.close()

    const output = try create(dst)
    ensure output.close()

    const data = try input.read_all()
    try output.write_all(data)
    Ok(())
}
```

Both files guaranteed to close on any exit path.

**Pattern 2: Transaction (ensure + explicit consumption)**

<!-- test: skip -->
```rask
func modify_database(db: Database) -> () or Error {
    const tx = try db.begin()
    ensure tx.rollback()    // Ensures unhappy path

    try tx.execute("UPDATE ...")
    try tx.execute("INSERT ...")

    tx.commit()             // Happy path: consumes tx, cancels ensure
    Ok(())
}
```

**Pattern 3: Pool cleanup**

<!-- test: skip -->
```rask
func process_many(paths: Vec<string>) -> () or Error {
    let resources: Pool<Resource> = Pool.new()
    ensure resources.take_all_with(|r| { r.cleanup(); })

    for path in paths {
        const r = try Resource.open(path)
        resources.insert(r)
    }

    // Process resources...
    Ok(())
}
```

**When NOT to use ensure:**

1. **Cleanup errors matter:** Use explicit `try file.close()` to propagate errors
2. **Conditional cleanup:** Use `if` + explicit consumption
3. **Cross-task cleanup:** Ensure runs on the owning task—use channels for cross-task coordination

### IDE Integration

IDE shows ensure execution points as ghost annotations at block end.

<!-- test: skip -->
```rask
{
    const a = try open("a.txt")
    ensure a.close()
    const b = try open("b.txt")
    ensure b.close()

    try do_work()
}                           // IDE ghost: [ensures: b.close(), a.close()]
```

Hover information shows:
- Which ensures will run at this block exit
- LIFO order
- Cancellation status (if value consumed explicitly)
- Whether errors are handled (else clause present)

### Concurrency Integration

Ensure runs on the task that owns the resource. If a resource is sent to another task, the ensure is cancelled (ownership transferred).

<!-- test: skip -->
```rask
func process() {
    const file = try open("data.txt")
    ensure file.close()

    channel.send(file)      // Transfers ownership, cancels ensure
}
```

Compiler error if you try to send a value with active ensure—must consume or cancel first.

### See Also

- [Resource Types](../memory/resource-types.md) — Linear resources (`mem.resources`)
- [Ownership Rules](../memory/ownership.md) — Single-owner model (`mem.ownership`)
- [Error Handling](../types/error-types.md) — Result types and `try` (`type.errors`)
