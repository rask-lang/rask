# Concurrency Specifications

This folder contains the concurrency model for Rask.

## Design Philosophy

**Go-like ergonomics, compile-time safety:** Simple spawn syntax with affine handles that catch forgotten tasks at compile time.

**Concurrency != Parallelism:**
- **Concurrency** (green tasks): Many tasks interleaved on few threads. For I/O-bound work.
- **Parallelism** (thread pool): True simultaneous execution. For CPU-bound work.

**No function coloring:** Functions are just functions. I/O pauses automatically (IDE shows pause points as ghost annotations). No ecosystem split.

**Affine handles:** All spawn constructs return handles that must be consumed (joined or detached). Compile error if forgotten.

**Explicit resources:** `with multitasking { }` and `with threading { }` clearly declare what's available.

## Specifications

| Spec | Status | Purpose |
|------|--------|---------|
| [async.md](async.md) | Draft | **Execution model**: Multitasking, Threads, spawn, handles |
| [sync.md](sync.md) | Draft | **Shared state**: Shared<T>, Mutex<T> for cross-task access |
| [select.md](select.md) | Draft | Select statement, multiplexing |

**Start here:** [async.md](async.md) for the execution model overview.

## Quick Reference

```rask
// Async mode - green tasks for I/O
func main() {
    with multitasking {
        spawn { handle_connection(conn) }.detach()
    }
}

// Async + CPU work
func main() {
    with multitasking, threading {
        spawn {
            const data = fetch(url)?                              // I/O - pauses
            const result = threading.spawn { analyze(data) }.join()?  // CPU on threads
            save(result)?                                       // I/O - pauses
        }.join()?
    }
}

// Sync mode - CPU parallelism only
func main() {
    with threading {
        const handles = files.map { |f| threading.spawn { process(f) } }
        for h in handles { h.join()? }
    }
}

// Spawn and wait for result
const h = spawn { compute() }
const result = h.join()?

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
const results = group.join_all()?

// Raw OS thread (works anywhere)
const h = raw_thread { needs_thread_affinity() }
h.join()?
```

## Three Spawn Constructs

| Construct | Purpose | Requires | Pauses? |
|-----------|---------|----------|---------|
| `spawn { }` | Green task | `with multitasking` | Yes (at I/O) |
| `threading.spawn { }` | Thread from pool | `with threading` | No |
| `raw_thread { }` | Raw OS thread | Nothing | No |

## Key Patterns

| Pattern | Syntax |
|---------|--------|
| Spawn and wait | `spawn { }.join()?` |
| Fire-and-forget | `spawn { }.detach()` |
| Wait for all | `join_all(spawn{}, spawn{})` |
| Dynamic spawning | `TaskGroup` |
| CPU parallelism | `threading.spawn { }` |
| Raw OS thread | `raw_thread { }` |
| Unused handle | **Compile error** |

## Resource Combinations

| Setup | Green Tasks | Thread Pool | Use Case |
|-------|-------------|-------------|----------|
| `with multitasking` | Yes | No | I/O-heavy servers |
| `with threading` | No | Yes | CLI tools, batch processing |
| `with multitasking, threading` | Yes | Yes | Full-featured applications |

## Validation Criteria

- Can build HTTP server handling 100k concurrent connections?
- Can build CLI pipeline tool (grep | sort | uniq)?
- Can build producer-consumer with multiple workers?
- Can process 1M items across all CPU cores?
- Is the model as simple as Go for web services?

## Key Principles

- `with multitasking { }` creates M:N scheduler for green tasks
- `with threading { }` creates thread pool for CPU work
- Configuration: `multitasking(N)`, `threading(N)` - just numbers
- Affine handles (must join or detach)
- `.join()` pauses in async mode, blocks in sync mode
- Tasks own their data (no shared mutable state)
- Channels work everywhere (pause in async, block in sync)
- No function coloring (no async/await keywords)
- Sync mode is default (no Multitasking needed for CLI/embedded)
