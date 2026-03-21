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
| `function` | Closure (bytecode index + arena offsets to upvalues) or host function name. |
| `host_ref` | Opaque reference to host-managed data. Host registers vtable for field access. |

## Number (Fixed-Point 32.32)

Stored as i64. 32 integer bits (signed), 32 fractional bits.

- **Range:** ±2.1 billion. **Precision:** ~2^-32 (~9.6 decimal digits).
- **Deterministic.** Integer math. Same result on every platform.
- **Fast.** Add/sub = single i64 op. Mul/div = 128-bit intermediate.
- **No NaN/infinity.** Division by zero = runtime error. Overflow saturates.
- Scripts write `3.14`, compiler converts to fixed-point.

`int` and `number` are separate. `int + int → int`, `int + number → number`. Division always returns `number`. `42 == 42.0` is true.

### Why 32.32 over 48.16

I chose 32.32 over 48.16. 48.16 gives more integer range (~140 trillion vs ~2.1 billion) but only ~4.8 decimal digits of fractional precision. Simulation showed that's too coarse for real use cases:

- **Physics accumulation** (10K frames at 1/60): 32.32 error 6e-7, 48.16 error 4e-2
- **Damping chains** (1000 frames × 0.999): 32.32 error 6e-5, 48.16 error 2.6
- **Financial sums** (0.01 × 10K): 32.32 error 2e-6, 48.16 error 5e-2

The 2.1B integer ceiling is real but manageable — `int` (i64) handles large values. Use `int` for entity IDs, large counters, and scores. Use `number` for positions, velocities, fractions, and math.

## Closures

A closure is a bytecode prototype index + an array of arena offsets pointing to captured upvalues. Upvalues live in the arena, not on the stack or in heap cells. Multiple closures capturing the same variable share the same arena offset — mutations are visible to all of them.

Serializable: the prototype index identifies which function, and the arena offsets are stable across serialize/deserialize (the arena is captured as a byte blob).

## Host References

Opaque references to data managed by the host. The VM doesn't know what's behind them — the host registers a vtable per ref type.

```raido
// Script just sees an object with fields
h.health -= 10
const name = h.name
```

The host decides what `h.health` means — it could be a pool lookup, an ECS component access, a database row read, a struct field. The VM dispatches through a vtable: field name → slot index, then calls the getter/setter at that slot.

```rask
// Host registers a vtable for the ref type
vm.register_ref_type("enemy", raido.RefType {
    fields: [
        raido.HostField.int("health", get_health, set_health),
        raido.HostField.number("x", get_x, set_x),
        raido.HostField.number("y", get_y, set_y),
        raido.HostField.string("name", get_name, null),  // read-only
    ],
})
```

**Why vtable over dynamic callbacks.** The current `register_ref_type` already declares fields up front — field names are known at registration time. A vtable makes this static dispatch:

1. **Field name → slot index at compile time.** When the compiler sees `target.health`, it resolves "health" to slot 0 in the "enemy" vtable. The `GET_REF_FIELD` instruction encodes the slot index, not a string. No hash lookup at runtime.
2. **Predictable cost.** Every field access is an indexed function pointer call. No string matching, no map lookup.
3. **Serializable.** Vtable structure is re-registered on restore. Slot indices are stable because field order is declared by the host.

Host refs are serializable as opaque IDs. The host assigns meaning to them on restore.

## Serialization

Every value serializes to bytes. No pointers — arena references use offsets. Closures store bytecode prototype index + arena offsets to upvalues. Host function values store names. Host refs store opaque IDs. The entire VM state round-trips through serialize/deserialize.

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to string, int, bool — types with stable, deterministic equality.
