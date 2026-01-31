# Primitives and Numeric Types

## Decision

Fixed-size primitives, IEEE 754 floats, explicit conversions. Lossy casts require explicit methods (consistent with integer-overflow philosophy).

## Specification

### Primitive Types

**Integers:**

| Type | Size | Range |
|------|------|-------|
| `i8`/`u8` | 1 byte | -128..127 / 0..255 |
| `i16`/`u16` | 2 bytes | -32768..32767 / 0..65535 |
| `i32`/`u32` | 4 bytes | ±2³¹ / 0..2³² |
| `i64`/`u64` | 8 bytes | ±2⁶³ / 0..2⁶⁴ |
| `i128`/`u128` | 16 bytes | ±2¹²⁷ / 0..2¹²⁸ |
| `isize`/`usize` | pointer | Platform-sized (indices, sizes) |

**Floats:** `f32` (4 bytes), `f64` (8 bytes) — IEEE 754.

**Other:** `bool` (1 byte), `char` (4 bytes, Unicode scalar), `()` (0 bytes, unit).

### Literals

| Form | Example | Default Type |
|------|---------|--------------|
| Decimal | `42`, `1_000` | `i32` |
| Hex/Bin/Oct | `0xFF`, `0b101`, `0o77` | `i32` |
| Suffixed | `42u8`, `3.14f32` | As specified |
| Float | `3.14` | `f64` |
| Char | `'a'`, `'\n'`, `'\u{1F600}'` | `char` |

### Type Conversions

**`as` — lossless only:**

| Conversion | Allowed |
|------------|---------|
| Widening integer (`i8` → `i32`) | ✅ |
| Unsigned to wider signed (`u8` → `i16`) | ✅ |
| Narrowing | ❌ |
| Sign reinterpret (same width) | ❌ |
| Float ↔ Int | ❌ |

**Lossy conversions — explicit operations:**

| Operation | Behavior |
|-----------|----------|
| truncate to T | Wrapping/bitwise truncation |
| saturate to T | Clamp to target range |
| try convert to T | `Option<T>`, `None` if out of range |

**Float to int:**

| Operation | Behavior |
|-----------|----------|
| float to int T | Truncate toward zero, panic on NaN/infinity |
| float to int T (saturating) | Clamp to T.MIN/T.MAX, NaN → 0 |
| try float to int T | `Option<T>` |

### Floating-Point Semantics

IEEE 754 compliant. Special values: `INFINITY`, `NEG_INFINITY`, `NAN`.

**NaN behavior:**
- `NaN == NaN` → `false` (IEEE semantics)
- `NaN` propagates through arithmetic
- Use `.is_nan()` to check, `.total_cmp()` for sorting

**Methods:** `.is_nan()`, `.is_finite()`, `.abs()`, `.ceil()`, `.floor()`, `.round()`, `.sqrt()`, `.total_cmp()`

### Boolean

`&&`, `||` short-circuit. `!` negates. No implicit int↔bool conversion.

### Numeric Traits

```
trait Integer: Numeric { const MIN, MAX, BITS; }
trait Float: Numeric { const INFINITY, NAN, EPSILON; fn is_nan(); }
```

All numeric types provide `ZERO`, `ONE`, `MIN`, `MAX`.

## Integration

- All primitives are Copy (≤16 bytes)
- Arithmetic overflow: see [Integer Overflow](integer-overflow.md)
- C interop: primitives have C-compatible layout

---

## Remaining Issues

### Medium Priority
1. **SIMD types** — Built-in vector types (`f32x4`)?
2. **`char` necessity** — Is `char` needed or just use `u32` + validation?
