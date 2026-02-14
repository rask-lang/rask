# Concurrency Specifications

Concurrency model for Rask.

## Design Philosophy

**Go-like ergonomics, compile-time safety.** Spawn syntax with affine handles. Forgotten tasks become compile errors.

**Concurrency vs parallelism:**
- **Concurrency** (green tasks): Interleaved execution on few threads. I/O-bound work.
- **Parallelism** (thread pool): Simultaneous execution. CPU-bound work.

**No function coloring.** Functions work the same way regardless of I/O. Pausing happens automatically—IDEs show pause points as ghost annotations. No ecosystem split.

**Affine handles.** All spawn constructs return handles that must be consumed (joined or detached). Forgetting one is a compile error.

**Explicit resources.** `using Multitasking { }` and `using ThreadPool { }` declare available capabilities.

## Specifications

| Spec | Status | Purpose |
|------|--------|---------|
| [async.md](async.md) | Draft | **Execution model**: Multitasking, ThreadPool, spawn, handles |
| [runtime.md](runtime.md) | Draft | **Runtime implementation**: M:N scheduler, reactor, task state machines |
| [runtime-strategy.md](runtime-strategy.md) | Draft | **Implementation strategy**: OS threads first, M:N later |
| [io-context.md](io-context.md) | Draft | **I/O dispatch**: How stdlib detects async context, sync/async code paths |
| [sync.md](sync.md) | Draft | **Shared state**: Shared<T>, Mutex<T> for cross-task access |
| [select.md](select.md) | Draft | Select statement, multiplexing |

**Start here:** [async.md](async.md) for the execution model overview, then [runtime-strategy.md](runtime-strategy.md) for implementation plan.

## Quick Reference

```rask
import async.spawn
import thread.{Thread, ThreadPool}

// Async mode - green tasks for I/O
func main() {
    using Multitasking {
        spawn(|| { handle_connection(conn) }).detach()
    }
}

// Async + CPU work
func main() {
    using Multitasking, ThreadPool {
        const h = spawn(|| {
            const data = try fetch(url)                                       // I/O - pauses
            const result = try ThreadPool.spawn(|| { analyze(data) }).join()  // CPU on threads
            try save(result)                                                // I/O - pauses
        })
        try h.join()
    }
}

// Sync mode - CPU parallelism only
func main() {
    using ThreadPool {
        const handles = files.map({ |f| ThreadPool.spawn(|| { process(f) }) })
        for h in handles { try h.join() }
    }
}

// Spawn and wait for result
const h = spawn(|| { compute() })
const result = try h.join()

// Fire-and-forget (explicit)
spawn(|| { background_work() }).detach()

// Multiple tasks
let (a, b) = join_all(
    spawn(|| { work1() }),
    spawn(|| { work2() })
)

// Dynamic spawning
const group = TaskGroup.new()
for url in urls {
    group.spawn(|| { fetch(url) })
}
const results = try group.join_all()

// Raw OS thread (works anywhere)
const h = Thread.spawn(|| { needs_thread_affinity() })
try h.join()
```

## Three Spawn Functions

| Function | Purpose | Requires | Pauses? |
|----------|---------|----------|---------|
| `spawn(|| {})` | Green task | `using Multitasking` | Yes (at I/O) |
| `ThreadPool.spawn(|| {})` | Thread from pool | `using ThreadPool` | No |
| `Thread.spawn(|| {})` | Raw OS thread | Nothing | No |

## Key Patterns

| Pattern | Syntax |
|---------|--------|
| Spawn and wait | `try spawn(|| {}).join()` |
| Fire-and-forget | `spawn(|| {}).detach()` |
| Wait for all | `join_all(spawn(|| {}), spawn(|| {}))` |
| Dynamic spawning | `TaskGroup` |
| CPU parallelism | `ThreadPool.spawn(|| {})` |
| Raw OS thread | `spawn raw { }` |
| Unused handle | **Compile error** |

## Resource Combinations

| Setup | Green Tasks | Thread Pool | Use Case |
|-------|-------------|-------------|----------|
| `using Multitasking` | Yes | No | I/O-heavy servers |
| `using ThreadPool` | No | Yes | CLI tools, batch processing |
| `using Multitasking, ThreadPool` | Yes | Yes | Full-featured applications |

## Validation Criteria

- HTTP server handling 100k concurrent connections
- CLI pipeline tool (grep | sort | uniq)
- Producer-consumer with multiple workers
- Process 1M items across all CPU cores
- Model as simple as Go for web services

## Key Principles

- `using Multitasking { }` creates M:N scheduler for green tasks
- `using ThreadPool { }` creates thread pool for CPU work
- Configuration via numbers: `Multitasking(workers: N)`, `ThreadPool(workers: N)`
- Affine handles must be joined or detached
- `.join()` pauses in async mode, blocks in sync mode
- Tasks own their data—no shared mutable state
- Channels work everywhere—pause in async, block in sync
- No function coloring, no async/await keywords
- Sync mode is default—multitasking optional for CLI/embedded
