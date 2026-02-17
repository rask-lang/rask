<!-- id: mem.ownership -->
<!-- status: decided -->
<!-- summary: Single owner, value transfer for large types, handle-based indirection -->
<!-- depends: memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-ownership/, compiler/crates/rask-interp/ -->

# Ownership

Value semantics with single ownership, scoped borrowing, and handle-based indirection. Safety emerges from structure, not annotations.

## Ownership Rules

| Rule | Description |
|------|-------------|
| **O1: Single owner** | Every value has exactly one owner at any time |
| **O2: Move on assignment** | For large types (>16 bytes), assignment transfers the value — the original variable becomes unusable |
| **O3: Invalid after move** | Using the original variable after a move is a compile error |
| **O4: Explicit clone** | To keep access while transferring, clone explicitly |

<!-- test: skip -->
```rask
const a = Vec.new()
const b = a              // a moved to b
a.push(1)              // ERROR: a is invalid after move

const c = b.clone()      // Explicit clone - visible allocation
c.push(1)              // OK: c is independent copy
b.push(2)              // OK: b still valid
```

## Cross-Task Ownership

Tasks are isolated. No shared mutable memory.

| Rule | Description |
|------|-------------|
| **T1: Send transfers** | Sending on channel transfers ownership |
| **T2: No shared mut** | Cannot share mutable references across tasks |
| **T2.1: Closure-based OK** | `Shared<T>` and `Mutex<T>` provide cross-task mutable access via closures |
| **T3: Borrows don't cross** | Block-scoped views cannot be sent to other tasks |

Rule T2.1 clarification: `Shared<T>` and `Mutex<T>` don't violate T2 because they provide *operation-scoped* access through closures, not storable mutable references. When the closure returns, access is released. See `conc.sync`.

<!-- test: skip -->
```rask
const data = load_data()
channel.send(data)        // Ownership transferred
data.process()            // ERROR: data was sent

// Receiving:
const received = channel.recv()   // Ownership acquired
received.process()              // OK: we own it now
```

## Explicit Drop: `discard`

`discard` explicitly drops a value and invalidates its binding.

| Rule | Description |
|------|-------------|
| **D1: Invalidates binding** | Using the binding after `discard` is a compile error |
| **D2: Non-Copy only** | `discard` on Copy types is a warning (they're trivially dropped) |
| **D3: Not for resources** | `@resource` types must be consumed properly — `discard` on them is a compile error. Use the type's consuming method (`.close()`, `.release()`, etc.) |

<!-- test: skip -->
```rask
const data = load_data()
process(data.clone())
discard data   // Explicit: data dropped here, not at end of scope
```

| Scenario | Example |
|----------|---------|
| Resource not needed | `const _ = acquire(); discard _` — prefer `ensure` instead |
| Clarity in long functions | Drop early to signal intent |
| Consuming move-only types | When no consuming function exists |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Borrow from temporary | S4 | Temporary duration extended to match borrow |
| Move in one branch | O3 | Value invalid in all subsequent code |
| Clone of borrowed | — | Allowed (creates independent copy) |
| Resource in error path | R1 | Must be consumed or in `ensure`; compiler tracks |

---

## Appendix (non-normative)

### Rationale

**O1–O4 (single ownership):** I wanted "safety without annotation." Strict ownership plus scoped borrowing that can't escape eliminates use-after-free, dangling pointers, and data races — no scope parameters in function signatures.

**D1–D3 (discard):** `_ = value` moves into a wildcard but doesn't communicate intent. `discard` says "I know this has value and I'm deliberately dropping it." It's the ownership equivalent of `// intentionally unused`.

### Patterns & Guidance

**Integration with other systems:**

| System | Integration |
|--------|-------------|
| Type System | Borrow types are compiler-internal; user sees owned types and parameter modes |
| Generics | Bounds can require Copy, which affects move/copy behavior |
| Pattern Matching | Match arms share borrow mode; highest mode wins |
| Concurrency | Channels transfer ownership; `Shared<T>` and `Mutex<T>` provide safe cross-task shared state via closures (`conc.sync`) |
| C Interop | Raw pointers in unsafe blocks; convert to/from safe types at boundaries |

### IDE Integration

IDE shows move/copy at each use site and borrow scopes.

### See Also

- [Value Semantics](value-semantics.md) — Copy threshold, move-only types (`mem.value`)
- [Borrowing](borrowing.md) — Scoped borrowing rules (`mem.borrowing`)
- [Resource Types](resource-types.md) — Resource consumption (`mem.resources`)
- [Closures](closures.md) — Capture semantics (`mem.closures`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
