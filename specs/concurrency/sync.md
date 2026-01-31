# Sync Concurrency

OS threads with structured nurseries and ownership-transfer channels.

## Overview

This is the **default concurrency mode** for Rask. Most programs use this layer exclusively.

| Property | Value |
|----------|-------|
| Task type | OS threads |
| Scaling | ~1000 concurrent tasks |
| Cost | ~2MB stack per thread |
| I/O model | Blocking |

> **Syntax Note:** The exact keywords and binding syntax (e.g., `nursery { |n| ... }` vs `parallel p { ... }`) are TBD pending full language syntax design. The semantics described below are stable.

## Nurseries (Structured Concurrency)

All tasks MUST be spawned within a `nursery` block:

```
nursery { |n|
    h1 = n.spawn { work1() }
    h2 = n.spawn { work2() }
    h1.join()?
    h2.join()?
}
```

### Nursery Rules

| Rule | Enforcement |
|------|-------------|
| `spawn` returns `TaskHandle<T>` (affine type) | Must be consumed via `.join()` or `.cancel()` before nursery exit |
| Nursery exit waits for all children | Blocks until all spawned tasks complete |
| TaskHandle cannot escape nursery | Compile error if returned without consuming |
| Early nursery exit | Requests cancellation of all children, waits for joins |
| Spawn outside nursery | Compile error (no global `spawn` function) |

**Affine enforcement:** Unused `TaskHandle` at scope exit is compile error.

### Task Return Values

Tasks return values via `join()`:

```
nursery { |n|
    h = n.spawn { expensive_compute() }  // Returns i32
    // ... do other work ...
    result = h.join()?                   // result: i32
}
```

**Signatures:**

| Function | Signature |
|----------|-----------|
| `n.spawn { body }` | `fn(body: FnOnce() -> T) -> TaskHandle<T>` |
| `n.spawn(v) { \|v\| body }` | `fn(v: V, body: FnOnce(V) -> T) -> TaskHandle<T>` |
| `h.join()` | `fn(self) -> Result<T, TaskError>` |
| `h.cancel()` | `fn(&self) -> ()` |

**TaskHandle type:**

```
struct TaskHandle<T> {
    // Opaque, affine (cannot be cloned)
    // T is the task's return type
}
```

**Join semantics:**

| Scenario | `join()` returns |
|----------|------------------|
| Task completes normally | `Ok(value)` |
| Task panics | `Err(TaskError::Panicked(msg))` |
| Task was cancelled and exited | `Err(TaskError::Cancelled)` |

**Example with return value:**

```
fn parallel_sum(items: Vec<i32>) -> Result<i32, Error> {
    let (left, right) = items.split_at(items.len() / 2)

    nursery { |n|
        let h1 = n.spawn(left) { |l| l.iter().sum() }
        let h2 = n.spawn(right) { |r| r.iter().sum() }

        let sum1 = h1.join()?   // i32
        let sum2 = h2.join()?   // i32
        Ok(sum1 + sum2)
    }
}
```

**Discarding return values:**

If you don't need the return value, `join()` still works:

```
n.spawn { do_side_effect() }.join()?  // Returns ()
```

### Task Capture (Move Semantics)

Tasks capture by move, never by borrow:

```
items = vec![1, 2, 3]
n.spawn(items) { |items|  // items moved into closure
    process(items)
}
// items invalid here (moved)
```

| Capture Rule | Behavior |
|--------------|----------|
| Small types (<=16 bytes) | Implicit copy |
| Large types | Explicit move via parameter OR `.clone()` to retain |
| Mutable captures | Forbidden (no mutable capture across tasks) |
| Borrow captures | Forbidden (violates no-storable-references) |

**Syntax:** `n.spawn(value) { |v| ... }` transfers ownership.

### Cancellation (Cooperative)

```
h = n.spawn {
    loop {
        if cancelled() { break }
        work()
    }
}

h.cancel()    // Requests cancellation (non-blocking)
h.join()?     // Waits for task to acknowledge and exit
```

| Operation | Behavior |
|-----------|----------|
| `h.cancel()` | Sets task-local flag, returns immediately |
| `cancelled()` | Returns `true` if cancellation requested |
| Task ignores cancel | Runs to completion (cooperative model) |
| Nursery early exit | Cancels all children, waits for all `.join()` |

**No forced termination:** Tasks may ignore cancellation. This is programmer responsibility.

### Cancellation and Blocked Operations

Blocking operations (`recv()`, `send()`) check cancellation and return early:

```
h = n.spawn {
    loop {
        match rx.recv() {
            Ok(item) => process(item),
            Err(RecvError::Cancelled) => break,  // Task was cancelled
            Err(RecvError::Closed) => break,     // Channel closed
        }
    }
}

h.cancel()   // Wakes blocked recv(), which returns Cancelled
h.join()?
```

| Blocked Operation | On Cancellation |
|-------------------|-----------------|
| `recv()` | Returns `Err(RecvError::Cancelled)` |
| `send(v)` | Returns `Err(SendError::Cancelled(v))` (value returned) |
| `try_recv()` | Not blocking, unaffected |
| `try_send(v)` | Not blocking, unaffected |

**Still cooperative:** The task receives the error but can choose to ignore it:

```
n.spawn {
    loop {
        match rx.recv() {
            Err(RecvError::Cancelled) => {
                // Could break here, but choosing to continue
                if !critical_work_done {
                    continue  // Ignore cancellation
                }
                break
            }
            // ...
        }
    }
}
```

**Cancellation propagates immediately:** When `cancel()` is called, any currently-blocked operation wakes up and returns `Cancelled`. Operations started after cancellation also return `Cancelled` immediately.

### Panic Handling

When a task panics, structured concurrency ensures orderly shutdown:

```
nursery { |n|
    h1 = n.spawn { work1() }          // Panics!
    h2 = n.spawn { work2() }          // Continues running
    h1.join()?                        // Returns Err(TaskError::Panicked)
}
```

| Rule | Behavior |
|------|----------|
| **P1: Ensure blocks run** | All `ensure` blocks execute during panic unwind (LIFO order) |
| **P2: Siblings notified** | Sibling tasks receive cancellation request (not forced) |
| **P3: Nursery waits** | Nursery blocks until ALL children complete (including siblings) |
| **P4: Join returns error** | `h.join()` returns `Err(TaskError::Panicked(msg))` |
| **P5: Double panic swallowed** | If `ensure` panics during unwind, panic is logged and swallowed |

**Panic propagation order:**

1. Task panics
2. Task's `ensure` blocks execute (LIFO)
3. Siblings receive cancellation request
4. Nursery waits for all tasks to complete
5. First `join()` on panicked task returns `Err(Panicked)`

**Ensure during panic:**

```
n.spawn {
    let file = open("data")?
    ensure file.close()           // Runs even on panic
    ensure log("task ending")     // Runs first (LIFO)

    panic("oops")                 // Triggers unwind
}
// Order: log("task ending"), file.close()
```

**Double panic (ensure panics):**

```
n.spawn {
    ensure panic("ensure panic")  // Swallowed, logged to stderr
    panic("original")             // Original panic propagates
}
// Only original panic reaches join()
```

**Catching panics:**

```
nursery { |n|
    h = n.spawn { risky_operation() }

    match h.join() {
        Ok(result) => process(result),
        Err(TaskError::Panicked(msg)) => {
            log("Task panicked: {msg}")
            // Recovery logic
        }
        Err(TaskError::Cancelled) => { /* ... */ }
    }
}
```

**TaskError type:**

```
enum TaskError {
    Panicked(string),    // Task panicked, message captured
    Cancelled,           // Task was cancelled
}
```

### Ensure + Tasks

Values with active `ensure` blocks CANNOT be moved into tasks:

```
file = open("data")?
ensure file.close()

// COMPILE ERROR: file has active ensure, cannot move
// n.spawn(file) { |f| ... }
```

**Workaround:** Transfer ownership, re-register ensure in task:

```
file = open("data")?
n.spawn(file) { |f|
    ensure f.close()
    process(f)
}.join()?
```

## Channels

### Creation

| Constructor | Capacity | Blocking Behavior |
|------------|----------|-------------------|
| `Channel<T>.unbounded()` | Unlimited | Never blocks on send |
| `Channel<T>.buffered(n)` | Fixed size `n` | Blocks send when full |
| `Channel<T>.rendezvous()` | 0 (no buffer) | Blocks until receiver ready |

### Basic Operations

```
(tx, rx) = Channel<T>.buffered(100)

tx.send(value)?       // Blocks if full
value = rx.recv()?    // Blocks until available
tx.close()            // Explicit close (consumes tx)
```

### Endpoint Types

| Type | Cloneable? | Cost | Use Case |
|------|------------|------|----------|
| `Sender<T>` | No (affine) | Zero | Single producer |
| `Receiver<T>` | No (affine) | Zero | Single consumer |
| `SharedSender<T>` | Yes | Refcount (atomic) | Multiple producers |
| `SharedReceiver<T>` | Yes | Refcount (atomic) | Multiple consumers (work-stealing) |

### Sharing Conversion

`share()` CONSUMES the affine endpoint:

```
(tx, rx) = Channel<i32>.buffered(10)
shared_tx = tx.share()  // tx MOVED, invalid after
tx1 = shared_tx.clone() // refcount = 2
```

| Conversion | Consumes? | Reversible? |
|------------|-----------|-------------|
| `tx.share()` -> `SharedSender<T>` | Yes | No |
| `rx.share()` -> `SharedReceiver<T>` | Yes | No |

**Mixed patterns:** All combinations allowed (1-to-1, 1-to-many, many-to-1, many-to-many).

**SharedReceiver semantics:** Work-stealing. Each value received by exactly one consumer. NOT broadcast.

### Channel Closure

| Operation | Consumes? | Blocks? | Effect |
|-----------|-----------|---------|--------|
| `tx.close()` | Yes | No | Marks send-side closed, wakes receivers |
| `rx.close()` | Yes | No | Marks receive-side closed, wakes senders |
| Drop | Yes | No | Same as explicit close |

**Pending operations on close:**

| Scenario | Behavior |
|----------|----------|
| `recv()` blocked, send-side closes | Returns buffered value OR `Err(Closed)` |
| `send()` blocked, recv-side closes | Returns `Err(SendError::Closed(value))` |

**Buffered values preserved:** Closing send-side does NOT discard buffered values. Receivers drain buffer before getting `Err(Closed)`.

**Deallocation:** Channel freed when both sides closed AND buffer empty.

### Error Types

```
enum SendError<T> {
    Closed(T),        // Channel closed, value returned
    Cancelled(T),     // Task cancelled while blocked, value returned
    OutOfMemory(T),   // Unbounded only, allocation failed
}

enum RecvError {
    Closed,      // Channel closed and empty
    Cancelled,   // Task cancelled while blocked
    Empty,       // try_recv() only: no value ready
}

enum TaskError {
    Panicked(string),   // Task panicked, message captured
    Cancelled,          // Task acknowledged cancellation
}
```

### Linear Resources in Channels

Channels CANNOT carry linear types (compile error):

```
// COMPILE ERROR: File is linear
// (tx, rx) = Channel<File>.new()
```

**Workaround:** Wrap in RAII type:

```
struct FileGuard(File)
impl Drop for FileGuard {
    fn drop() { self.0.close().ok() }  // Best-effort cleanup
}
```

**Known issue:** This silences close errors. See Remaining Issues.

## Daemons

Long-running background tasks that run for program lifetime. Unlike nursery tasks, daemons are **not** structured concurrency — they run independently until program exit.

### Basic Usage

```
fn main() -> Result<()> {
    // Fire-and-forget (default: ignore crashes)
    spawn_daemon { optional_telemetry() }

    // Restart on crash (unlimited restarts)
    spawn_daemon(.restart) { logger_loop() }

    // Restart with limits (5 attempts in 60s, then give up)
    spawn_daemon(.restart(max: 5, within: 60.seconds)) { flaky_service() }

    // Critical — crash brings down program
    spawn_daemon(.shutdown) { critical_watchdog() }

    // Normal structured concurrency
    nursery { |n|
        // Application logic...
    }
}
// When main() exits, all daemons are cancelled
```

### Daemon Rules

| Rule | Behavior |
|------|----------|
| **D1: Program lifetime** | Daemons run until `main()` returns or panics |
| **D2: No TaskHandle** | `spawn_daemon` returns nothing (or optional `DaemonHandle` for manual stop) |
| **D3: No parent** | Daemons are not part of any nursery |
| **D4: Auto-cancelled on exit** | All daemons receive cancellation when program exits |
| **D5: Crash policy optional** | Default: `.ignore` (log and let daemon stay dead) |
| **D6: No return value** | Daemon body returns `()` — they're not meant to complete |

### Crash Policies

| Policy | Behavior |
|--------|----------|
| `.ignore` | Log the crash and let daemon stay dead **(default)** |
| `.restart` | Restart immediately, unlimited attempts |
| `.restart(max: N, within: Duration)` | Restart up to N times within duration, then `.ignore` |
| `.shutdown` | Initiate program shutdown |

```
spawn_daemon { optional_telemetry() }                              // Default: ignore
spawn_daemon(.restart) { logger_loop() }                           // Restart forever
spawn_daemon(.restart(max: 5, within: 60.seconds)) { flaky() }     // 5 tries, then give up
spawn_daemon(.shutdown) { db_connection() }                        // Critical — crash = exit
```

### Custom Crash Handler

For complex crash handling logic:

```
spawn_daemon(|error| {
    alert("Daemon crashed: {error}")
    if error.is_recoverable() {
        .restart
    } else {
        .ignore   // Give up on this daemon
    }
}) {
    important_service()
}
```

### Optional: DaemonHandle

If you need to stop a daemon manually:

```
let logger = spawn_daemon(.restart) { logger_loop() }

// Later, during graceful shutdown:
logger.stop()   // Request cancellation
```

**No `.join()`** — daemons don't return values. `stop()` is fire-and-forget.

### Daemon vs Nursery Task

| Aspect | Nursery Task | Daemon |
|--------|--------------|--------|
| Lifetime | Until nursery exits | Until program exits |
| Return value | Yes, via `join()` | No |
| Parent tracking | Yes (structured) | No (independent) |
| Crash handling | Propagates to `join()` | Crash policy |
| Must be joined | Yes (affine) | No |
| Spawn location | Inside nursery only | Anywhere |

**Why no spawn restriction?** Daemons can be spawned anywhere, not just `main()`. The limitations above (no return, no join, program lifetime) naturally discourage misuse — using daemons for regular work items is painful. If you need results, coordination, or scoped lifetime, you'll reach for nursery tasks.

### Example: Application with Services

```
fn main() -> Result<()> {
    // Background services
    spawn_daemon(.restart) {
        loop {
            flush_logs()
            sleep(1.second)
        }
    }

    spawn_daemon(.restart(max: 10, within: 60.seconds)) {
        loop {
            report_metrics()
            sleep(10.seconds)
        }
    }

    spawn_daemon(.shutdown) {
        loop {
            if !check_health() {
                panic("Health check failed")  // Triggers program shutdown
            }
            sleep(30.seconds)
        }
    }

    // Main server
    let server = start_server()?
    ensure server.shutdown()

    nursery { |n|
        loop {
            let conn = server.accept()?
            n.spawn(conn) { |c| handle_request(c) }
        }
    }
}
```

## Examples

### HTTP Server (~1000 concurrent)

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
    shared_tx = tx.share()

    nursery { |n|
        for source in sources {
            let tx_clone = shared_tx.clone()
            n.spawn(source, tx_clone) { |source, tx|
                for entry in source.read() {
                    tx.send(entry)?
                }
            }
        }

        drop(shared_tx)  // Signal "no more sends"

        n.spawn(rx) { |rx|
            while let Ok(entry) = rx.recv() {
                write_disk(entry)?
            }
        }.join()?
    }
}
```

## Edge Cases

| Case | Handling |
|------|----------|
| TaskHandle not joined | Compile error (affine violation) |
| Linear type in Channel<T> | Compile error |
| Channel drop with items | Items dropped (best-effort) |
| Transfer with active ensure | Compile error |
| Panic in task | Propagates to `.join()` as `Err(Panicked)` |
| Deadlock | Not detected (programmer responsibility) |

---

## Remaining Issues

### High Priority

1. **Linear types + channels silent failure**
   - Wrapping in RAII silences close errors
   - Violates "errors are visible" principle
   - Need alternative pattern or error channel

### Medium Priority

2. **Nursery nesting rules**
   - Can nurseries nest? (Appears yes from examples)
   - Can handles pass between nested nurseries?
   - How does cancellation propagate through nested nurseries?

3. **Thread pool and resource limits**
   - Is there a thread pool or new thread per spawn?
   - Can stack size be configured?
   - What happens if OS thread limit reached?

### Low Priority

4. **Channel drop with items**
   - What exactly does "best-effort" drop mean?
   - Are items guaranteed to be dropped?

5. **Early nursery exit on break/return**
   - Timeout for waiting on children?
   - What if children deadlock?
