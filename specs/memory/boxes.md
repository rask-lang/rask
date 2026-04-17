<!-- id: mem.boxes -->
<!-- status: decided -->
<!-- summary: Container types that give scoped access through `with` — one shape, several access disciplines -->
<!-- depends: memory/borrowing.md, memory/ownership.md -->

# The Box Family

A **box** wraps a value and gives access through a scoped construct — inline expression access, or a `with` block. You don't touch the inner value directly; you ask for scoped access, do your work, and the scope ends.

One shape, one syntax, several access disciplines.

## The family

| Box | Access discipline | Cross-task? | Use when |
|-----|-------------------|-------------|----------|
| [`Cell<T>`](cell.md) | Exclusive, single-task | No | One mutable value shared across closures |
| [`Pool<T>`](pools.md) + `Handle<T>` | Identity-based (generation-checked) | Sendable | Collections, graphs, entity systems |
| [`Shared<T>`](../concurrency/sync.md) | Read-heavy (many readers XOR one writer) | Yes | Config, feature flags |
| [`Mutex<T>`](../concurrency/sync.md) | Exclusive lock | Yes | Queues, state machines |
| [`Owned<T>`](owned.md) | Linear (single consumer) | Sendable | Recursive types, AST nodes |

`Atomic*<T>` (see [`mem.atomics`](atomics.md)) is adjacent but not a box — its access is intrinsic operations, not `with`.

## The shared shape

All boxes support two access patterns.

**Inline** — single expression, scope is the expression:

<!-- test: skip -->
```rask
pool[h].health -= 10           // Pool
shared.read().timeout          // Shared (expression-scoped read lock)
mutex.lock().push(item)        // Mutex (expression-scoped lock)
cell.get()                     // Cell (Copy types only)
```

**`with` block** — multi-statement, scope is the block:

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.update()
    entity.mark_dirty()
}
with shared.write() as c {
    c.timeout = 60.seconds
    c.retries = 5
}
with mutex as q {
    q.push(a)
    q.push(b)
}
with cell as v { v.count += 1 }
```

`return`, `try`, `break`, and `continue` work through every `with` block (`mem.borrowing/W1`). This is why Rask uses `with` instead of closure-based access — control flow propagates naturally.

## Why scoped access, not guards

Rust-style guards (`MutexGuard`, `Ref`, `RefMut`) let a reference escape the acquisition site. Rask's boxes don't — the inner value is reachable only inside the `with` block or inline expression. This falls out of "no storable references" and gives three properties:

- **No escaping references** — the view can't outlive the scope, by construction.
- **Explicit unlock timing** — lock released at block/expression end, visible in code.
- **Control flow works** — `return`/`try`/`break`/`continue` propagate naturally; closures can't do this.

## Choosing a box

Quick decision path:

1. **Cross-task shared state?** → `Shared<T>` (read-heavy), `Mutex<T>` (write-heavy), or a channel (transfer ownership).
2. **Many values with stable identity, graphs, or ECS?** → `Pool<T>` + `Handle<T>`.
3. **Recursive type (tree, AST, linked list)?** → `Owned<T>`.
4. **One mutable value shared across closures, single-task?** → `Cell<T>`.
5. **Single counter, flag, or pointer-sized value?** → Atomic (not a box — lock-free primitive).

Don't nest boxes without a reason. `Cell<Cell<T>>`, `Shared<Mutex<T>>`, and similar compositions usually indicate the wrong box was chosen first. If you need cross-task mutation, reach for `Mutex<T>` directly — don't wrap a `Cell<T>`.

## Cross-cutting properties

| Property | Cell | Pool | Shared | Mutex | Owned |
|----------|------|------|--------|-------|-------|
| Copy | No (@unique) | No | No | No | No |
| Sendable cross-task | No | If `T: Send` | Yes | Yes | If `T: Send` |
| Blocking access | — | — | Yes (writers) | Yes | — |
| Linear (must consume) | No | If `T` is linear | No | No | Yes |
| Heap-allocated inner value | Yes | Yes | Yes | Yes | Yes |

Every box heap-allocates its contents — that's part of being a box. The `with`-scoped access is what keeps the indirection safe without tracking lifetimes.

## See Also

- [Cell](cell.md) — Single-value mutable box (`mem.cell`)
- [Pools](pools.md) — Handle-based identity box (`mem.pools`)
- [Owned Pointers](owned.md) — Linear heap box (`mem.owned`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>` and `Mutex<T>` (`conc.sync`)
- [Ownership](ownership.md) — Why boxes hold heap data by value (`mem.ownership`)
- [Borrowing](borrowing.md) — `with` semantics and rules (`mem.borrowing`)
- [Linearity](linear.md) — Must-consume rules (`mem.linear`)

---

## Appendix (non-normative)

### Why name the family?

Before this spec, Cell, Shared, Mutex, Pool, and Owned each stood alone with their own "when to use what" tables duplicated across specs. Readers had to cross-reference five pages to build a mental model.

They're one family with one syntax. Naming it collapses five decisions ("which type do I pick?") into one (`with` access is the common shape; pick the access discipline that fits your problem). The individual specs still own their details — this page just makes the family visible.

### Is every type with `with` access a box?

Not quite. Vec, Map, and arrays also work with `with <source>[key] as binding` — but they're collections, not boxes. The distinction: a box wraps *one* value (or one value per handle, for Pool). Vec/Map wrap a sequence/mapping and have structural operations (push, remove, clear) that boxes don't. The shared piece is the `with`-based element access.

Think of it this way: `with` is the universal scoped-access syntax; boxes are the types whose primary purpose is to *be* accessed through it.
