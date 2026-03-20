<!-- id: raido.values -->
<!-- status: proposed -->
<!-- summary: Raido value model — NaN-boxed dynamic types with arrays, maps, handles -->
<!-- depends: raido/README.md, memory/pools.md -->

# Values

All values are 8 bytes, NaN-boxed.

## Types

| Type | Description |
|------|-------------|
| `nil` | Absence. Falsy. |
| `bool` | `true`/`false`. Only `nil` and `false` are falsy. |
| `int` | i64. Entity IDs, counters, bitfields. |
| `number` | f64. Floating-point math. |
| `string` | Immutable, UTF-8, arena-allocated. Copied at host boundary. |
| `array` | 0-indexed growable sequence. `[a, b, c]`. |
| `map` | Unordered key-value store. `{k: v}`. Keys: string, int, number, bool. |
| `function` | Closure (bytecode + upvalues) or host function. |
| `handle` | `Handle<T>` from Rask pools. `h.field` does a pool lookup. First-class. |
| `userdata` | Opaque box containing a Rask value. No `@resource` types allowed. |

## NaN-Boxing Layout

| Bits | Meaning |
|------|---------|
| Normal f64 | `number` |
| Quiet NaN + tag 0 | `nil` |
| Quiet NaN + tag 1 | `bool` |
| Quiet NaN + tag 2 | `int` (48-bit; heap-box for larger) |
| Quiet NaN + tag 3 | Arena pointer (array, map, function, handle, userdata) |
| Quiet NaN + tag 4 | `string` (arena pointer) |

## Int/Number

- `42` is int, `42.0` is number.
- `int + int → int`, `int + number → number`.
- `/` always returns number. Use `math.floor(a / b)` for integer division.
- `42 == 42.0` is `true` (mathematical comparison).
- Int overflow wraps.

## Handles

`h.field` reads/writes the pool entry. The VM resolves the handle against the pool provided via `exec_with`. Accessing a dead handle (generation mismatch) raises a runtime error.

```raido
for h in handles("enemies") {
    h.x = h.x + h.vx * dt
    if h.health <= 0 { remove(h) }
}
```

## Strings

Arena-allocated. Copied at the Rask/Raido boundary (both directions). Interned string literals for fast equality. This is simpler than sharing Rask's refcounted representation — no shared lifetime management.

## Arrays vs Maps

Separate types, not Lua's hybrid table. `[1, 2, 3]` is an array. `{x: 1}` is a map. Clean mapping to Rask's `Vec` and `Map` at the boundary. No `#t` confusion, no `pairs()`/`ipairs()` distinction.
