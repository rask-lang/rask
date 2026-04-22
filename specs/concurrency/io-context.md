<!-- id: conc.io-context -->
<!-- status: decided -->
<!-- summary: How stdlib I/O functions detect async context and dispatch between sync/async paths -->
<!-- depends: concurrency/async.md, concurrency/runtime-strategy.md, stdlib/io.md, memory/context-clauses.md -->

# I/O Context Integration

Stdlib I/O functions discover the active runtime by reading a process-global slot installed by `using Multitasking { ... }`. When the slot is `Some`, I/O is non-blocking (task parks). When it is `None`, I/O blocks the thread. Same function, two code paths, no async/await split, no hidden parameters, no signature coloring.

## Runtime Discovery

| Rule | Description |
|------|-------------|
| **CTX1: Process-global slot** | The runtime lives in one slot per process (see `conc.runtime`). Programmers never construct or inspect it |
| **CTX2: Installed by `using`** | `using Multitasking { }` fills the slot on entry, clears it on exit (`conc.runtime/R1-R2`) |
| **CTX3: No signature parameter** | Stdlib I/O functions have clean signatures — they call into the runtime via the slot at execution time |
| **CTX4: Concrete type** | The slot holds a concrete `Arc<Runtime>`. No dynamic dispatch on context detection |

<!-- test: skip -->
```rask
// What the programmer writes:
using Multitasking {
    const file = try File.open("data.txt")
    const data = try file.read_text()
}

// What the compiler generates (conceptual):
__runtime_enter(default_config)            // fills RUNTIME_SLOT (panics if already full)
const file = try File.open("data.txt")      // reads RUNTIME_SLOT at call time
const data = try file.read_text()           // same
__runtime_exit()                            // drains tasks, clears RUNTIME_SLOT
```

## I/O Function Pattern

| Rule | Description |
|------|-------------|
| **IO1: Dual-path implementation** | Every I/O function has a blocking path and an async path, selected by reading RUNTIME_SLOT |
| **IO2: Blocking is fallback** | When the slot is empty (no block active), I/O functions call blocking syscalls directly |
| **IO3: Async registers with reactor** | When the slot is full, non-blocking syscall → EAGAIN → register FD with reactor → park task |
| **IO4: Phase A ignores the slot contents** | Phase A uses OS threads per spawn — I/O blocks the thread even when the slot is full, because there's no reactor |

### Concrete implementation pattern

Every I/O function in `rask-rt` follows this template:

```rust
// rask-rt implementation (Rust)
pub fn rask_file_read(file: &File, buf: &mut [u8]) -> Result<usize, IoError> {
    match RUNTIME_SLOT.read().as_deref() {
        None => {
            // Sync path: blocking read (IO2)
            blocking_read(file.fd(), buf)
        }
        Some(runtime) if runtime.phase == Phase::A => {
            // Phase A: still blocking, runtime has no reactor
            blocking_read(file.fd(), buf)
        }
        Some(runtime) => {
            // Phase B: non-blocking + reactor
            match nonblocking_read(file.fd(), buf) {
                Ok(n) => Ok(n),
                Err(EAGAIN) => {
                    let task = runtime.current_task();
                    runtime.reactor.register(file.fd(), Interest::Readable, task.waker());
                    task.park();  // Yield to scheduler
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
    if let Some(runtime) = RUNTIME_SLOT.read() {
        if runtime.current_task().cancel_flag.load() {
            return Err(IoError.Cancelled)
        }
    }
    // Proceed with actual I/O...
}
```

## Reader/Writer Trait Integration

| Rule | Description |
|------|-------------|
| **IO7: Clean trait signatures** | `Reader` and `Writer` signatures carry no runtime annotation — true for traits AND concrete implementations |
| **IO8: Implementations read the slot directly** | Concrete implementations (File, TcpConnection) read RUNTIME_SLOT when they run |
| **IO9: Generic I/O stays clean** | `io.copy(reader, writer)` is a plain generic call — no hidden parameter propagation |

<!-- test: skip -->
```rask
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
}

extend File with Reader {
    func read(self, buf: []u8) -> usize or IoError {
        // Reads RUNTIME_SLOT at execution time
    }
}

func io.copy(reader: any Reader, writer: any Writer) -> usize or IoError {
    const buf = [0u8; 8192]
    mut total = 0
    loop {
        const n = try reader.read(buf)
        if n == 0 { break }
        try writer.write_all(buf[..n])
        total += n
    }
    return total
}
```

The key insight: trait dispatch and runtime discovery are orthogonal. Signatures stay the same whether I/O is sync or async; the runtime slot decides at call time.

## Error Types

| Rule | Description |
|------|-------------|
| **IO10: IoError.Cancelled** | New variant for cancellation during I/O |
| **IO11: No RuntimePoisoned in Phase A** | Phase A doesn't have a reactor that can poison. Phase B adds `IoError.RuntimePoisoned` |

<!-- test: parse -->
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
ERROR [conc.async/CC1]: spawn requires a Multitasking scope
   |
5  |  spawn(|| { File.open("x.txt") })
   |  ^^^^^ no `using Multitasking { ... }` block encloses this call
   |
FIX: wrap the caller chain in `using Multitasking { ... }`, typically near main.
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

**CTX1 (process-global slot):** The earlier design threaded `RuntimeContext` as a hidden parameter through every function transitively reachable from a `using Multitasking` block. That colored every such signature and propagated a requirement through the call graph. The process-global slot avoids the coloring: exactly one slot exists per process by design (`conc.async/C1`), every thread reads it, and signatures stay clean.

**IO7 (clean trait signatures):** Because runtime discovery is global-slot-based, trait signatures never need to mention a runtime context — for traits or implementations. Generic I/O stays uncolored.

**IO4 (Phase A ignores the slot contents):** Phase A creates real OS threads per spawn; each thread blocks independently on I/O. The scaling limit (~10k) comes from OS thread count, not from missing async I/O.

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

The annotation appears on any call that can read RUNTIME_SLOT and park a task. Pure computation calls (json, math, collections) never show the annotation.

### See Also

- `conc.async/IO1-IO2` — Transparent pausing and sync fallback semantics
- `conc.runtime/IO1-IO3` — Async I/O flow, reactor registration protocol
- `conc.strategy` — Phase A vs Phase B runtime implementation
- `conc.hidden-params` — Hidden-parameter compiler pass (scoped to Pool contexts)
- `std.io` — Reader/Writer traits, IoError
- `std.fs` — File type and convenience functions
- `std.net` — TcpListener, TcpConnection
- `mem.context` — `using` clause mechanism for Pool contexts
