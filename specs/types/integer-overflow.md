<!-- id: type.overflow -->
<!-- status: decided -->
<!-- summary: Panic on overflow in all builds; Wrapping<T> and Saturating<T> from num for opt-out -->
<!-- depends: types/primitives.md, types/operators.md -->

# Integer Overflow Semantics

Default: panic on overflow, consistent in debug and release. Use `Wrapping<T>` or `Saturating<T>` from `num` for non-panicking arithmetic.

## Default Arithmetic (Checked)

| Rule | Description |
|------|-------------|
| **OV1: Panic on overflow** | Standard `+`, `-`, `*`, unary `-` panic on overflow in all builds |
| **OV2: Division by zero** | `/` and `%` panic on divide-by-zero |
| **OV3: Signed division** | `i32.MIN / -1` and `i32.MIN % -1` panic |
| **OV4: Consistent builds** | No behavioral divergence between debug and release |

| Operator | On Overflow |
|----------|-------------|
| `+` | Panic |
| `-` | Panic |
| `*` | Panic |
| `/` | Panic on divide-by-zero |
| `%` | Panic on divide-by-zero |
| `-x` | Panic (e.g., `-i32.MIN`) |

<!-- test: skip -->
```rask
let x: u8 = 255
const y = x + 1   // Panic: "integer overflow: 255 + 1 exceeds u8 range"
```

## Wrapping and Saturating Types

| Rule | Description |
|------|-------------|
| **W1: Module location** | `Wrapping<T>` and `Saturating<T>` live in `num`, not the global prelude |
| **W2: Wrapping semantics** | Operators on `Wrapping<T>` wrap on overflow |
| **W3: Saturating semantics** | Operators on `Saturating<T>` clamp to bounds |
| **W4: Copy preservation** | `Wrapping<T>` and `Saturating<T>` are Copy if T is Copy |
| **W5: Unwrap** | `.0` unwraps to inner value |

<!-- test: skip -->
```rask
import num.{Wrapping, Saturating}

// Wrapping: for hashing, checksums, cyclic counters
const h = Wrapping(5381u32)
const result = h * Wrapping(33) + Wrapping(65)  // wraps on overflow

// Saturating: for audio, DSP, color
const s = Saturating(255u8) + Saturating(1)     // Saturating(255)
```

## One-Off Methods

| Rule | Description |
|------|-------------|
| **M1: No import needed** | Methods on integer types directly, no import required |

| Method | Returns | Behavior |
|--------|---------|----------|
| `.wrapping_add(b)` | `T` | Wrapping add |
| `.wrapping_sub(b)` | `T` | Wrapping subtract |
| `.wrapping_mul(b)` | `T` | Wrapping multiply |
| `.saturating_add(b)` | `T` | Saturating add |
| `.saturating_sub(b)` | `T` | Saturating subtract |
| `.saturating_mul(b)` | `T` | Saturating multiply |
| `.checked_add(b)` | `Option<T>` | `None` on overflow |
| `.checked_sub(b)` | `Option<T>` | `None` on overflow |
| `.checked_mul(b)` | `Option<T>` | `None` on overflow |
| `.checked_div(b)` | `Option<T>` | `None` on zero |
| `.overflowing_add(b)` | `(T, bool)` | Result + overflow flag |
| `.overflowing_sub(b)` | `(T, bool)` | Result + overflow flag |
| `.overflowing_mul(b)` | `(T, bool)` | Result + overflow flag |

## Shift Operators

| Rule | Description |
|------|-------------|
| **SH1: Checked shifts** | Shift amount exceeding bit width panics |
| **SH2: Wrapping shifts** | `.wrapping_shl()` / `.wrapping_shr()` mask the shift amount |

| Case | Behavior |
|------|----------|
| `1u8 << 7` | `128` (valid) |
| `1u8 << 8` | Panic |
| `value.wrapping_shl(shift)` | Masks shift amount |

## Compiler-Elided Overflow Checks

| Rule | Description |
|------|-------------|
| **EL1: Range analysis** | Compiler uses range analysis to prove overflow impossible and elide checks |
| **EL2: No special syntax** | Elision is automatic — no programmer action needed |
| **EL3: Explicit widening** | Casting to wider type proves no overflow: `(a as u16) + (b as u16)` |

| Pattern | Compiler Reasoning | Check? |
|---------|-------------------|--------|
| `for i in 0..100 { sum += i }` | max sum = 4950 < u32.MAX | Elided |
| `let x = a & 0xFF; x + 1` | x <= 255, result fits u16 | Elided |
| `if x < 100 { x + 50 }` | Branch proves x < 100 | Elided |
| `sum += user_input` | user_input unbounded | Check needed |

## Comptime Arithmetic

| Rule | Description |
|------|-------------|
| **CT1: Always checked** | Compile-time overflow is a compile error |
| **CT2: Wrapping allowed** | `Wrapping<T>` and `.wrapping_add()` work at comptime |

<!-- test: skip -->
```rask
import num.Wrapping

const X: u8 = 200 + 100                       // Compile error: overflow
const Y: u8 = Wrapping(200u8) + Wrapping(100)  // OK via wrapping type
const Z: u8 = (200u8).wrapping_add(100)        // OK via method
```

## Unchecked Arithmetic (Unsafe)

| Rule | Description |
|------|-------------|
| **UN1: Unsafe required** | `.unchecked_add()` etc. require `unsafe` block |
| **UN2: UB on overflow** | Undefined behavior if overflow actually occurs |
| **UN3: Last resort** | Only use after proving safety externally and benchmarking shows bottleneck |

<!-- test: skip -->
```rask
unsafe {
    const result = a.unchecked_add(b)
}
```

## Error Messages

```
ERROR [type.overflow/OV1]: integer overflow
   |
42 |  let y = x + 1
   |              ^ 255 + 1 exceeds u8 range [0, 255]

WHY: Default arithmetic panics on overflow in all builds.

FIX: Use Wrapping<T> for intentional wrapping, or widen the type:
  import num.Wrapping
  const y = Wrapping(x) + Wrapping(1)
  // or
  const y = (x as u16) + 1
```

```
ERROR [type.overflow/SH1]: shift amount exceeds bit width
   |
15 |  value << shift
   |        ^^ shift amount 9 exceeds u8 bit width (8)

FIX: Use .wrapping_shl() to mask the shift amount.
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| `u8.MAX + 1` | OV1 | Panic |
| `Wrapping(u8.MAX) + Wrapping(1)` | W2 | `Wrapping(0)` |
| `Saturating(u8.MAX) + Saturating(1)` | W3 | `Saturating(255)` |
| `i8.MIN - 1` | OV1 | Panic |
| `0u8 - 1` | OV1 | Panic |
| `-i32.MIN` | OV1 | Panic |
| `1u32 << 32` | SH1 | Panic |
| `200u8 + 100` at comptime | CT1 | Compile error |

---

## Appendix (non-normative)

### Rationale

**OV1/OV4 (panic in all builds):** Unlike Rust (panic-debug/wrap-release), no behavioral divergence between builds. Safer: no release-only bugs from silent wrapping. Aligns with "safety is invisible — just how the language works."

**W1 (module location):** `Wrapping<T>` and `Saturating<T>` are specialized types for niche algorithms. The safe default shouldn't share namespace with opt-outs.

**No custom operators:** Rather than `+%` and `+|` (mental tax), wrapper types are used. Regular `+` on `Wrapping<T>` wraps. Clearer and self-documenting.

| Language | Debug | Release | Opt-out |
|----------|-------|---------|---------|
| C | Wrap (UB for signed) | Wrap | None |
| Rust | Panic | Wrap | `.wrapping_add()` |
| Swift | Panic | Panic | `&+` operator |
| **Rask** | Panic | Panic | `Wrapping<T>` from `num` |

### Patterns & Guidance

**Performance approach:**

| Approach | Overhead | When to use |
|----------|----------|-------------|
| Default `+` | ~5-10% in tight loops | Most code |
| Compiler-elided | 0% | Automatic when provable |
| `Wrapping<T>` | 0% | Hash, checksum algorithms |
| Explicit widening | 0% | Known-small operands |
| `unsafe unchecked_*` | 0% | Extreme hot paths (rare) |

Start with default `+`. Profile. If overflow checks are a bottleneck: check if compiler already elided (look at assembly), try explicit widening or bounded loops, use `Wrapping<T>` from `num` for inherently wrapping algorithms, use `.wrapping_add()` for isolated operations. Last resort: `unsafe unchecked_*`.

**Generic code uses the same operators:**

<!-- test: skip -->
```rask
import num.Wrapping

func increment<T: Integer>(x: T) -> T {
    return x + T.ONE   // Checked by default
}

func wrapping_increment<T: Integer>(x: Wrapping<T>) -> Wrapping<T> {
    return x + Wrapping(T.ONE)  // Wraps via type
}
```

### See Also

- `type.operators` — Operator precedence and traits
- `type.primitives` — Primitive integer types
- `mem.unsafe` — Unsafe blocks
