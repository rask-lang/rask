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

```
{
    let file = open("data.txt")?
    ensure file.close()           // Scheduled, not executed yet

    let data = file.read()?       // If this fails...
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

```
{
    let a = open("a.txt")?
    ensure a.close()              // Runs second

    let b = open("b.txt")?
    ensure b.close()              // Runs first

    // use a and b
}
// Order: b.close(), then a.close()
```

This matches acquisition order—resources acquired last are released first.

### Linear Resource Integration

`ensure` satisfies linear consumption requirements:

```
fn process() -> Result<(), Error> {
    let file = open("data.txt")?  // file is linear
    ensure file.close()           // Compiler: file WILL be consumed

    let data = file.read()?       // Safe to use ? now
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

```
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

```
ensure file.close()?                       // ❌ Error: cannot use ? inside ensure
ensure file.close() catch |e| fallible()?  // ❌ Error: catch handler cannot use ?
```

**When to use explicit handling instead:**
```
// When cleanup errors actually matter (rare), don't use ensure:
fn write_important(data: Data) -> Result<(), Error> {
    let file = create("important.txt")?
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

```
fn process(a: File, b: File) -> Result<(), Error> {
    ensure a.close()

    let data = some_op()?     // ✅ Safe: a is ensured
                              // ❌ Error: b may leak on early return
}
```

### Nested Scopes

`ensure` is block-scoped, enabling precise lifetime control:

```
fn process() -> Result<(), Error> {
    let config = load_config()?

    {
        let file = open(config.path)?
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

```
ensure file.close()       // ✅ Valid
ensure println("done")    // ✅ Valid (side effect)
ensure file.read()?       // ❌ Invalid: ? in ensure
let x = ensure foo()      // ❌ Invalid: ensure doesn't return
```

### IDE Support

- IDE SHOULD show ensure execution points as ghost annotations at block end
- IDE SHOULD show LIFO order when multiple ensures exist
- IDE SHOULD highlight which linear resources are covered by ensure

```
{
    let a = open("a.txt")?
    ensure a.close()
    let b = open("b.txt")?
    ensure b.close()

    do_work()?
}                           // IDE ghost: [ensures: b.close(), a.close()]
```

## Examples

### File Processing
```
fn copy_file(src: string, dst: string) -> Result<(), Error> {
    let input = open(src)?
    ensure input.close()

    let output = create(dst)?
    ensure output.close()

    let data = input.read_all()?
    output.write_all(data)?
    Ok(())
}
```

### Database Transaction
```
fn transfer(db: Database, from: AccountId, to: AccountId, amount: i64) -> Result<(), Error> {
    let tx = db.begin()?
    ensure tx.rollback()      // Rollback if we don't commit

    let from_balance = tx.get_balance(from)?
    if from_balance < amount {
        return Err(InsufficientFunds)
    }

    tx.set_balance(from, from_balance - amount)?
    tx.set_balance(to, tx.get_balance(to)? + amount)?

    tx.commit()               // Consumes tx, cancels ensure
    Ok(())
}
```

### Ensure + Explicit Consumption Conflict

**Problem:** What if you `ensure` something but then consume it explicitly?

```
let tx = db.begin()?
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

```
let tx = db.begin()?
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
