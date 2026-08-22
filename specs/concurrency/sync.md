<!-- id: conc.sync -->
<!-- status: decided -->
<!-- summary: One box, `Shared<T, S>` — several accessors reach one value; the strategy says what synchronization it costs -->
<!-- depends: memory/ownership.md, types/generics.md -->
<!-- implemented-by: compiler/crates/rask-interp/ -->

# Shared Values

One value, reached by several accessors, mutated through a scoped view. That's
one concept, so it's one type:

<!-- test: skip -->
```rask
let config  = Shared.new(cfg)               // many tasks, concurrent reads
let queue   = Shared.mutex(Vec.new())       // many tasks, one at a time
let counter = Shared.local(0)               // one task, no lock at all
```

`Cell<T>` and `Mutex<T>` used to be separate types. They aren't different
concepts — they're the same box with different synchronization — so they're now
**strategies** on one type. The words survive; the fork in the road doesn't.

## The type

| Rule | Description |
|------|-------------|
| **SH1: One box** | `Shared<T, S>` holds one value that several names reach. `S` is the access strategy: `Local`, `Readers`, or `Mutex` |
| **SH2: Strategy is a defaulted type parameter** | `Shared<T, S = Readers>`, resolved at monomorphization like the allocator parameter (`mem.alloc/AL4`). Zero cost — no dispatch, no stored tag. Defaulted, not absent: `Shared<T>` *is* `Shared<T, Readers>`, and mixing it with another strategy is a type error (E0381), not a coercion |
| **SH3: Bare means `Readers`, everywhere** | `Shared<T>` is `Shared<T, Readers>` in a `let`, a parameter, a field and a return type alike. One type expression, one meaning, whatever position it sits in |
| **SH4: Strategy-agnostic code says so** | A function that works with any strategy writes the parameter: `func serve<S>(c: Shared<Config, S>)`. Leaving it off means `Readers`, so a `Local` box handed to `serve(c: Shared<Config>)` is rejected — the strategy picks which lock the accessors take, and getting it wrong deadlocks rather than misbehaving visibly |
| **SH5: Two verbs** | `read()` and `write()`, inline or as a `with` block. Both exist under every strategy — `read()` under `Mutex` takes the exclusive lock: slower than `Readers` would be, never wrong |
| **SH6: Bare access forbidden** | `with s as v { }` is a compile error. Say `read()` or `write()`, so the page shows which one you meant |
| **SH7: `Local` can't cross a task** | Sending a `Shared<T, Local>` to another task is a compile error. `Local` takes no lock, so two tasks touching it would race. This rule is what makes the opt-out safe to reach for |
| **SH8: The default serves the common case** | Most boxes are reached by more than one task, so the default locks. Opting out is a word (`Shared.local`) and the compiler catches you if you were wrong; not opting out costs some time you can measure. The direction that can't be caught is the one that isn't the default |

| Strategy | Who reaches it | Synchronization | Crosses tasks | Constructor |
|---|---|---|---|---|
| `Readers` *(default)* | many tasks | read-write lock | yes | `Shared.new(v)` |
| `Mutex` | many tasks | plain lock | yes | `Shared.mutex(v)` |
| `Local` | closures and scopes in one task | none | no (SH7) | `Shared.local(v)` |

The strategy lives in the type, so it is always writable and always inspectable.
A constructor that silently changed behaviour with no type-level trace would be
magic:

<!-- test: skip -->
```rask
let config:  Shared<Config> = Shared.new(cfg)             // Readers, the default
let queue:   Shared<Queue, Mutex> = Shared.mutex(q)       // writes dominate
let counter: Shared<i64, Local> = Shared.local(0)         // never leaves this task
```

Constructor and annotation agree, and either one alone tells a reader what they
have.

### Reading and writing

| Rule | Description |
|------|-------------|
| **R1: Read** | `with s.read() as v { ... }` — shared read access. Mutating through a read binding is a compile error (E0360) and never writes back |
| **R2: Write** | `with s.write() as v { ... }` — exclusive access |
| **R2a: Unused write warning** | `.write()` whose binding is never mutated warns and suggests `.read()` |
| **R3: Try variants** | `try_read(f)` / `try_write(f)` — non-blocking closures, `none` if contended. Always succeed under `Local` |
| **R4: Bare access forbidden** | See SH6 |
| **R5: Inline access** | `s.read().chain` and `s.write().chain` — scope is the expression (`mem.borrowing/E5`). A standalone `.read()`/`.write()` with nothing chained is a compile error |

<!-- test: skip -->
```rask
mut config = Shared.new(AppConfig {
    timeout: 30.seconds,
    max_retries: 3,
})

// Inline — scope is the expression
let timeout = config.read().timeout
config.write().timeout = 60.seconds

// Block — scope is the block
with config.write() as c {
    c.timeout = 60.seconds
    c.max_retries = 5
}
```

`return`, `try`, `break` and `continue` propagate through the block (`mem.borrowing/W1`).

### API

<!-- test: skip -->
```rask
struct Shared<T, S = Readers> { }

extend Shared<T, S> {
    func new(value: T) -> Shared<T, Readers>       // the default
    func mutex(value: T) -> Shared<T, Mutex>
    func local(value: T) -> Shared<T, Local>

    func read(self) -> T             // inline or `with` (R5)
    func write(self) -> T            // inline or `with` (R5)
    func staged(self) -> T           // staged access (ST1) — Readers and Mutex only
    func try_read<R>(self, f: |T| -> R) -> R?
    func try_write<R>(self, f: |T| -> R) -> R?

    func get(self) -> T              // copy the value out; Copy types only
    func set(self, value: T)         // replace the value
    func replace(self, value: T) -> T
    func into_inner(take self) -> T
}
```

`get`/`set`/`replace`/`into_inner` are the single-expression shorthands `Cell`
had. They work under every strategy and take the appropriate access for one
operation.

### Sending one across a task boundary

The default crosses freely. The opt-out doesn't, and that is the whole reason
it's safe to reach for:

<!-- test: skip -->
```rask
let counter = Shared.local(0)
spawn(own || { counter.write() += 1 })     // compile error, SH7
```

```
error[E0346]: this `Shared` is task-local and cannot be sent
   |
 7 |             with counter.write() as c { c += 1 }
   |             ^^^^^^^^^^^^ `counter` uses the `Local` strategy
   |
   = fix: drop the `.local` — `Shared.new(…)` locks, and `Shared.mutex(…)`
          locks more cheaply when writes dominate
   = why: `Local` takes no lock at all, so two tasks touching it would race. It
          is the opt-out, not the default, and this error is what makes it safe
          to reach for
```

## Atomics are not a strategy

`Atomic<T>` keeps its own type and its own vocabulary — `add`, `load`, `store`,
`compare_swap`, no `with` (`mem.atomics`). The whole appeal of an atomic is that
no lock is taken, and `write()` is lock-shaped syntax: dressing a one-instruction
operation in lock ceremony would misprice it at every use site, which is the
opposite of what transparency asks for.

An atomic is a measured optimization, reached for after benchmarking. It is not
an answer to "where does my data live".
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
with account.write() as data {
    data.field = value
    try validate(data)    // propagates to enclosing function
}

// Guard-based (Rust) — reference can escape scope
// mut guard = account.write()  // NOT in Rask
```

## Deadlock Prevention

| Rule | Description |
|------|-------------|
| **DL1: Direct nesting** | Nested `with` on different sync primitives is a compile error |
| **DL2: Same lock** | `with shared.read() as v { with shared.write() as v2 { ... } }` is a compile error |
| **DL3: Indirect — your responsibility** | Locks acquired through function calls or dynamic dispatch are NOT detected |
| **DL4: Multiple inline accesses** | Multiple `.read()`/`.write()` calls in the same expression is a compile error — same deadlock risk as DL1 |

```
ERROR [conc.sync/DL1]: nested lock acquisition
   |
5  |  with a.write() as av {
6  |      with b.write() as bv {
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
  let x = shared_a.read().x
  process(x, shared_b.read().y)
```

<!-- test: skip -->
```rask
// OK: multiple elements from same collection (not a lock)
with pool[h1] as e1, pool[h2] as e2 {
    // runtime panic if h1 == h2
}
```

## Staged Access

Panic-atomic updates, opt-in per site. A panic mid-`with` releases the lock cleanly but keeps whatever writes already happened (`ctrl.panic/LK1–LK3`) — when other tasks will read a multi-field invariant, that torn state is unacceptable. `staged()` makes the update atomic with respect to panics *by construction*: the `with` block has exact boundaries, so the runtime can work on a copy and commit it as one move.

| Rule | Description |
|------|-------------|
| **ST1: Staged access** | `with s.staged() as v { }` — takes the exclusive lock, binds `v` to a working copy of the value (clone at entry) |
| **ST2: Commit on exit** | Every non-panic exit (normal, `return`, `try`, `break`/`continue`) commits the copy back as one move |
| **ST3: Panic discards** | Unwind drops the copy uncommitted — survivors see the last committed state. Torn state impossible at staged sites |
| **ST3a: Not on `Local`** | `staged()` under `Local` is a compile error — there is no other task to observe a torn update and no unwind boundary to protect against |
| **ST4: Panic-only scope** | Staged is not error rollback — `try` exits commit (ST2). Rollback-on-error already has a mechanism: `ensure tx.rollback()` + explicit commit (`ctrl.ensure/C1`) |

<!-- test: parse -->
```rask
// Vulnerable: panic between the two writes leaves state torn
func transfer_torn(amount: i64) {
    with accounts as a {
        a.checking -= amount
        a.savings += amount      // panic here → survivors see money destroyed
    }
}

// Staged: both writes land as one commit, or not at all
func transfer(amount: i64) {
    with accounts.staged() as a {
        a.checking -= amount
        a.savings += amount      // panic here → nothing committed (ST3)
    }                            // clean exit → one commit (ST2)
}
```

The clone is the price and the method name says so — same visibility deal as `.clone()`. Plain `with mutex as v` stays free; only sites guarding a real invariant pay. Cross-box invariants stay out of reach, but nested locks are already forbidden (DL1), so per-box atomicity is all the language promises anyway.

The compiler backs this up by default: a `with` block over a sync box that assigns two or more fields of the locked value without `staged()` gets the `torn_lock_update` warning (`tool.warnings/W9`). Suppress with `@allow(torn_lock_update)` where partial state is genuinely harmless.

Panic is the only way another task can see a half-done update. Suspension keeps the lock held, and cancellation surfaces as an ordinary error return — never a kill at the pause point (`ctrl.panic/LK4`, `conc.async/CN4`).

## Non-blocking variants

`try_read`, `try_write`, and `try_lock` stay as closures. These are uncommon and closure-based is fine for them. `with` is always blocking.

<!-- test: skip -->
```rask
// Blocking: with
with m.write() as v { v.push(item) }

// Non-blocking: closure
let got_it = m.try_write(|v| v.push(item))
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

| Standalone `shared.read()` without chaining | R5 | Compile error |
| Inline access inside `with` on same primitive | DL2 | Compile error |
| Panic inside `with` on a sync primitive | — | Lock releases cleanly, no poisoning; writes so far kept (`ctrl.panic/LK1–LK3`) |
| Panic inside `with x.staged()` | ST3 | Working copy discarded — nothing committed |
| Task pauses on I/O inside `with` on a sync primitive | — | Lock stays held while parked; waiters block, never see intermediate state (`ctrl.panic/LK4`) |
| Task cancelled inside `with` on a sync primitive | — | Cancellation is an error return (`conc.async/CN4`); the block exits through normal control flow, writes kept — same as any early `try` exit |
| Multi-field write without `staged()` | — | `torn_lock_update` warning, on by default (`tool.warnings/W9`) |
| Writers starve under read load | SY1 | By design — read performance prioritized |

---

## Appendix (non-normative)

### Rationale

**WS1 (`with`-based access):** Rask's "no storable references" principle naturally leads to scoped access. Guards (Rust's `MutexGuard`) require lifetime tracking and allow references to escape scope. `with` blocks make unlock timing explicit and prevent escaping references by construction. The win over the old closure-based API: `return`, `try`, `break`, and `continue` work naturally.

**DL3 (indirect locks):** Detecting all lock acquisition paths requires whole-program analysis, which violates local-only compilation. Syntactic detection catches the most common mistakes; ordering discipline handles the rest.

**Why `Readers` is the default and `Local` isn't.** The first draft had it the other way round, on the grounds that you should never accidentally pay for synchronization you didn't need. That reasoning is fine and the conclusion was still wrong, because it ignored how often each case actually turns up. A box that never leaves its task — the old `Cell` — is rare. A box several tasks reach is the ordinary reason to have one at all. A default that serves the rare case makes every common program write a word to get what it wanted, and `Shared<T>` would have meant the one thing a reader of the name least expects.

The costs aren't symmetric either. Taking a lock you didn't need costs time you can measure and then opt out of with `Shared.local`. Skipping one you did need costs correctness and shows up as a race. Defaulting to the locked strategy puts the recoverable mistake on the default path and leaves the unrecoverable one behind a word and a compile error (SH7).

**Why one type and not two.** The only thing separating the old `Shared` from the old `Mutex` was whether many readers get in at once. That is a benchmarking question — do concurrent readers matter more than write overhead here? — and it was being answered by picking a type at declaration time, before the program existed. `Cell` was the same shape again with the lock removed. One type with a defaulted strategy parameter removes a question users can't answer, and turns "I sent a task-local value to another task" from a race into a compile error. Full argument in `analysis.storage-consolidation`.

**R1/R2 (explicit .read()/.write()):** The old `with mutex as v` never said whether you were reading or writing. Under one type you always say, at every use site — read/write intent becomes visible in code that used to hide it.

**try_* stay as closures:** Non-blocking access is uncommon. The inconsistency is justified — `with` is inherently blocking (it's a scope, not a conditional). Could add `with try mutex as v { ... } else { ... }` later if the pattern is common enough.

### When to Use What

| Scenario | Primitive | Why |
|----------|-----------|-----|
| Config read by many tasks | `Shared<T>` | Read-heavy, writes rare |
| Feature flags | `Shared<T>` | Read-heavy |
| Connection pool | `Shared<T, Mutex>` | Checkout/checkin is write-heavy |
| Request queue | `Shared<T, Mutex>` | Push/pop are mutations |
| Metrics counter | `Atomic<u64>` | Single value, lock-free |
| Shutdown flag | `Atomic<bool>` | Single value, lock-free |
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
func transfer(from: Shared<Account, Mutex>, to: Shared<Account, Mutex>, amount: u64) {
    let from_balance = from.write().balance
    from.write().balance -= amount
    to.write().balance += amount
}

// Copy out, modify, copy back
func swap_values(a: Shared<i32, Mutex>, b: Shared<i32, Mutex>) {
    let a_val = a.write().clone()
    let b_val = b.write().clone()
    with a as v { v = b_val }
    with b as v { v = a_val }
}
```

### Performance Characteristics

| Primitive | Uncontended | Read Contention | Write Contention |
|-----------|-------------|-----------------|------------------|
| `Shared<T>` | ~20ns | Scales linearly | Blocks all |
| `Shared<T, Mutex>` | ~20ns | N/A (no read mode) | Serialized |
| `Atomic<u64>` | ~1ns | ~1ns | ~10ns (CAS retry) |
| Channel | ~50ns | N/A | Bounded: blocks, Unbounded: allocates |

### Examples

**Application config:**
<!-- test: skip -->
```rask
static CONFIG: Shared<AppConfig> = Shared.new(AppConfig {})

func get_timeout() -> Duration {
    return CONFIG.read().timeout
}
```

**Metrics collection:**
<!-- test: skip -->
```rask
struct Metrics {
    requests: Atomic<u64>,
    errors: Atomic<u64>,
    latencies: Shared<Vec<Duration>, Mutex>,
}

func record_request(latency: Duration, success: bool) {
    METRICS.requests.fetch_add(1, Relaxed)
    if !success { METRICS.errors.fetch_add(1, Relaxed) }
    METRICS.latencies.write().push(latency)
}
```

### Design Decisions

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Access pattern | `with`-based blocks + inline `.read()`/`.write()`/`.write()` | Guard-based / closure-based | No escaping references, `return`/`try` work, prevents nested deadlock. Inline access for single-expression convenience |
| Read-heavy vs write-heavy | A strategy on one type | Two types | Picking a type from an expected read/write ratio is a benchmarking decision at declaration time, before the program exists |
| Naming | `Shared<T>` | `RwLock<T>` | Describes intent, not mechanism |
| Task-local sharing | `Shared<T, Local>` | A separate `Cell<T>` | Same concept, different synchronization — a strategy, not a type |
| Direct nested locks | Compile error (syntactic) | Whole-program analysis | Local analysis only |
| Non-blocking variants | Closure-based (`try_*`) | `with try` syntax | Uncommon pattern, closures are fine |

### See Also

- `mem.atomics` — lock-free primitives for single values
- `conc.async` — channels and task spawning
- `mem.pools` — single-task dynamic data structures
- `mem.borrowing` — `with` semantics and rules
