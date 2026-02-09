<!-- depends: memory/ownership.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Synchronization Primitives

Cross-task shared state when channels aren't enough.

## The Question

Channels work for message-passing, but some patterns are awkward:
- **Shared config**: Read by many tasks, occasionally updated
- **Metrics/counters**: Written by many, read for reporting
- **Caches**: Read-heavy with occasional invalidation

What primitives does Rask provide for shared state beyond channels and atomics?

## Decision

Three primitives for different access patterns:

| Primitive | Pattern | Contention | Use Case |
|-----------|---------|------------|----------|
| `Shared<T>` | Read-heavy | Low (readers don't block) | Config, feature flags |
| `Mutex<T>` | Write-heavy | High (all access exclusive) | Queues, state machines |
| Atomics | Single values | None (lock-free) | Counters, flags |

All three are **explicit escape hatches** from "no shared mutable memory". Usage visible and intentional.

## Rationale

**Transparency (TC ≥ 0.90).** Shared state explicit—`Shared.new()`, `Mutex.new()`, not hidden in language magic.

**Mechanical Correctness (MC ≥ 0.90).** Closure-based access prevents escaping references. Data races impossible by construction.

**Ergonomic Delta (ED ≤ 1.2).** Matches Go's `sync.RWMutex` ergonomics without forgetting-to-unlock footgun.

**Use Case Coverage (UCC ≥ 0.80).** Read-heavy patterns (config, caches) common in servers. Without `Shared<T>`, users resort to awkward channel request/response patterns.

## Specification

### Shared<T> — Read-Heavy Concurrent Access

`Shared<T>` provides efficient read-heavy access with occasional writes.

<!-- test: skip -->
```rask
let config = Shared.new(AppConfig {
    timeout: 30.seconds,
    max_retries: 3,
})

// Multiple readers (concurrent, non-blocking)
let timeout = config.read(|c| c.timeout)

// Exclusive writer (blocks readers during write)
config.write(|c| c.timeout = 60.seconds)
```

**Semantics:**

| Operation | Behavior |
|-----------|----------|
| `read(f)` | Shared access; multiple readers concurrent |
| `write(f)` | Exclusive access; blocks until readers finish |
| `try_read(f)` | Non-blocking; returns `None` if write in progress |
| `try_write(f)` | Non-blocking; returns `None` if readers/writer active |

**API:**

<!-- test: skip -->
```rask
struct Shared<T> { ... }

extend Shared<T> {
    func new(value: T) -> Shared<T>
    func read<R>(self, f: |T| -> R) -> R
    func write<R>(self, f: |T| -> R) -> R
    func try_read<R>(self, f: |T| -> R) -> Option<R>
    func try_write<R>(self, f: |T| -> R) -> Option<R>
}
```

Method names indicate access mode—`read` gives read-only access, `write` gives mutable access. Interior mutability pattern (like atomics).

**Properties:**

| Property | Value |
|----------|-------|
| `Sync` | Yes (safe to share across tasks) |
| `Send` | Yes (safe to transfer across tasks) |
| Interior mutability | Yes (wrapper borrows immutably, closure mutates inner value) |
| Direct nested deadlock | Prevented (syntactic detection) |
| Starvation | Writers may starve under heavy read load |

**Implementation:** Built on `RwLock` internally. Platform-specific—pthread_rwlock, SRWLock, etc.

### Mutex<T> — Exclusive Access

`Mutex<T>` provides exclusive access for write-heavy patterns.

<!-- test: skip -->
```rask
const queue = Mutex.new(Vec.new())

// Exclusive access
queue.lock(|q| {
    q.push(item)
})

// With result
const len = queue.lock(|q| q.len())
```

**API:**

<!-- test: skip -->
```rask
struct Mutex<T> { ... }

extend Mutex<T> {
    func new(value: T) -> Mutex<T>
    func lock<R>(self, f: |T| -> R) -> R
    func try_lock<R>(self, f: |T| -> R) -> Option<R>
}
```

**Properties:**

| Property | Value |
|----------|-------|
| `Sync` | Yes |
| `Send` | Yes |
| Direct nested deadlock | Prevented (syntactic detection) |
| Fairness | Implementation-defined |

### When to Use What

| Scenario | Primitive | Why |
|----------|-----------|-----|
| Config read by many tasks | `Shared<T>` | Read-heavy, writes rare |
| Feature flags | `Shared<T>` | Read-heavy |
| Connection pool | `Mutex<T>` | Checkout/checkin is write-heavy |
| Request queue | `Mutex<T>` | Push/pop are mutations |
| Metrics counter | `AtomicU64` | Single value, lock-free |
| Shutdown flag | `AtomicBool` | Single value, lock-free |
| Rate limiter | `Mutex<T>` | Token bucket state |
| Cache | `Shared<T>` or Channel | Depends on invalidation pattern |

### Design: Closure-Based Access

**Why closures instead of guards?**

Guard-based (Rust style):
```rask
let guard = mutex.lock()  // Returns MutexGuard<T>
guard.field = value
// guard dropped, unlocks
```

Closure-based (Rask style):
```rask
mutex.lock(|data| {
    data.field = value
})  // Unlocked when closure returns
```

**Advantages of closure-based:**

| Aspect | Guards | Closures |
|--------|--------|----------|
| Escape risk | Reference can escape scope | Cannot escape closure |
| Unlock timing | When guard drops (implicit) | When closure returns (explicit) |
| Nested locks | Easy to deadlock | Compiler error (closure borrows mutex) |
| Borrow checker | Needs lifetime tracking | No lifetimes needed |

Key insight—Rask's "no storable references" principle naturally leads to closure-based APIs. Not a restriction, but the design working as intended.

### Preventing Deadlock

Closure-based access prevents **direct** nested locking:

```rask
let a = Mutex.new(1)
let b = Mutex.new(2)

// Direct nesting is a compile error:
a.lock(|a_val| {
    b.lock(|b_val| {     // ❌ ERROR: cannot borrow `b` - already in closure
        *a_val + *b_val
    })
})
```

That's an error because `b.lock()` would borrow `b` while already inside a closure that might hold references. Closure-based API prevents direct nested locking by construction.

### Nested Lock Detection Scope

Compiler uses **syntactic analysis only** (per Principle 5: Local Analysis).

**Guaranteed (compile error):**
- `a.lock(|_| b.lock(|_| ...))` — direct nested lock call
- `shared.read(|_| shared.write(|_| ...))` — same lock re-acquisition

**NOT guaranteed (your responsibility):**
- `a.lock(|_| some_function())` where `some_function` acquires locks
- Locks behind trait method calls
- Locks acquired through dynamic dispatch

Detecting all lock acquisition paths requires whole-program analysis, which violates local-only compilation.

For patterns that genuinely need multiple locks:

```rask
// Pattern 1: Lock ordering (acquire in consistent order)
func transfer(from: Mutex<Account>, to: Mutex<Account>, amount: u64) {
    let (first, second) = if from.id < to.id {
        (from, to)
    } else {
        (to, from)
    }

    first.lock(|f| {
        // Release first lock before acquiring second
    })
    second.lock(|s| {
        // ...
    })
}

// Pattern 2: Copy out, modify, copy back
func swap_values(a: Mutex<i32>, b: Mutex<i32>) {
    let a_val = a.lock(|v| *v)  // Copy out
    let b_val = b.lock(|v| *v)  // Copy out
    a.lock(|v| *v = b_val)      // Write back
    b.lock(|v| *v = a_val)      // Write back
}

// Pattern 3: Use channels for coordination
func coordinate(a: Mutex<State>, b: Mutex<State>, tx: Sender<Event>) {
    a.lock(|state| {
        state.process()
        try tx.send(Event.AProcessed)
    })
    // Other task handles B after receiving event
}
```

### Shared<T> vs Channel

When should you use `Shared<T>` vs a channel?

| Pattern | Shared<T> | Channel |
|---------|-----------|---------|
| Many readers, rare writes | ✅ Optimal | Awkward (request/response) |
| Request/response | Awkward | ✅ Natural |
| Streaming data | ❌ Wrong tool | ✅ Natural |
| Pub/sub | Use channel | ✅ Natural |
| Latest value | ✅ Natural | Need "watch" channel |
| Historical values | ❌ Wrong tool | ✅ Buffer |

**Example — Config that needs both patterns:**

```rask
// Config with watch channel for updates
struct ConfigService {
    current: Shared<Config>,
    updates: Sender<Config>,
}

extend ConfigService {
    func get(self) -> Config {
        self.current.read(|c| c.clone())
    }

    func update(self, new_config: Config) {
        self.current.write(|c| *c = new_config.clone())
        try self.updates.send(new_config)  // Notify watchers
    }

    func subscribe(self) -> Receiver<Config> {
        // Return receiver for update notifications
    }
}
```

### Performance Characteristics

| Primitive | Uncontended | Read Contention | Write Contention |
|-----------|-------------|-----------------|------------------|
| `Shared<T>` | ~20ns | Scales linearly | Blocks all |
| `Mutex<T>` | ~20ns | N/A (no read mode) | Serialized |
| `AtomicU64` | ~1ns | ~1ns | ~10ns (CAS retry) |
| Channel | ~50ns | N/A | Bounded: blocks, Unbounded: allocates |

**Guidance:**
- Single value → Atomic
- Read-heavy struct → Shared
- Write-heavy / queue → Mutex
- Task coordination → Channel

## Examples

### Application Config

```rask
static CONFIG: Shared<AppConfig> = Shared.new(AppConfig.default())

func init_config(path: string) -> () or Error {
    let loaded = try load_config_file(path)
    CONFIG.write(|c| *c = loaded)
    Ok(())
}

func get_timeout() -> Duration {
    CONFIG.read(|c| c.timeout)
}

func handle_request(req: Request) -> Response {
    let max_size = CONFIG.read(|c| c.max_request_size)
    if req.size > max_size {
        return Response.error(413)
    }
    // ...
}
```

### Metrics Collection

```rask
struct Metrics {
    requests: AtomicU64,
    errors: AtomicU64,
    latencies: Mutex<Vec<Duration>>,  // For percentile calculation
}

static METRICS: Metrics = Metrics {
    requests: AtomicU64.new(0),
    errors: AtomicU64.new(0),
    latencies: Mutex.new(Vec.new()),
}

func record_request(latency: Duration, success: bool) {
    METRICS.requests.fetch_add(1, Relaxed)
    if !success {
        METRICS.errors.fetch_add(1, Relaxed)
    }
    METRICS.latencies.lock(|v| v.push(latency))
}

func get_p99_latency() -> Duration {
    METRICS.latencies.lock(|v| {
        let sorted = v.clone()
        sorted.sort()
        sorted[sorted.len() * 99 / 100]
    })
}
```

### Connection Pool

```rask
struct ConnectionPool {
    connections: Mutex<Vec<Connection>>,
    max_size: u32,
}

extend ConnectionPool {
    func checkout(self) -> Connection or PoolError {
        self.connections.lock(|conns| {
            conns.pop().ok_or(PoolError.Empty)
        })
    }

    func checkin(self, conn: Connection) {
        self.connections.lock(|conns| {
            if conns.len() < self.max_size as usize {
                conns.push(conn)
            }
            // else: drop connection (pool full)
        })
    }
}
```

### Feature Flags

```rask
struct FeatureFlags {
    flags: Shared<Map<string, bool>>,
}

extend FeatureFlags {
    func is_enabled(self, flag: string) -> bool {
        self.flags.read(|f| f.get(flag).unwrap_or(false))
    }

    func set(self, flag: string, enabled: bool) {
        self.flags.write(|f| f.insert(flag, enabled))
    }

    func reload_from_server(self) -> () or Error {
        let new_flags = try fetch_flags_from_server()
        self.flags.write(|f| *f = new_flags)
        Ok(())
    }
}
```

## Integration Notes

- **Atomics:** Use atomics for single values; `Shared`/`Mutex` for compound data. See [atomics.md](../memory/atomics.md).
- **Channels:** Use channels for task coordination and streaming; `Shared`/`Mutex` for shared state. See [async.md](async.md).
- **Pools:** `Pool<T>` is single-task. For cross-task entity access, use channels to send handles. See [pools.md](../memory/pools.md).
- **Closures:** Closure-based access follows the same pattern as `pool.modify()`. See [closures.md](../memory/closures.md).

**Anti-pattern—Don't wrap Pool in Mutex:**

```rask
// ❌ WRONG: Wrapping a pool in Mutex causes contention
let entities = Mutex.new(Pool.new())
entities.lock(|p| p[h].update())  // Every access locks!

// ✅ RIGHT: Keep pool single-task, send handles via channels
let entities = Pool.new()
let (tx, rx) = Channel.unbuffered()
spawn { try tx.send(entity_handle) }.detach()
let h = try rx.recv()
entities[h].update()  // No lock needed
```

Handles are Copy and cheap to send. Pool stays fast—only coordination crosses task boundaries.

## Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Access pattern | Closure-based | Guard-based | No escaping references, prevents direct nested deadlock |
| Read-heavy primitive | `Shared<T>` | Just `Mutex<T>` | Common pattern deserves optimization |
| Naming | `Shared<T>` | `RwLock<T>` | Describes intent, not mechanism |
| Starvation | Writers may starve | Fair scheduling | Read performance priority |
| Direct nested locks | Compile error (syntactic) | Whole-program analysis | Local analysis only (Principle 5) |

## See Also

- [Atomics](../memory/atomics.md) — Lock-free primitives for single values
- [Async](async.md) — Channels and task spawning
- [Pools](../memory/pools.md) — Single-task dynamic data structures
