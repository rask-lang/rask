<!-- depends: memory/ownership.md, control/ensure.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Solution: Resource Types

## The Question
How do we ensure resources like files, connections, and locks are properly consumed before going out of scope?

## Decision
`@resource` marks types that must be consumed exactly once. Compiler enforces it. `ensure` provides deferred consumption.

## Rationale
Resource types prevent leaks by construction. Unlike RAII (automatic destructors), resources need explicit consumption—cleanup is visible while still guaranteed.

`ensure` bridges resources with error handling: commit to cleanup early, then use `try` freely knowing it'll happen.

## Specification

### Resource Type Declaration

<!-- test: parse -->
```rask
@resource
struct File {
    handle: RawHandle
    path: string
}

@resource
struct Connection {
    socket: RawSocket
    state: ConnectionState
}
```

### Consumption Rules

| Rule | Description |
|------|-------------|
| **R1: Must consume** | Resource value must be consumed before scope exit |
| **R2: Consume once** | Cannot consume same resource value twice |
| **R3: Read allowed** | Can borrow for reading without consuming |
| **R4: `ensure` satisfies** | Registering with `ensure` counts as consumption commitment |

### Consuming Operations

A resource value is consumed by:
- Calling a method with `take self` (takes ownership)
- Passing to a function with `take` parameter mode
- Explicit consumption function (e.g., `file.close()`)

<!-- test: skip -->
```rask
@resource
struct File { ... }

extend File {
    // Consuming method - takes ownership
    func close(take self) -> () or Error {
        // ... close logic ...
    }

    // Non-consuming - borrows (default)
    func read(self, buf: [u8]) -> usize or Error {
        // ... read logic ...
    }
}
```

### Basic Usage

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try File.open("data.txt")    // file is a resource

    const data = try file.read_all()           // Borrow: file still valid
    try process_data(data)

    try file.close()                          // Consumed: file no longer valid
    Ok(())
}
```

**Forgetting to consume:**
<!-- test: compile-fail -->
```rask
func bad() -> () or Error {
    const file = try File.open("data.txt")
    const data = try file.read_all()
    Ok(())
    // ❌ ERROR: file not consumed before scope exit
}
```

**Double consumption:**
<!-- test: compile-fail -->
```rask
func also_bad() -> () or Error {
    const file = try File.open("data.txt")
    try file.close()
    try file.close()    // ❌ ERROR: file already consumed
    Ok(())
}
```

### The `ensure` Statement

`ensure` commits to consuming a resource at scope exit, satisfying R1 immediately.

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try File.open("data.txt")
    ensure file.close()        // Consumption committed

    const header = try file.read_header()    // ✅ OK: can still read
    try validate(header)                    // Can use try freely

    const body = try file.read_body()
    try transform(body)

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

<!-- test: parse -->
```rask
func risky() -> () or Error {
    const file = try File.open("data.txt")
    ensure file.close()        // May fail

    try risky_operation()         // If this fails, ensure still runs

    Ok(())
}
// If risky_operation() fails: file.close() runs, then try propagates
// If file.close() fails: that error is returned
```

### Resource Types + Error Paths

Resource types integrate with `try` through `ensure`:

<!-- test: parse -->
```rask
func process(path: string) -> Data or Error {
    const file = try File.open(path)
    ensure file.close()        // Guarantees consumption on any exit

    const header = try file.read_header()  // Early return? ensure runs
    if !header.valid {
        return Err(InvalidHeader)      // ensure runs, file closed
    }

    const data = try file.read_body()      // Early return? ensure runs
    Ok(data)                           // Normal exit: ensure runs
}
```

**Without `ensure`, error handling is verbose:**
<!-- test: parse -->
```rask
func process_verbose(path: string) -> Data or Error {
    const file = try File.open(path)

    const header = match file.read_header() {
        Ok(h) => h,
        Err(e) => {
            try file.close()     // Must close before returning
            return Err(e)
        }
    }

    // ... repeat for every try ...

    try file.close()
    Ok(data)
}
```

### Resources in Collections

Resource types have restrictions in collections:

| Collection | Resource allowed? | Reason |
|------------|-------------------|--------|
| `Vec<Resource>` | ❌ No | Vec drop would need to consume each element |
| `Pool<Resource>` | ✅ Yes | Explicit removal required anyway |
| `Map<K, Resource>` | ❌ No | Map drop same problem as Vec |
| `Option<Resource>` | ✅ Yes | Must match and consume |

**Pool pattern for resources:**
<!-- test: skip -->
```rask
const connections: Pool<Connection> = Pool.new()
const h = try connections.insert(try Connection.open(addr))

// Later: explicit consumption required
const conn = connections.remove(h).unwrap()
try conn.close()
```

**All connections must be consumed before pool drops:**
<!-- test: skip -->
```rask
for h in connections.handles() {
    const conn = connections.remove(h).unwrap()
    try conn.close()
}
// connections can now be dropped (empty)
```

### Pool<Resource> Drop Behavior

A `Pool<Resource>` MUST enforce consumption of all resource elements before the pool can be safely dropped.

**Rule R5: Pool Drop Enforcement**

| Scenario | Behavior |
|----------|----------|
| Pool is empty at drop | Normal drop, no action |
| Pool contains resource elements at drop | Runtime panic |

**Rationale:** The compiler cannot statically track the dynamic contents of a pool. Runtime enforcement is necessary to prevent silent resource leaks. A panic is preferable to a silent leak because:
- The program fails loudly rather than silently leaking resources
- Resource types' purpose (resource safety) is maintained
- The developer is immediately alerted to the bug

**The take_all pattern (REQUIRED for Pool<Resource>):**

<!-- test: skip -->
```rask
const files: Pool<File> = Pool.new()
const h1 = try files.insert(try File.open("a.txt"))
const h2 = try files.insert(try File.open("b.txt"))

// Before allowing pool to drop, consume all elements:
for file in files.take_all() {
    try file.close()
}
// Pool is now empty, safe to drop
```

**With ensure (errors ignored):**

<!-- test: skip -->
```rask
const files: Pool<File> = Pool.new()
ensure for file in files.take_all() {
    file.close()  // Result ignored - cannot use try in ensure
}

// ... use files ...
// At scope exit: all files taken and closed
```

**With ensure and error handling:**

<!-- test: skip -->
```rask
const files: Pool<File> = Pool.new()
ensure for file in files.take_all() {
    file.close()
} else |e| log("Cleanup error: {}", e)
```

### Pool Helper Methods for Resource Types

When `T` is a resource type, `Pool<T>` provides additional convenience methods:

| Method | Signature | Description |
|--------|-----------|-------------|
| `take_all_with(f)` | `func(T) -> ()` | Take all and apply consuming function to each element |
| `take_all_with_result(f)` | `func(T) -> () or E -> () or E` | Take all with fallible consumer, stops on first error |

**Usage:**

<!-- test: skip -->
```rask
// Ignore close errors
files.take_all_with(|f| { f.close(); })

// Propagate close errors
try files.take_all_with_result(|f| f.close())
```

### Why Runtime Panic for Pool<Resource> Drop?

**Q: Why not a compile error?**

The compiler would need to track whether a pool is empty at every point where it could be dropped. This requires:
- Escape analysis (pool passed to function that might not call take_all)
- Cross-function dataflow analysis
- Tracking dynamic insert/remove operations

This violates Rask's "local analysis only" principle (CS metric: no whole-program analysis).

**Q: Why not just leak the resources?**

This defeats the purpose of resource types. Resource types exist to guarantee resources are properly cleaned up. Silent leaks make the entire feature pointless.

**Q: Is a runtime panic "mechanical correctness"?**

The MC metric requires bugs be "impossible by construction." A runtime panic on `Pool<Resource>` drop is analogous to bounds checking:
- The bug (resource leak) is impossible - program terminates rather than leaking
- The mechanism is runtime, not compile-time
- This is acceptable per METRICS.md which lists bounds checks as "implicit OK"

### Panic Message

The panic message MUST clearly indicate:
1. That a Pool with resource elements was dropped while non-empty
2. The number of unconsumed elements
3. The element type

Example:
<!-- test: skip -->
```rask
panic: Pool<File> dropped with 3 unconsumed resource elements.
Resources must be explicitly consumed (use take_all() before drop).
```

### Comparison with Other Mechanisms

| Mechanism | Cleanup | Visible? | Guaranteed? |
|-----------|---------|----------|-------------|
| RAII (Rust/C++) | Automatic in drop | ❌ Hidden | ✅ Yes |
| Manual (C) | Explicit call | ✅ Yes | ❌ No |
| GC finalizers | Eventual | ❌ Hidden | ❌ No |
| Resource types | Explicit + compiler | ✅ Yes | ✅ Yes |

Resource types are "visible RAII"—you see it, the compiler guarantees it.

### Unique vs Resource

| Aspect | Unique (`@unique`) | Resource (`@resource`) |
|--------|--------------------|--------------------|
| Implicit copy | ❌ Disabled | ❌ Disabled |
| Can drop | ✅ Yes | ❌ No (must consume) |
| Explicit clone | ✅ Allowed | ❌ Not allowed |
| Use case | Semantic safety | Resource safety |
| Example | Unique ID | File handle |

Unique is "don't duplicate"; resource is "must properly close."

## Resources in Error Types

When an operation fails, the resource must still be accounted for. The standard pattern is to return the resource in the error type.

### Basic Pattern

<!-- test: skip -->
```rask
enum FileError {
    ReadFailed { file: File, reason: string },
    WriteFailed { file: File, reason: string },
}

func read_config(file: File) -> Config or FileError {
    const data = match file.read_all() {
        Ok(d) => d,
        Err(reason) => return Err(FileError.ReadFailed { file, reason }),
    }

    const config = try parse(data)
    try file.close()
    Ok(config)
}
```

**Caller must handle the file in error paths:**
<!-- test: skip -->
```rask
func load_config(path: string) -> Config or Error {
    const file = try File.open(path)

    match read_config(file) {
        Ok(config) => Ok(config),
        Err(FileError.ReadFailed { file, reason }) => {
            try file.close()  // Must still consume the file
            Err(Error.ConfigRead(reason))
        }
        Err(FileError.WriteFailed { file, reason }) => {
            try file.close()
            Err(Error.ConfigWrite(reason))
        }
    }
}
```

### Multiple Resources

When errors contain multiple resources, all must be consumed:

<!-- test: skip -->
```rask
enum TransferError {
    SourceReadFailed {
        source: File,
        dest: File,
        reason: string
    },
    DestWriteFailed {
        source: File,
        dest: File,
        reason: string
    },
}

func handle_transfer_error(err: TransferError) -> () or Error {
    match err {
        TransferError.SourceReadFailed { source, dest, reason } => {
            try source.close()
            try dest.close()
            Err(Error.Transfer(reason))
        }
        TransferError.DestWriteFailed { source, dest, reason } => {
            try source.close()
            try dest.close()
            Err(Error.Transfer(reason))
        }
    }
}
```

### Simplifying with `ensure`

The `ensure` pattern reduces verbosity when the cleanup is the same:

<!-- test: parse -->
```rask
func transfer(source_path: string, dest_path: string) -> () or Error {
    const source = try File.open(source_path)
    ensure source.close()

    const dest = try File.create(dest_path)
    ensure dest.close()

    // Now try works naturally - both files cleaned up on any error
    const data = try source.read_all()
    try dest.write_all(data)

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

<!-- test: parse -->
```rask
func process_files(paths: Vec<string>) -> () or Error {
    const files = Vec.new()

    for path in paths {
        const file = try File.open(path)
        ensure file.close()  // Each file gets its own ensure
        files.push(file)
    }

    // Process all files...
    for file in files.iter() {
        try process(file)
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
<!-- test: skip -->
```rask
extend FileError {
    func close_and_convert(take self) -> Error or Error {
        match self {
            FileError.ReadFailed { file, reason } => {
                try file.close()
                Ok(Error.Read(reason))
            }
            FileError.WriteFailed { file, reason } => {
                try file.close()
                Ok(Error.Write(reason))
            }
        }
    }
}

// Usage:
try read_config(file).map_err(|e| e.close_and_convert())
```

## Edge Cases

| Case | Handling |
|------|----------|
| Resource in error path | Must consume or register with `ensure` |
| Resource in error type | Caller must extract and consume from error |
| Resource across match arms | Each arm must consume (or share `ensure`) |
| Nested resource values | Each level must be consumed |
| Resource + panic | `ensure` runs during unwind |
| Conditional consumption | Both branches must consume |
| Loop with resource | Can't create resource in loop without consuming each iteration |

**Conditional consumption:**
<!-- test: parse -->
```rask
func conditional(file: File, keep_open: bool) -> () or Error {
    if keep_open {
        GLOBAL_FILES.store(file)  // Consumes by transfer
    } else {
        try file.close()             // Consumes by close
    }
    // ✅ Both branches consume
    Ok(())
}
```

## Examples

### File Processing
<!-- test: parse -->
```rask
func process_file(path: string) -> Data or Error {
    const file = try File.open(path)
    ensure file.close()

    const header = try file.read_header()
    const data = try file.read_body()

    Ok(data)
}
```

### Database Transaction
<!-- test: parse -->
```rask
func update_user(db: Database, user_id: u64) -> () or Error {
    const txn = try db.begin_transaction()
    ensure txn.rollback()     // Default: rollback on error

    const user = try txn.query_user(user_id)
    user.last_login = now()
    try txn.update_user(user)

    try txn.commit()             // Explicit commit consumes txn
                              // ensure no longer needed (already consumed)
    Ok(())
}
```

### Connection Pool
<!-- test: parse -->
```rask
func handle_connections(pool: Pool<Connection>) -> () or Error {
    // Process all connections
    for h in pool.cursor() {
        try pool.modify(h, |conn| {
            if conn.should_close() {
                // Remove and consume
                const removed = pool.remove(h).unwrap()
                try removed.close()
            }
            Ok(())
        })
    }

    // Clean up remaining
    for h in pool.handles().collect<Vec<_>>() {
        const conn = pool.remove(h).unwrap()
        try conn.close()
    }

    Ok(())
}
```

## Integration Notes

- **Value Semantics:** Resource types are move-only (never Copy) (see [value-semantics.md](value-semantics.md))
- **Ownership:** Resource adds consumption requirement on top of single ownership (see [ownership.md](ownership.md))
- **Error Handling:** `ensure` integrates with Result and `try` (see [ensure.md](../control/ensure.md))
- **Pools:** Pool<Resource> requires explicit removal (see [pools.md](pools.md))
- **Tooling:** IDE tracks resource value state, warns on missing consumption

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move behavior
- [Ownership Rules](ownership.md) — Single-owner model
- [Ensure](../control/ensure.md) — Deferred execution
- [Pools](pools.md) — Handle-based storage for resource types
