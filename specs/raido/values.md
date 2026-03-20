<!-- id: raido.values -->
<!-- status: proposed -->
<!-- summary: Raido value model — tagged values, fixed-point numbers, deterministic, serializable -->
<!-- depends: raido/README.md -->

# Values

All values are 8 bytes. Deterministic, fully serializable.

## Types

| Type | Description |
|------|-------------|
| `nil` | Absence. Falsy. |
| `bool` | `true`/`false`. Only `nil` and `false` are falsy. |
| `int` | i64. Counters, IDs, bitfields. |
| `number` | Fixed-point 32.32. Deterministic, hardware-speed. |
| `string` | Immutable, UTF-8, arena-allocated. |
| `array` | 0-indexed growable sequence. `[a, b, c]`. |
| `map` | Key-value store. `{k: v}`. Insertion-ordered. Keys: string, int, bool. |
| `function` | Closure (bytecode index + upvalues) or host function name. |
| `host_ref` | Opaque reference to host-managed data. Host registers field accessors. |

## Number (Fixed-Point 32.32)

Stored as i64. 32 integer bits (signed), 32 fractional bits.

- **Range:** ±2.1 billion. **Precision:** ~2^-32.
- **Deterministic.** Integer math. Same result on every platform.
- **Fast.** Add/sub = single i64 op. Mul/div = 128-bit intermediate.
- **No NaN/infinity.** Division by zero = runtime error. Overflow saturates.
- Scripts write `3.14`, compiler converts to fixed-point.

`int` and `number` are separate. `int + int → int`, `int + number → number`. Division always returns `number`. `42 == 42.0` is true.

## Host References

Opaque references to data managed by the host. The VM doesn't know what's behind them — the host registers field accessors.

```raido
// Script just sees an object with fields
h.health -= 10
const name = h.name
```

The host decides what `h.health` means — it could be a pool lookup, an ECS component access, a database row read, a struct field. The VM calls the host's registered getter/setter.

```rask
// Host registers how fields resolve
vm.register_ref_type("enemy", raido.RefType {
    get: |ref, field| { /* return value for field */ },
    set: |ref, field, value| { /* write value to field */ },
})
```

Host refs are serializable as opaque IDs. The host assigns meaning to them on restore.

## Serialization

Every value serializes to bytes. No pointers — arena references use offsets. Host function values store names. Host refs store opaque IDs. The entire VM state round-trips through serialize/deserialize.

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to string, int, bool — types with stable, deterministic equality.
