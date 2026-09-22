<!-- id: mem.linear -->
<!-- status: decided -->
<!-- summary: Values that must be consumed exactly once — one rule set shared by @resource and Heap<T> -->
<!-- depends: memory/ownership.md -->

# Linearity

A value is *linear* when the compiler requires it to be consumed exactly once before its binding goes out of scope. Not zero times (can't silently drop it), not twice (can't double-use it). Exactly once.

The everyday version: a paper concert ticket. The gate takes it and tears it — you can't leave without handing it over (can't skip consumption), and you can't hand it over twice (can't double-spend). A file handle, a database transaction, an `Heap<T>` — they all behave the same way.

"Affine" is the cousin term: *at most once* — consume it, or drop it, either is fine. Rust's default ownership is affine (dropping a value runs its `Drop` impl). Rask's linear values are strictly linear: dropping without an explicit consumption is a compile error.

## Why linear, not affine?

Rust's affine model hides cleanup behind a `Drop` impl. That's elegant for memory but invisible at the use site — you never see the `close()` call.

Linear values keep cleanup **visible**. You write `file.close()` or `ensure file.close()` in your source. The compiler still guarantees exactly-once, but the call lives where the reader can see it.

Same tradeoff as "everything is a value": cost transparency over hidden mechanism.

## Rules

| Rule | Description |
|------|-------------|
| **L1: Must consume** | A linear value must be consumed before its binding goes out of scope |
| **L2: Consume once** | A linear value cannot be consumed twice |
| **L3: Borrow allowed** | Borrowing a linear value for reading or mutation does not consume it |
| **L4: `ensure` satisfies L1** | Registering with `ensure` commits to consumption at scope exit |
| **L5: Move consumes** | Passing to a `take` parameter, assigning to another binding, or sending on a channel consumes the value |
| **L6: Explicit consumption cancels `ensure`** | If the value is consumed before scope exit, the registered `ensure` is void (`ctrl.ensure/C1`) |
| **L7: Commit before anything else** | Nothing may stand between acquiring a linear value and committing its cleanup. The statement after an acquisition commits it — `ensure`, a consuming call, a move, or a `return` |

Consumption happens via:
- A method declared with `take self` (e.g. `file.close()`, `tx.commit()`)
- Passing to a `take` parameter (`consume(file)`)
- Channel send (`ch.send(file)` — ownership transfers to the receiver)
- `ensure expr` (defers consumption to scope exit; satisfies L1 immediately)

## What makes a value linear

Three ways a value acquires the linear property:

| Mechanism | Applies to | Specified in |
|-----------|------------|--------------|
| `@resource` annotation | Struct types (File, Connection, Transaction) | [resource-types.md](resource-types.md) |
| `Heap<T>` type constructor | Any T, heap-allocated | [heap.md](heap.md) |

Rules L1–L7 apply identically in all three cases. The individual specs cite them instead of restating.

## Linearity + `ensure` + `try`

The common pattern: acquire a linear value, commit to consumption, then use `try` freely.

<!-- test: parse -->
```rask
func process(path: string) -> Data or Error {
    let file = try File.open(path)
    ensure file.close()                    // L4: consumption committed

    let header = try file.read_header()  // try is safe after ensure
    let body = try file.read_body()
    return body
}
```

Without `ensure`, the first `try` after acquisition is a compile error — the file might leak on error propagation. With `ensure`, the commitment is in place, and errors can propagate knowing cleanup still runs.

This is the trio the language design leans on: linearity gives the guarantee, `ensure` gives the deferral, `try` gives the propagation. Each alone is limited; together they cover most I/O code in three lines.

## L7: the window has to be empty

`try` is not the only way out of a scope. A panic leaves through any line — an
index, an overflow, a call that asserts — and Rask has no destructor to catch
what falls out. So a linear value with no cleanup scheduled is one panic away
from being gone for good:

<!-- test: compile-fail: ownership -->
```rask
@resource
struct DbConn {
    handle: i32
}

extend DbConn {
    func open(path: string) -> DbConn or Error {
        return DbConn { handle: 1 }
    }

    func read_text(self) -> string or Error {
        return "data"
    }

    func close(take self) -> void or Error {
        return
    }
}

func process(path: string) -> string or Error {
    let conn = try DbConn.open(path)
    let limit = path.len()        // ERROR: a panic here and `conn` is gone
    ensure conn.close()
    return try conn.read_text()
}
```

Keeping that gap short is a habit, and habits are not what the rest of this spec
is made of. L7 makes it a rule instead: the statement after an acquisition
commits the value, or the program doesn't build. Committing means `ensure`, a
consuming call, a move, or handing it back — and since consuming it later
cancels the `ensure` (L6), the line that used to sit at the bottom just moves to
the top. Same code, one line earlier:

<!-- test: parse -->
```rask
func process(path: string) -> string or Error {
    let conn = try DbConn.open(path)
    ensure conn.close()
    let limit = path.len()        // panics here run the ensure
    return try conn.read_text()
}
```

A `take` parameter arrives owed too, so the first statement of the body commits
it. The one place the rule doesn't reach is a `take self` method of the linear
type itself: that method *is* the consumption, and there is nothing left to
commit to.

Two values acquired together may take one statement each — every `ensure`
shortens the window, and registering them in acquisition order is what makes the
LIFO teardown come out right (`ctrl.ensure/EN2`).

## Linearity + explicit consumption (transaction pattern)

If the value is consumed explicitly before scope exit, any registered `ensure` is cancelled (L6). This is how the transaction pattern works:

<!-- test: parse -->
```rask
func transfer(db: Database) -> void or Error {
    let tx = try db.begin()
    ensure tx.rollback()     // Default: rollback on any exit

    try tx.execute("UPDATE ...")
    try tx.execute("INSERT ...")

    tx.commit()              // Consumes tx, cancels ensure (L6)
    return
}
```

Ensure the unhappy path, explicitly consume the happy path.

## Linearity in containers

No container can hold a linear value. A `Vec` or a `Map` drop would need to consume each element, and drop can't return errors; a `Rack.delete` frees the node rather than handing it back, so nothing can consume one. `Pool<T>` used to be the exception — `remove` answered `T?` — and went with the pool (rask-lang/rask#908).

| Container | Linear allowed? | Why |
|-----------|-----------------|-----|
| `Vec<T>` | No | Drop would need to consume each element |
| `Map<K, V>` | No | Same as Vec |
| `Rack<T>` | No | `delete` answers nothing, so a node can't be consumed |
| `T?` | Yes | Must narrow (`? as v`) and consume the present case |

See `mem.resource-types/RC1`–RC4 for where a linear value may live.

## Error messages

Base error identifiers live here; per-context specs (resource-types, owned) show worked examples.

**Cleanup not committed [L7]:**
```
ERROR [mem.linear/L7]: `file` has no cleanup committed yet

WHY: A panic here would leak it — nothing is scheduled to clean it up, and
     there are no destructors to fall back on.

FIX: Move the cleanup up to directly after the acquisition:

  let file = try File.open(path)
  ensure file.close()
```

**Not consumed [L1]:**
```
ERROR [mem.linear/L1]: linear value not consumed before scope exit

WHY: Linear values must be explicitly consumed. Silently dropping them
     would hide the cleanup the compiler is trying to guarantee.

FIX: Consume with a method or register with ensure:

  try file.close()       // Explicit consumption
  ensure file.close()    // Deferred consumption
```

**Consumed twice [L2]:**
```
ERROR [mem.linear/L2]: linear value already consumed

WHY: Linear values can be consumed exactly once. A second consumption
     would be a use-after-free.
```

## Edge cases

| Case | Rule | Handling |
|------|------|----------|
| Linear value in error path | L1 | Must be consumed, registered with `ensure`, or returned in the error type |
| Linear value across match arms | L1 | Every arm must consume (or share an outer `ensure`) |
| Conditional consumption | L1 | Both branches must consume |
| Linear value + panic | L4 | `ensure` runs during unwind |
| Linear value in loop | L1 | Each iteration's binding must be consumed that iteration |
| `take` parameter | L7 | Arrives owed; the body's first statement commits it |
| `take self` method of the linear type | — | The method is the consumption, so L7 doesn't apply to `self` |

## See Also

- [Resource Types](resource-types.md) — `@resource` struct annotation (`mem.resources`)
- [Heap Values](heap.md) — A value on the heap, consumed once (`mem.heap`)
- [Ensure](../control/ensure.md) — Deferred consumption (`ctrl.ensure`)
- [Ownership](ownership.md) — Single-owner model that linearity refines (`mem.ownership`)
- [Value Semantics](value-semantics.md) — Copy/move rules that linear values opt out of (`mem.value`)

---

## Appendix (non-normative)

### Why one spec for linearity?

Before this spec, the same rule set was restated in `resource-types.md` (R1–R4) and `heap.md` (OW1–OW4) with different identifiers. A reader learning about `Heap<T>` had no reason to connect it to `@resource` — the rules looked parallel but separate. They were the same rules.

Pulling the rule set up into one spec and citing it from both contexts makes the shared idea visible. `@resource` and `Heap<T>` stop being two concepts and become two applications of one concept.

### Why L7 is a rule and not a lint

The narrower version of this — "no statement that can *panic* in the window" —
states the invariant more exactly, and in practice permits almost nothing more.
Conservatively, a call can panic, an index can panic, arithmetic can panic; what
is left over is `let n = 5`. Trading a rule you can check by eye for one that
needs a panic analysis, to buy the right to write a constant in the gap, is a bad
trade.

Panic-only drop glue is the other alternative, and it is the one `ctrl.panic/U5`
rules out: cleanup that runs where nobody wrote it is the thing linear types
exist to avoid. L7 keeps the cleanup written down and moves it one line up.

### What linearity does not cover

- **Uniqueness without must-consume.** `@unique` prevents implicit copying but allows silent drop. Use when you want single-owner semantics without cleanup guarantees.
- **Reference counting.** `string` is Copy+refcounted — not linear, and shouldn't be. Linearity is for values where silent drop would lose information (I/O handles, heap allocations with non-trivial cleanup, transactions).
- **Borrows of linear values.** A `mutate` borrow of a `@resource` value is fine (L3) — borrowing doesn't consume.
