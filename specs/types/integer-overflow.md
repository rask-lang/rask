# Integer Overflow Semantics

## Decision

**Default: Panic on overflow.** Consistent in debug and release. Use `Wrapping<T>` or `Saturating<T>` from `num` for non-panicking arithmetic.

## Rationale

I prioritize safety over silent bugs. Unlike Rust (panic-debug/wrap-release), no behavioral divergence between builds. Aligns with "safety is invisible—just how the language works."

**No custom operators.** Rather than `+%` and `+|` (mental tax), I use wrapper types. Regular `+` on `Wrapping<T>` wraps. Clearer and self-documenting.

**Comparison:**
| Language | Debug | Release | Opt-out |
|----------|-------|---------|---------|
| C | Wrap (UB for signed) | Wrap | None |
| Rust | Panic | Wrap | `.wrapping_add()` |
| Swift | Panic | Panic | `&+` operator |
| **Rask** | Panic | Panic | `Wrapping<T>` from `num` |

Safer than Rust: no release-only bugs from silent wrapping.

## Specification

### Module Location

`Wrapping<T>` and `Saturating<T>` live in `num`, not the global prelude. They're specialized types for niche algorithms — the safe default shouldn't share namespace with opt-outs.

```rask
import num.{Wrapping, Saturating}
```

One-off methods (`.wrapping_add()`, `.saturating_add()`, `.checked_add()`, etc.) stay on integer types directly — no import needed.

### Default Arithmetic (Checked)

Standard operators panic on overflow:

| Operator | Operation | On Overflow |
|----------|-----------|-------------|
| `+` | Addition | Panic |
| `-` | Subtraction | Panic |
| `*` | Multiplication | Panic |
| `/` | Division | Panic on divide-by-zero |
| `%` | Remainder | Panic on divide-by-zero |
| `-x` | Negation | Panic (e.g., `-i32.MIN`) |

```rask
let x: u8 = 255
const y = x + 1   // Panic: "integer overflow: 255 + 1 exceeds u8 range"
```

### Wrapping Type

For algorithms that intentionally wrap (hashing, checksums, cyclic counters):

```rask
import num.Wrapping

func hash(data: []u8) -> u32 {
    const h = Wrapping(5381u32)
    for byte in data {
        h = h * Wrapping(33) + Wrapping(byte as u32)
    }
    h.0  // Unwrap to get raw value
}
```

**Operators on `Wrapping<T>`:**
| Expression | Behavior |
|------------|----------|
| `Wrapping(255u8) + Wrapping(1)` | `Wrapping(0)` |
| `Wrapping(0u8) - Wrapping(1)` | `Wrapping(255)` |
| `w.0` | Unwrap to inner value |
| `Wrapping(x)` | Wrap a value |

`Wrapping<T>` is Copy if `T` is Copy.

### Saturating Type

For algorithms that clamp to bounds (audio, DSP, color):

```rask
import num.Saturating

func apply_gain(samples: []Saturating<i16>, gain: i16) {
    for s in samples {
        s = s * Saturating(gain)
    }
}
```

**Operators on `Saturating<T>`:**
| Expression | Behavior |
|------------|----------|
| `Saturating(255u8) + Saturating(1)` | `Saturating(255)` |
| `Saturating(0u8) - Saturating(1)` | `Saturating(0)` |
| `s.0` | Unwrap to inner value |

### Methods for One-Off Operations

When you need a single wrapping/saturating operation without changing types:

| Method | Returns | Behavior |
|--------|---------|----------|
| `a.wrapping_add(b)` | `T` | Wrapping add |
| `a.wrapping_sub(b)` | `T` | Wrapping subtract |
| `a.wrapping_mul(b)` | `T` | Wrapping multiply |
| `a.saturating_add(b)` | `T` | Saturating add |
| `a.saturating_sub(b)` | `T` | Saturating subtract |
| `a.saturating_mul(b)` | `T` | Saturating multiply |

```rask
let counter: u8 = 255
const next = counter.wrapping_add(1)  // 0
```

### Checked Methods

For explicit handling of overflow conditions:

| Method | Returns | Behavior |
|--------|---------|----------|
| `a.checked_add(b)` | `Option<T>` | `None` on overflow |
| `a.checked_sub(b)` | `Option<T>` | `None` on overflow |
| `a.checked_mul(b)` | `Option<T>` | `None` on overflow |
| `a.checked_div(b)` | `Option<T>` | `None` on zero |

**Use cases:** Parsing user input, validating calculations.

```rask
func parse_quantity(s: string) -> u32 or Error {
    const base = try parse_u32(s)
    const total = try base.checked_mul(unit_size)
        .ok_or(Error.Overflow)
    Ok(total)
}
```

### Overflowing Methods

Returns result and overflow flag:

| Method | Returns |
|--------|---------|
| `a.overflowing_add(b)` | `(T, bool)` |
| `a.overflowing_sub(b)` | `(T, bool)` |
| `a.overflowing_mul(b)` | `(T, bool)` |

```rask
let (result, overflowed) = x.overflowing_add(y)
if overflowed {
    log_warning("overflow occurred")
}
```

### Shift Operators

Shift amounts are checked:

| Case | Behavior |
|------|----------|
| `1u8 << 7` | `128` (valid) |
| `1u8 << 8` | Panic: "shift amount 8 exceeds u8 bit width" |
| `1u8 >> 9` | Panic |

For wrapping shifts, use methods:
```rask
const result = value.wrapping_shl(shift)  // Masks shift amount
```

### Division and Remainder

| Case | Behavior |
|------|----------|
| `x / 0` | Panic: "division by zero" |
| `x % 0` | Panic: "remainder by zero" |
| `i32.MIN / -1` | Panic: "signed division overflow" |
| `i32.MIN % -1` | Panic: "signed remainder overflow" |

## Compiler-Elided Overflow Checks

Compiler uses **range analysis** to prove when overflow impossible. No special syntax—checks automatically elided:

### What the Compiler Can Prove

| Pattern | Compiler Reasoning | Check? |
|---------|-------------------|--------|
| `for i in 0..100 { sum += i }` | `i < 100`, max sum = 4950 < u32.MAX | Elided |
| `let x = a & 0xFF; x + 1` | `x <= 255`, result fits u16 | Elided |
| `if x < 100 { x + 50 }` | Branch proves `x < 100` | Elided |
| `sum += user_input` | `user_input` unbounded | Check needed |

### Range Propagation

```rask
let a: u8 = read_byte()       // Range: [0, 255]
const b = a & 0x0F              // Range: [0, 15]
const c = b + 10                // Range: [10, 25] — no check needed

const d = a + 10                // Range: [10, 265] — check needed
```

### Explicit Widening

Cast to wider type to prove no overflow:

```rask
// Instead of u8 + u8 (may overflow):
const sum = (a as u16) + (b as u16)   // Can't overflow

// Narrow back if needed:
let result: u8 = try sum.try_into()    // Or .truncate() for wrapping
```

### Loop Analysis

```rask
// Compiler proves: max iterations = 1000, max item = u8.MAX
// Total max = 1000 * 255 = 255,000 < u32.MAX
let sum: u32 = 0
for item in buffer {   // buffer: []u8, len <= 1000
    sum += item as u32
}
```

### When Checks Remain

- Unbounded input: `sum += try parse_int(input)`
- Unknown loop bounds: `for i in 0..n { ... }` where `n` is runtime
- Potential overflow despite analysis: large multiplications

### Unchecked Arithmetic (Unsafe Escape Hatch)

For extreme performance cases where you've proven safety externally:

```rask
unsafe {
    const result = a.unchecked_add(b)   // No check, UB if overflows
}
```

**MUST NOT** use unless:
1. You've proven overflow impossible via external analysis
2. Benchmarks show the checked version is a bottleneck
3. The code is in a hot inner loop

## Performance Summary

| Approach | Overhead | When to use |
|----------|----------|-------------|
| Default `+` | ~5-10% in tight loops | Most code |
| Compiler-elided | 0% | Automatic when provable |
| `Wrapping<T>` | 0% | Hash, checksum algorithms |
| Explicit widening | 0% | Known-small operands |
| `unsafe unchecked_*` | 0% | Extreme hot paths (rare) |

**Guidance:** Start with default `+`. Profile. If overflow checks are a bottleneck:
1. Check if compiler already elided (look at assembly)
2. Try explicit widening or bounded loops
3. Use `Wrapping<T>` from `num` for inherently wrapping algorithms
4. Use `.wrapping_add()` for isolated operations
5. Last resort: `unsafe unchecked_*`

## Error Messages

Panic messages include context:

```rask
thread 'main' panicked at 'integer overflow: 255 + 1 exceeds u8 range'
  --> src/main.rsk:42:15
   |
42 |     let y = x + 1
   |               ^

thread 'main' panicked at 'shift amount 9 exceeds u8 bit width (8)'
  --> src/main.rsk:15:12
   |
15 |     value << shift
   |           ^^
```

## Edge Cases

| Expression | Result |
|------------|--------|
| `u8.MAX + 1` | Panic |
| `Wrapping(u8.MAX) + Wrapping(1)` | `Wrapping(0)` |
| `Saturating(u8.MAX) + Saturating(1)` | `Saturating(255)` |
| `i8.MIN - 1` | Panic |
| `0u8 - 1` | Panic |
| `-i32.MIN` | Panic |
| `1u32 << 32` | Panic |

## Comptime Arithmetic

Compile-time arithmetic is always checked. Overflow is a compile error:

```rask
import num.Wrapping

const X: u8 = 200 + 100   // Compile error: overflow in constant
const Y: u8 = Wrapping(200u8) + Wrapping(100)  // OK via wrapping type
const Z: u8 = (200u8).wrapping_add(100)        // OK via method
```

## Interaction with Generics

Generic code uses the same operators:

```rask
import num.Wrapping

func increment<T: Integer>(x: T) -> T {
    x + T.ONE   // Checked by default
}

func wrapping_increment<T: Integer>(x: Wrapping<T>) -> Wrapping<T> {
    x + Wrapping(T.ONE)  // Wraps via type
}
```

## Summary

| Need | Use |
|------|-----|
| Safe arithmetic (default) | `+`, `-`, `*` |
| Intentional wrapping (algorithm) | `Wrapping<T>` from `num` |
| Intentional wrapping (one-off) | `.wrapping_add()` method |
| Clamping to bounds (algorithm) | `Saturating<T>` from `num` |
| Clamping to bounds (one-off) | `.saturating_add()` method |
| Handle overflow explicitly | `.checked_add()` |
| Check if overflow occurred | `.overflowing_add()` |
