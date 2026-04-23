<!-- id: mem.linear -->
<!-- status: decided -->
<!-- summary: Values that must be consumed exactly once — one rule set shared by @resource, Owned<T>, and Pool<Linear> -->
<!-- depends: memory/ownership.md -->

# Linearity

A value is *linear* when the compiler requires it to be consumed exactly once before its binding goes out of scope. Not zero times (can't silently drop it), not twice (can't double-use it). Exactly once.

The everyday version: a paper concert ticket. The gate takes it and tears it — you can't leave without handing it over (can't skip consumption), and you can't hand it over twice (can't double-spend). A file handle, a database transaction, an `Owned<T>` — they all behave the same way.

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
| `Owned<T>` type constructor | Any T, heap-allocated | [owned.md](owned.md) |
| `Pool<Linear>` | Pool holding any linear element type | [pools.md](pools.md) |

Rules L1–L6 apply identically in all three cases. The individual specs cite them instead of restating.

## Linearity + `ensure` + `try`

The common pattern: acquire a linear value, commit to consumption, then use `try` freely.

<!-- test: parse -->
```rask
func process(path: string) -> Data or Error {
    const file = try File.open(path)
    ensure file.close()                    // L4: consumption committed

    const header = try file.read_header()  // try is safe after ensure
    const body = try file.read_body()
    return body
}
```

Without `ensure`, the first `try` after acquisition is a compile error — the file might leak on error propagation. With `ensure`, the commitment is in place, and errors can propagate knowing cleanup still runs.

This is the trio the language design leans on: linearity gives the guarantee, `ensure` gives the deferral, `try` gives the propagation. Each alone is limited; together they cover most I/O code in three lines.

## Linearity + explicit consumption (transaction pattern)

If the value is consumed explicitly before scope exit, any registered `ensure` is cancelled (L6). This is how the transaction pattern works:

<!-- test: parse -->
```rask
func transfer(db: Database) -> void or Error {
    const tx = try db.begin()
    ensure tx.rollback()     // Default: rollback on any exit

    try tx.execute("UPDATE ...")
    try tx.execute("INSERT ...")

    tx.commit()              // Consumes tx, cancels ensure (L6)
    return
}
```

Ensure the unhappy path, explicitly consume the happy path.

## Linearity in containers

`Vec` and `Map` cannot hold linear values — their drop would need to consume each element, and drop can't return errors. `Pool` can hold linear values because removal is already explicit; the pool panics at runtime if it goes out of scope with linear elements still inside.

| Container | Linear allowed? | Why |
|-----------|-----------------|-----|
| `Vec<T>` | No | Drop would need to consume each element |
| `Map<K, V>` | No | Same as Vec |
| `Pool<T>` | Yes | Explicit removal required; runtime panic if dropped non-empty |
| `T?` | Yes | Must narrow (`? as v`) and consume the present case |

See `mem.pools/PL9` and `mem.resources/R5` for the Pool<Linear> cleanup semantics.

## Error messages

Base error identifiers live here; per-context specs (resource-types, owned) show worked examples.

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

## See Also

- [Resource Types](resource-types.md) — `@resource` struct annotation (`mem.resources`)
- [Owned Pointers](owned.md) — Linear heap box (`mem.owned`)
- [Ensure](../control/ensure.md) — Deferred consumption (`ctrl.ensure`)
- [Pools](pools.md) — `Pool<Linear>` cleanup rules (`mem.pools`)
- [Ownership](ownership.md) — Single-owner model that linearity refines (`mem.ownership`)
- [Value Semantics](value-semantics.md) — Copy/move rules that linear values opt out of (`mem.value`)

---

## Appendix (non-normative)

### Why one spec for linearity?

Before this spec, the same rule set was restated in `resource-types.md` (R1–R4) and `owned.md` (OW1–OW4) with different identifiers. A reader learning about `Owned<T>` had no reason to connect it to `@resource` — the rules looked parallel but separate. They were the same rules.

Pulling the rule set up into one spec and citing it from both contexts makes the shared idea visible. `@resource` and `Owned<T>` stop being two concepts and become two applications of one concept.

### What linearity does not cover

- **Uniqueness without must-consume.** `@unique` prevents implicit copying but allows silent drop. Use when you want single-owner semantics without cleanup guarantees.
- **Reference counting.** `string` is Copy+refcounted — not linear, and shouldn't be. Linearity is for values where silent drop would lose information (I/O handles, heap allocations with non-trivial cleanup, transactions).
- **Borrows of linear values.** A `mutate` borrow of a `@resource` value is fine (L3) — borrowing doesn't consume.
