# Async Runtime

Green tasks for high-concurrency scenarios (10k+ connections).

## Overview

**Opt-in layer** for programs needing massive concurrency. Most programs should use sync-concurrency instead.

| Property | Value |
|----------|-------|
| Task type | Green tasks (stackless coroutines) |
| Scaling | 100k+ concurrent tasks |
| Cost | ~4KB per task |
| I/O model | Non-blocking (yields at await points) |

## When to Use Async

| Scenario | Recommendation |
|----------|----------------|
| HTTP server, <1000 connections | Sync (simpler) |
| Proxy, 10k+ connections | Async |
| CLI tool | Sync |
| Database with connection pool | Sync |
| Real-time game server | Async |
| Background job processor | Sync |

**Rule of thumb:** If you're not sure, use sync. Async adds complexity.

## Async Functions

### Declaration

```
async fn fetch_user(id: u64) -> Result<User, Error> {
    let response = async_http_get(url).await?
    parse_user(response)
}
```

- `async` keyword precedes `fn`
- Return type is the eventual value (not a task/future type)
- Body can use `.await` to suspend

### Await Operator

Postfix `.await` suspends until completion:

```
async fn caller() {
    let user = fetch_user(123).await?  // Suspends, then unwraps
    print(user.name)
}
```

### Async Blocks and Closures

```
let task = async { expensive_work().await }
let result = task.await?

urls.map(async |url| async_fetch(url).await)
```

## Async Nurseries

```
async nursery { |n|
    n.async_spawn { work1().await }
    n.async_spawn { work2().await }
}
```

### Syntax Rules

| Syntax | Spawns | Method | Available In |
|--------|--------|--------|--------------|
| `nursery { \|n\| ... }` | OS threads | `n.spawn` | Anywhere |
| `async nursery { \|n\| ... }` | Green tasks | `n.async_spawn` | Async context only |

**Keyword determines task type, NOT context:**

```
async fn main() {
    // BOTH allowed in async context:

    nursery { |n|
        n.spawn { blocking_work() }  // OS thread
    }

    async nursery { |n|
        n.async_spawn { async_work().await }  // Green task
    }
}
```

**Compile errors:**

```
fn main() {
    // COMPILE ERROR: async nursery requires async context
    async nursery { |n| ... }
}
```

### Nursery Parameter Types

- `nursery` -> `n: SyncNursery` (has `spawn` method)
- `async nursery` -> `n: AsyncNursery` (has `async_spawn` method)

Using the wrong method is a compile error (method doesn't exist on type).

## Async Runtime

### Properties

| Property | Value |
|----------|-------|
| Scope | Per-thread (thread-local) |
| Initialization | Lazy, on first async operation |
| Shutdown | Automatic at thread exit |
| Initial cost | ~64KB heap allocation |
| Worker threads | 1 per runtime |

### Main Function

```
async fn main() -> Result<()> {
    // Async runtime initialized for main thread
    server().await
}
```

`async fn main()` MUST be used for async programs.

### Multi-core Parallelism

Each OS thread has its own async runtime:

```
nursery { |n|
    for cpu in 0..num_cpus() {
        n.spawn {
            async nursery { |a|
                // Each thread has own async runtime
                a.async_spawn { work_chunk(cpu).await }
            }
        }
    }
}
```

### Binary Overhead

Runtime (~50KB) included only if `async` keyword used.

## Sync/Async Boundaries

### Function Color

| From Context | Calling Async | Calling Sync |
|--------------|---------------|--------------|
| Async | `.await` | Direct call (blocks runtime!) |
| Sync | `block_on(...)` | Direct call |

### block_on

Bridges sync to async:

```
fn sync_main() {
    let user = block_on(fetch_user(123))?
}
```

Creates a runtime, blocks thread until complete.

### Blocking in Async Context

Calling sync I/O from async context blocks the runtime thread:

```
async fn handler() {
    let data = blocking_file_read(path)  // BLOCKS RUNTIME
    process(data).await
}
```

**IDE warning:** "blocks runtime" shown at call site.

**Allowed but dangerous:** All other async tasks on this runtime are frozen.

---

## Critical Design Issues

### Issue 1: Sync Nursery Blocks Async Runtime

```
async fn handler() {
    nursery { |n|           // Blocks async runtime thread!
        n.spawn { cpu_work() }
    }
    // All other async tasks frozen until nursery exits
}
```

**Current status:** Allowed but dangerous.

**Options:**

| Option | Tradeoff |
|--------|----------|
| Compile error | Too restrictive, can't mix CPU/IO work |
| `nursery.await` | Yield while waiting for OS threads |
| Warning only | Easy to miss, causes subtle bugs |
| spawn_blocking | Explicit offload to thread pool |

**Recommended:** Add `nursery.await` or `spawn_blocking` pattern.

### Issue 2: Async Block Capture

Do async blocks capture by move (like task closures)?

```
let data = load_data()
let task = async { process(data).await }  // Move or borrow?
```

**Current status:** Unspecified.

**Recommendation:** Move semantics (consistent with task closures).

### Issue 3: block_on Nesting

What happens if block_on called from async context?

```
async fn outer() {
    block_on(inner())  // Nested runtime? Deadlock?
}
```

**Current status:** Unspecified.

**Recommendation:** Compile error or panic.

---

## Remaining Issues

### High Priority

1. **Sync nursery blocking async runtime** (see Critical Issues)
2. **Async block capture semantics**
3. **block_on nesting behavior**

### Medium Priority

4. **Async cancellation**
   - Does `.await` check cancelled() flag?
   - Can async I/O be interrupted?

5. **Runtime configuration**
   - Worker thread count?
   - Task scheduling policy?

### Low Priority

6. **Async drop**
   - Can destructors be async?
   - Currently: no (drop is sync)
