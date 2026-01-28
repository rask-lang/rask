# Solution: Async and Concurrency

## The Question
How should Rask handle async/concurrency given constraints of safety without annotations, value semantics, no storable references, transparent costs, and linear resource cleanup?

## Decision
**OS threads with structured nurseries, ownership-transfer channels, and opt-in async runtime.** Synchronous I/O by default (blocking explicit), async mode for high-concurrency needs (100k+ tasks). Tasks capture by move (not borrow), channels are affine with explicit sharing, linear resources use dedicated cleanup patterns.

## Rationale
This preserves all core principles: no lifetime annotations (nurseries enforce structure via affine handles), transparent costs (thread spawn and I/O blocking visible, async opt-in), mechanical safety (no shared mutable state, channels transfer ownership), and local analysis (affine types checked per-function). Accepts ~1000 concurrent thread limit for default mode, covers 80%+ use cases (HTTP servers, pipelines, CLI tools).

## Specification

### Task Model

| Mode | Syntax | Scaling | Cost |
|------|--------|---------|------|
| Default (sync) | `spawn { ... }` | ~1000 OS threads | ~2MB stack per thread, explicit |
| Async (opt-in) | `async spawn { ... }` | 100k+ green tasks | Yields at `async_*()` calls, runtime overhead |

**Rules:**
- MUST spawn tasks within a `nursery` block
- Sync tasks use blocking I/O (`read()`, `write()`)
- Async tasks use yielding I/O (`async_read()`, `async_write()`)
- Mixing sync I/O in async context blocks the entire runtime (allowed, documented)

### Nurseries (Structured Concurrency)

```
nursery { |n|
    h1 = n.spawn { work1() }
    h2 = n.spawn { work2() }
    h1.join()?
    h2.join()?
}
```

| Rule | Enforcement |
|------|-------------|
| `spawn` returns `TaskHandle` (affine type) | Must be consumed via `.join()` or `.cancel()` before nursery exit |
| Nursery exit waits for all children | Blocks until all spawned tasks complete |
| TaskHandle cannot escape nursery | Compile error if returned without consuming |
| Early nursery exit | Requests cancellation of all children, waits for joins |

**Affine enforcement:** Unused `TaskHandle` at scope exit is compile error (linear type violation).

### Task Capture — Move Semantics

Tasks capture by move (transfer ownership), never by borrow.

```
items = vec![1, 2, 3]
n.spawn(items) { |items|  // items moved into closure
    process(items)
}
// items invalid here (moved)
```

| Capture Rule | Behavior |
|--------------|----------|
| Small types (≤16 bytes) | Implicit copy |
| Large types | Explicit move via parameter OR `.clone()` if caller needs to retain |
| Mutable captures | Forbidden (no `var` capture across tasks) |
| Borrow captures | Forbidden (violates no-storable-references) |

**Syntax:** `n.spawn(value) { |v| ... }` transfers ownership. To retain: `n.spawn(value.clone()) { |v| ... }`.

### Channels — Affine Endpoints

```
(tx, rx) = Channel<T>.new()

tx.send(value)?       // Consumes value, returns Result<(), SendError<T>>
value = rx.recv()?    // Blocks, returns Result<T, RecvError>

tx.close()            // Explicit close (consumes tx)
```

| Type | Cloneable? | Cost | Use Case |
|------|------------|------|----------|
| `Sender<T>` | No (affine) | Zero | Single producer |
| `Receiver<T>` | No (affine) | Zero | Single consumer |
| `SharedSender<T>` | Yes | Refcount (atomic ops) | Multiple producers |
| `SharedReceiver<T>` | Yes | Refcount (atomic ops) | Multiple consumers (broadcast) |

**Sharing:** Call `tx.share()` to convert `Sender<T>` → `SharedSender<T>` (visible cost).

**Error handling:**
- `send()` on closed channel: `Err(SendError::Closed(value))`
- `recv()` on closed empty channel: `Err(RecvError::Closed)`

### Linear Resources in Channels

Regular channels CANNOT carry linear types (compile error). Wrap in RAII type or consume before sending.

```
// COMPILE ERROR: File is linear
// (tx, rx) = Channel<File>.new()

// CORRECT: wrap in guard
struct FileGuard(File)
impl Drop for FileGuard {
    fn drop() { self.0.close().ok() }  // Best-effort cleanup
}

(tx, rx) = Channel<FileGuard>.new()
tx.send(FileGuard(open("data")?))?
```

**Rationale:** Channel drop with unconsumed linear resources cannot propagate errors (drop is infallible). Forbidding linear types in channels prevents silent resource leaks.

### Parallel Compute — Move Semantics

```
items = vec![1, 2, 3, 4]

results = parallel_map(items) { |item|
    compute(item)  // item moved to this unit
}
// items consumed (moved into parallel units)
```

| Primitive | Signature | Semantics |
|-----------|-----------|-----------|
| `parallel_map(items, f)` | `fn<T, U>(Vec<T>, fn(T) -> U) -> Vec<U>` | Consumes `items`, each `f` owns one element |
| `parallel_reduce(items, init, f)` | `fn<T, U>(Vec<T>, U, fn(U, T) -> U) -> U` | Consumes `items`, fold with ownership |
| `parallel_for(items, f)` | `fn<T>(Vec<T>, fn(T))` | Side-effect only, consumes `items` |

**Retain access:** Clone before parallel: `parallel_map(items.clone(), f)` (visible cost).

### Cancellation — Cooperative

```
h = n.spawn {
    loop {
        if cancelled() { break }
        work()
    }
}

h.cancel()    // Requests cancellation (non-blocking)
h.join()?     // Waits for task to acknowledge cancellation and exit
```

| Rule | Behavior |
|------|----------|
| `h.cancel()` | Sets task-local flag, returns immediately |
| `cancelled()` | Returns `true` if cancellation requested |
| Task ignores cancel | Runs to completion (cooperative model) |
| Nursery early exit | Cancels all children, waits for all `.join()` |

**No guarantee of termination:** Tasks may ignore cancellation (programmer responsibility, same as deadlock).

### Ensure + Tasks

Values with active `ensure` blocks CANNOT be moved into tasks.

```
file = open("data")?
ensure file.close()

// COMPILE ERROR: file has active ensure, cannot move
// n.spawn(file) { |f| ... }
```

**Workaround:** Transfer ownership to task, re-register ensure there:

```
file = open("data")?
// No ensure registered

n.spawn(file) { |f|
    ensure f.close()  // Register in new task
    process(f)
}.join()?
```

### Select — Multiplexing

```
result = select {
    case rx1.recv() -> |v| handle(v),
    case rx2.recv() -> |v| handle(v),
    case tx.send(msg) -> |()| sent(),
    timeout 5.seconds -> timed_out(),
}
```

| Arm Type | Syntax | Semantics |
|----------|--------|-----------|
| Receive | `case rx.recv() -> \|v\| expr` | Waits for receive, binds value |
| Send | `case tx.send(val) -> \|_\| expr` | Waits for send completion |
| Timeout | `timeout duration -> expr` | Fires after duration |
| Default | `default -> expr` | Non-blocking fallback |

**Ownership:** Non-selected send arms return value to caller (send not consumed). Selected arm transfers ownership as normal.

### Edge Cases

| Case | Handling |
|------|----------|
| Spawn outside nursery | Compile error (no `spawn` function, only `nursery.spawn` method) |
| TaskHandle not joined | Compile error (affine violation) |
| TaskHandle returned from nursery | Allowed; caller must consume (affine transfer) |
| Linear type in `Channel<T>` | Compile error (type bound restriction) |
| Channel drop with unconsumed items | Items dropped (best-effort cleanup, may leak if `T` linear) |
| Transfer value with active `ensure` | Compile error (ensure blocks move) |
| Ensure block hangs | Nursery exit hangs (programmer responsibility) |
| Panic in ensure | Logged, remaining ensures run (LIFO) |
| Panic in task | Propagates to `.join()` caller as `Err(Panicked)` |
| Deadlock | Not detected (programmer responsibility) |
| Cancellation ignored | Task runs to completion, `.join()` waits |
| Select with 0 arms | Compile error |
| Select with all closed channels | `recv` arms return `Err(Closed)` |
| Async I/O in sync task | Compile error (type mismatch, no async runtime) |
| Sync I/O in async task | Blocks runtime thread (allowed, documented) |

## Examples

### HTTP Server (Sync, ~1000 concurrent)

```
fn main() -> Result<()> {
    listener = TcpListener.bind("0.0.0.0:8080")?
    ensure listener.close()
    
    nursery { |n|
        loop {
            conn = listener.accept()?
            
            n.spawn(conn) { |conn|
                ensure conn.close()
                request = parse_request(conn.read()?)?
                response = handle(request)
                conn.write(response)?
            }
        }
    }
}
```

### Producer-Consumer Pipeline

```
fn pipeline(sources: Vec<LogSource>) -> Result<()> {
    (tx, rx) = Channel<Entry>.buffered(1000)
    shared_tx = tx.share()  // Explicit: refcounted sender
    
    nursery { |n|
        for source in sources {
            let tx_clone = shared_tx.clone()
            n.spawn(source, tx_clone) { |source, tx|
                for entry in source.read() {
                    tx.send(entry)?
                }
            }
        }
        
        drop(shared_tx)  // Close sender side
        
        let consumer = n.spawn(rx) { |rx|
            while let Ok(entry) = rx.recv() {
                write_disk(entry)?
            }
        }
        
        consumer.join()?
    }
}
```

### Parallel Map (Non-Copy Types)

```
fn process_images(images: Vec<Image>) -> Result<Vec<Thumbnail>> {
    // Images moved into parallel units (one per element)
    results = parallel_map(images) { |img|
        thumbnail(img)  // img owned by this closure
    }
    Ok(results)
}
```

### Async High-Concurrency Proxy

```
async fn proxy(listener: AsyncTcpListener) {
    async nursery { |n|
        loop {
            client = listener.async_accept()?
            
            n.async_spawn(client) { |client|
                ensure client.close()
                
                upstream = AsyncTcp.connect("backend:8080")?
                ensure upstream.close()
                
                // Bidirectional relay
                async nursery { |relay|
                    relay.async_spawn { pipe(client, upstream)? }
                    relay.async_spawn { pipe(upstream, client)? }
                }
            }
        }
    }
}
```

## Integration Notes

- **Memory model:** Channels and TaskHandles are affine types (consumed exactly once). Follows existing linear resource tracking.
- **Closures:** Task closures capture by move (consistent with "no storable references"). IDE shows capture list as ghost annotation.
- **Error handling:** `?` propagation works in nurseries; early return triggers cancellation of children and waits for joins.
- **Ensure cleanup:** Cannot move values with active ensure (prevents use-after-ensure). Transfer ownership to task, re-register there.
- **Type system:** `async fn` is a function color (bifurcates ecosystem). Accept tradeoff: sync for simplicity, async for scale.
- **Compiler:** Affine tracking (TaskHandle, Sender, Receiver) is local per-function. No cross-function lifetime analysis required.
- **Standard library:** `Channel`, `nursery`, `parallel_map` are built-ins. Thread pool for parallel primitives (bounded by CPU cores, explicit initialization).
- **Tooling:** IDE SHOULD show task capture modes, channel refcount transitions, and ensure block scopes as ghost annotations.