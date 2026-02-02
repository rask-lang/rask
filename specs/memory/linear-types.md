# Solution: Linear Types

## The Question
How do we ensure resources like files, connections, and locks are properly consumed (closed, released) before going out of scope?

## Decision
The `linear` keyword marks types that must be consumed exactly once. The compiler enforces consumption before scope exit. The `ensure` statement provides deferred consumption that satisfies the linear requirement.

## Rationale
Linear types prevent resource leaks by construction. Unlike RAII (which runs destructors automatically), linear types require explicit consumption—making cleanup visible (TC ≥ 0.90) while guaranteeing it happens (MC ≥ 0.90).

The `ensure` statement bridges linear types with error handling: you can commit to consuming a resource early, then use `?` freely knowing cleanup will happen.

## Specification

### Linear Type Declaration

```rask
@linear
struct File {
    handle: RawHandle,
    path: String,
}

@linear
struct Connection {
    socket: RawSocket,
    state: ConnectionState,
}
```

### Consumption Rules

| Rule | Description |
|------|-------------|
| **L1: Must consume** | Linear value must be consumed before scope exit |
| **L2: Consume once** | Cannot consume same linear value twice |
| **L3: Read allowed** | Can borrow for reading without consuming |
| **L4: `ensure` satisfies** | Registering with `ensure` counts as consumption commitment |

### Consuming Operations

A linear value is consumed by:
- Calling a method with `take self` (takes ownership)
- Passing to a function with `take` parameter mode
- Explicit consumption function (e.g., `file.close()`)

```rask
@linear
struct File { ... }

extend File {
    // Consuming method - takes ownership
    func close(take self) -> Result<(), Error> {
        // ... close logic ...
    }

    // Non-consuming - borrows (default)
    func read(self, buf: [u8]) -> Result<usize, Error> {
        // ... read logic ...
    }
}
```

### Basic Usage

```rask
func process() -> Result<(), Error> {
    const file = File.open("data.txt")?    // file is linear

    const data = file.read_all()?           // Borrow: file still valid
    process_data(data)?

    file.close()?                          // Consumed: file no longer valid
    Ok(())
}
```

**Forgetting to consume:**
```rask
func bad() -> Result<(), Error> {
    const file = File.open("data.txt")?
    const data = file.read_all()?
    Ok(())
    // ❌ ERROR: file not consumed before scope exit
}
```

**Double consumption:**
```rask
func also_bad() -> Result<(), Error> {
    const file = File.open("data.txt")?
    file.close()?
    file.close()?    // ❌ ERROR: file already consumed
    Ok(())
}
```

### The `ensure` Statement

`ensure` commits to consuming a linear resource at scope exit, satisfying L1 immediately.

```rask
func process() -> Result<(), Error> {
    const file = File.open("data.txt")?
    ensure file.close()        // Consumption committed

    const header = file.read_header()?    // ✅ OK: can still read
    validate(header)?                    // Can use ? freely

    const body = file.read_body()?
    transform(body)?

    Ok(())
    // ensure runs: file.close() called
}
```

**How `ensure` works:**

| Phase | What happens |
|-------|--------------|
| Registration | `ensure file.close()` marks `file` as "consumption committed" |
| During scope | `file` can be borrowed (read/mutate) but not consumed |
| Scope exit | `ensure` block runs, consuming `file` |

**Error handling in `ensure`:**

If the ensured operation returns `Result`, errors are:
1. Logged (in debug mode)
2. Accumulated if multiple ensures fail
3. Returned as the scope's error if no explicit return

```rask
func risky() -> Result<(), Error> {
    const file = File.open("data.txt")?
    ensure file.close()        // May fail

    risky_operation()?         // If this fails, ensure still runs

    Ok(())
}
// If risky_operation() fails: file.close() runs, then ? propagates
// If file.close() fails: that error is returned
```

### Linear + Error Paths

Linear types integrate with `?` through `ensure`:

```rask
func process(path: String) -> Result<Data, Error> {
    const file = File.open(path)?
    ensure file.close()        // Guarantees consumption on any exit

    const header = file.read_header()?  // Early return? ensure runs
    if !header.valid {
        return Err(InvalidHeader)      // ensure runs, file closed
    }

    const data = file.read_body()?      // Early return? ensure runs
    Ok(data)                           // Normal exit: ensure runs
}
```

**Without `ensure`, error handling is verbose:**
```rask
func process_verbose(path: String) -> Result<Data, Error> {
    const file = File.open(path)?

    const header = match file.read_header() {
        Ok(h) => h,
        Err(e) => {
            file.close()?     // Must close before returning
            return Err(e)
        }
    }

    // ... repeat for every ? ...

    file.close()?
    Ok(data)
}
```

### Linear in Collections

Linear types have restrictions in collections:

| Collection | Linear allowed? | Reason |
|------------|-----------------|--------|
| `Vec<Linear>` | ❌ No | Vec drop would need to consume each element |
| `Pool<Linear>` | ✅ Yes | Explicit removal required anyway |
| `Map<K, Linear>` | ❌ No | Map drop same problem as Vec |
| `Option<Linear>` | ✅ Yes | Must match and consume |

**Pool pattern for linear resources:**
```rask
const connections: Pool<Connection> = Pool.new()
const h = connections.insert(Connection.open(addr)?)?

// Later: explicit consumption required
const conn = connections.remove(h).unwrap()
conn.close()?
```

**All connections must be consumed before pool drops:**
```rask
for h in connections.handles() {
    const conn = connections.remove(h).unwrap()
    conn.close()?
}
// connections can now be dropped (empty)
```

### Pool<Linear> Drop Behavior

A `Pool<Linear>` MUST enforce consumption of all linear elements before the pool can be safely dropped.

**Rule L5: Pool Drop Enforcement**

| Scenario | Behavior |
|----------|----------|
| Pool is empty at drop | Normal drop, no action |
| Pool contains linear elements at drop | Runtime panic |

**Rationale:** The compiler cannot statically track the dynamic contents of a pool. Runtime enforcement is necessary to prevent silent resource leaks. A panic is preferable to a silent leak because:
- The program fails loudly rather than silently leaking resources
- Linear types' purpose (resource safety) is maintained
- The developer is immediately alerted to the bug

**The take_all pattern (REQUIRED for Pool<Linear>):**

```rask
const files: Pool<File> = Pool.new()
const h1 = files.insert(File.open("a.txt")?)?
const h2 = files.insert(File.open("b.txt")?)?

// Before allowing pool to drop, consume all elements:
for file in files.take_all() {
    file.close()?
}
// Pool is now empty, safe to drop
```

**With ensure (errors ignored):**

```rask
const files: Pool<File> = Pool.new()
ensure for file in files.take_all() {
    file.close()  // Result ignored - cannot use ? in ensure
}

// ... use files ...
// At scope exit: all files taken and closed
```

**With ensure and error handling:**

```rask
const files: Pool<File> = Pool.new()
ensure for file in files.take_all() {
    file.close()
} catch |e| log("Cleanup error: {}", e)
```

### Pool Helper Methods for Linear Types

When `T` is linear, `Pool<T>` provides additional convenience methods:

| Method | Signature | Description |
|--------|-----------|-------------|
| `take_all_with(f)` | `func(T) -> ()` | Take all and apply consuming function to each element |
| `take_all_with_result(f)` | `func(T) -> Result<(), E> -> Result<(), E>` | Take all with fallible consumer, stops on first error |

**Usage:**

```rask
// Ignore close errors
files.take_all_with(|f| { f.close(); })

// Propagate close errors
files.take_all_with_result(|f| f.close())?
```

### Why Runtime Panic for Pool<Linear> Drop?

**Q: Why not a compile error?**

The compiler would need to track whether a pool is empty at every point where it could be dropped. This requires:
- Escape analysis (pool passed to function that might not call take_all)
- Cross-function dataflow analysis
- Tracking dynamic insert/remove operations

This violates Rask's "local analysis only" principle (CS metric: no whole-program analysis).

**Q: Why not just leak the resources?**

This defeats the purpose of linear types. Linear types exist to guarantee resources are properly cleaned up. Silent leaks make the entire feature pointless.

**Q: Is a runtime panic "mechanical correctness"?**

The MC metric requires bugs be "impossible by construction." A runtime panic on `Pool<Linear>` drop is analogous to bounds checking:
- The bug (resource leak) is impossible - program terminates rather than leaking
- The mechanism is runtime, not compile-time
- This is acceptable per METRICS.md which lists bounds checks as "implicit OK"

### Panic Message

The panic message MUST clearly indicate:
1. That a Pool with linear elements was dropped while non-empty
2. The number of unconsumed elements
3. The element type

Example:
```rask
panic: Pool<File> dropped with 3 unconsumed linear elements.
Linear resources must be explicitly consumed (use take_all() before drop).
```

### Comparison with Other Mechanisms

| Mechanism | Cleanup | Visible? | Guaranteed? |
|-----------|---------|----------|-------------|
| RAII (Rust/C++) | Automatic in drop | ❌ Hidden | ✅ Yes |
| Manual (C) | Explicit call | ✅ Yes | ❌ No |
| GC finalizers | Eventual | ❌ Hidden | ❌ No |
| Linear types | Explicit + compiler | ✅ Yes | ✅ Yes |

Linear types are "visible RAII"—you see the cleanup, and the compiler guarantees it happens.

### Unique vs Linear

| Aspect | Unique (`@unique`) | Linear (`@linear`) |
|--------|--------------------|--------------------|
| Implicit copy | ❌ Disabled | ❌ Disabled |
| Can drop | ✅ Yes | ❌ No (must consume) |
| Explicit clone | ✅ Allowed | ❌ Not allowed |
| Use case | Semantic safety | Resource safety |
| Example | Unique ID | File handle |

Unique is "don't duplicate"; linear is "must properly close."

## Linear Resources in Error Types

When an operation fails, the linear resource must still be accounted for. The standard pattern is to return the resource in the error type.

### Basic Pattern

```rask
enum FileError {
    ReadFailed { file: File, reason: String },
    WriteFailed { file: File, reason: String },
}

func read_config(file: File) -> Result<Config, FileError> {
    const data = match file.read_all() {
        Ok(d) => d,
        Err(reason) => return Err(FileError.ReadFailed { file, reason }),
    }

    const config = parse(data)?
    file.close()?
    Ok(config)
}
```

**Caller must handle the file in error paths:**
```rask
func load_config(path: String) -> Result<Config, Error> {
    const file = File.open(path)?

    match read_config(file) {
        Ok(config) => Ok(config),
        Err(FileError.ReadFailed { file, reason }) => {
            file.close()?  // Must still consume the file
            Err(Error.ConfigRead(reason))
        }
        Err(FileError.WriteFailed { file, reason }) => {
            file.close()?
            Err(Error.ConfigWrite(reason))
        }
    }
}
```

### Multiple Linear Resources

When errors contain multiple linear resources, all must be consumed:

```rask
enum TransferError {
    SourceReadFailed {
        source: File,
        dest: File,
        reason: String
    },
    DestWriteFailed {
        source: File,
        dest: File,
        reason: String
    },
}

func handle_transfer_error(err: TransferError) -> Result<(), Error> {
    match err {
        TransferError.SourceReadFailed { source, dest, reason } => {
            source.close()?
            dest.close()?
            Err(Error.Transfer(reason))
        }
        TransferError.DestWriteFailed { source, dest, reason } => {
            source.close()?
            dest.close()?
            Err(Error.Transfer(reason))
        }
    }
}
```

### Simplifying with `ensure`

The `ensure` pattern reduces verbosity when the cleanup is the same:

```rask
func transfer(source_path: String, dest_path: String) -> Result<(), Error> {
    const source = File.open(source_path)?
    ensure source.close()

    const dest = File.create(dest_path)?
    ensure dest.close()

    // Now ? works naturally - both files cleaned up on any error
    const data = source.read_all()?
    dest.write_all(data)?

    Ok(())
}
```

**When to use each pattern:**

| Pattern | When to use |
|---------|-------------|
| Resource in error type | Caller needs the resource for recovery/retry |
| `ensure` | Cleanup is always the same (just close it) |
| Hybrid | Different cleanup depending on error type |

### Compound Errors with `ensure`

For multiple resources with uniform cleanup, `ensure` handles everything:

```rask
func process_files(paths: Vec<String>) -> Result<(), Error> {
    const files = Vec.new()

    for path in paths {
        const file = File.open(path)?
        ensure file.close()  // Each file gets its own ensure
        files.push(file)
    }

    // Process all files...
    for file in files.iter() {
        process(file)?
    }

    Ok(())
    // All ensures run in reverse order
}
```

### Error Type Design Guidelines

| Guideline | Rationale |
|-----------|-----------|
| Return resource if caller might retry | Caller can attempt recovery with same resource |
| Use `ensure` if cleanup is uniform | Less boilerplate, same safety |
| Document which errors contain resources | API clarity |
| Consider a `close_and_convert` helper | Reduces repetitive patterns |

**Helper pattern:**
```rask
extend FileError {
    func close_and_convert(take self) -> Result<Error, Error> {
        match self {
            FileError.ReadFailed { file, reason } => {
                file.close()?
                Ok(Error.Read(reason))
            }
            FileError.WriteFailed { file, reason } => {
                file.close()?
                Ok(Error.Write(reason))
            }
        }
    }
}

// Usage:
read_config(file).map_err(|e| e.close_and_convert())?
```

## Edge Cases

| Case | Handling |
|------|----------|
| Linear in error path | Must consume or register with `ensure` |
| Linear in error type | Caller must extract and consume from error |
| Linear across match arms | Each arm must consume (or share `ensure`) |
| Nested linear values | Each level must be consumed |
| Linear + panic | `ensure` runs during unwind |
| Conditional consumption | Both branches must consume |
| Loop with linear | Can't create linear in loop without consuming each iteration |

**Conditional consumption:**
```rask
func conditional(file: File, keep_open: bool) -> Result<(), Error> {
    if keep_open {
        GLOBAL_FILES.store(file)  // Consumes by transfer
    } else {
        file.close()?             // Consumes by close
    }
    // ✅ Both branches consume
    Ok(())
}
```

## Examples

### File Processing
```rask
func process_file(path: String) -> Result<Data, Error> {
    const file = File.open(path)?
    ensure file.close()

    const header = file.read_header()?
    const data = file.read_body()?

    Ok(data)
}
```

### Database Transaction
```rask
func update_user(db: Database, user_id: u64) -> Result<(), Error> {
    const txn = db.begin_transaction()?
    ensure txn.rollback()     // Default: rollback on error

    const user = txn.query_user(user_id)?
    user.last_login = now()
    txn.update_user(user)?

    txn.commit()?             // Explicit commit consumes txn
                              // ensure no longer needed (already consumed)
    Ok(())
}
```

### Connection Pool
```rask
func handle_connections(pool: Pool<Connection>) -> Result<(), Error> {
    // Process all connections
    for h in pool.cursor() {
        pool.modify(h, |conn| {
            if conn.should_close() {
                // Remove and consume
                const removed = pool.remove(h).unwrap()
                removed.close()?
            }
            Ok(())
        })?
    }

    // Clean up remaining
    for h in pool.handles().collect<Vec<_>>() {
        const conn = pool.remove(h).unwrap()
        conn.close()?
    }

    Ok(())
}
```

## Integration Notes

- **Value Semantics:** Linear types are move-only (never Copy) (see [value-semantics.md](value-semantics.md))
- **Ownership:** Linear adds consumption requirement on top of single ownership (see [ownership.md](ownership.md))
- **Error Handling:** `ensure` integrates with Result and `?` (see [ensure.md](../control/ensure.md))
- **Pools:** Pool<Linear> requires explicit removal (see [pools.md](pools.md))
- **Tooling:** IDE tracks linear value state, warns on missing consumption

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Ownership Rules](ownership.md) — Single-owner model
- [Ensure](../control/ensure.md) — Deferred execution
- [Pools](pools.md) — Handle-based storage for linear types
