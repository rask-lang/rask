<!-- depends: memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Solution: Ownership Model

## The Question
How does Rask achieve memory safety without garbage collection, reference counting overhead, or Rust-style lifetime annotations?

## Decision
Value semantics with single ownership, scoped borrowing, and handle-based indirection. Safety emerges from structure, not annotations.

## Rationale
I wanted "safety without annotation." Strict ownership plus scoped borrowing that can't escape eliminates use-after-free, dangling pointers, and data races—no lifetime parameters in signatures.

## Specification

### Ownership Rules

Every value has exactly one owner. Ownership transfers on assignment/passing (for non-Copy types).

| Rule | Description |
|------|-------------|
| **O1: Single owner** | Every value has exactly one owner at any time |
| **O2: Move on assignment** | For non-Copy types, assignment transfers ownership |
| **O3: Invalid after move** | Source binding is invalid after move; use is compile error |
| **O4: Explicit clone** | To keep access while transferring, clone explicitly |

<!-- test: skip -->
```rask
const a = Vec.new()
const b = a              // a moved to b
a.push(1)              // ❌ ERROR: a is invalid after move

const c = b.clone()      // Explicit clone - visible allocation
c.push(1)              // ✅ OK: c is independent copy
b.push(2)              // ✅ OK: b still valid
```

### Cross-Task Ownership

Tasks are isolated. No shared mutable memory.

| Rule | Description |
|------|-------------|
| **T1: Send transfers** | Sending on channel transfers ownership |
| **T2: No shared mut** | Cannot share mutable references across tasks |
| **T2.1: Closure-based OK** | `Shared<T>` and `Mutex<T>` provide cross-task mutable access via closures |
| **T3: Borrows don't cross** | Block-scoped borrows cannot be sent to other tasks |

**Rule T2.1 clarification:** `Shared<T>` and `Mutex<T>` don't violate T2 because they provide *operation-scoped* access through closures, not storable mutable references. When the closure returns, access is released. See [sync.md](../concurrency/sync.md).

<!-- test: skip -->
```rask
const data = load_data()
channel.send(data)        // Ownership transferred
data.process()            // ❌ ERROR: data was sent

// Receiving:
const received = channel.recv()   // Ownership acquired
received.process()              // ✅ OK: we own it now
```

### Explicit Drop: `discard`

For non-Copy types, simply letting a binding go out of scope drops it. But sometimes you want to explicitly signal "I'm done with this" — especially for clarity or when the compiler warns about an unused value.

`discard` explicitly drops a value and invalidates its binding:

<!-- test: skip -->
```rask
const data = load_data()
process(data.clone())
discard data   // Explicit: data dropped here, not at end of scope
```

**When to use `discard`:**

| Scenario | Example |
|----------|---------|
| Resource not needed | `const _ = acquire(); discard _` — prefer `ensure` instead |
| Clarity in long functions | Drop early to signal intent |
| Consuming move-only types | When no consuming function exists |

**Rules:**

| Rule | Description |
|------|-------------|
| **D1: Invalidates binding** | Using the binding after `discard` is a compile error |
| **D2: Non-Copy only** | `discard` on Copy types is a warning (they're trivially dropped) |
| **D3: Not for linear resources** | `@resource` types must be consumed properly — `discard` on them is a compile error. Use the type's consuming method (`.close()`, `.release()`, etc.) |

**Why not just `_ = value`?** `_ = value` moves into a wildcard but doesn't communicate intent. `discard` says "I know this has value and I'm deliberately dropping it." It's the ownership equivalent of `// intentionally unused`.

### Edge Cases

| Case | Handling |
|------|----------|
| Borrow from temporary | Temporary lifetime extended to match borrow |
| Move in one branch | Value invalid in all subsequent code |
| Clone of borrowed | Allowed (creates independent copy) |
| Linear value in error path | Must be consumed or in `ensure`; compiler tracks |

## Related Specifications

The memory model is split across focused specifications:

| Topic | Specification |
|-------|---------------|
| Copy vs move semantics | [value-semantics.md](value-semantics.md) |
| Borrowing (one rule: "can it grow?") | [borrowing.md](borrowing.md) |
| Must-consume resources | [resource-types.md](resource-types.md) |
| Closure capture rules | [closures.md](closures.md) |
| Handle-based indirection | [pools.md](pools.md) |

## Integration Notes

- **Type System:** Borrow types are compiler-internal; user sees owned types and parameter modes
- **Generics:** Bounds can require Copy, which affects move/copy behavior
- **Pattern Matching:** Match arms share borrow mode; highest mode wins
- **Concurrency:** Channels transfer ownership; `Shared<T>` and `Mutex<T>` provide safe cross-task shared state via closures (see [sync.md](../concurrency/sync.md))
- **C Interop:** Raw pointers in unsafe blocks; convert to/from safe types at boundaries
- **Tooling:** IDE shows move/copy at each use site, borrow scopes

## See Also

- [Value Semantics](value-semantics.md) — Copy threshold, move-only types
- [Borrowing](borrowing.md) — Scoped borrowing rules
- [Resource Types](resource-types.md) — Resource consumption
- [Closures](closures.md) — Capture semantics
- [Pools](pools.md) — Handle-based indirection
