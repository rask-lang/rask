# Elaboration: Async Runtime Initialization

## Runtime Model

**Decision:** Implicit per-thread runtime, initialized on first async operation.

**Rationale:** Maximizes ergonomics (no boilerplate), preserves local analysis (no global state), follows "cost visible when used" principle.

## Initialization

### Automatic Initialization

The async runtime is initialized automatically when:
- An `async` function is called
- An `async nursery` is created
- Any `async_*` I/O function is called

**No explicit initialization required.**

```
// Runtime automatically initialized here
async fn main() {
    async nursery { |n|
        n.async_spawn { work() }
    }
}
```

### Runtime Properties

| Property | Value |
|----------|-------|
| Scope | Per-thread (thread-local) |
| Initialization | Implicit on first async operation |
| Shutdown | Automatic at thread exit |
| Cost | Lazy allocation (~64KB initial heap) |

**Cost transparency:** Async operations (async fn, async nursery) explicitly marked with `async` keyword.

## Thread Model

### Main Thread

```
async fn main() -> Result<()> {
    // Async runtime initialized for main thread
    async_server().run()?
    Ok(())
}
```

The `async` keyword on `main` signals that main thread uses async runtime.

**Rules:**
- `async fn main()` MUST be used for async programs
- Sync `fn main()` for pure-sync programs (no async runtime)
- Mixing: sync main can spawn async threads via explicit runtime

### Worker Threads

Each OS thread has its own async runtime if it uses async operations.

```
nursery { |n|
    // Thread 1: sync task, no async runtime
    n.spawn { sync_work() }

    // Thread 2: async task, runtime initialized
    n.spawn {
        async nursery { |a|
            a.async_spawn { async_work() }
        }
    }
}
```

**Rule:** Async runtime MUST NOT be shared across threads. Each thread initializes its own.

## Async/Sync Mixing

### Async from Sync

**Not allowed directly.** Async functions cannot be called from sync context.

```
fn sync_function() {
    async_operation()  // COMPILE ERROR: cannot call async fn from sync context
}
```

**Workaround:** Use blocking adapter (runtime overhead explicit):

```
fn sync_function() {
    let result = block_on(async_operation())  // Explicit: creates mini-runtime
}
```

**Cost:** `block_on` creates temporary runtime, blocks thread until completion. Only use at boundaries (main, tests, FFI).

### Sync from Async

**Allowed but discouraged.** Sync I/O blocks entire async runtime thread.

```
async fn process() {
    let file = open("data.txt")?  // Sync I/O - BLOCKS runtime thread
    let data = file.read()?       // All other async tasks stalled
}
```

**Better:** Use async I/O:

```
async fn process() {
    let file = async_open("data.txt")?  // Yields to other tasks
    let data = async_read(&file)?       // Cooperative
}
```

**Rule:** Sync I/O in async context is allowed but not idiomatic. IDE SHOULD warn: `sync I/O blocks runtime`.

## Runtime Configuration

### Default Settings

| Setting | Default | Rationale |
|---------|---------|-----------|
| Worker threads | 1 per runtime (thread-local) | No parallelism within single async context |
| Heap size | 64KB initial, grows as needed | Lazy allocation |
| Task stack size | 4KB per async task | Green threads |
| Max concurrent tasks | Unbounded (limited by memory) | No artificial limits |

**No configuration API.** Defaults chosen for 80% use case. Power users can use native threads for parallelism.

### Multi-Core Async Parallelism

Async runtime is single-threaded per OS thread. For parallelism:

**Pattern 1: Multiple async threads**

```
nursery { |n|
    for cpu in 0..num_cpus() {
        n.spawn {
            async nursery { |a|
                // Each thread runs own async runtime
                a.async_spawn { work_chunk(cpu) }
            }
        }
    }
}
```

**Pattern 2: Sync nursery with async tasks**

```
async fn main() {
    async nursery { |a|
        // Single-threaded async I/O multiplexing
        for _ in 0..10000 {
            a.async_spawn { handle_connection() }
        }
    }
}
```

**Guidance:**
- Use async for I/O concurrency (many connections, few threads)
- Use nursery for CPU parallelism (one thread per core)
- Combine: async within each parallel worker thread

## Runtime Shutdown

### Automatic Shutdown

Runtime shuts down when:
- Thread exits
- No async operations remain in scope
- All async nurseries have completed

**No explicit shutdown required.**

```
async fn main() {
    async nursery { |a|
        a.async_spawn { work() }
    }
    // Nursery exit waits for all tasks
}
// Runtime automatically cleaned up at thread exit
```

### Resource Cleanup

| Resource | Cleanup Behavior |
|----------|------------------|
| Async tasks | Nursery exit waits for completion |
| I/O handles | Closed via `ensure` or drop |
| Runtime heap | Freed at thread exit |
| Channels | Drop triggers close |

**Linear resources:** Must use `ensure` for guaranteed cleanup (existing rule applies in async).

## Edge Cases

| Case | Handling |
|------|----------|
| Async function called from sync | COMPILE ERROR |
| Sync I/O in async task | Blocks runtime (allowed, warned) |
| Panic in async task | Propagates to join(), runtime continues |
| Memory exhaustion (task spawn) | Returns error, runtime continues |
| Deadlock in async | Not detected (programmer responsibility) |
| Multiple runtimes per thread | Forbidden (single thread-local runtime) |
| Async in library, sync main | Library functions require `async fn main()` to call |

## Binary Size and Opt-Out

### Compilation

| Program Type | Runtime Linked | Binary Overhead |
|--------------|----------------|-----------------|
| Sync only | No | Zero |
| Async used | Yes | ~50KB runtime code |

**Automatic:** Linker only includes async runtime if `async` keyword used in program.

### Feature Flags

No feature flags needed. Runtime inclusion is automatic based on code analysis.

## Integration Notes

**Concurrency spec:** Async nursery follows same structured concurrency rules as sync nursery (task handles, join, cancellation).

**Linear types:** `ensure` works identically in async context. Async operations can be registered in ensure blocks.

**Channels:** Same channel types work in both sync and async (blocking behavior adapts to context).

**Tooling:** IDE SHOULD show async/sync boundaries, warn on sync I/O in async, and display runtime initialization points.

## Cost Transparency Validation

| Cost | Visibility |
|------|------------|
| Runtime initialization | `async` keyword on function/nursery |
| Green task spawn | `async_spawn` explicit |
| Context switch (yield) | `async_*` I/O functions |
| Blocking sync I/O | IDE warning "blocks runtime" |
| Runtime memory | Implicit but bounded per-thread |

**Passes TC ≥ 0.90:** Major costs (async mode, task spawn, yielding I/O) visible in code.
