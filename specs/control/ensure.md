<!-- depends: memory/resource-types.md, memory/ownership.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Solution: Scope-Exit Cleanup (`ensure`)

## The Question
How do we guarantee cleanup of linear resources without verbose manual handling on every exit path?

## Decision
Block-scoped `ensure` statement that schedules an expression to run when the enclosing block exits, regardless of how it exits (normal flow, early return, `try` propagation).

## Rationale
Linear resources must be consumed exactly once, but manual cleanup on every exit path is verbose and error-prone. `ensure` provides guaranteed cleanup with minimal ceremony while keeping cleanup visible (transparent costs). Block-scoped (not function-scoped) gives precise control over resource lifetime.

Name reads naturally: "ensure this happens before we leave this scope."

## Specification

### Basic Syntax

<!-- test: parse -->
```rask
func read_file(){
    const file = try open("data.txt")
    ensure file.close()           // Scheduled, not executed yet

    const data = try file.read()       // If this fails...
    try process(data)                // ...or this...
}                                 // file.close() runs HERE
```

### Semantics

| Property | Behavior |
|----------|----------|
| Execution timing | When enclosing block exits |
| Exit triggers | Normal flow, `return`, `try`, `break`, `continue` |
| Execution order | LIFO (last `ensure` runs first) |
| Scope | Block-scoped (not function-scoped) |

### LIFO Ordering

Multiple `ensure` statements run in reverse order (LIFO).

<!-- test: parse -->
```rask
func read_two_files(){
    const a = try open("a.txt")
    ensure a.close()              // Runs second

    const b = try open("b.txt")
    ensure b.close()              // Runs first

    // use a and b
}
// Order: b.close(), then a.close()
```

This matches acquisition order—resources acquired last are released first.

### Linear Resource Integration

`ensure` satisfies linear consumption requirements:

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try open("data.txt")   // file is linear
    ensure file.close()              // Compiler: file WILL be consumed

    const data = try file.read()        // Safe to use try now
    try transform(data)
    Ok(())
}
// Compiler accepts: file's consumption is guaranteed
```

**Rules:**
- `ensure` on linear resource counts as consumption commitment
- Compiler tracks that linear value will be consumed at scope exit
- `try` after `ensure` is safe—cleanup guaranteed

### Error Handling in `ensure`

What if the cleanup action itself fails?

**Decision: Ignore by default, opt-in handling with `else`**

<!-- test: skip -->
```rask
ensure file.close()                        // Default: errors silently ignored

ensure file.close() else |e| log(e)       // Opt-in: handle the error

ensure file.close() else |_| panic("!")   // Opt-in: panic on error
```

**Rationale:**
- Most cleanup errors are unrecoverable (what do you do when close() fails?)
- Resource released to OS regardless
- Silent ignore keeps simple cases simple
- `else` clause provides opt-in visibility when needed

**Rules:**
- `ensure` body returns `Result<T, E>` and evaluates to `Err(e)`:
  - Without `else`: error silently ignored
  - With `else |e| expr`: error passed to handler
- `else` handler must be infallible (no `try` inside—nowhere to propagate)
- `try` inside `ensure` body forbidden

<!-- test: skip -->
```rask
ensure { try file.close() }                     // ❌ Error: cannot use try inside ensure
ensure file.close() else |e| { try fallible() }   // ❌ Error: else handler cannot use try
```

**When to use explicit handling instead:**
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

### Interaction with Linear Tracking

| Scenario | Behavior |
|----------|----------|
| Linear resource with `ensure` | Consumption guaranteed, `try` allowed after |
| Linear resource without `ensure` | Standard rules: must consume before `try` or scope exit |
| Multiple linears, partial `ensure` | Only ensured ones safe; others require manual handling |

<!-- test: skip -->
```rask
func process(a: File, b: File) -> () or Error {
    ensure a.close()

    const data = try some_op()     // ✅ Safe: a is ensured
                              // ❌ Error: b may leak on early return
}
```

### Nested Scopes

`ensure` is block-scoped, enabling precise lifetime control.

<!-- test: parse -->
```rask
func process() -> () or Error {
    const config = try load_config()

    {
        const file = try open(config.path)
        ensure file.close()

        try process_file(file)
    }  // file.close() runs here

    // file is already closed, config still available
    log(config.summary)
    Ok(())
}
```

### What Cannot Be Ensured

| Case | Allowed? | Reason |
|------|----------|--------|
| Side-effect expression | ✅ | `ensure log("exiting")` |
| Linear consumption | ✅ | `ensure file.close()` |
| Fallible operation | ❌ | Cannot propagate error from cleanup |
| Value-returning expression | ❌ | Result is discarded |

<!-- test: skip -->
```rask
ensure file.close()       // ✅ Valid
ensure println("done")    // ✅ Valid (side effect)
ensure { try file.read() }   // ❌ Invalid: try in ensure
const x = ensure foo()    // ❌ Invalid: ensure doesn't return
```

### IDE Support

- IDE should show ensure execution points as ghost annotations at block end
- IDE should show LIFO order when multiple ensures exist
- IDE should highlight which linear resources are covered by ensure

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

## Examples

### File Processing
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

### Database Transaction
<!-- test: parse -->
```rask
func transfer(db: Database, from: AccountId, to: AccountId, amount: i64) -> () or Error {
    const tx = try db.begin()
    ensure tx.rollback()      // Rollback if we don't commit

    const from_balance = try tx.get_balance(from)
    if from_balance < amount {
        return Err(InsufficientFunds)
    }

    try tx.set_balance(from, from_balance - amount)
    const to_balance = try tx.get_balance(to)
    try tx.set_balance(to, to_balance + amount)

    tx.commit()               // Consumes tx, cancels ensure
    Ok(())
}
```

### Pool<Linear> Cleanup

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

**Note:** Errors during cleanup (e.g., close() fails) ignored in ensure block. If cleanup errors matter, don't use ensure—explicitly take_all before returning:

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

### Ensure + Explicit Consumption Conflict

What if you `ensure` something but then consume it explicitly?

<!-- test: skip -->
```rask
const tx = try db.begin()
ensure tx.rollback()    // Scheduled
// ...
tx.commit()             // Consumes tx
// At scope exit: tx.rollback() would use consumed tx!
```

**Solution:** Explicit consumption cancels ensure.

| Scenario | Behavior |
|----------|----------|
| `ensure` + scope exit | Ensure runs |
| `ensure` + explicit consumption | Ensure cancelled, explicit consumption wins |

Compiler tracks:
1. `ensure tx.rollback()` → tx consumed by rollback at scope exit
2. `tx.commit()` → tx consumed now, ensure void

<!-- test: skip -->
```rask
const tx = try db.begin()
ensure tx.rollback()        // IDE ghost: [cancelled if consumed]

// ... operations ...

tx.commit()                 // Consumes tx, cancels ensure
Ok(())
```

Scope exits early (before commit): rollback runs. Commit succeeds: rollback doesn't run.

Transaction pattern—ensure the unhappy path, explicitly handle the happy path.

## Integration Notes

- **Linear resource types:** `ensure` counts as consumption commitment; enables `try` after ensure
- **Error handling:** Errors ignored by default; use `else` clause for opt-in handling
- **Concurrency:** `ensure` runs on the task that owns the resource
- **Compiler:** Local analysis only—ensure tracked within function scope
- **Tooling:** IDE shows ensure execution points, cancellation status, and else clauses

## Alternatives Considered

| Alternative | Why Not |
|-------------|---------|
| Go-style `defer` (function-scoped) | Block-scoped is more precise |
| Python `with` (protocol-based) | Creates nesting, requires protocol |
| RAII/Drop (implicit) | Hides cleanup, violates transparent costs |
| Manual on every path | Too verbose, error-prone |
