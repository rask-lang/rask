<!-- id: conc.sync -->
<!-- status: decided -->
<!-- summary: `with`-based Shared<T> and Mutex<T> for cross-task shared state -->
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
| **R1: Read** | `with shared.read() as v { ... }` — shared read lock; multiple readers concurrent. Mutation through binding is a compile error |
| **R2: Write** | `with shared.write() as v { ... }` — exclusive write lock; blocks until readers finish |
| **R2a: Unused write warning** | Compiler warns when `.write()` used but binding never mutated — suggests `.read()` |
| **R3: Try variants** | `try_read(f)` / `try_write(f)` — non-blocking closures, return `None` if contended |
| **R4: Bare access forbidden** | `with shared as v { ... }` is a compile error — must use `.read()` or `.write()` |
| **R5: Inline access** | `shared.read().chain` and `shared.write().chain` — expression-scoped lock for single-expression access. Follows `mem.borrowing/E5` rules. Standalone `.read()`/`.write()` without chaining is a compile error |

<!-- test: skip -->
```rask
let config = Shared.new(AppConfig {
    timeout: 30.seconds,
    max_retries: 3,
})

// Inline access (single expression)
const timeout = config.read().timeout
config.write().timeout = 60.seconds
const name = config.read().user.name.clone()

// Multi-statement access (with block)
const timeout = with config.read() as c { c.timeout }
with config.write() as c {
    c.timeout = 60.seconds
    c.max_retries = 5
}
```

### API

<!-- test: skip -->
```rask
struct Shared<T> { }

extend Shared<T> {
    func new(value: T) -> Shared<T>
    func read(self) -> T             // inline access (R5) — expression-scoped read lock
    func write(self) -> T            // inline access (R5) — expression-scoped write lock
    func try_read<R>(self, f: |T| -> R) -> Option<R>
    func try_write<R>(self, f: |T| -> R) -> Option<R>
}
```

Three access patterns:
- **Inline:** `shared.read().field` / `shared.write().field = x` — single-expression access, lock scoped to expression
- **`with` block:** `with shared.read() as v { ... }` / `with shared.write() as v { ... }` — multi-statement access
- **Non-blocking closures:** `try_read(f)` / `try_write(f)` — return `None` if contended

Bare `with shared as v` is a compile error — the lock type must be explicit.

## Mutex\<T\>

| Rule | Description |
|------|-------------|
| **MX1: Lock** | `with mutex as v { ... }` — exclusive lock; blocks until available |
| **MX2: Try lock** | `try_lock(f)` — non-blocking closure, returns `None` if held |
| **MX3: Inline access** | `mutex.lock().chain` — expression-scoped exclusive lock for single-expression access. Follows `mem.borrowing/E5` rules |

<!-- test: skip -->
```rask
const queue = Mutex.new(Vec.new())

// Inline access (single expression)
queue.lock().push(item)
const len = queue.lock().len()

// Multi-statement access (with block)
with queue as q {
    q.push(item_a)
    q.push(item_b)
}
```

### API

<!-- test: skip -->
```rask
struct Mutex<T> { }

extend Mutex<T> {
    func new(value: T) -> Mutex<T>
    func lock(self) -> T             // inline access (MX3) — expression-scoped exclusive lock
    func try_lock<R>(self, f: |T| -> R) -> Option<R>
}
```

Three access patterns:
- **Inline:** `mutex.lock().field` — single-expression access, lock scoped to expression
- **`with` block:** `with mutex as v { ... }` — multi-statement access
- **Non-blocking closure:** `try_lock(f)` — returns `None` if held

## `with`-Based Access

| Rule | Description |
|------|-------------|
| **WS1: No escape** | Data accessed via `with` cannot escape — no guard objects, no dangling references |
| **WS2: Scoped unlock** | Lock released when `with` block exits — timing is explicit |
| **WS3: Direct nesting prevented** | Nested `with` blocks on sync primitives are compile errors (syntactic detection) |
| **WS4: First-class block** | `return`, `try`, `break`, `continue` work naturally inside `with` blocks |

<!-- test: skip -->
```rask
// with-based (Rask) — reference cannot escape, control flow works
with mutex as data {
    data.field = value
    try validate(data)    // propagates to enclosing function
}

// Guard-based (Rust) — reference can escape scope
// let guard = mutex.lock()  // NOT in Rask
```

## Deadlock Prevention

| Rule | Description |
|------|-------------|
| **DL1: Direct nesting** | Nested `with` on different sync primitives is a compile error |
| **DL2: Same lock** | `with shared.read() as v { with shared.write() as v2 { ... } }` is a compile error |
| **DL3: Indirect — your responsibility** | Locks acquired through function calls or dynamic dispatch are NOT detected |
| **DL4: Multiple inline accesses** | Multiple `.read()`/`.write()`/`.lock()` calls in the same expression is a compile error — same deadlock risk as DL1 |

```
ERROR [conc.sync/DL1]: nested lock acquisition
   |
5  |  with mutex_a as a {
6  |      with mutex_b as b {
   |      ^^^^ cannot acquire lock inside another with block

WHY: Nested locks risk deadlock. Copy values out, then lock separately.
```

```
ERROR [conc.sync/DL2]: same lock re-acquisition
   |
5  |  with shared.read() as c {
6  |      with shared.write() as c2 {
   |      ^^^^ cannot acquire write lock — already holding read lock

WHY: Re-acquiring the same lock inside a with block would deadlock.
```

```
ERROR [conc.sync/DL4]: multiple lock acquisitions in one expression
   |
5  |  process(shared_a.read().x, shared_b.read().y)
   |          ^^^^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^ second lock acquisition
   |          first lock acquisition

WHY: Multiple locks in one expression risk deadlock. Copy values out first.

FIX:
  const x = shared_a.read().x
  process(x, shared_b.read().y)
```

<!-- test: skip -->
```rask
// OK: multiple elements from same collection (not a lock)
with pool[h1] as e1, pool[h2] as e2 {
    // runtime panic if h1 == h2
}
```

## Non-blocking variants

`try_read`, `try_write`, and `try_lock` stay as closures. These are uncommon and closure-based is fine for them. `with` is always blocking.

<!-- test: skip -->
```rask
// Blocking: with
with mutex as v { v.push(item) }

// Non-blocking: closure
const got_it = mutex.try_lock(|v| v.push(item))
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Direct nested `with` on sync primitives | DL1 | Compile error |
| Same-lock re-acquisition | DL2 | Compile error |
| Lock via function call | DL3 | Not detected — programmer responsibility |
| Multiple inline sync accesses in one expression | DL4 | Compile error |
| `shared.read().field` | R5 | Expression-scoped read lock |
| `shared.write().field = value` | R5 | Expression-scoped write lock |
| `mutex.lock().field` | MX3 | Expression-scoped exclusive lock |
| Standalone `shared.read()` without chaining | R5 | Compile error |
| Inline access inside `with` on same primitive | DL2 | Compile error |
| Writers starve under read load | SY1 | By design — read performance prioritized |

---

## Appendix (non-normative)

### Rationale

**WS1 (`with`-based access):** Rask's "no storable references" principle naturally leads to scoped access. Guards (Rust's `MutexGuard`) require lifetime tracking and allow references to escape scope. `with` blocks make unlock timing explicit and prevent escaping references by construction. The win over the old closure-based API: `return`, `try`, `break`, and `continue` work naturally.

**DL3 (indirect locks):** Detecting all lock acquisition paths requires whole-program analysis, which violates local-only compilation. Syntactic detection catches the most common mistakes; ordering discipline handles the rest.

**SY1 (Shared naming):** `Shared<T>` describes intent, not mechanism. `RwLock<T>` is implementation jargon.

**R1/R2 (explicit .read()/.write()):** With implicit lock selection via `const`, the same keyword means different things for Shared (changes lock type) vs Mutex (changes only binding mutability). Explicit `.read()`/`.write()` makes the lock type visible and removes the semantic inconsistency.

**try_* stay as closures:** Non-blocking access is uncommon. The inconsistency is justified — `with` is inherently blocking (it's a scope, not a conditional). Could add `with try mutex as v { ... } else { ... }` later if the pattern is common enough.

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
// Lock ordering — copy out, then lock separately
func transfer(from: Mutex<Account>, to: Mutex<Account>, amount: u64) {
    const from_balance = from.lock().balance
    from.lock().balance -= amount
    to.lock().balance += amount
}

// Copy out, modify, copy back
func swap_values(a: Mutex<i32>, b: Mutex<i32>) {
    const a_val = a.lock().clone()
    const b_val = b.lock().clone()
    with a as v { v = b_val }
    with b as v { v = a_val }
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
    return CONFIG.read().timeout
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
    METRICS.latencies.lock().push(latency)
}
```

### Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Access pattern | `with`-based blocks + inline `.read()`/`.write()`/`.lock()` | Guard-based / closure-based | No escaping references, `return`/`try` work, prevents nested deadlock. Inline access for single-expression convenience |
| Read-heavy primitive | `Shared<T>` | Just `Mutex<T>` | Common pattern deserves optimization |
| Naming | `Shared<T>` | `RwLock<T>` | Describes intent, not mechanism |
| Direct nested locks | Compile error (syntactic) | Whole-program analysis | Local analysis only |
| Non-blocking variants | Closure-based (`try_*`) | `with try` syntax | Uncommon pattern, closures are fine |

### See Also

- `mem.atomics` — lock-free primitives for single values
- `conc.async` — channels and task spawning
- `mem.pools` — single-task dynamic data structures
- `mem.borrowing` — `with` semantics and rules
