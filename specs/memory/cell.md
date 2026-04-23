<!-- id: mem.cell -->
<!-- status: decided -->
<!-- summary: Single-value mutable container with `with`-based access -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->

# Cell

`Cell<T>` is a single heap-allocated value with `with`-based access. When you need one mutable value shared across multiple closures without the ceremony of Pool+Handle.

## Why Cell Exists

Pool+Handle solves the general case: collections of values with stable identity, graph structures, cross-task sharing. But for the common case — "I have one mutable value that multiple closures need" — a full pool is overkill:

<!-- test: skip -->
```rask
// Pool+Handle for one value is ceremony
const pool = Pool.new()
const h = pool.insert(AppState{...})
button.on_click(|event| pool[h].count += 1)

// Cell: direct
const state = Cell.new(AppState{...})
button.on_click(|event, state| {
    with state as s { s.count += 1 }
})
```

## Rules

| Rule | Description |
|------|-------------|
| **CE1: Heap-allocated** | `Cell.new(value)` heap-allocates the value |
| **CE2: Value semantics** | `Cell<T>` is a value that owns its heap data (like Vec, string) |
| **CE3: Move-only** | `Cell<T>` is never Copy; assignment moves |
| **CE4: with access** | Access through `with cell as v { ... }` — always mutable binding |
| **CE5: Exclusive mutation** | `with...as v` (mutable, default) takes exclusive access; no concurrent reads or writes |
| **CE6: Convenience methods** | `.get()` returns a copy (Copy types only), `.set(value)` replaces the inner value — single-expression alternatives to `with` |

## API

| Method | Signature | Description |
|--------|-----------|-------------|
| `Cell.new(value)` | `T -> Cell<T>` | Create cell with initial value |
| `cell.get()` | `Cell<T> -> T` | Copy out the inner value (CE6, Copy types only) |
| `cell.set(value)` | `(Cell<T>, T) -> void` | Replace inner value (CE6) |
| `cell.replace(value)` | `T -> T` | Swap in new value, return old |
| `cell.into_inner()` | `take Cell<T> -> T` | Consume cell, return value |

Access is through `with`:

<!-- test: skip -->
```rask
const counter = Cell.new(0)

// Convenience methods (CE6)
const current = counter.get()            // Copy out (Copy types only)
counter.set(42)                          // Replace inner value

// with access (CE4) — still needed for multi-statement or non-Copy
with counter as c { c += 1 }
with counter as c: c += 1               // one-liner shorthand

// Expression context
const doubled = with counter as c { c * 2 }

// Replace (returns old value)
const old = counter.replace(0)

// Consume
const final_value = counter.into_inner()
```

## Shared Across Closures

`Cell<T>` is Copy-sized (one pointer, 8 bytes) but NOT Copy — it moves. To share across multiple closures, closures capture the cell by copy of the pointer (the cell value itself is small enough to be Copy-eligible at 8 bytes, but is `@unique` to prevent accidental duplication of ownership).

For multiple closures to share a Cell, use a handle or pass it as a parameter:

<!-- test: skip -->
```rask
// Pattern: closures receive cell as parameter
func setup(state: Cell<AppState>) {
    button1.on_click(|event, state| {
        with state as s { s.mode = Mode.Edit }
    })
    button2.on_click(|event, state| {
        with state as s { s.mode = Mode.View }
    })
    app.run_with(state)
}

// Pattern: Cell in a struct
struct App {
    state: Cell<AppState>
}

extend App {
    func on_click(self, event: Event) {
        with self.state as s { s.click_count += 1 }
    }
}
```

## When to Use What

| Need | Use | Why |
|------|-----|-----|
| One mutable value, local scope | `mutate` capture | Simplest, no allocation |
| One mutable value, multiple closures | `Cell<T>` | No pool ceremony |
| Collection of values with identity | `Pool<T>` + `Handle<T>` | Stable handles, generation checks |
| Cross-task shared state | `Shared<T>` / `Mutex<T>` | Thread-safe access |

## Edge Cases

| Case | Handling |
|------|----------|
| `Cell<Cell<T>>` | Allowed but discouraged — flatten to one Cell |
| `Cell<@resource>` | Allowed; `into_inner()` returns the resource for consumption |
| Recursive mutable `with...as` | Panic: cell is exclusively borrowed |
| `Cell<T>` in Vec | Allowed (Cell is a value type) |
| Drop | Heap data freed, inner T dropped normally |

## Error Messages

**Recursive access [CE5]:**
```
PANIC: Cell is exclusively borrowed — recursive access in with block
```

---

## Appendix (non-normative)

### Rationale

**CE1 (heap-allocated):** Cell needs a stable address so closures can share it. Stack allocation would require borrow tracking. Heap allocation with value ownership keeps it simple — same model as Vec or string.

**CE4 (with access):** Direct field access (`cell.value.field`) would create a reference that escapes the cell's control. `with`-based access ensures exclusive mutation and prevents dangling references. Same pattern as `Shared<T>`, `Mutex<T>`, and collection `with` blocks — one construct for all container types.

**Why not just use `Shared<T>`?** `Shared<T>` is thread-safe (atomic operations, cross-task sending). Cell is single-task — no synchronization overhead. Use Cell when you don't need cross-task sharing.

### See Also

- [Boxes](boxes.md) — The container family Cell belongs to (`mem.boxes`)
- [Closures](closures.md) — Mutable capture, closure patterns (`mem.closures`)
- [Pools](pools.md) — Handle-based collections (`mem.pools`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>` for cross-task access (`conc.sync`)
- [Borrowing](borrowing.md) — `with` semantics and rules (`mem.borrowing`)
