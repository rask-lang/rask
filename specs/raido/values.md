<!-- id: raido.values -->
<!-- status: proposed -->
<!-- summary: Raido value model — tagged values, fixed-point numbers, deterministic, serializable -->
<!-- depends: raido/README.md, memory/pools.md -->

# Values

All values are 8 bytes. Deterministic, fully serializable, no platform-dependent representation.

## Types

| Type | Description |
|------|-------------|
| `nil` | Absence. Falsy. |
| `bool` | `true`/`false`. Only `nil` and `false` are falsy. |
| `int` | i64. Entity IDs, counters, bitfields. |
| `number` | Fixed-point (32.32). Deterministic, hardware-speed. |
| `string` | Immutable, UTF-8, arena-allocated. Copied at host boundary. |
| `array` | 0-indexed growable sequence. `[a, b, c]`. |
| `map` | Key-value store. `{k: v}`. Keys: string, int, bool. Insertion-ordered. |
| `function` | Closure (bytecode index + captured upvalues) or host function name. |
| `handle` | `Handle<T>` from Rask pools. `h.field` does a pool lookup. |
| `userdata` | Opaque serializable blob. Host registers serialize/deserialize pair. |

## Number (Fixed-Point)

32.32 fixed-point stored as i64. 32 integer bits (signed), 32 fractional bits.

- **Range:** ±2,147,483,647 (~±2.1 billion). Enough for any game coordinate/value.
- **Precision:** ~0.00000000023 (2^-32). Finer than any game needs.
- **Deterministic.** It's integer math. Same result on every platform, every time.
- **Fast.** Add/sub are single i64 ops. Mul/div use 128-bit intermediate (two instructions on 64-bit CPUs).
- **No NaN/infinity.** Division by zero is a runtime error. Overflow saturates (clamps to min/max).

```raido
const speed = 3.14          // internally: 13,493,037,867 (3.14 * 2^32)
const dt = 0.016            // internally: 68,719,476 (0.016 * 2^32)
const distance = speed * dt // fixed-point mul, deterministic
```

Scripts write normal decimal literals. The compiler converts to fixed-point at compile time. Modders never see the representation.

**Overflow saturates** instead of wrapping. If `a + b` exceeds the range, the result clamps to `number.max` / `number.min`. Saturation is safer than wrapping for game values — a position that overflows staying at the boundary is better than teleporting to the opposite side.

## Conversion at Host Boundary

When Rask passes an `f64` to Raido (e.g., `raido.Value.number(dt)`), it converts to fixed-point. When Raido returns a number to Rask, it converts back to `f64`. The conversion is deterministic (round-to-nearest).

This means the host does `f64 → fixed` on the way in and `fixed → f64` on the way out. The host side is non-deterministic (hardware floats), but that's fine — the host doesn't need lockstep. Only the VM's internal computation is deterministic.

## Int vs Number

- `42` is int (i64), `42.0` is number (fixed-point).
- `int + int → int`, `int + number → number` (int promoted to fixed-point).
- `int / int → number` (division always returns fixed-point).
- `42 == 42.0` is `true`.
- Int overflow wraps (i64 semantics). Number overflow saturates.

## Strings

Arena-allocated. Copied at the Rask/Raido boundary. Interned literals. Serialized as length + bytes.

## Arrays and Maps

Separate types. `[1, 2, 3]` is an array, `{x: 1}` is a map.

Maps are insertion-ordered — iteration order matches insertion order. Deterministic iteration is required for deterministic execution.

Map keys: string, int, bool only. No number keys (fixed-point equality is fine but it's a footgun — `1.0/3.0*3.0` might not equal `1.0`).

## Handles

Pool-name + generation + index. Serializable as-is — just integers.

## Functions

Script functions: bytecode chunk index + upvalue list. Host functions: stored as string names, re-resolved on deserialize. Closures are serializable — bytecode reference + captured values, not a native pointer.

## Userdata

Must be serializable. Host registers a serialize/deserialize pair per type. Non-serializable Rask types cannot be userdata.
