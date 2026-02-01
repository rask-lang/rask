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

```
linear struct File {
    handle: RawHandle,
    path: String,
}

linear struct Connection {
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
- Calling a method with `transfer self` (takes ownership)
- Passing to a function with `transfer` parameter mode
- Explicit consumption function (e.g., `file.close()`)

```
linear struct File { ... }

impl File {
    // Consuming method - takes ownership
    fn close(transfer self) -> Result<(), Error> {
        // ... close logic ...
    }

    // Non-consuming - borrows only
    fn read(read self, buf: mut [u8]) -> Result<usize, Error> {
        // ... read logic ...
    }
}
```

### Basic Usage

```
fn process() -> Result<(), Error> {
    let file = File::open("data.txt")?    // file is linear

    let data = file.read_all()?           // Borrow: file still valid
    process_data(data)?

    file.close()?                          // Consumed: file no longer valid
    Ok(())
}
```

**Forgetting to consume:**
```
fn bad() -> Result<(), Error> {
    let file = File::open("data.txt")?
    let data = file.read_all()?
    Ok(())
    // ❌ ERROR: file not consumed before scope exit
}
```

**Double consumption:**
```
fn also_bad() -> Result<(), Error> {
    let file = File::open("data.txt")?
    file.close()?
    file.close()?    // ❌ ERROR: file already consumed
    Ok(())
}
```

### The `ensure` Statement

`ensure` commits to consuming a linear resource at scope exit, satisfying L1 immediately.

```
fn process() -> Result<(), Error> {
    let file = File::open("data.txt")?
    ensure file.close()        // Consumption committed

    let header = file.read_header()?    // ✅ OK: can still read
    validate(header)?                    // Can use ? freely

    let body = file.read_body()?
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

```
fn risky() -> Result<(), Error> {
    let file = File::open("data.txt")?
    ensure file.close()        // May fail

    risky_operation()?         // If this fails, ensure still runs

    Ok(())
}
// If risky_operation() fails: file.close() runs, then ? propagates
// If file.close() fails: that error is returned
```

### Linear + Error Paths

Linear types integrate with `?` through `ensure`:

```
fn process(path: String) -> Result<Data, Error> {
    let file = File::open(path)?
    ensure file.close()        // Guarantees consumption on any exit

    let header = file.read_header()?  // Early return? ensure runs
    if !header.valid {
        return Err(InvalidHeader)      // ensure runs, file closed
    }

    let data = file.read_body()?      // Early return? ensure runs
    Ok(data)                           // Normal exit: ensure runs
}
```

**Without `ensure`, error handling is verbose:**
```
fn process_verbose(path: String) -> Result<Data, Error> {
    let file = File::open(path)?

    let header = match file.read_header() {
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
```
let connections: Pool<Connection> = Pool::new()
let h = connections.insert(Connection::open(addr)?)?

// Later: explicit consumption required
let conn = connections.remove(h).unwrap()
conn.close()?
```

**All connections must be consumed before pool drops:**
```
for h in connections.handles() {
    let conn = connections.remove(h).unwrap()
    conn.close()?
}
// connections can now be dropped (empty)
```

### Comparison with Other Mechanisms

| Mechanism | Cleanup | Visible? | Guaranteed? |
|-----------|---------|----------|-------------|
| RAII (Rust/C++) | Automatic in drop | ❌ Hidden | ✅ Yes |
| Manual (C) | Explicit call | ✅ Yes | ❌ No |
| GC finalizers | Eventual | ❌ Hidden | ❌ No |
| Linear types | Explicit + compiler | ✅ Yes | ✅ Yes |

Linear types are "visible RAII"—you see the cleanup, and the compiler guarantees it happens.

### Move-Only vs Linear

| Aspect | Move-only (`move struct`) | Linear (`linear struct`) |
|--------|---------------------------|--------------------------|
| Implicit copy | ❌ Disabled | ❌ Disabled |
| Can drop | ✅ Yes | ❌ No (must consume) |
| Explicit clone | ✅ Allowed | ❌ Not allowed |
| Use case | Semantic safety | Resource safety |
| Example | Unique ID | File handle |

Move-only is "don't duplicate"; linear is "must properly close."

## Linear Resources in Error Types

When an operation fails, the linear resource must still be accounted for. The standard pattern is to return the resource in the error type.

### Basic Pattern

```
enum FileError {
    ReadFailed { file: File, reason: String },
    WriteFailed { file: File, reason: String },
}

fn read_config(file: File) -> Result<Config, FileError> {
    let data = match file.read_all() {
        Ok(d) => d,
        Err(reason) => return Err(FileError::ReadFailed { file, reason }),
    }

    let config = parse(data)?
    file.close()?
    Ok(config)
}
```

**Caller must handle the file in error paths:**
```
fn load_config(path: String) -> Result<Config, Error> {
    let file = File::open(path)?

    match read_config(file) {
        Ok(config) => Ok(config),
        Err(FileError::ReadFailed { file, reason }) => {
            file.close()?  // Must still consume the file
            Err(Error::ConfigRead(reason))
        }
        Err(FileError::WriteFailed { file, reason }) => {
            file.close()?
            Err(Error::ConfigWrite(reason))
        }
    }
}
```

### Multiple Linear Resources

When errors contain multiple linear resources, all must be consumed:

```
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

fn handle_transfer_error(err: TransferError) -> Result<(), Error> {
    match err {
        TransferError::SourceReadFailed { source, dest, reason } => {
            source.close()?
            dest.close()?
            Err(Error::Transfer(reason))
        }
        TransferError::DestWriteFailed { source, dest, reason } => {
            source.close()?
            dest.close()?
            Err(Error::Transfer(reason))
        }
    }
}
```

### Simplifying with `ensure`

The `ensure` pattern reduces verbosity when the cleanup is the same:

```
fn transfer(source_path: String, dest_path: String) -> Result<(), Error> {
    let source = File::open(source_path)?
    ensure source.close()

    let dest = File::create(dest_path)?
    ensure dest.close()

    // Now ? works naturally - both files cleaned up on any error
    let data = source.read_all()?
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

```
fn process_files(paths: Vec<String>) -> Result<(), Error> {
    let mut files = Vec::new()

    for path in paths {
        let file = File::open(path)?
        ensure file.close()  // Each file gets its own ensure
        files.push(file)
    }

    // Process all files...
    for file in &files {
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
```
impl FileError {
    fn close_and_convert(self) -> Result<Error, Error> {
        match self {
            FileError::ReadFailed { file, reason } => {
                file.close()?
                Ok(Error::Read(reason))
            }
            FileError::WriteFailed { file, reason } => {
                file.close()?
                Ok(Error::Write(reason))
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
```
fn conditional(file: File, keep_open: bool) -> Result<(), Error> {
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
```
fn process_file(path: String) -> Result<Data, Error> {
    let file = File::open(path)?
    ensure file.close()

    let header = file.read_header()?
    let data = file.read_body()?

    Ok(data)
}
```

### Database Transaction
```
fn update_user(db: Database, user_id: u64) -> Result<(), Error> {
    let txn = db.begin_transaction()?
    ensure txn.rollback()     // Default: rollback on error

    let user = txn.query_user(user_id)?
    user.last_login = now()
    txn.update_user(user)?

    txn.commit()?             // Explicit commit consumes txn
                              // ensure no longer needed (already consumed)
    Ok(())
}
```

### Connection Pool
```
fn handle_connections(pool: mut Pool<Connection>) -> Result<(), Error> {
    // Process all connections
    for h in pool.cursor() {
        pool.modify(h, |conn| {
            if conn.should_close() {
                // Remove and consume
                let removed = pool.remove(h).unwrap()
                removed.close()?
            }
            Ok(())
        })?
    }

    // Clean up remaining
    for h in pool.handles().collect::<Vec<_>>() {
        let conn = pool.remove(h).unwrap()
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
