# Execution Model

Green tasks with affine handles. No function coloring. Explicit resource declaration.

## Overview

Rask uses **green tasks** (lightweight coroutines) for concurrent I/O-bound work. No `async`/`await` syntax—functions work the same way regardless of whether they do I/O.

| Property | Value |
|----------|-------|
| Green tasks | ~4KB each, 100k+ concurrent |
| I/O model | Non-blocking (multitasking handles pausing) |
| Function coloring | **None** |
| Task tracking | Affine handles (compile-time) |

## Three Spawn Constructs

| Construct | Purpose | Requires | Pauses? |
|-----------|---------|----------|---------|
| `spawn { }` | Green task | `with multitasking` | Yes (at I/O) |
| `spawn_thread { }` | Thread from pool | `with threading` | No |
| `spawn_raw { }` | Raw OS thread | Nothing | No |

**All return affine handles**—must be joined or detached (compile error if forgotten).

### Naming Rationale

- `multitasking` — describes capability (cooperative green tasks, M:N scheduling)
- `threading` — describes capability (thread pool), consistent with `multitasking`
- `spawn_*` — consistent spawn family:
  - `spawn` — green task (requires `with multitasking`)
  - `spawn_thread` — pooled thread (requires `with threading`)
  - `spawn_raw` — raw OS thread (works anywhere)

## Concurrency vs Parallelism

| Concept | What it means | Rask construct |
|---------|--------------|----------------|
| **Concurrency** | Interleaved execution | Green tasks via `spawn { }` |
| **Parallelism** | Simultaneous execution | Thread pool via `spawn_thread { }` |

Green tasks are **concurrent, not parallel**. 100k tasks can be in-flight, but they interleave on a few OS threads. CPU-bound work needing true parallelism requires `spawn_thread { }`.

## Basic Usage

```rask
func fetch_user(id: u64) -> User or Error {
    const response = try http_get(format("/users/{id}"))  // Pauses task, not thread
    parse_user(response)
}

func main() -> () or Error {
    with multitasking {
        const listener = try TcpListener.bind("0.0.0.0:8080")

        loop {
            const conn = try listener.accept()
            spawn { handle_connection(conn) }.detach()  // Fire-and-forget
        }
    }
}

func handle_connection(conn: TcpConnection) -> () or Error {
    const request = try conn.read()
    const user = try fetch_user(request.id)
    try conn.write(user.to_json())
}
```

**Key points:**
- `with multitasking { }` enables green tasks
- `spawn { }` returns `TaskHandle` (affine type)
- `.detach()` opts out of tracking (fire-and-forget)
- No `.await`, no `async` keywords
- 100k concurrent connections supported

## Task Spawning

### Affine Handles

`spawn { }` returns a `TaskHandle<T>` that **must be consumed**.

```rask
// Get result - must join
const h = spawn { compute() }
const result = try h.join()

// Fire and forget - explicit detach
spawn { background_work() }.detach()

// Compile error - handle not consumed
spawn { work() }  // ERROR: unused TaskHandle
```

| Pattern | Syntax | Handle consumed? |
|---------|--------|------------------|
| Wait for result | `try spawn { }.join()` | Yes |
| Fire-and-forget | `spawn { }.detach()` | Yes |
| Unused | `spawn { }` | **Compile error** |

### Multiple Tasks

```rask
// Wait for all
let (a, b) = join_all(
    spawn { work1() },
    spawn { work2() }
)

// First to complete
const result = select_first(
    spawn { fast_path() },
    spawn { slow_path() }
)  // Remaining task cancelled
```

### TaskGroup (Dynamic Spawning)

For loops or dynamic number of tasks:

```rask
const group = TaskGroup.new()

for url in urls {
    group.spawn { fetch(url) }
}

const results = try group.join_all()  // Vec<Result<T>>
```

### Handle API

```rask
struct TaskHandle<T> {
    // Affine - cannot be cloned
}

extend TaskHandle<T> {
    func join(take self) -> T or TaskError    // Wait and get result
    func detach(take self)                           // Fire-and-forget
    func cancel(take self) -> T or TaskError  // Request cancel, wait
}
```

## Multitasking

### Setup

```rask
func main() {
    with multitasking {
        run_server()
    }
}
```

The `with` block creates and scopes the multitasking scheduler. No explicit construction needed.

Configuration:

```rask
with multitasking(4) { }              // 4 scheduler threads
with threading(8) { }                    // 8 pool threads
with multitasking(4), threading(8) { }  // Both
```

### What Multitasking Provides

| Component | Purpose |
|-----------|---------|
| M:N Scheduler | Many green tasks on few OS threads |
| I/O Event Loop | epoll/kqueue for non-blocking I/O |

**Default:** Scheduler threads = num_cpus

### Architecture

```rask
┌─────────────────────────────────────────────────┐
│                  Multitasking                    │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│  │ Thread 1 │ │ Thread 2 │ │ Thread N │        │
│  │   ◇◇◇◇   │ │   ◇◇◇◇   │ │   ◇◇◇◇   │        │
│  └──────────┘ └──────────┘ └──────────┘        │
└─────────────────────────────────────────────────┘
  ◇ = green task (concurrent, interleaved)
```

### Lifecycle

1. `with multitasking { }` creates scheduler (threads start lazily)
2. Multitasking is ambient within the block
3. `spawn { }` uses the ambient scheduler
4. Block exit waits for non-detached tasks

## How I/O Works

**Stdlib I/O automatically pauses tasks.**

```rask
func process_file(path: string) -> Data or Error {
    const file = try File.open(path)      // Pauses while opening
    const contents = try file.read_all()  // Pauses while reading
    parse(contents)
}
```

No `.await` needed. Stdlib handles pausing internally:

1. Function calls `file.read_all()`
2. Stdlib issues non-blocking syscall
3. If not ready, scheduler parks task
4. Scheduler runs other tasks
5. When I/O completes, scheduler wakes task
6. Function continues from where it paused

**IDEs show pause points as ghost annotations:**

```rask
const data = try file.read()  // IDE shows: ⟨pauses⟩
```

No code ceremony. Transparency through tooling.

## Thread Pool (CPU Parallelism)

For CPU-bound work that needs true parallelism, use an explicit thread pool:

```rask
func main() {
    with multitasking, threading {
        try spawn {
            const data = try fetch(url)                              // I/O - pauses
            const result = try spawn_thread { analyze(data) }.join()  // CPU on threads
            try save(result)                                       // I/O - pauses
        }.join()
    }
}
```

### Why Separate Thread Pool?

Without thread pool, CPU-heavy code starves other tasks:

```rask
spawn { cpu_intensive() }.detach()  // BAD: Hogs scheduler thread
spawn { handle_io() }.detach()       // Starved!
```

With a thread pool:

```rask
with multitasking, threading {
    spawn {
        try spawn_thread { cpu_intensive() }.join()  // Runs on thread pool
    }.detach()
    spawn { handle_io() }.detach()                  // Runs fine
}
```

### Threads API

```rask
with threading {
    // Spawn on thread pool, get handle
    const h = spawn_thread { work() }
    const result = try h.join()

    // Fire-and-forget
    spawn_thread { background() }.detach()
}
```

| Method | Returns | Description |
|--------|---------|-------------|
| `spawn_thread { expr }` | `ThreadHandle<T>` | Run on thread pool, return handle |

### Thread Pool Without Multitasking

Thread pool works independently for pure CPU-parallelism—CLI tools, batch processing.

```rask
func main() {
    with threading {
        const handles = files.map { |f|
            spawn_thread { process(f) }
        }
        for h in handles {
            print(try h.join())
        }
    }
}
```

| Setup | Green Tasks | Thread Pool | Use Case |
|-------|-------------|-------------|----------|
| `with multitasking` | Yes | No | I/O-heavy servers |
| `with threading` | No | Yes | CLI tools, batch processing |
| `with multitasking, threading` | Yes | Yes | Full-featured applications |

## Raw OS Thread

For code requiring thread affinity (OpenGL, thread-local FFI):

```rask
const h = spawn_raw {
    init_graphics_context()  // Needs stable thread identity
    render_loop()
}
try h.join()
```

Same affine handle rules apply. Works anywhere (no multitasking or threading required).

## Sync Mode (Default)

Without multitasking, I/O operations block the thread.

```rask
func main() {
    // No Multitasking = sync mode (default)
    const data = try file.read()  // Blocks thread

    // Thread pool still works
    with threading {
        const handles = files.map { |f| spawn_thread { process(f) } }
        for h in handles { try h.join() }
    }
}
```

| Feature | With Multitasking | Without Multitasking (default) |
|---------|-------------------|--------------------------------|
| `spawn { }` | Green tasks | Compile error (no scheduler) |
| `spawn_thread { }` | Thread pool | Thread pool (same) |
| Stdlib I/O | Pauses task | Blocks thread |

No special attribute needed. The presence of `with multitasking { }` is the opt-in.

## Join Semantics

`.join()` behavior depends on calling context:

| Calling from | `.join()` behavior |
|--------------|-------------------|
| Green task | **Pauses** the task (scheduler runs others) |
| Sync mode | **Blocks** the thread |

This is consistent with all wait operations (I/O, channels, etc.).

```rask
with multitasking, threading {
    spawn {
        const h = spawn_thread { cpu_work() }
        try h.join()  // YIELDS the green task, doesn't block scheduler
    }
}
```

## Comparison with Go

```go
// Go - fire and forget, no tracking
go handleRequest(conn)
```

```rask
// Rask - explicit detach required
spawn { handle_request(conn) }.detach()
```

| Aspect | Go | Rask |
|--------|-----|------|
| Spawn syntax | `go func()` | `spawn { }.detach()` |
| Track tasks | Manual (WaitGroup) | Compile-time (affine) |
| Forgotten tasks | Silent | **Compile error** |
| Function coloring | No | No |

Safety difference—Rask catches forgotten tasks at compile time.

## Cancellation

Cooperative model with cleanup guarantees.

```rask
const h = spawn {
    const file = try File.open("data.txt")
    ensure file.close()       // ALWAYS runs, even on cancel

    loop {
        if cancelled() { break }
        do_work()
    }
}  // ensure runs here

sleep(5.seconds)
try h.cancel()  // Request cancellation, wait for exit
```

**Rules:**
- `ensure` blocks always run—cancellation doesn't skip cleanup
- Cancellation is cooperative—task checks `cancelled()` flag
- If task ignores flag, it keeps running
- I/O operations check flag and return `Err(Cancelled)` if set
- Linear resources handled by `ensure` blocks

## Channels

Channels work in both modes.

| Mode | Channel behavior |
|------|------------------|
| With multitasking | Pauses task on send/recv |
| Without multitasking | Blocks thread on send/recv |

```rask
let (tx, rx) = Channel<Message>.buffered(100)

const producer = spawn {
    for msg in generate_messages() {
        try tx.send(msg)  // Pauses if buffer full
    }
}

const consumer = spawn {
    while rx.recv() is Ok(msg) {  // Pauses if buffer empty
        process(msg)
    }
}

try join_all(producer, consumer)
```

Channels are useful for inter-thread communication even without green tasks.

### Channel Types

```rask
struct Sender<T> { ... }     // NOT linear - can be dropped
struct Receiver<T> { ... }   // NOT linear - can be dropped
```

**Channel handles are NOT linear resource types.** They can be dropped without explicit close.

Why: Fire-and-forget patterns (`.detach()` tasks) would require close ceremony. Go's channels drop without explicit close (ED ≤ 1.2). Matches `ensure` philosophy—explicit handling available, implicit path simple.

### Channel Creation

| Constructor | Description |
|-------------|-------------|
| `Channel<T>.unbuffered()` | Synchronous - send pauses until recv |
| `Channel<T>.buffered(n)` | Async buffer of size n |

### Channel Operations

| Operation | Returns | Description |
|-----------|---------|-------------|
| `tx.send(val)` | `Result<(), SendError>` | Send value, pauses/blocks if full |
| `rx.recv()` | `Result<T, RecvError>` | Receive value, pauses/blocks if empty |
| `tx.close()` | `Result<(), CloseError>` | Explicit close with error handling |
| `rx.close()` | `Result<(), CloseError>` | Explicit close with error handling |
| `tx.try_send(val)` | `Result<(), TrySendError>` | Non-blocking send |
| `rx.try_recv()` | `Result<T, TryRecvError>` | Non-blocking receive |

### Close and Drop Semantics

Explicit `close()` for error handling, implicit drop ignores errors.

| Action | Behavior |
|--------|----------|
| `tx.close()` | Explicit close, returns `Result` for error handling |
| `rx.close()` | Explicit close, returns `Result` for error handling |
| `tx` dropped | Implicit close, errors silently ignored |
| `rx` dropped | Implicit close, errors silently ignored |

Matches `ensure` semantics—explicit handling when needed, simple implicit path.

**Example - explicit close when errors matter:**
```rask
func reliable_producer(tx: Sender<Data>) -> () or Error {
    for item in items {
        try tx.send(item)
    }
    try tx.close()  // Explicit: propagate close errors
    Ok(())
}
```

**Example - implicit close (fire-and-forget):**
```rask
spawn {
    for item in items {
        try tx.send(item)
    }
    // tx drops here - close errors ignored
}.detach()
```

### Buffered Items on Drop

| Scenario | Behavior |
|----------|----------|
| Sender dropped, buffer has items | Items remain - receivers can drain them |
| All senders dropped | Channel closed for writing, readable until empty |
| Receiver dropped, buffer has items | Items are **dropped** (lost) |
| All receivers dropped | Senders get `Err(Closed)` on next send |

Standard MPSC/MPMC semantics. Items aren't "lost" unless all receivers are gone.

**Example - draining a closed channel:**
```rask
let (tx, rx) = Channel<i32>.buffered(10)

try spawn {
    for i in 0..5 {
        try tx.send(i)
    }
    // tx drops, channel closed for writing
}.join()

// Can still drain remaining items
while rx.recv() is Ok(item) {
    process(item)
}
// Eventually: Err(Closed) when buffer empty
```

### Error Types

```rask
enum SendError {
    Closed,           // All receivers dropped
}

enum RecvError {
    Closed,           // All senders dropped AND buffer empty
}

enum CloseError {
    AlreadyClosed,    // Channel already closed
    FlushFailed,      // Buffered data couldn't be flushed (rare)
}

enum TrySendError {
    Full(T),          // Buffer full, value returned
    Closed(T),        // Channel closed, value returned
}

enum TryRecvError {
    Empty,            // Buffer empty, no senders blocked
    Closed,           // Channel closed
}
```

### Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Channel linearity | Non-linear | Linear | Fire-and-forget ergonomics (ED ≤ 1.2) |
| Close on drop | Implicit, errors ignored | Require explicit close | Matches `ensure` philosophy |
| Buffered items on rx drop | Dropped (lost) | Error, keep forever | Standard semantics, predictable |
| Close error handling | Explicit `close()` method | Always propagate | User chooses when errors matter |

## Select

Wait on multiple operations.

```rask
loop {
    select {
        rx1 -> msg: handle_a(msg),
        rx2 -> msg: handle_b(msg),
        Timer.after(5.seconds) -> _: handle_timeout(),
    }
}
```

---

## Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Task grouping | Affine handles | Mandatory nursery | Ergonomics (ED <= 1.2) |
| Fire-and-forget | Explicit `.detach()` | Implicit (Go-style) | Safety (MC >= 0.90) |
| Function coloring | None | async/await | No ecosystem split |
| Green task keyword | `multitasking` | `runtime` | More intuitive |
| Thread pool keyword | `threading` | `threads`, `pool` | Consistent with `multitasking`, avoids `threads` variable collision |
| CPU work | Explicit `spawn_thread` | Implicit pool | Transparency (TC >= 0.90) |

## Metrics Validation

| Metric | Target | This Design |
|--------|--------|-------------|
| TC (Transparency) | >= 0.90 | `with multitasking`, `with threading`, spawns all visible |
| ED (Ergonomic Delta) | <= 1.2 | Close to Go ergonomics |
| SN (Syntactic Noise) | <= 0.30 | No `.await`, no boilerplate |
| MC (Mechanical Correctness) | >= 0.90 | Affine handles catch forgotten tasks |
