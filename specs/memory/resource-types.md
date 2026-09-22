<!-- id: mem.resources -->
<!-- status: decided -->
<!-- summary: @resource marks struct types as linear — must be consumed exactly once -->
<!-- depends: memory/linear.md, memory/ownership.md, control/ensure.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Resource Types

`@resource` marks a struct type as **linear** — every value of that type must be consumed exactly once. You can't forget to close a file or commit a transaction; the compiler enforces it.

The consume-exactly-once rules live in [`mem.linear`](linear.md) and apply identically to `@resource` structs, `Heap<T>`, and linear elements in pools. This spec describes the `@resource` annotation and the patterns specific to I/O handles and transactions.

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

`@resource` values follow the linearity rules `mem.linear/L1–L7`. This table restates them in `@resource` context with the rule identifiers they're cited by in other specs:

| Rule | Citation | Description |
|------|----------|-------------|
| **R1** | `mem.linear/L1` | Must be consumed before scope exit |
| **R2** | `mem.linear/L2` | Cannot be consumed twice |
| **R3** | `mem.linear/L3` | Can borrow for reading without consuming |
| **R4** | `mem.linear/L4` | Registering with `ensure` counts as consumption commitment |
| **R6** | `mem.linear/L7` | Nothing may stand between acquiring the resource and committing its cleanup |
| **EO1** | `mem.linear/L7` | `ensure` bodies run LIFO, so a resource derived from another has its `ensure` registered **second** — the source order reads backwards from the run order. That order is the only one L7 permits: deriving from a resource is a statement in that resource's window, so the dependency's `ensure` has to come first. Registered the other way round, the dependency would be torn down while its dependent is still live |

A resource is consumed by calling a method with `take self`, passing to a `take` parameter, or explicit consumption (e.g., `file.close()`).

<!-- test: skip -->
```rask
@resource
struct File { ... }

extend File {
    func close(take self) -> void or Error {
        // ... close logic ...
    }

    func read(self, buf: [u8]) -> usize or Error {
        // ... read logic (non-consuming) ...
    }
}
```

<!-- test: parse -->
```rask
func process() -> void or Error {
    let file = try File.open("data.txt")
    let data = try file.read_text()
    try process_data(data)
    try file.close()                          // Consumed
    return
}
```

**Forgetting to consume (L1):**
<!-- test: compile-fail: ownership -->
```rask
@resource
struct DbConn {
    handle: i32
}

extend DbConn {
    func open(path: string) -> DbConn or Error {
        return DbConn { handle: 1 }
    }

    func read_text(self) -> string or Error {
        return "data"
    }

    func close(take self) -> void or Error {
        return
    }
}

func bad() -> void or Error {
    let conn = try DbConn.open("data.txt")
    let data = try conn.read_text()
    return
    // ERROR: conn not consumed before scope exit
}
```

**Double consumption (L2):**
<!-- test: compile-fail: ownership -->
```rask
@resource
struct DbConn {
    handle: i32
}

extend DbConn {
    func open(path: string) -> DbConn or Error {
        return DbConn { handle: 1 }
    }

    func close(take self) -> void or Error {
        return
    }
}

func also_bad() -> void or Error {
    let conn = try DbConn.open("data.txt")
    try conn.close()
    try conn.close()    // ERROR: conn already consumed
    return
}
```

## The `ensure` Statement

`ensure` commits to consuming a resource at scope exit, satisfying L1 immediately.

| Phase | What happens |
|-------|--------------|
| Registration | `ensure file.close()` marks `file` as "consumption committed" |
| During scope | `file` can be borrowed (read/mutate) but not consumed |
| Scope exit | `ensure` block runs, consuming `file` |

<!-- test: parse -->
```rask
func process() -> void or Error {
    let file = try File.open("data.txt")
    ensure file.close()        // Consumption committed

    let header = try file.read_header()
    try validate(header)       // Can use try freely

    let body = try file.read_body()
    try transform(body)

    return
    // ensure runs: file.close() called
}
```

**Error handling in `ensure`:** If the ensured operation returns `T or E`, errors are logged (debug mode), accumulated if multiple ensures fail, and returned as the scope's error if no explicit return.

<!-- test: parse -->
```rask
func risky() -> void or Error {
    let file = try File.open("data.txt")
    ensure file.close()        // May fail

    try risky_operation()         // If this fails, ensure still runs

    return
}
// If risky_operation() fails: file.close() runs, then try propagates
// If file.close() fails: that error is returned
```

## Resources + Error Paths

`ensure` bridges resources with error handling: commit to cleanup early, then use `try` freely.

<!-- test: parse -->
```rask
func process(path: string) -> Data or Error {
    let file = try File.open(path)
    ensure file.close()        // Guarantees consumption on any exit

    let header = try file.read_header()  // Early return? ensure runs
    if !header.valid {
        return InvalidHeader      // ensure runs, file closed
    }

    let data = try file.read_body()      // Early return? ensure runs
    return data                           // Normal exit: ensure runs
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
    let data = if file.read_text() ? as d { d } else as reason {
        return FileError.ReadFailed { file, reason }
    }

    let config = try parse(data)
    try file.close()
    return config
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
| **RC2** | `Rack<Resource>` | No | `delete` frees the node rather than handing it back, so nothing can consume one |
| **RC3** | `Map<K, Resource>` | No | Map drop same problem as Vec |
| **RC4** | `Resource?` | Yes | Must match and consume |

So: no container holds a linear value. An optional is what's left, and matching
it is the consumption.

<!-- test: skip -->
```rask
mut conn: Connection? = try Connection.open(addr)

// Later: match to consume
if conn? as c {
    try c.close()
    conn = none
}
```

`Pool<Resource>` used to be the one that worked, because `Pool.remove` answered
`T?` — there was always a way to get the value back out (rask-lang/rask#908).
Nothing replaced it: a rack fails RC1's test in different words, so the three
rejections are one rule with three receivers. If a container for linear values
comes back, it needs a `take` that hands the value over.

## Error Messages

**Resource not consumed [L1]:**
```
ERROR [mem.linear/L1]: resource not consumed before scope exit
   |
3  |  let file = try File.open("data.txt")
   |        ^^^^ File created here
8  |  }
   |  ^ scope ends without consuming file

WHY: @resource types must be explicitly consumed. They cannot be silently discarded.

FIX: Consume with a method or register with ensure:

  try file.close()           // Explicit consumption
  ensure file.close()        // Deferred consumption
```

**Double consumption [L2]:**
```
ERROR [mem.linear/L2]: resource already consumed
   |
5  |  try file.close()
   |      ^^^^ consumed here
6  |  try file.close()
   |      ^^^^ cannot consume again
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Resource in error path | L1 | Must consume or register with `ensure` |
| Resource in error type | L1 | Caller must extract and consume from error |
| Resource across match arms | L1 | Each arm must consume (or share `ensure`) |
| Nested resource values | L1 | Each level must be consumed |
| Resource + panic | L4 | `ensure` runs during unwind |
| Conditional consumption | L1 | Both branches must consume |
| Loop with resource | L1 | Can't create resource in loop without consuming each iteration |
| `Rack<Resource>` anywhere | RC2 | Compile error at the type (E0820) |

**Conditional consumption:**
<!-- test: parse -->
```rask
func conditional(file: File, keep_open: bool) -> void or Error {
    if keep_open {
        GLOBAL_FILES.store(file)  // Consumes by transfer
    } else {
        try file.close()             // Consumes by close
    }
    // Both branches consume
    return
}
```

## Examples

### File Processing
<!-- test: parse -->
```rask
func process_file(path: string) -> Data or Error {
    let file = try File.open(path)
    ensure file.close()

    let header = try file.read_header()
    let data = try file.read_body()

    return data
}
```

### Database Transaction
<!-- test: parse -->
```rask
func update_user(db: Database, user_id: u64) -> void or Error {
    let txn = try db.begin_transaction()
    ensure txn.rollback()     // Default: rollback on error

    let user = try txn.query_user(user_id)
    user.last_login = now()
    try txn.update_user(user)

    try txn.commit()             // Explicit commit consumes txn
                              // ensure no longer needed (already consumed)
    return
}
```

### Many connections

A resource can't live in a container (RC1–RC3), so "many of them" is a fixed set
of names, each consumed on its own path.

<!-- test: parse -->
```rask
func serve_two(a: Connection, b: Connection) -> void or Error {
    ensure a.close()
    ensure b.close()

    try a.handle_request()
    try b.handle_request()
    return
}
```

Growing that to a real server means the connections stay where they were
acquired — one per task, consumed by the task that took it — rather than
gathered into a pool the program then has to remember to drain.

---

## Appendix (non-normative)

### Rationale

**Why `@resource` exists:** Linearity is a property, but in real code you want to attach it to a specific kind of value — a file, a socket, a transaction. `@resource` is the annotation that says "every value of this struct type is linear." Rules L1–L7 do the work; the annotation just scopes them to a concrete type.

**L4 (ensure):** The bridge between linearity and error handling. Commit to cleanup early, then use `try` freely knowing it'll happen.

**RC1–RC3 (no container at all):** a drop can't return errors, so a `Vec` or a `Map` that held linear elements would have to drop them silently. A rack's `delete` is explicit, which is what made a pool's `remove` acceptable — but `delete` answers nothing, so there is no call that consumes a node. Three receivers, one reason, and `T?` is what's left.

There used to be an R5 as well: a `Pool<Resource>` dropped non-empty panicked at run time, because the compiler couldn't statically track what a pool held. That was the only runtime rule in this spec, and it went with the pool. Everything here is a compile error now.

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
                return Error.Read(reason)
            }
            FileError.WriteFailed { file, reason } => {
                try file.close()
                return Error.Write(reason)
            }
        }
    }
}

// Usage:
read_config(file) catch e => return e.close_and_convert()
```

**Compound resources with ensure:**
<!-- test: parse -->
```rask
func process_files(paths: Vec<string>) -> void or Error {
    let files = Vec.new()

    for path in paths {
        let file = try File.open(path)
        ensure file.close()  // Each file gets its own ensure
        files.push(file)
    }

    // Process all files...
    for file in files {
        try process(file)
    }

    return
    // All ensures run in reverse order
}
```

### See Also

- [Linearity](linear.md) — Rule set (L1–L7) shared by `@resource` and `Heap<T>` (`mem.linear`)
- [Heap Values](heap.md) — `Heap<T>`, the other linear value (`mem.heap`)
- [Value Semantics](value-semantics.md) — Copy vs move, `@unique` (`mem.value`)
- [Ownership Rules](ownership.md) — Single-owner model (`mem.ownership`)
- [Ensure](../control/ensure.md) — Deferred execution (`ctrl.ensure`)
