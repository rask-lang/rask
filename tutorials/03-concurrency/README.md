# Level 3 — Concurrency

Are threads and channels simple enough for the common case?

## Challenges

1. [Parallel Sum](parallel-sum.md) — split work across threads, collect results
2. [Producer-Consumer Pipeline](pipeline.md) — multi-stage pipeline with channels

## What You Need

Everything from [Level 2](../02-ownership-errors/) plus:

### Spawning Work

```rask
import async.spawn

// Green tasks (lightweight, like goroutines)
using Multitasking {
    const h = spawn(|| {
        return do_work()
    })
    const result = try h.join()     // wait for result

    // Fire-and-forget
    spawn(|| { background_work() }).detach()
}

// Thread pool (for CPU-heavy work)
using Multitasking, ThreadPool {
    const h = ThreadPool.spawn(|| { compute() })
    const result = try h.join()
}

// Raw OS thread
Thread.spawn(|| { work() })
```

### Channels

```rask
// Create a buffered channel
let (tx, rx) = Channel<i32>.buffered(100)

// Send (transfers ownership of the value)
tx.send(42)

// Receive
const val = rx.recv()    // blocks until a value is available

// Receive returns Result — Err when channel is closed
while rx.recv() is Ok(msg) {
    process(msg)
}

// Close the sender to signal "done"
tx.close()
```

Sending on a channel transfers ownership — the sender can't use the value after sending.

### Shared State

```rask
// Read-heavy shared data
const config = Shared.new(AppConfig { timeout: 30 })
const t = config.read(|c| c.timeout)        // many concurrent readers
config.write(|c| c.timeout = 60)            // exclusive writer

// Write-heavy shared data
const queue = Mutex.new(Vec.new())
queue.lock(|q| q.push(item))

// Atomic counters
const count = AtomicU64.new(0)
count.fetch_add(1, Relaxed)
```

Both `Shared` and `Mutex` use closures — the lock is released when the closure returns. No way to hold a lock across an await point or forget to unlock.

### Ownership Across Threads

Values sent to another thread must be owned, not borrowed. You can't send a reference across threads:

```rask
const data = Vec.from([1, 2, 3])

// This takes ownership of data
spawn(|| {
    process(own data)
}).detach()

// data is invalid here — it was moved into the spawned task
```

If multiple threads need the same data, either:
- Clone it for each thread
- Wrap it in `Shared<T>` or `Mutex<T>`
- Send pieces through channels

### Vec Slicing

```rask
const v = Vec.from([1, 2, 3, 4, 5])
const chunk = v[0..3]           // elements 0, 1, 2
const len = v.len()
```
