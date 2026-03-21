<!-- id: raido.types -->
<!-- status: proposed -->
<!-- summary: Raido type system — dynamically typed values, fixed-point numbers, closures, host references -->
<!-- depends: raido/README.md -->

# Types

All values are 8 bytes. Dynamically typed. Deterministic.

## Value Types

| Type | Description |
|------|-------------|
| `nil` | Absence. Falsy. |
| `bool` | `true`/`false`. Only `nil` and `false` are falsy. |
| `int` | i64. Counters, IDs, bitfields. |
| `number` | Fixed-point 32.32. Deterministic, hardware-speed. |
| `string` | Immutable, UTF-8. |
| `array` | 0-indexed growable sequence. `[a, b, c]`. |
| `map` | Key-value store. `{k: v}`. Insertion-ordered. Keys: string, int, bool. |
| `function` | Closure or host function. |
| `host_ref` | Opaque reference to host-managed data. |

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

A closure captures variables from enclosing scopes. Multiple closures capturing the same variable share it — mutations are visible to all of them.

```raido
func make_counter() {
    let count = 0
    return || {
        count = count + 1
        return count
    }
}

const c = make_counter()
c()  // 1
c()  // 2
```

See [vm/architecture.md](../vm/architecture.md#closures-and-upvalues) for how the VM stores captured variables.

## Host References

Opaque references to data managed by the host. Scripts see them as objects with fields:

```raido
target.health -= 10
const name = target.name
```

The host decides what the fields mean — a pool lookup, an ECS component, a database row. Scripts don't know or care. See [vm/interop.md](../vm/interop.md#host-references) for how the host registers field accessors.

## Maps

Insertion-ordered. Deterministic iteration order. Keys restricted to string, int, bool — types with stable, deterministic equality.
