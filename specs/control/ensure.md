# Solution: Scope-Exit Cleanup (`ensure`)

## The Question
How do we guarantee cleanup of linear resources without verbose manual handling on every exit path?

## Decision
Block-scoped `ensure` statement that schedules an expression to run when the enclosing block exits, regardless of how it exits (normal flow, early return, `?` propagation).

## Rationale
Linear resources must be consumed exactly once, but manual cleanup on every exit path is verbose and error-prone. `ensure` provides guaranteed cleanup with minimal ceremony while keeping the cleanup action visible (transparent costs). Block-scoped (not function-scoped) gives precise control over resource lifetime.

The name `ensure` reads naturally: "ensure this happens before we leave this scope."

## Specification

### Basic Syntax

<!-- test: parse -->
```rask
{
    const file = open("data.txt")?
    ensure file.close()           // Scheduled, not executed yet

    const data = file.read()?       // If this fails...
    process(data)?                // ...or this...
}                                 // file.close() runs HERE
```

### Semantics

| Property | Behavior |
|----------|----------|
| Execution timing | When enclosing block exits |
| Exit triggers | Normal flow, `return`, `?`, `break`, `continue` |
| Execution order | LIFO (last `ensure` runs first) |
| Scope | Block-scoped (not function-scoped) |

### LIFO Ordering

Multiple `ensure` statements run in reverse order (Last In, First Out):

<!-- test: parse -->
```rask
{
    const a = open("a.txt")?
    ensure a.close()              // Runs second

    const b = open("b.txt")?
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
func process() -> Result<(), Error> {
    const file = open("data.txt")?   // file is linear
    ensure file.close()              // Compiler: file WILL be consumed

    const data = file.read()?        // Safe to use ? now
    transform(data)?
    Ok(())
}
// Compiler accepts: file's consumption is guaranteed
```

**Rules:**
- `ensure` on linear resource counts as consumption commitment
- Compiler tracks that the linear value will be consumed at scope exit
- Using `?` after `ensure` is safe—cleanup is guaranteed

### Error Handling in `ensure`

What if the cleanup action itself fails?

**Decision: Ignore by default, opt-in handling with `catch`**

```rask
ensure file.close()                        // Default: errors silently ignored

ensure file.close() catch |e| log(e)       // Opt-in: handle the error

ensure file.close() catch |_| panic("!")   // Opt-in: panic on error
```

**Rationale:**
- Most cleanup errors are unrecoverable (what do you do when close() fails?)
- The resource IS released to the OS regardless
- Silent ignore keeps simple cases simple
- `catch` clause provides opt-in visibility when needed

**Rules:**
- If `ensure` body returns `Result<T, E>` and evaluates to `Err(e)`:
  - Without `catch`: error is silently ignored
  - With `catch |e| expr`: error passed to handler
- The `catch` handler must be infallible (no `?` inside—nowhere to propagate)
- `?` inside `ensure` body is forbidden

```rask
ensure file.close()?                        // ❌ Error: cannot use ? inside ensure
ensure file.close() catch |e| fallible()?   // ❌ Error: catch handler cannot use ?
```

**When to use explicit handling instead:**
```rask
// When cleanup errors actually matter (rare), don't use ensure:
func write_important(data: Data) -> Result<(), Error> {
    const file = create("important.txt")?
    file.write(data)?
    file.close()?                 // Explicit: propagate close error
    Ok(())
}
```

### Interaction with Linear Tracking

| Scenario | Behavior |
|----------|----------|
| Linear resource with `ensure` | Consumption guaranteed, `?` allowed after |
| Linear resource without `ensure` | Standard rules: must consume before `?` or scope exit |
| Multiple linears, partial `ensure` | Only ensured ones are safe; others still require manual handling |

```rask
func process(a: File, b: File) -> Result<(), Error> {
    ensure a.close()

    const data = some_op()?     // ✅ Safe: a is ensured
                              // ❌ Error: b may leak on early return
}
```

### Nested Scopes

`ensure` is block-scoped, enabling precise lifetime control:

```rask
func process() -> Result<(), Error> {
    const config = load_config()?

    {
        const file = open(config.path)?
        ensure file.close()

        process_file(file)?
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

```rask
ensure file.close()       // ✅ Valid
ensure println("done")    // ✅ Valid (side effect)
ensure file.read()?       // ❌ Invalid: ? in ensure
const x = ensure foo()    // ❌ Invalid: ensure doesn't return
```

### IDE Support

- IDE SHOULD show ensure execution points as ghost annotations at block end
- IDE SHOULD show LIFO order when multiple ensures exist
- IDE SHOULD highlight which linear resources are covered by ensure

```rask
{
    const a = open("a.txt")?
    ensure a.close()
    const b = open("b.txt")?
    ensure b.close()

    do_work()?
}                           // IDE ghost: [ensures: b.close(), a.close()]
```

## Examples

### File Processing
<!-- test: parse -->
```rask
func copy_file(src: string, dst: string) -> Result<(), Error> {
    const input = open(src)?
    ensure input.close()

    const output = create(dst)?
    ensure output.close()

    const data = input.read_all()?
    output.write_all(data)?
    Ok(())
}
```

### Database Transaction
```rask
func transfer(db: Database, from: AccountId, to: AccountId, amount: i64) -> Result<(), Error> {
    const tx = db.begin()?
    ensure tx.rollback()      // Rollback if we don't commit

    const from_balance = tx.get_balance(from)?
    if from_balance < amount {
        return Err(InsufficientFunds)
    }

    tx.set_balance(from, from_balance - amount)?
    tx.set_balance(to, tx.get_balance(to)? + amount)?

    tx.commit()               // Consumes tx, cancels ensure
    Ok(())
}
```

### Pool<Linear> Cleanup

Cleaning up pools of linear resources:

```rask
func process_many_files(paths: Vec<String>) -> Result<(), Error> {
    let files: Pool<File> = Pool.new()
    ensure files.take_all_with(|f| { f.close(); })

    for path in paths {
        const h = files.insert(File.open(path)?)?
        // ... use files[h] ...
    }

    // Normal exit: ensure takes and closes all files
    // Early return (error): ensure still takes and closes all files
    Ok(())
}
```

**Note:** Errors during cleanup (e.g., close() fails) are ignored in the ensure block. If cleanup errors matter, don't use ensure - explicitly take_all before returning:

```rask
func process_many_files_careful(paths: Vec<String>) -> Result<(), Error> {
    let files: Pool<File> = Pool.new()

    for path in paths {
        const h = files.insert(File.open(path)?)?
        // ... use files[h] ...
    }

    // Explicit take_all - propagate close errors
    for file in files.take_all() {
        file.close()?
    }
    Ok(())
}
```

### Ensure + Explicit Consumption Conflict

**Problem:** What if you `ensure` something but then consume it explicitly?

```rask
const tx = db.begin()?
ensure tx.rollback()    // Scheduled
// ...
tx.commit()             // Consumes tx
// At scope exit: tx.rollback() would use consumed tx!
```

**Solution:** Explicit consumption cancels the ensure.

| Scenario | Behavior |
|----------|----------|
| `ensure` + scope exit | Ensure runs |
| `ensure` + explicit consumption | Ensure cancelled, explicit consumption wins |

The compiler tracks:
1. `ensure tx.rollback()` → tx will be consumed by rollback at scope exit
2. `tx.commit()` → tx consumed now, ensure is void

```rask
const tx = db.begin()?
ensure tx.rollback()        // IDE ghost: [cancelled if consumed]

// ... operations ...

tx.commit()                 // Consumes tx, cancels ensure
Ok(())
```

If scope exits early (before commit), rollback runs. If commit succeeds, rollback doesn't run.

This is the "transaction pattern"—ensure the unhappy path, explicitly handle the happy path.

## Integration Notes

- **Linear types:** `ensure` counts as consumption commitment; enables `?` after ensure
- **Error handling:** Errors ignored by default; use `catch` clause for opt-in handling
- **Concurrency:** `ensure` runs on the task that owns the resource
- **Compiler:** Local analysis only—ensure tracked within function scope
- **Tooling:** IDE shows ensure execution points, cancellation status, and catch clauses

## Alternatives Considered

| Alternative | Why Not |
|-------------|---------|
| Go-style `defer` (function-scoped) | Block-scoped is more precise |
| Python `with` (protocol-based) | Creates nesting, requires protocol |
| RAII/Drop (implicit) | Hides cleanup, violates transparent costs |
| Manual on every path | Too verbose, error-prone |
