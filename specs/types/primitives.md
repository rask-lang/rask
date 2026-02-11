<!-- id: type.primitives -->
<!-- status: decided -->
<!-- summary: Fixed-size primitives, IEEE 754 floats, explicit conversions -->

# Primitives and Numeric Types

Fixed-size primitives, IEEE 754 floats, explicit conversions. Lossy casts need explicit methods (consistent with overflow philosophy).

## Primitive Types

| Rule | Type | Size | Range / Notes |
|------|------|------|---------------|
| **P1: Fixed-size integers** | `i8`/`u8` | 1 byte | -128..127 / 0..255 |
| | `i16`/`u16` | 2 bytes | -32768..32767 / 0..65535 |
| | `i32`/`u32` | 4 bytes | ±2³¹ / 0..2³² |
| | `i64`/`u64` | 8 bytes | ±2⁶³ / 0..2⁶⁴ |
| | `i128`/`u128` | 16 bytes | ±2¹²⁷ / 0..2¹²⁸ |
| **P2: Platform-sized** | `isize`/`usize` | pointer | Indices, sizes |
| **P3: IEEE 754 floats** | `f32` | 4 bytes | Single precision |
| | `f64` | 8 bytes | Double precision |
| **P4: Boolean** | `bool` | 1 byte | `true`/`false`, no implicit int↔bool |
| **P5: Unicode scalar** | `char` | 4 bytes | 0x0000–0xD7FF, 0xE000–0x10FFFF |
| **P6: Unit** | `()` | 0 bytes | Zero-sized |
| **P7: Copy** | All primitives | ≤16 bytes | All primitives are Copy |

## Literals

| Rule | Form | Example | Default Type |
|------|------|---------|--------------|
| **L1: Integer default** | Decimal | `42`, `1_000` | `i32` |
| **L2: Alternate bases** | Hex/Bin/Oct | `0xFF`, `0b101`, `0o77` | `i32` |
| **L3: Suffixed** | Type suffix | `42u8`, `3.14f32` | As specified |
| **L4: Float default** | Decimal with `.` | `3.14` | `f64` |
| **L5: Char literal** | Quoted | `'a'`, `'\n'`, `'\u{1F600}'` | `char` |

## Type Conversions

| Rule | Conversion | Allowed | Notes |
|------|------------|---------|-------|
| **CV1: Widening** | `i8` → `i32`, `u8` → `i16` | `as` | Always lossless |
| **CV2: Narrowing blocked** | `i32` → `i8` | ❌ via `as` | Use explicit operations below |
| **CV3: Sign reinterpret** | `i32` → `u32` (same width) | ❌ via `as` | Use explicit operations below |
| **CV4: Float↔Int** | Any float↔int | ❌ via `as` | Use explicit operations below |

```rask
const wide: i32 = narrow_val as i32   // CV1: OK, lossless
const x: i8 = big_val as i8           // CV2: ERROR, narrowing
```

**Lossy conversions — explicit operations:**

| Rule | Operation | Behavior |
|------|-----------|----------|
| **CV5: Truncate** | `truncate to T` | Wrapping/bitwise truncation |
| **CV6: Saturate** | `saturate to T` | Clamp to target range |
| **CV7: Try convert** | `try convert to T` | `Option<T>`, `None` if out of range |

**Float to int:**

| Rule | Operation | Behavior |
|------|-----------|----------|
| **CV8: Float truncate** | `float to int T` | Truncate toward zero, panic on NaN/infinity |
| **CV9: Float saturate** | `float to int T (saturating)` | Clamp to T.MIN/T.MAX, NaN → 0 |
| **CV10: Float try** | `try float to int T` | `Option<T>` |

## `char` Type

`char` is a 4-byte Unicode scalar value — guaranteed valid by construction.

| Rule | Description |
|------|-------------|
| **CH1: Valid range** | Code point in 0x0000–0xD7FF or 0xE000–0x10FFFF; surrogates excluded |
| **CH2: Literal validation** | `'a'`, `'\n'`, `'\u{1F600}'` — compile-time validated |
| **CH3: Runtime construction** | `char.from_u32(n)` returns `Option<char>` — `None` if invalid |
| **CH4: Lossless to u32** | `c as u32` always succeeds |
| **CH5: No direct cast from u32** | `n as char` is a compile error — use `char.from_u32(n)` |

```rask
const c = 'a'                              // CH2: compile-time validated
const n: u32 = c as u32                    // CH4: lossless
const maybe = char.from_u32(0x1F600)       // CH3: runtime validation
```

**Methods:**

| Category | Method | Return |
|----------|--------|--------|
| Properties | `c.len_utf8()` | `usize` (1–4) |
| | `c.is_ascii()` | `bool` |
| Unicode | `c.is_alphabetic()` | `bool` |
| | `c.is_numeric()` | `bool` |
| | `c.is_alphanumeric()` | `bool` |
| | `c.is_whitespace()` | `bool` |
| | `c.is_control()` | `bool` |
| Case | `c.to_lowercase()` | `char` (simple 1:1 mapping) |
| | `c.to_uppercase()` | `char` (simple 1:1 mapping) |
| | `c.is_lowercase()` | `bool` |
| | `c.is_uppercase()` | `bool` |
| ASCII | `c.to_ascii_lowercase()` | `char` (fast, ASCII-only) |
| | `c.to_ascii_uppercase()` | `char` (fast, ASCII-only) |
| | `c.is_ascii_alphabetic()` | `bool` |
| | `c.is_ascii_digit()` | `bool` |
| | `c.is_ascii_hexdigit()` | `bool` |
| | `c.is_ascii_punctuation()` | `bool` |

For full case mapping (e.g., 'ß' → "SS"), use string methods.

## Floating-Point Semantics

| Rule | Description |
|------|-------------|
| **F1: IEEE 754** | Full compliance. Special values: `INFINITY`, `NEG_INFINITY`, `NAN` |
| **F2: NaN equality** | `NaN == NaN` → `false` (IEEE semantics) |
| **F3: NaN propagation** | `NaN` propagates through arithmetic |
| **F4: NaN checking** | Use `.is_nan()` to check, `.total_cmp()` for sorting |

**Methods:** `.is_nan()`, `.is_finite()`, `.abs()`, `.ceil()`, `.floor()`, `.round()`, `.sqrt()`, `.total_cmp()`

## Boolean

| Rule | Description |
|------|-------------|
| **BL1: Short-circuit** | `&&`, `\|\|` short-circuit evaluation |
| **BL2: Negation** | `!` negates |
| **BL3: No implicit conversion** | No implicit int↔bool conversion |

## Endian-Explicit Types

For binary data (`type.binary`), endian-explicit aliases specify byte order. Runtime values stored in native byte order — endian suffix only affects parsing and building.

| Rule | Description |
|------|-------------|
| **E1: Endian aliases** | `u16be`, `u16le`, `i32be`, `i32le`, etc. — specify byte order |
| **E2: Runtime type** | Stored as native type (`u16be` → `u16` at runtime) |
| **E3: No single-byte variants** | `u8`/`i8` have no endian variants — byte order irrelevant |

| Type | Size | Byte Order | Runtime Type |
|------|------|------------|--------------|
| `u16be`, `i16be` | 2 bytes | Big-endian | u16, i16 |
| `u16le`, `i16le` | 2 bytes | Little-endian | u16, i16 |
| `u32be`, `i32be` | 4 bytes | Big-endian | u32, i32 |
| `u32le`, `i32le` | 4 bytes | Little-endian | u32, i32 |
| `u64be`, `i64be` | 8 bytes | Big-endian | u64, i64 |
| `u64le`, `i64le` | 8 bytes | Little-endian | u64, i64 |
| `f32be`, `f32le` | 4 bytes | Big/Little | f32 |
| `f64be`, `f64le` | 8 bytes | Big/Little | f64 |

```rask
@binary
struct NetworkHeader {
    port: u16be      // Parsed/built as big-endian, stored as native u16
    addr: u32be
}

const header = try NetworkHeader.parse(bytes)
let port: u16 = header.port   // Native u16
```

## Numeric Traits

| Rule | Description |
|------|-------------|
| **NT1: Common constants** | All numeric types provide `ZERO`, `ONE`, `MIN`, `MAX` |
| **NT2: Integer trait** | `trait Integer: Numeric { const MIN, MAX, BITS; }` |
| **NT3: Float trait** | `trait Float: Numeric { const INFINITY, NAN, EPSILON; func is_nan(); }` |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Integer literal out of range | L1/L3 | Compile error |
| Unsuffixed literal ambiguous | L1/L4 | Defaults to `i32` or `f64` |
| `n as char` | CH5 | Compile error — use `char.from_u32(n)` |
| Surrogate code point via `char.from_u32` | CH1/CH3 | Returns `None` |
| `char.from_u32_unchecked` with invalid | CH1 | Unsafe — undefined behavior |
| NaN in comparison | F2 | `NaN == NaN` is `false`, `NaN < x` is `false` |
| Float-to-int with NaN | CV8 | Panics (use CV9 or CV10 for safe alternatives) |
| Narrowing via `as` | CV2 | Compile error |
| `true as i32` or `1 as bool` | BL3 | Compile error |

## Error Messages

**Narrowing cast via `as` [CV2]:**
```
ERROR [type.primitives/CV2]: cannot narrow i32 to i8 with `as`
   |
5  |  const x: i8 = big_val as i8
   |                 ^^^^^^^^^^^^^ narrowing conversion not allowed

WHY: `as` only permits lossless widening. Narrowing may lose data.

FIX: Use an explicit conversion:

  const x: i8 = big_val truncate to i8    // wraps
  const x: i8 = big_val saturate to i8    // clamps
  const x = try big_val convert to i8     // Option<i8>
```

**Direct u32-to-char cast [CH5]:**
```
ERROR [type.primitives/CH5]: cannot cast u32 to char with `as`
   |
3  |  const c = n as char
   |            ^^^^^^^^^ not all u32 values are valid Unicode scalars

WHY: char must be a valid Unicode scalar value. Use runtime validation.

FIX: const c = char.from_u32(n)   // returns Option<char>
```

**Implicit int↔bool [BL3]:**
```
ERROR [type.primitives/BL3]: no implicit conversion between bool and integer
   |
4  |  const flag: bool = 1
   |                      ^ expected bool, found i32

FIX: const flag: bool = n != 0
```

---

## Appendix (non-normative)

### Rationale

**P5 (char as dedicated type):** A dedicated `char` type guarantees validity at the type level. Without it, every function taking a "character" would need runtime validation. The compiler knows the value is always a valid Unicode scalar, enabling better optimization and clearer APIs (`c.is_alphabetic()` makes sense on a char, not on an arbitrary `u32`).

**CV1–CV4 (as = lossless only):** `as` being lossless-only means you can read `x as i64` and know nothing was lost. Lossy conversions use named operations (`truncate`, `saturate`, `try convert`) that document intent. Consistent with the overflow philosophy in `type.integer-overflow`.

**CH3 (runtime construction returns Option):** `char.from_u32(n)` returning `Option<char>` forces handling of invalid code points. The unsafe `char.from_u32_unchecked(n)` exists for performance-critical paths where validity is known.

**E1–E3 (endian types):** Endian-explicit types make byte order visible in struct definitions without runtime overhead. The type system handles conversion at parse/build boundaries, so application code works with native types.

### Patterns & Guidance

**Case conversion:** `to_lowercase()`/`to_uppercase()` use simple (1:1) Unicode mappings. For full case mapping (e.g., 'ß' → "SS"), use string methods. ASCII shortcuts (`to_ascii_lowercase()`) are faster when you know input is ASCII.

### See Also

- [Integer Overflow](integer-overflow.md) — Overflow behavior (`type.integer-overflow`)
- [Binary Structs](binary.md) — Endian-explicit types in binary parsing (`type.binary`)
- [SIMD Types](simd.md) — `Vec[T, N]` and shorthand `f32x4` etc. (`type.simd`)
- C interop: primitives have C-compatible layout (`struct.c-interop`)
