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

### The `char` Type

`char` is a 4-byte value representing a **Unicode scalar value**—any code point in the range 0x0000–0xD7FF or 0xE000–0x10FFFF. Surrogate code points (0xD800–0xDFFF) are explicitly excluded.

**Why `char` exists (not just `u32` + validation):**

| Concern | `char` | `u32` + validation |
|---------|--------|-------------------|
| Type safety | Guaranteed valid scalar | Can hold invalid values |
| API clarity | `c.is_alphabetic()` makes sense | Methods on arbitrary integers? |
| Intent | `func process(c: char)` documents expectation | Ambiguous |
| Optimization | Compiler knows value is valid | Must re-validate on every use |

**Construction:**

| Operation | Return | Notes |
|-----------|--------|-------|
| `'a'`, `'\n'`, `'\u{1F600}'` | `char` | Compile-time validated literal |
| `char.from_u32(n)` | `Option<char>` | Runtime validation, `None` if invalid |
| `char.from_u32_unchecked(n)` | `char` | Unsafe, no validation |

**Conversion:**

| Operation | Notes |
|-----------|-------|
| `c as u32` | Always succeeds (lossless) |
| `n as char` | Compile error—use `char.from_u32(n)` |

**Properties:**

| Method | Return | Notes |
|--------|--------|-------|
| `c.len_utf8()` | `usize` | Bytes needed to encode (1–4) |
| `c.is_ascii()` | `bool` | True if 0x00–0x7F |

**Unicode Categories (common subset):**

| Method | Unicode Category |
|--------|-----------------|
| `c.is_alphabetic()` | Letter (L) |
| `c.is_numeric()` | Number (N) |
| `c.is_alphanumeric()` | Letter or Number |
| `c.is_whitespace()` | Whitespace (includes tabs, newlines, space) |
| `c.is_control()` | Control (Cc) |

**Case Conversion:**

| Method | Return | Notes |
|--------|--------|-------|
| `c.to_lowercase()` | `char` | Simple lowercase mapping |
| `c.to_uppercase()` | `char` | Simple uppercase mapping |
| `c.is_lowercase()` | `bool` | |
| `c.is_uppercase()` | `bool` | |

**Note:** `to_lowercase()`/`to_uppercase()` use simple (1:1) Unicode case mappings. For full case mappings (e.g., 'ß' → "SS"), use string methods.

**ASCII Shortcuts:**

| Method | Notes |
|--------|-------|
| `c.to_ascii_lowercase()` | Fast, ASCII-only |
| `c.to_ascii_uppercase()` | Fast, ASCII-only |
| `c.is_ascii_alphabetic()` | a-z, A-Z |
| `c.is_ascii_digit()` | 0-9 |
| `c.is_ascii_hexdigit()` | 0-9, a-f, A-F |
| `c.is_ascii_punctuation()` | ASCII punctuation |

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

### Endian-Explicit Types

For binary data parsing/building (see [Binary Structs](binary.md)), endian-explicit type aliases specify byte order:

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

**Usage context:** These types are primarily used in `@binary` struct field declarations. At runtime, values are stored in native byte order—the endian suffix only affects parsing and building.

```rask
@binary
struct NetworkHeader {
    port: u16be      // Parsed/built as big-endian, stored as native u16
    addr: u32be
}

const header = try NetworkHeader.parse(bytes)
let port: u16 = header.port   // Native u16
```

**Note:** Single-byte types (`u8`, `i8`) have no endian variants—byte order is irrelevant for single bytes.

### Numeric Traits

```rask
trait Integer: Numeric { const MIN, MAX, BITS; }
trait Float: Numeric { const INFINITY, NAN, EPSILON; func is_nan(); }
```

All numeric types provide `ZERO`, `ONE`, `MIN`, `MAX`.

## Integration

- All primitives are Copy (≤16 bytes)
- Arithmetic overflow: see [Integer Overflow](integer-overflow.md)
- C interop: primitives have C-compatible layout
- SIMD vectors: see [SIMD Types](simd.md) for `Vec[T, N]` and shorthand `f32x4` etc.
