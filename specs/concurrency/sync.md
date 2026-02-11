<!-- id: conc.sync -->
<!-- status: decided -->
<!-- summary: Closure-based Shared<T> and Mutex<T> for cross-task shared state -->
<!-- depends: memory/ownership.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Synchronization Primitives

Cross-task shared state when channels aren't enough.

## Primitives

| Rule | Description |
|------|-------------|
| **SY1: Shared** | `Shared<T>` — read-heavy concurrent access (multiple readers, exclusive writer) |
| **SY2: Mutex** | `Mutex<T>` — exclusive access for write-heavy patterns |
| **SY3: Atomics** | Atomic types for single values — lock-free (see `mem.atomics`) |
| **SY4: Explicit escape hatches** | All three are visible escape hatches from "no shared mutable memory" |

| Primitive | Pattern | Contention | Use Case |
|-----------|---------|------------|----------|
| `Shared<T>` | Read-heavy | Low (readers don't block) | Config, feature flags |
| `Mutex<T>` | Write-heavy | High (all access exclusive) | Queues, state machines |
| Atomics | Single values | None (lock-free) | Counters, flags |

## Shared\<T\>

| Rule | Description |
|------|-------------|
| **R1: Read** | `read(f)` — shared access; multiple readers concurrent |
| **R2: Write** | `write(f)` — exclusive access; blocks until readers finish |
| **R3: Try variants** | `try_read(f)` / `try_write(f)` — non-blocking, return `None` if contended |

<!-- test: skip -->
```rask
let config = Shared.new(AppConfig {
    timeout: 30.seconds,
    max_retries: 3,
})

let timeout = config.read(|c| c.timeout)
config.write(|c| c.timeout = 60.seconds)
```

### API

<!-- test: skip -->
```rask
struct Shared<T> { }

extend Shared<T> {
    func new(value: T) -> Shared<T>
    func read<R>(self, f: |T| -> R) -> R
    func write<R>(self, f: |T| -> R) -> R
    func try_read<R>(self, f: |T| -> R) -> Option<R>
    func try_write<R>(self, f: |T| -> R) -> Option<R>
}
```

## Mutex\<T\>

| Rule | Description |
|------|-------------|
| **MX1: Lock** | `lock(f)` — exclusive access; blocks until available |
| **MX2: Try lock** | `try_lock(f)` — non-blocking, returns `None` if held |

<!-- test: skip -->
```rask
const queue = Mutex.new(Vec.new())
queue.lock(|q| q.push(item))
const len = queue.lock(|q| q.len())
```

### API

<!-- test: skip -->
```rask
struct Mutex<T> { }

extend Mutex<T> {
    func new(value: T) -> Mutex<T>
    func lock<R>(self, f: |T| -> R) -> R
    func try_lock<R>(self, f: |T| -> R) -> Option<R>
}
```

## Closure-Based Access

| Rule | Description |
|------|-------------|
| **CB1: No escape** | Data accessed via closure cannot escape — no guard objects, no dangling references |
| **CB2: Scoped unlock** | Lock released when closure returns — timing is explicit |
| **CB3: Direct nesting prevented** | Nested lock/read/write calls inside a closure are compile errors (syntactic detection) |

<!-- test: skip -->
```rask
// Closure-based (Rask) — reference cannot escape
mutex.lock(|data| {
    data.field = value
})

// Guard-based (Rust) — reference can escape scope
// let guard = mutex.lock()  // NOT in Rask
```

## Deadlock Prevention

| Rule | Description |
|------|-------------|
| **DL1: Direct nesting** | `a.lock(\|_\| b.lock(\|_\| ...))` is a compile error |
| **DL2: Same lock** | `shared.read(\|_\| shared.write(\|_\| ...))` is a compile error |
| **DL3: Indirect — your responsibility** | Locks acquired through function calls or dynamic dispatch are NOT detected |

```
ERROR [conc.sync/DL1]: nested lock acquisition
   |
5  |  a.lock(|_| {
6  |      b.lock(|_| { ... })
   |      ^^^^^^ cannot acquire lock inside another lock closure

WHY: Nested locks risk deadlock. Copy values out, then lock separately.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Direct nested lock | DL1 | Compile error |
| Same-lock re-acquisition | DL2 | Compile error |
| Lock via function call | DL3 | Not detected — programmer responsibility |
| Writers starve under read load | SY1 | By design — read performance prioritized |

---

## Appendix (non-normative)

### Rationale

**CB1 (closure-based access):** Rask's "no storable references" principle naturally leads to closure-based APIs. Guards (Rust's `MutexGuard`) require lifetime tracking and allow references to escape scope. Closures make unlock timing explicit and prevent escaping references by construction.

**DL3 (indirect locks):** Detecting all lock acquisition paths requires whole-program analysis, which violates local-only compilation. Syntactic detection catches the most common mistakes; ordering discipline handles the rest.

**SY1 (Shared naming):** `Shared<T>` describes intent, not mechanism. `RwLock<T>` is implementation jargon.

### When to Use What

| Scenario | Primitive | Why |
|----------|-----------|-----|
| Config read by many tasks | `Shared<T>` | Read-heavy, writes rare |
| Feature flags | `Shared<T>` | Read-heavy |
| Connection pool | `Mutex<T>` | Checkout/checkin is write-heavy |
| Request queue | `Mutex<T>` | Push/pop are mutations |
| Metrics counter | `AtomicU64` | Single value, lock-free |
| Shutdown flag | `AtomicBool` | Single value, lock-free |
| Cache | `Shared<T>` or Channel | Depends on invalidation pattern |

### Shared\<T\> vs Channel

| Pattern | Shared\<T\> | Channel |
|---------|-----------|---------|
| Many readers, rare writes | Optimal | Awkward (request/response) |
| Request/response | Awkward | Natural |
| Streaming data | Wrong tool | Natural |
| Latest value | Natural | Need "watch" channel |

### Multiple Lock Patterns

For patterns that genuinely need multiple locks:

<!-- test: skip -->
```rask
// Lock ordering — acquire in consistent order
func transfer(from: Mutex<Account>, to: Mutex<Account>, amount: u64) {
    let (first, second) = if from.id < to.id { (from, to) } else { (to, from) }
    first.lock(|f| { })
    second.lock(|s| { })
}

// Copy out, modify, copy back
func swap_values(a: Mutex<i32>, b: Mutex<i32>) {
    let a_val = a.lock(|v| *v)
    let b_val = b.lock(|v| *v)
    a.lock(|v| *v = b_val)
    b.lock(|v| *v = a_val)
}
```

### Performance Characteristics

| Primitive | Uncontended | Read Contention | Write Contention |
|-----------|-------------|-----------------|------------------|
| `Shared<T>` | ~20ns | Scales linearly | Blocks all |
| `Mutex<T>` | ~20ns | N/A (no read mode) | Serialized |
| `AtomicU64` | ~1ns | ~1ns | ~10ns (CAS retry) |
| Channel | ~50ns | N/A | Bounded: blocks, Unbounded: allocates |

### Examples

**Application config:**
<!-- test: skip -->
```rask
static CONFIG: Shared<AppConfig> = Shared.new(AppConfig.default())

func get_timeout() -> Duration {
    CONFIG.read(|c| c.timeout)
}
```

**Metrics collection:**
<!-- test: skip -->
```rask
struct Metrics {
    requests: AtomicU64,
    errors: AtomicU64,
    latencies: Mutex<Vec<Duration>>,
}

func record_request(latency: Duration, success: bool) {
    METRICS.requests.fetch_add(1, Relaxed)
    if !success { METRICS.errors.fetch_add(1, Relaxed) }
    METRICS.latencies.lock(|v| v.push(latency))
}
```

### Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Access pattern | Closure-based | Guard-based | No escaping references, prevents direct nested deadlock |
| Read-heavy primitive | `Shared<T>` | Just `Mutex<T>` | Common pattern deserves optimization |
| Naming | `Shared<T>` | `RwLock<T>` | Describes intent, not mechanism |
| Direct nested locks | Compile error (syntactic) | Whole-program analysis | Local analysis only |

### See Also

- `mem.atomics` — lock-free primitives for single values
- `conc.async` — channels and task spawning
- `mem.pools` — single-task dynamic data structures
