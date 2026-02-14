<!-- id: conc.async -->
<!-- status: decided -->
<!-- summary: Green tasks with affine handles, no function coloring, explicit resource declaration -->
<!-- depends: memory/ownership.md, memory/context-clauses.md, memory/resource-types.md -->

# Execution Model

Green tasks with affine handles. No function coloring. Explicit resource declaration.

## Spawn Constructs

| Rule | Description |
|------|-------------|
| **S1: Green task** | `spawn(|| {})` creates a green task; requires `using Multitasking` |
| **S2: Pooled thread** | `ThreadPool.spawn(|| {})` runs on thread pool; requires `using ThreadPool` |
| **S3: Raw thread** | `Thread.spawn(|| {})` creates OS thread; no context required |
| **S4: Affine handle** | All spawn forms return affine handles — must be joined or detached |

```rask
func main() -> () or Error {
    using Multitasking {
        const listener = try TcpListener.bind("0.0.0.0:8080")

        loop {
            const conn = try listener.accept()
            spawn(|| { handle_connection(conn) }).detach()
        }
    }
}

func handle_connection(conn: TcpConnection) -> () or Error {
    const request = try conn.read()
    const user = try fetch_user(request.id)
    try conn.write(user.to_json())
}
```

## Affine Handles

| Rule | Description |
|------|-------------|
| **H1: Must consume** | `TaskHandle<T>` must be joined or detached — compile error if dropped |
| **H2: Join** | `h.join()` waits for result, returns `T or JoinError`, consumes handle |
| **H3: Detach** | `h.detach()` opts out of tracking (fire-and-forget), consumes handle |
| **H4: Cancel** | `h.cancel()` requests cooperative cancellation, waits for exit, returns `T or JoinError` |

<!-- test: skip -->
```rask
// Propagate errors
const h = spawn(|| { compute() })
const result = try h.join()

// Panic on task failure
const h = spawn(|| { work() })
h.join()!

// Handle explicitly
const h = spawn(|| { fallible_work() })
match h.join() {
    Ok(val) => process(val),
    Err(JoinError.Panicked(msg)) => println("task panicked: {msg}"),
    Err(JoinError.Cancelled) => println("task was cancelled"),
}

spawn(|| { background_work() }).detach()

spawn(|| { work() })  // ERROR [conc.async/H1]: unused TaskHandle
```

### Handle API

<!-- test: skip -->
```rask
struct TaskHandle<T> { }

extend TaskHandle<T> {
    func join(take self) -> T or JoinError
    func detach(take self)
    func cancel(take self) -> T or JoinError
}

enum JoinError {
    Panicked(string),  // task panicked with message
    Cancelled,         // task was cancelled
}
```

## Multiple Tasks

| Rule | Description |
|------|-------------|
| **M1: Join all** | `join_all(...)` waits for all tasks |
| **M2: Select first** | `select_first(...)` returns first result, cancels remaining |
| **M3: Task group** | `TaskGroup` for dynamic task counts |

<!-- test: skip -->
```rask
let (a, b) = join_all(
    spawn(|| { work1() }),
    spawn(|| { work2() })
)

const group = TaskGroup.new()
for url in urls {
    group.spawn(|| { fetch(url) })
}
const results = try group.join_all()
```

## Context Resources

| Rule | Description |
|------|-------------|
| **C1: Multitasking** | `using Multitasking { }` provides M:N green task scheduler + I/O event loop |
| **C2: ThreadPool** | `using ThreadPool { }` provides thread pool for CPU-bound work |
| **C3: Composable** | `using Multitasking, ThreadPool { }` enables both |
| **C4: Block exit** | Exiting a `using` block waits for non-detached tasks |

<!-- test: skip -->
```rask
using Multitasking(workers: 4) { }
using ThreadPool(workers: 8) { }
using Multitasking, ThreadPool { }
```

| Setup | Green Tasks | Thread Pool | Use Case |
|-------|-------------|-------------|----------|
| `using Multitasking` | Yes | No | I/O-heavy servers |
| `using ThreadPool` | No | Yes | CLI tools, batch processing |
| `using Multitasking, ThreadPool` | Yes | Yes | Full-featured applications |

## I/O Model

| Rule | Description |
|------|-------------|
| **IO1: Transparent pausing** | Stdlib I/O pauses the task, not the thread — no `.await` needed |
| **IO2: Sync fallback** | Without `using Multitasking`, I/O blocks the calling thread |

```rask
func process_file(path: string) -> Data or Error {
    const file = try File.open(path)
    const contents = try file.read_all()
    parse(contents)
}
```

I/O flow: function calls stdlib → stdlib issues non-blocking syscall → scheduler parks task → other tasks run → I/O completes → task wakes. IDEs show pause points as ghost annotations: `⟨pauses⟩`.

## Join Semantics

| Rule | Description |
|------|-------------|
| **J1: Context-dependent** | `.join()` pauses the green task (scheduler runs others) or blocks the thread (sync mode) |

| Calling from | `.join()` behavior |
|--------------|-------------------|
| Green task | Pauses task (scheduler runs others) |
| Sync mode | Blocks thread |

**Error handling:**
```rask
try h.join()          // propagate JoinError
h.join()!             // panic if task panicked
match h.join() { }    // explicit handling
```

## Cancellation

| Rule | Description |
|------|-------------|
| **CN1: Cooperative** | Cancellation sets a flag; task checks `cancelled()` |
| **CN2: Ensure runs** | `ensure` blocks always run, even on cancellation |
| **CN3: I/O checks** | I/O operations check cancel flag and return `Err(Cancelled)` if set |

<!-- test: skip -->
```rask
const h = spawn(|| {
    const file = try File.open("data.txt")
    ensure file.close()

    loop {
        if cancelled() { break })
        do_work()
    }
}

sleep(5.seconds)
try h.cancel()
```

## Channels

| Rule | Description |
|------|-------------|
| **CH1: Non-linear** | `Sender<T>` and `Receiver<T>` can be dropped without explicit close |
| **CH2: Buffered/unbuffered** | `Channel<T>.unbuffered()` (sync) or `Channel<T>.buffered(n)` (async buffer) |
| **CH3: Close on drop** | Dropping sender/receiver implicitly closes; errors silently ignored |
| **CH4: Explicit close** | `tx.close()` / `rx.close()` return `Result` for error handling |

<!-- test: skip -->
```rask
let (tx, rx) = Channel<Message>.buffered(100)

const producer = spawn(|| {
    for msg in generate_messages() {
        try tx.send(msg)
    })
}

const consumer = spawn(|| {
    while rx.recv() is Ok(msg) {
        process(msg)
    })
}

try join_all(producer, consumer)
```

### Channel Operations

| Operation | Returns | Description |
|-----------|---------|-------------|
| `tx.send(val)` | `() or SendError` | Send value, pauses/blocks if full |
| `rx.recv()` | `T or RecvError` | Receive value, pauses/blocks if empty |
| `tx.close()` | `() or CloseError` | Explicit close with error handling |
| `rx.close()` | `() or CloseError` | Explicit close with error handling |
| `tx.try_send(val)` | `() or TrySendError` | Non-blocking send |
| `rx.try_recv()` | `T or TryRecvError` | Non-blocking receive |

### Buffered Items on Drop

| Scenario | Behavior |
|----------|----------|
| Sender dropped, buffer has items | Items remain — receivers can drain |
| All senders dropped | Channel closed for writing, readable until empty |
| Receiver dropped, buffer has items | Items dropped (lost) |
| All receivers dropped | Senders get `Err(Closed)` on next send |

## Error Messages

```
ERROR [conc.async/H1]: unused TaskHandle
   |
12 |  spawn(|| { work() })
   |  ^^^^^^^^^^^^^^^^ TaskHandle must be joined or detached
```

```
ERROR [conc.async/S1]: spawn requires Multitasking context
   |
5  |  spawn(|| { fetch(url) })
   |  ^^^^^ no `using Multitasking` in scope

FIX: using Multitasking { spawn(|| { fetch(url) }).detach() }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Spawn outside `using Multitasking` | S1 | Compile error |
| `.join()` on cancelled task | H2, CN1 | Returns `Err(Cancelled)` |
| Channel send after all receivers dropped | CH3 | Returns `Err(Closed)` |
| Nested `using Multitasking` blocks | C1 | Compile error — one scheduler per program |
| Detached task outlives `using` block | C4 | Detached tasks run to completion independently |

---

## Appendix (non-normative)

### Rationale

**S4 (affine handles):** I wanted compile-time tracking of spawned tasks. Go's fire-and-forget `go` is ergonomic but loses track of goroutines — forgotten tasks are silent bugs. Affine handles make the choice explicit: `.join()` or `.detach()`.

**IO1 (transparent pausing):** No function coloring means no ecosystem split. The same function works whether called from a green task or sync context. IDEs show pause points — transparency through tooling, not syntax.

**CH1 (non-linear channels):** Channels aren't `@resource` types. Fire-and-forget patterns (`.detach()` tasks) would require close ceremony. Go's channels drop without explicit close. Matches `ensure` philosophy — explicit handling available, implicit path simple.

### Comparison with Go

| Aspect | Go | Rask |
|--------|-----|------|
| Spawn syntax | `go func()` | `spawn(|| { }).detach()` |
| Track tasks | Manual (WaitGroup) | Compile-time (affine) |
| Forgotten tasks | Silent | Compile error |
| Function coloring | No | No |

### Channel Error Types

<!-- test: skip -->
```rask
enum SendError { Closed }
enum RecvError { Closed }
enum CloseError { AlreadyClosed, FlushFailed }
enum TrySendError { Full(T), Closed(T) }
enum TryRecvError { Empty, Closed }
```

### Architecture

```
┌─────────────────────────────────────────────────┐
│                  Multitasking                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│  │ Thread 1 │ │ Thread 2 │ │ Thread N │        │
│  │   ◇◇◇◇   │ │   ◇◇◇◇   │ │   ◇◇◇◇   │        │
│  └──────────┘ └──────────┘ └──────────┘        │
└─────────────────────────────────────────────────┘
  ◇ = green task (concurrent, interleaved)
```

### Metrics Validation

| Metric | Target | This Design |
|--------|--------|-------------|
| TC (Transparency) | >= 0.90 | `using Multitasking`, `using ThreadPool`, spawns all visible |
| ED (Ergonomic Delta) | <= 1.2 | Close to Go ergonomics |
| SN (Syntactic Noise) | <= 0.30 | No `.await`, no boilerplate |
| MC (Mechanical Correctness) | >= 0.90 | Affine handles catch forgotten tasks |

### See Also

- `conc.select` — select and multiplex
- `conc.sync` — synchronization primitives
- `mem.context` — `using` clauses for pool contexts
- `mem.resources` — `@resource` types and `ensure` cleanup
