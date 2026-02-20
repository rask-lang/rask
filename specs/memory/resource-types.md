<!-- id: mem.resources -->
<!-- status: decided -->
<!-- summary: @resource types must be consumed exactly once; ensure provides deferred consumption -->
<!-- depends: memory/ownership.md, control/ensure.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Resource Types

`@resource` marks types that must be consumed exactly once — you can't forget to close a file or commit a transaction. The compiler enforces it. `ensure` provides deferred consumption. (These are sometimes called "linear types" in academic literature.)

## Declaration

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

## Consumption Rules

| Rule | Description |
|------|-------------|
| **R1: Must consume** | Resource value must be consumed before scope exit |
| **R2: Consume once** | Cannot consume same resource value twice |
| **R3: Read allowed** | Can borrow for reading without consuming |
| **R4: `ensure` satisfies** | Registering with `ensure` counts as consumption commitment |
| **R5: Pool cleanup enforcement** | `Pool<Resource>` panics at runtime if non-empty when it goes out of scope |

A resource is consumed by calling a method with `take self`, passing to a `take` parameter, or explicit consumption (e.g., `file.close()`).

<!-- test: skip -->
```rask
@resource
struct File { ... }

extend File {
    func close(take self) -> () or Error {
        // ... close logic ...
    }

    func read(self, buf: [u8]) -> usize or Error {
        // ... read logic (non-consuming) ...
    }
}
```

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try File.open("data.txt")
    const data = try file.read_all()
    try process_data(data)
    try file.close()                          // Consumed
    Ok(())
}
```

**Forgetting to consume:**
<!-- test: compile-fail -->
```rask
@resource
struct DbConn {
    handle: i32
}

extend DbConn {
    func open(path: string) -> DbConn or Error {
        return Ok(DbConn { handle: 1 })
    }

    func read_all(self) -> string or Error {
        return Ok("data")
    }

    func close(take self) -> () or Error {
        return Ok(())
    }
}

func bad() -> () or Error {
    const conn = try DbConn.open("data.txt")
    const data = try conn.read_all()
    Ok(())
    // ERROR: conn not consumed before scope exit
}
```

**Double consumption:**
<!-- test: compile-fail -->
```rask
@resource
struct DbConn {
    handle: i32
}

extend DbConn {
    func open(path: string) -> DbConn or Error {
        return Ok(DbConn { handle: 1 })
    }

    func close(take self) -> () or Error {
        return Ok(())
    }
}

func also_bad() -> () or Error {
    const conn = try DbConn.open("data.txt")
    try conn.close()
    try conn.close()    // ERROR: conn already consumed
    Ok(())
}
```

## The `ensure` Statement

`ensure` commits to consuming a resource at scope exit, satisfying R1 immediately.

| Phase | What happens |
|-------|--------------|
| Registration | `ensure file.close()` marks `file` as "consumption committed" |
| During scope | `file` can be borrowed (read/mutate) but not consumed |
| Scope exit | `ensure` block runs, consuming `file` |

<!-- test: parse -->
```rask
func process() -> () or Error {
    const file = try File.open("data.txt")
    ensure file.close()        // Consumption committed

    const header = try file.read_header()
    try validate(header)       // Can use try freely

    const body = try file.read_body()
    try transform(body)

    Ok(())
    // ensure runs: file.close() called
}
```

**Error handling in `ensure`:** If the ensured operation returns `Result`, errors are logged (debug mode), accumulated if multiple ensures fail, and returned as the scope's error if no explicit return.

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

## Resources + Error Paths

`ensure` bridges resources with error handling: commit to cleanup early, then use `try` freely.

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

## Resources in Error Types

When the caller needs the resource for recovery/retry, return it in the error type.

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

| Pattern | When to use |
|---------|-------------|
| Resource in error type | Caller needs the resource for recovery/retry |
| `ensure` | Cleanup is always the same (just close it) |
| Hybrid | Different cleanup depending on error type |

## Resources in Collections

| Rule | Collection | Resource allowed? | Reason |
|------|------------|-------------------|--------|
| **RC1** | `Vec<Resource>` | No | Vec drop would need to consume each element |
| **RC2** | `Pool<Resource>` | Yes | Explicit removal required anyway |
| **RC3** | `Map<K, Resource>` | No | Map drop same problem as Vec |
| **RC4** | `Option<Resource>` | Yes | Must match and consume |

**Pool pattern for resources:**
<!-- test: skip -->
```rask
const connections: Pool<Connection> = Pool.new()
const h = try connections.insert(try Connection.open(addr))

// Later: explicit consumption required
const conn = connections.remove(h)!
try conn.close()
```

**Pool<Resource> cleanup (R5):** If non-empty at scope exit, runtime panic. All elements must be consumed first.

<!-- test: skip -->
```rask
// Required: consume all before pool drops
for file in files.take_all() {
    try file.close()
}
// Pool is now empty, safe to drop
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `take_all()` | `Pool<T> -> Iterator<T>` | Take all elements for consumption |
| `take_all_with(f)` | `func(T) -> ()` | Take all and apply consuming function |
| `take_all_with_result(f)` | `func(T) -> () or E -> () or E` | Take all with fallible consumer |

## Error Messages

**Resource not consumed [R1]:**
```
ERROR [mem.resources/R1]: resource not consumed before scope exit
   |
3  |  const file = try File.open("data.txt")
   |        ^^^^ File created here
8  |  }
   |  ^ scope ends without consuming file

WHY: @resource types must be explicitly consumed. They cannot be silently discarded.

FIX: Consume with a method or register with ensure:

  try file.close()           // Explicit consumption
  ensure file.close()        // Deferred consumption
```

**Double consumption [R2]:**
```
ERROR [mem.resources/R2]: resource already consumed
   |
5  |  try file.close()
   |      ^^^^ consumed here
6  |  try file.close()
   |      ^^^^ cannot consume again
```

**Pool<Resource> cleanup panic [R5]:**
```
panic: Pool<File> has 3 unconsumed resource elements at scope exit.
Resources must be explicitly consumed (use take_all() before scope ends).
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Resource in error path | R1 | Must consume or register with `ensure` |
| Resource in error type | R1 | Caller must extract and consume from error |
| Resource across match arms | R1 | Each arm must consume (or share `ensure`) |
| Nested resource values | R1 | Each level must be consumed |
| Resource + panic | R4 | `ensure` runs during unwind |
| Conditional consumption | R1 | Both branches must consume |
| Loop with resource | R1 | Can't create resource in loop without consuming each iteration |
| `clear()` on Pool<Resource> | RC2 | Compile error (would abandon linear elements) |

**Conditional consumption:**
<!-- test: parse -->
```rask
func conditional(file: File, keep_open: bool) -> () or Error {
    if keep_open {
        GLOBAL_FILES.store(file)  // Consumes by transfer
    } else {
        try file.close()             // Consumes by close
    }
    // Both branches consume
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
                const removed = pool.remove(h)!
                try removed.close()
            }
            Ok(())
        })
    }

    // Clean up remaining
    for h in pool.handles().collect<Vec<_>>() {
        const conn = pool.remove(h)!
        try conn.close()
    }

    Ok(())
}
```

---

## Appendix (non-normative)

### Rationale

**R1–R4 (must-consume):** Resource types prevent leaks by construction. Unlike RAII (automatic destructors, as in C++ or Rust), resources need explicit consumption — cleanup is visible while still guaranteed.

**R4 (ensure):** `ensure` bridges resources with error handling: commit to cleanup early, then use `try` freely knowing it'll happen.

**R5 (pool drop panic):** The compiler can't statically track dynamic pool contents — that would require whole-program analysis. Runtime panic is preferable to silent leaks because the program fails loudly rather than leaking resources.

**RC1/RC3 (no Vec/Map):** Vec and Map drops would need to consume each element, but drop can't return errors. Pools work because removal is already explicit.

### Patterns & Guidance

**Comparison with other mechanisms:**

| Mechanism | Cleanup | Visible? | Guaranteed? |
|-----------|---------|----------|-------------|
| RAII (Rust/C++) | Automatic in drop | No | Yes |
| Manual (C) | Explicit call | Yes | No |
| GC finalizers | Eventual | No | No |
| Resource types | Explicit + compiler | Yes | Yes |

Resource types are "visible RAII" — you see it, the compiler guarantees it.

**Unique vs resource types:**

| Aspect | Unique (`@unique`) | Resource (`@resource`) |
|--------|--------------------|--------------------|
| Implicit copy | Disabled | Disabled |
| Can drop | Yes | No (must consume) |
| Explicit clone | Allowed | Not allowed |
| Use case | Semantic safety | Resource safety |
| Example | Unique ID | File handle |

**Resources in errors — helper pattern:**

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

**Compound resources with ensure:**
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
    for file in files {
        try process(file)
    }

    Ok(())
    // All ensures run in reverse order
}
```

### See Also

- [Value Semantics](value-semantics.md) — Copy vs move, `@unique` (`mem.value`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Ensure](../control/ensure.md) — Deferred execution (`ctrl.ensure`)
- [Pools](pools.md) — Handle-based storage for resource types (`mem.pools`)
