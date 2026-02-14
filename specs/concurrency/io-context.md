<!-- id: conc.io-context -->
<!-- status: decided -->
<!-- summary: How stdlib I/O functions detect async context and dispatch between sync/async paths -->
<!-- depends: concurrency/async.md, concurrency/runtime-strategy.md, stdlib/io.md, memory/context-clauses.md -->

# I/O Context Integration

Stdlib I/O functions accept a hidden `RuntimeContext` parameter. When present, I/O is non-blocking (task parks). When absent, I/O blocks the thread. Same function, two code paths, no function coloring.

## RuntimeContext Type

| Rule | Description |
|------|-------------|
| **CTX1: Opaque handle** | `RuntimeContext` is an opaque type — programmers never construct or inspect it |
| **CTX2: Provided by `using`** | `using Multitasking { }` creates a `RuntimeContext` and threads it through all calls in the block |
| **CTX3: Optional parameter** | Stdlib I/O functions accept `__ctx?: RuntimeContext` — present in async context, absent in sync |
| **CTX4: Not a trait** | `RuntimeContext` is a concrete type, not a trait. No dynamic dispatch on context detection |

<!-- test: skip -->
```rask
// What the programmer writes:
using Multitasking {
    const file = try File.open("data.txt")
    const data = try file.read_text()
}

// What the compiler generates (conceptual):
const __ctx = RuntimeContext.__new()
const file = try File.open("data.txt", __ctx)
const data = try file.read_text(__ctx)
__ctx.__shutdown()
```

## Context Structure

### Phase A (OS threads, `conc.strategy/A1`)

<!-- test: skip -->
```rask
// Minimal marker — enough to thread through call chains
struct RuntimeContext {
    mode: ContextMode
}

enum ContextMode {
    ThreadBacked    // Phase A: OS threads, blocking I/O
}
```

In Phase A, `RuntimeContext` is a marker. I/O functions receive it but ignore it — all I/O blocks the calling thread regardless. The value of threading it now is validating the desugaring pass (`conc.strategy/A5`).

### Phase B (M:N green tasks, `conc.runtime`)

<!-- test: skip -->
```rask
struct RuntimeContext {
    mode: ContextMode
    scheduler: Scheduler      // Work-stealing scheduler handle
    reactor: Reactor          // I/O event loop handle
    current_task: Task        // Currently executing task
}

enum ContextMode {
    ThreadBacked    // Fallback
    GreenTask       // Phase B: non-blocking I/O, task parking
}
```

## I/O Function Pattern

| Rule | Description |
|------|-------------|
| **IO1: Dual-path implementation** | Every I/O function has a blocking path and an async path, selected by `__ctx` presence |
| **IO2: Blocking is default** | Without context, I/O functions call blocking syscalls directly |
| **IO3: Async registers with reactor** | With context (Phase B), non-blocking syscall → EAGAIN → register FD with reactor → park task |
| **IO4: Phase A ignores context** | With context but in Phase A (`ThreadBacked`), I/O blocks the thread (same as no context) |

### Concrete implementation pattern

Every I/O function in `rask-rt` follows this template:

```rust
// rask-rt implementation (Rust)
pub fn rask_file_read(
    file: &File,
    buf: &mut [u8],
    ctx: Option<&RuntimeContext>,
) -> Result<usize, IoError> {
    match ctx {
        None => {
            // Sync path: blocking read
            blocking_read(file.fd(), buf)
        }
        Some(ctx) if ctx.mode == ContextMode::ThreadBacked => {
            // Phase A: still blocking, context is just a marker
            blocking_read(file.fd(), buf)
        }
        Some(ctx) => {
            // Phase B: non-blocking + reactor
            match nonblocking_read(file.fd(), buf) {
                Ok(n) => Ok(n),
                Err(EAGAIN) => {
                    ctx.reactor.register(file.fd(), Interest::Readable, ctx.current_task.waker());
                    ctx.current_task.park();  // Yield to scheduler
                    // Re-polled when I/O ready
                    nonblocking_read(file.fd(), buf)
                }
                Err(e) => Err(e),
            }
        }
    }
}
```

### Which functions need context

| Module | Functions | Context needed? |
|--------|-----------|----------------|
| `fs` | `File.open`, `File.read`, `File.write`, `File.close` | Yes — file I/O blocks |
| `fs` | `fs.read_file`, `fs.write_file`, `fs.exists` | Yes — convenience functions do I/O |
| `net` | `TcpListener.accept`, `TcpConnection.read/write` | Yes — network I/O blocks |
| `io` | `Stdin.read`, `Stdout.write`, `Stderr.write` | Yes — stream I/O blocks |
| `io` | `Buffer.read`, `Buffer.write` | No — in-memory, never blocks |
| `async` | `sleep`, `timeout` | Yes — needs timer/scheduler |
| `async` | `spawn`, `Channel.send/recv` | Yes — needs scheduler/reactor |
| collections | `Vec`, `Map`, `Pool` | No — pure memory operations |
| `json` | `json.encode`, `json.decode` | No — pure computation |
| `fmt` | `format` | No — pure computation |
| `math` | All functions | No — pure computation |

**Rule of thumb:** If the function touches file descriptors, the network, or sleeps, it takes `__ctx`. If it's pure computation or in-memory, it doesn't.

## Cancellation Integration

| Rule | Description |
|------|-------------|
| **IO5: Cancel check before I/O** | I/O functions check `cancel_flag` before initiating syscalls (`conc.async/CN3`) |
| **IO6: Cancel returns error** | If cancelled, return `Err(IoError.Cancelled)` before the syscall happens |

<!-- test: skip -->
```rask
// Cancellation check woven into I/O
func File.read(self, buf: []u8) -> usize or IoError {
    // Check cancel before doing work
    if __ctx is Some(ctx) {
        if ctx.current_task.cancel_flag.load() {
            return Err(IoError.Cancelled)
        }
    }

    // Proceed with actual I/O...
}
```

## Reader/Writer Trait Integration

| Rule | Description |
|------|-------------|
| **IO7: Traits don't mention context** | `Reader` and `Writer` trait signatures stay clean — no `__ctx` parameter |
| **IO8: Implementors receive context** | Concrete implementations (File, TcpConnection) receive `__ctx` via compiler desugaring |
| **IO9: Generic I/O propagates** | `io.copy(reader, writer)` threads `__ctx` to both `reader.read()` and `writer.write()` calls |

<!-- test: skip -->
```rask
// Trait signature — no context visible
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
}

// File implements Reader — compiler adds __ctx
extend File with Reader {
    func read(self, buf: []u8) -> usize or IoError {
        // Compiler desugars to: read(self, buf, __ctx?)
        // __ctx available if caller is in async context
    }
}

// Generic function — context threads through
func io.copy(reader: any Reader, writer: any Writer) -> usize or IoError {
    // Compiler desugars to: io.copy(reader, writer, __ctx?)
    const buf = [0u8; 8192]
    let total = 0
    loop {
        const n = try reader.read(buf)   // __ctx forwarded
        if n == 0 { break }
        try writer.write_all(buf[..n])    // __ctx forwarded
        total += n
    }
    return total
}
```

The key insight: trait dispatch and context threading are orthogonal. The compiler inserts `__ctx` at every call site that has one available. Trait implementations receive it like any other function.

## Error Types

| Rule | Description |
|------|-------------|
| **IO10: IoError.Cancelled** | New variant for cancellation during I/O |
| **IO11: No RuntimePoisoned in Phase A** | Phase A doesn't have a reactor that can poison. Phase B adds `IoError.RuntimePoisoned` |

<!-- test: skip -->
```rask
// Extended IoError (additions from this spec)
enum IoError {
    // ... existing variants from std.io/E1 ...
    Cancelled           // Task was cancelled before I/O completed (IO6)
    RuntimePoisoned     // Reactor thread panicked (Phase B only, conc.runtime/E7)
}
```

## Error Messages

```
ERROR [conc.io-context/CTX2]: I/O outside async context
   |
5  |  spawn(|| { File.open("x.txt") })
   |              ^^^^^^^^^ spawn() requires 'using Multitasking'
   |
WHY: spawn() needs a runtime context to manage the spawned task.

FIX: Wrap in using Multitasking { spawn(|| { ... }) }
```

```
ERROR [conc.io-context/IO7]: trait method cannot declare context parameter
   |
3  |  trait Reader {
4  |      func read(self, buf: []u8, ctx: RuntimeContext) -> usize or IoError
   |                                 ^^^^^^^^^^^^^^^^^^^^ context is implicit
   |
WHY: Trait signatures must not include RuntimeContext. The compiler threads
     context automatically through implementations.

FIX: Remove the context parameter from the trait signature.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| I/O in `comptime` block | Compile error (comptime can't do I/O) | `ctrl.comptime/CT33` |
| I/O without `using Multitasking` | Blocks calling thread (sync path) | IO2 |
| `Buffer.read` in async context | No parking (in-memory, never blocks) | IO1 |
| Cancelled task does I/O | `IoError.Cancelled` before syscall | IO5, IO6 |
| Nested `using Multitasking` | Compile error | `conc.async/C1` |
| I/O in `ThreadPool.spawn` closure | Blocks pool thread (no reactor in pool) | IO2 |
| `io.copy` in async context | Both read and write can park | IO9 |

---

## Appendix (non-normative)

### Rationale

**CTX4 (not a trait):** I considered making `RuntimeContext` a trait so Phase A and Phase B could be different implementations behind dynamic dispatch. But that adds vtable overhead on every I/O call, and the context type is known at compile time. Concrete type with a `mode` discriminant is simpler and zero-cost once the branch is predictable.

**IO7 (traits don't mention context):** Putting `__ctx` in `Reader` would infect every generic function that uses `Reader`. The whole point of hidden parameters is that they're hidden. Traits define the programmer-visible contract; context is a compiler implementation detail.

**IO4 (Phase A ignores context):** This means Phase A programs behave identically whether `using Multitasking` is present or not — I/O always blocks. That's correct: Phase A's `using Multitasking` creates real threads per spawn, and each thread can block independently. The scaling limit (~10k) comes from OS thread count, not from blocking I/O.

### I/O in ThreadPool context

`ThreadPool.spawn` closures don't get async I/O even in Phase B. Thread pool workers are OS threads that run jobs to completion — no parking, no reactor. I/O in a thread pool closure blocks the pool thread.

This is by design: `ThreadPool` is for CPU-bound work. If you need I/O, use `spawn()` (green tasks) instead.

<!-- test: skip -->
```rask
using Multitasking, ThreadPool {
    // Good: I/O in green task
    spawn(|| {
        const data = try File.read("big.csv")  // Parks task
        const result = try ThreadPool.spawn(|| {
            parse_csv(data)  // CPU-bound, no I/O
        }).join()
        try File.write("output.json", result)   // Parks task
    }).detach()

    // Bad: I/O in thread pool (blocks pool thread)
    ThreadPool.spawn(|| {
        const data = try File.read("big.csv")  // Blocks pool thread!
        parse_csv(data)
    }).detach()
}
```

Linter rule `conc.runtime/HP2.4` warns about I/O inside `ThreadPool.spawn`.

### Pause point visibility

IDE ghost annotations show where I/O can park a task:

<!-- test: skip -->
```rask
using Multitasking {
    const file = try File.open("data.txt")    // ⟨pauses⟩
    const data = try file.read_text()          // ⟨pauses⟩
    const parsed = json.decode<Config>(data)   // (no annotation)
    try file.close()                           // ⟨pauses⟩
}
```

The annotation appears on any call where `__ctx` is threaded to an I/O function. Pure computation calls (json, math, collections) never show the annotation.

### See Also

- `conc.async/IO1-IO2` — Transparent pausing and sync fallback semantics
- `conc.runtime/IO1-IO3` — Async I/O flow, reactor registration protocol
- `conc.strategy` — Phase A vs Phase B runtime implementation
- `conc.hidden-params` — Compiler pass that inserts `__ctx` parameters
- `std.io` — Reader/Writer traits, IoError
- `std.fs` — File type and convenience functions
- `std.net` — TcpListener, TcpConnection
- `mem.context` — `using` clause mechanism for Pool contexts
