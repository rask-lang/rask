<!-- id: conc.async -->
<!-- status: decided -->
<!-- summary: Green tasks with must-use handles, no async/await split, explicit resource declaration -->
<!-- depends: memory/ownership.md, memory/context-clauses.md, memory/resource-types.md -->

# Execution Model

Green tasks with must-use handles. No async/await split — the same function works whether called from a green task or sync code. Explicit resource declaration.

## Spawn Constructs

| Rule | Description |
|------|-------------|
| **S1: Green task** | `spawn(|| {})` creates a green task; must run with an active `using Multitasking` block in the process |
| **S2: Pooled thread** | `ThreadPool.spawn(|| {})` runs on thread pool; must run with an active `using ThreadPool` block |
| **S3: Raw thread** | `Thread.spawn(|| {})` creates OS thread; no runtime required |
| **S4: Must-use handle** | All spawn forms return handles that must be joined or detached — dropping one is a compile error |

Spawn functions do not appear in signatures. No function declares `using Multitasking` — the compiler infers which functions (transitively) need a runtime and checks callers against the current lexical scope. See [Runtime Scope](#runtime-scope) below.

```rask
func main() -> void or Error {
    using Multitasking {
        const listener = try TcpListener.bind("0.0.0.0:8080")

        loop {
            const conn = try listener.accept()
            spawn(|| { handle_connection(conn) }).detach()
        }
    }
}

func handle_connection(conn: TcpConnection) -> void or Error {
    const request = try conn.read()
    const user = try fetch_user(request.id)
    try conn.write(user.to_json())
}
```

## Must-Use Handles

| Rule | Description |
|------|-------------|
| **H1: Must consume** | `TaskHandle<T>` must be joined or detached — compile error if unused |
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
mut (a, b) = join_all(
    spawn(|| { work1() }),
    spawn(|| { work2() })
)

const group = TaskGroup.new()
for url in urls {
    group.spawn(|| { fetch(url) })
}
const results = try group.join_all()
```

## Runtime Scope

`using Multitasking(config) { ... }` is a block that opts the program into concurrency. It does not appear on function signatures — only as a block, typically near the top of `main`.

| Rule | Description |
|------|-------------|
| **C1: Single active runtime** | At most one `using Multitasking` block is active in the process at any time. Entering a second while one is active is an error |
| **C2: Process-global visibility** | While the block is active, every thread in the process can `spawn()` — the runtime lives in a process-global slot |
| **C3: Block-scoped lifetime** | The runtime starts on block entry and shuts down on block exit. No refcounting, no persistence across blocks |
| **C4: Drain on exit** | Normal block exit waits for all tasks (including detached ones) to finish before returning. Panic-unwinding the block aborts remaining tasks |
| **C5: Sequential blocks OK** | After one block exits cleanly, another may be opened (new runtime, possibly different config). Non-overlapping only |
| **C6: Libraries don't install runtimes** | Only application code opens `using Multitasking`. Libraries call `spawn()` assuming the caller already did. Violation triggers C1's nesting error |

`using ThreadPool(config) { ... }` works the same way for CPU-bound pools. The two can be combined with `using Multitasking, ThreadPool { }` (installs both; teardown in reverse order on block exit).

<!-- test: parse -->
```rask
func main() {
    using Multitasking(workers: 4) {
        // all spawn() calls below, on any thread, use this runtime
        ...
    }
    // block exit: all spawned tasks drained, runtime shut down
}
```

### Compile-time checking

The compiler infers which functions transitively require a runtime (reach `spawn` through their call graph). This inference is **internal compiler metadata** — users write no annotations on signatures.

| Rule | Description |
|------|-------------|
| **CC1: Direct spawn check** | A lexical `spawn()` call outside any `using Multitasking` block → compile error |
| **CC2: Inferred-requirement check** | A call to any function inferred as requiring the runtime, lexically outside any block → compile error |
| **CC3: Runtime fallback** | Cases the compiler cannot prove statically — closures stored and called across block boundaries, trait-object dispatch, FFI — fall through to a runtime panic with a clear message |

Inference is invisible in source: writing or reading a function's body never involves Multitasking annotations. Users see the compile error at the **call site** ("calling `X` requires a `using Multitasking` scope; `X` needs it because it calls `spawn` at `f.rk:42`"), not at the definition.

## I/O Model

| Rule | Description |
|------|-------------|
| **IO1: Transparent pausing** | Stdlib I/O pauses the task, not the thread — no `.await` needed |
| **IO2: Sync fallback** | Outside any `using Multitasking` block, I/O blocks the calling thread |

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
| **CH1: Non-linear** | `Sender<T>` and `Receiver<T>` can go out of scope without explicit close |
| **CH2: Buffered/unbuffered** | `Channel<T>.unbuffered()` (sync) or `Channel<T>.buffered(n)` (async buffer) |
| **CH3: Close on scope exit** | Sender/receiver going out of scope implicitly closes; errors silently ignored |
| **CH4: Explicit close** | `tx.close()` / `rx.close()` return `Result` for error handling |

<!-- test: skip -->
```rask
mut (tx, rx) = Channel<Message>.buffered(100)

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
| `tx.send(val)` | `void or SendError` | Send value, pauses/blocks if full |
| `rx.recv()` | `T or RecvError` | Receive value, pauses/blocks if empty |
| `tx.close()` | `void or CloseError` | Explicit close with error handling |
| `rx.close()` | `void or CloseError` | Explicit close with error handling |
| `tx.try_send(val)` | `void or TrySendError` | Non-blocking send |
| `rx.try_recv()` | `T or TryRecvError` | Non-blocking receive |

### Buffered Items on Close

| Scenario | Behavior |
|----------|----------|
| Sender closed, buffer has items | Items remain — receivers can drain |
| All senders closed | Channel closed for writing, readable until empty |
| Receiver closed, buffer has items | Items discarded (lost) |
| All receivers closed | Senders get `Err(Closed)` on next send |

## Error Messages

```
ERROR [conc.async/H1]: unused TaskHandle
   |
12 |  spawn(|| { work() })
   |  ^^^^^^^^^^^^^^^^ TaskHandle must be joined or detached
```

```
ERROR [conc.async/CC1]: spawn requires a Multitasking scope
   |
5  |  spawn(|| { fetch(url) })
   |  ^^^^^ no `using Multitasking { ... }` block encloses this call

FIX: wrap the caller chain in `using Multitasking { ... }`, typically near main:

    func main() {
        using Multitasking {
            spawn(|| { fetch(url) }).detach()
        }
    }
```

```
ERROR [conc.async/CC2]: calling `fetch_page` requires a Multitasking scope
   |
12 |  fetch_page(url)
   |  ^^^^^^^^^^ this function transitively requires a runtime
   |
NOTE: `fetch_page` reaches `spawn` at stdlib/http.rk:42

FIX: wrap the caller chain in `using Multitasking { ... }`.
```

```
RUNTIME PANIC: spawn() called with no active `using Multitasking` scope

This can happen when:
  - A closure containing spawn is stored and called outside a block
  - A trait object dispatches to an impl that spawns
  - FFI calls back into Rask outside any scope

Install a `using Multitasking { ... }` block that encloses the call.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Direct `spawn` outside any block | CC1 | Compile error |
| Call to function transitively reaching `spawn`, outside any block | CC2 | Compile error |
| Closure stored / trait object dispatch reaches `spawn` outside a block | CC3 | Runtime panic |
| `.join()` on cancelled task | H2, CN1 | Returns `Err(Cancelled)` |
| Channel send after all receivers closed | CH3 | Returns `Err(Closed)` |
| Nested `using Multitasking` blocks | C1 | Error — second `enter` aborts (compile error if lexically nested, runtime panic otherwise) |
| Library opens `using Multitasking` while app already did | C6 | Falls under C1 — runtime panic |
| Detached task outlives `using` block body | C4 | Block exit still drains detached tasks. Truly outliving the block is impossible |

---

## Appendix (non-normative)

### Rationale

**S4 (must-use handles):** I wanted compile-time tracking of spawned tasks. Go's fire-and-forget `go` is ergonomic but loses track of goroutines — forgotten tasks are silent bugs. Must-use handles make the choice explicit: `.join()` or `.detach()`. (In type theory these are called "affine types" — values that must be used at most once.)

**IO1 (transparent pausing):** No async/await split means no ecosystem split. The same function works whether called from a green task or sync context. IDEs show pause points — transparency through tooling, not syntax.

**CH1 (non-linear channels):** Channels aren't `@resource` types. Fire-and-forget patterns (`.detach()` tasks) would require close ceremony. Go's channels drop without explicit close. Matches `ensure` philosophy — explicit handling available, implicit path simple.

### Comparison with Go

| Aspect | Go | Rask |
|--------|-----|------|
| Spawn syntax | `go func()` | `spawn(|| { }).detach()` |
| Track tasks | Manual (WaitGroup) | Compile-time (must-use handles) |
| Forgotten tasks | Silent | Compile error |
| Async/sync split | No | No |

### Channel Error Types

<!-- test: parse -->
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
| TC (Transparency) | >= 0.90 | `using Multitasking { ... }` block visible at application entry; spawns visible at callsite |
| ED (Ergonomic Delta) | <= 1.2 | Close to Go ergonomics |
| SN (Syntactic Noise) | <= 0.30 | No `.await`, no boilerplate |
| MC (Mechanical Correctness) | >= 0.90 | Must-use handles catch forgotten tasks |

### See Also

- `conc.select` — select and multiplex
- `conc.sync` — synchronization primitives
- `mem.context` — `using` clauses for pool contexts
- `mem.resources` — `@resource` types and `ensure` cleanup
