# Concurrency Specifications

Concurrency model for Rask.

## Design Philosophy

**Go-like ergonomics, compile-time safety.** Spawn syntax with affine handles. Forgotten tasks become compile errors.

**Concurrency vs parallelism:**
- **Concurrency** (green tasks): Interleaved execution on few threads. I/O-bound work.
- **Parallelism** (thread pool): Simultaneous execution. CPU-bound work.

**No function coloring.** Functions work the same way regardless of I/O. Pausing happens automatically—IDEs show pause points as ghost annotations. No ecosystem split.

**Affine handles.** All spawn constructs return handles that must be consumed (joined or detached). Forgetting one is a compile error.

**Context clause resources.** `Multitasking` and `ThreadPool` are types you create as local variables—same pattern as `Pool<T>`. Functions declare requirements via `with` clauses.

## Specifications

| Spec | Status | Purpose |
|------|--------|---------|
| [async.md](async.md) | Draft | **Execution model**: Multitasking, ThreadPool, spawn, handles |
| [sync.md](sync.md) | Draft | **Shared state**: Shared<T>, Mutex<T> for cross-task access |
| [select.md](select.md) | Draft | Select statement, multiplexing |

**Start here:** [async.md](async.md) for the execution model overview.

## Quick Reference

```rask
// Async mode - green tasks for I/O
func main() {
    const scheduler = Multitasking.new()
    spawn { handle_connection(conn) }.detach()
}

// Async + CPU work
func main() {
    const scheduler = Multitasking.new()
    const pool = ThreadPool.new()

    const h = spawn {
        const data = try fetch(url)                              // I/O - pauses
        const result = try spawn thread { analyze(data) }.join()  // CPU on threads
        try save(result)                                       // I/O - pauses
    }
    try h.join()
}

// Sync mode - CPU parallelism only
func main() {
    const pool = ThreadPool.new()
    const handles = files.map { |f| spawn thread { process(f) } }
    for h in handles { try h.join() }
}

// Spawn and wait for result
const h = spawn { compute() }
const result = try h.join()

// Fire-and-forget (explicit)
spawn { background_work() }.detach()

// Multiple tasks
let (a, b) = join_all(
    spawn { work1() },
    spawn { work2() }
)

// Dynamic spawning
const group = TaskGroup.new()
for url in urls {
    group.spawn { fetch(url) }
}
const results = try group.join_all()

// Raw OS thread (works anywhere)
const h = spawn raw { needs_thread_affinity() }
try h.join()
```

## Three Spawn Constructs

| Construct | Purpose | Requires | Pauses? |
|-----------|---------|----------|---------|
| `spawn { }` | Green task | `Multitasking` in scope | Yes (at I/O) |
| `spawn thread { }` | Thread from pool | `ThreadPool` in scope | No |
| `spawn raw { }` | Raw OS thread | Nothing | No |

## Key Patterns

| Pattern | Syntax |
|---------|--------|
| Spawn and wait | `try spawn { }.join()` |
| Fire-and-forget | `spawn { }.detach()` |
| Wait for all | `join_all(spawn{}, spawn{})` |
| Dynamic spawning | `TaskGroup` |
| CPU parallelism | `spawn thread { }` |
| Raw OS thread | `spawn raw { }` |
| Unused handle | **Compile error** |

## Resource Combinations

| Setup | Green Tasks | Thread Pool | Use Case |
|-------|-------------|-------------|----------|
| `Multitasking` only | Yes | No | I/O-heavy servers |
| `ThreadPool` only | No | Yes | CLI tools, batch processing |
| Both | Yes | Yes | Full-featured applications |

## Validation Criteria

- HTTP server handling 100k concurrent connections
- CLI pipeline tool (grep | sort | uniq)
- Producer-consumer with multiple workers
- Process 1M items across all CPU cores
- Model as simple as Go for web services

## Key Principles

- `Multitasking.new()` creates M:N scheduler for green tasks
- `ThreadPool.new()` creates thread pool for CPU work
- Configuration via constructor args: `Multitasking.new(workers: N)`, `ThreadPool.new(workers: N)`
- Both are context clause objects—same pattern as `Pool<T>`
- Affine handles must be joined or detached
- `.join()` pauses in async mode, blocks in sync mode
- Tasks own their data—no shared mutable state
- Channels work everywhere—pause in async, block in sync
- No function coloring, no async/await keywords
- Sync mode is default—Multitasking optional for CLI/embedded
