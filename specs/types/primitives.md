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
| **P6: Unit** | `void` | 0 bytes | Zero-sized. Keyword. Canonical value is `{}` (empty block); functions fall through or `return` bare |
| **P7: Absent** | `none` | 0 bytes | Zero-sized. Keyword. One inhabitant, also spelled `none`. Used as the absent variant of `T?` (sugar for `T or none`) — see [optionals.md](optionals.md) |
| **P8: Copy** | All primitives | ≤16 bytes | All primitives are Copy |

## Literals

| Rule | Form | Example | Default Type |
|------|------|---------|--------------|
| **L1: Integer default** | Decimal | `42`, `1_000` | `i32` |
| **L2: Alternate bases** | Hex/Bin/Oct | `0xFF`, `0b101`, `0o77` | `i32` |
| **L3: Suffixed** | Type suffix | `42u8`, `3.14f32` | As specified |
| **L4: Float default** | Decimal with `.` | `3.14` | `f64` |
| **L5: Char literal** | Quoted | `'a'`, `'\n'`, `'\u{1F600}'` | `char` |
| **L6: Default widens** | Decimal/hex too big for `i32` | `3000000000` | `i64`, then `u64` |
| **L7: Must fit** | Any literal | `const b: u8 = 300` | Compile error |

L6 only moves the *default*. Context still wins: `const x: i64 = 5` is an `i64`.
A literal above `i64::MAX` can only be a `u64`, so that's where it lands.

L7 is the reason L6 exists. A literal that doesn't fit its type has to wrap, and
nothing wraps silently here — `const b: u8 = 300` is an error, not `44`. Say
what you mean with `.to<T>()!`, `.wrap<T>()` or `.clamp<T>()` (CV5–CV8).

## Type Conversions

One question decides everything: **can every value of the source be represented
in the target?** If yes the conversion is free — implicit, or `as` when you want
to say it out loud. If no, you have to say what happens to the values that don't
fit.

| Rule | Conversion | Allowed | Notes |
|------|------------|---------|-------|
| **CV1: Lossless** | `i8` → `i32`, `u8` → `i16`, `f32` → `f64`, `i32` → `f64` | `as` | Every source value has an exact representation in the target |
| **CV1a: Lossless is implicit** | `u32` → `i64` with no cast | implicit | An `as` there tells the reader nothing. `as` is for saying it out loud |
| **CV2: Narrowing blocked** | `i32` → `i8` | ❌ via `as` | Name a policy (CV5–CV8) |
| **CV3: Sign reinterpret** | `i32` → `u32` (same width) | ❌ via `as` | Name a policy (CV5–CV8) |
| **CV4: Float→Int** | Any float→int | ❌ via `as` | Name a policy (CV5–CV8) |

```rask
let wide: i32 = narrow_val as i32   // CV1: OK, lossless
let x: i8 = big_val as i8           // CV2: ERROR, narrowing
```

### CV1: which int→float casts are lossless

`as` promises nothing was lost, so it covers int→float only where the float holds
every source value exactly:

| Target | Sources |
|---|---|
| `f64` | `i8` `i16` `i32` `u8` `u16` `u32` |
| `f32` | `i8` `i16` `u8` `u16` |

`i64 as f32` is a compile error. Past 2^24 an `f32` can only land on multiples of
128, so a billion-scale count comes back wrong by hundreds — the same silent
precision loss the overflow rules exist to prevent, riding the one operator that
promises the opposite. Precision loss gets named like any other loss:
`total.round<f32>()`.

### CV1a: which conversions are implicit

The test is one question: **can every value of the source be represented in the
target?** If yes, the conversion is implicit — an `as` there would tell the
reader nothing, and ceremony that informs nobody is a design bug
(NORTH_STAR commitment 5). If no, it has to name a policy (CV5–CV8), because
choosing what to lose is a real decision.

| From → To | Implicit? | Why |
|---|---|---|
| `i32` → `i64` | yes | Same sign, wider |
| `u16` → `u64` | yes | Same sign, wider |
| `u32` → `i64` | yes | Every `u32` fits an `i64` |
| `u8` → `i16` | yes | Strictly wider, so the sign bit is free |
| `u8` → `i8` | **no** | 200 doesn't fit — the target loses a bit to the sign |
| `u64` → `i64` | **no** | A `u64` above `i64.MAX` doesn't fit |
| `i64` → `u64` | **no** | Negatives have nowhere to go |
| `i64` → `i16` | **no** | Narrowing |

Unsigned → signed needs the target *strictly* wider; same-signedness only needs
it no narrower; signed → unsigned never coerces.

**Positions, not arithmetic.** CV1a applies where a value fills a typed slot:
assignment, argument, return, struct field. It is **not** operator promotion —
operators are homogeneous (`type.operators`), so `a + b` on mixed integer types
stays an error, and you widen one side yourself. This is the line C's "usual
arithmetic conversions" crossed, and why `-1 < 1u` is *true* in C. Rask doesn't
have that bug because it doesn't have that feature.

**Why implicit widening is safe here and isn't in some other languages.** Go,
Rust and Swift require a cast for every numeric conversion. That's the right call
*for them*, because their lossy conversions are quiet — Rust's `300u32 as u8` is
`44`, silently. When truncation is silent you can't let any conversion be
implicit, because the reader can't tell the safe ones from the dangerous ones.
Rask doesn't have that problem: the lossy directions are named verbs, and
unnamed arithmetic panics on overflow in all builds (`type.overflow/OV1`). A
value that doesn't fit can never quietly become a wrong number, so the only
question left is whether the conversion is worth writing down — and when it
cannot fail, it isn't.

### Conversions that can lose something

Four methods on every numeric primitive. The type argument is the target; the
name is the policy.

| Rule | Form | Yields | Behavior |
|------|------|--------|----------|
| **CV5: Convert** | `x.to<T>()` | `T or ConvertError` | Exact, or it fails |
| **CV6: Wrap** | `x.wrap<T>()` | `T` | **Integers only.** Keeps the low bits |
| **CV7: Clamp** | `x.clamp<T>()` | `T` | **Integers only.** Pins to the target's range |
| **CV8: Round** | `x.round<T>()` | `T` / `T or ConvertError` | Nearest representable. Total to a float target, fallible to an integer one |

```rask
let count = rows.len().to<i32>()!           // I know it fits — panic if I'm wrong
self.data.push((value >> 8).wrap<u8>())     // serializing: the low byte
let level = raw.clamp<u8>()                 // out of range is expected
let ratio = total.round<f32>()              // accept the rounding
let tick = seconds.round<i64>()!            // nearest whole, panic on NaN
```

**`to` is the default, and it's the one most code wants.** Most conversions in
real code are a length or a count going into a smaller slot, where the author
knows it fits. `to<T>()!` says exactly that, and being wrong turns into a panic
instead of a wrong number. The other three are for when a value that doesn't fit
is expected rather than a bug — and reaching for one of them says so.

`to` yields a result (`type.errors/ER1`), so the whole error vocabulary works on
it without inventing anything: `!` panics with the reason (ER15), `try`
propagates it, `catch e =>` handles it, and `! "msg"` replaces the panic message.

```rask
let n = try text_len.to<u16>()              // propagate to the caller
let n = value.to<u8>() catch _ => 0         // fall back
let n = rows.to<i32>()! "row count exceeds i32"
```

### What "fails" means

Anything that can fail yields a result; anything total yields the value bare.
That's the only rule, and it's the same question CV1 asks — can this value be
represented in the target?

```rask
public enum ConvertError {
    OutOfRange   // doesn't fit the target type
    NotExact     // a float with a fraction, going to an integer
    NotFinite    // NaN or infinity
}
```

`OutOfRange` is spelled to match `ParseError.OutOfRange` (`stdlib/string.rk`),
which already means exactly this.

`to` from a float is therefore exact-or-fail: `3.0.to<i32>()` gives `3`, and
`3.7.to<i32>()` fails with `NotExact`. That's the honest reading of "convert if
it fits", and it replaces the hand-rolled round-trip test — `json.rk` currently
writes `if i as f64 == n && n >= -9007199254740992.0 && …` to ask the same
question.

### Float→int is `to` and `round`, and nothing else

Getting an integer out of a float answers two questions: what happens to the
fraction, and what happens if the result doesn't fit. The verb answers the first,
the return type answers the second — every float→int conversion can fail, so
every one of them yields a result and nothing is decided quietly.

| | fraction | doesn't fit, NaN, infinity |
|---|---|---|
| `x.to<i32>()` | must be none — else `NotExact` | fails |
| `x.round<i32>()` | nearest | fails |

`wrap` and `clamp` are integers-only, because on a float neither one means
anything you'd want. Bit-truncating an IEEE pattern into an integer is a
reinterpretation nobody asks for, and toward-zero isn't wrapping at all — it's a
rounding mode wearing the wrong word. `clamp` could be defined, but it would have
to pick a fraction policy silently to stay total, and it's the one member whose
name couldn't say which it picked.

`floor`, `ceil` and `trunc` are the same shape as `round` and get added when
something needs them — that's the openness methods bought over syntax, and
there's no reason to spend it up front. Toward-zero is deliberately not anyone's
default; it's a C artifact, and nearest is what people mean.

## `char` Type

`char` is a 4-byte Unicode scalar value — guaranteed valid by construction.

| Rule | Description |
|------|-------------|
| **CH1: Valid range** | Code point in 0x0000–0xD7FF or 0xE000–0x10FFFF; surrogates excluded |
| **CH2: Literal validation** | `'a'`, `'\n'`, `'\u{1F600}'` — compile-time validated |
| **CH3: Runtime construction** | `char.from_u32(n)` returns `char?` — `none` if invalid |
| **CH4: Lossless to u32** | `c as u32` always succeeds |
| **CH5: No direct cast from u32** | `n as char` is a compile error — use `char.from_u32(n)` |

```rask
let c = 'a'                              // CH2: compile-time validated
let n: u32 = c as u32                    // CH4: lossless
let maybe = char.from_u32(0x1F600)       // CH3: runtime validation
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

let header = try NetworkHeader.parse(bytes)
mut port: u16 = header.port   // Native u16
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
| Surrogate code point via `char.from_u32` | CH1/CH3 | Returns `none` |
| `char.from_u32_unchecked` with invalid | CH1 | Unsafe — undefined behavior |
| NaN in comparison | F2 | `NaN == NaN` is `false`, `NaN < x` is `false` |
| Float-to-int with NaN | CV5/CV8 | `.to<T>()` and `.round<T>()` fail with `NotFinite` |
| Float with a fraction → int | CV5 | `.to<T>()` fails with `NotExact` — use `.round<T>()` |
| `x.wrap<T>()` or `x.clamp<T>()` on a float | CV6/CV7 | Compile error — both are integers-only |
| Narrowing via `as` | CV2 | Compile error |
| `i64 as f32`, `i64 as f64` | CV1 | Compile error — inexact past 2^24 / 2^53. Use `.round<f32>()` |
| `true as i32` or `1 as bool` | BL3 | Compile error |

## Error Messages

**Narrowing cast via `as` [CV2]:**
```
ERROR [type.primitives/CV2]: cannot narrow i32 to i8 with `as`
   |
5  |  let x: i8 = big_val as i8
   |              ^^^^^^^^^^^^^ an i32 doesn't always fit in an i8

WHY: `as` is for conversions that can't lose anything. This one can, so it has
     to say what happens to a value that doesn't fit.

FIX: If it always fits, say so — being wrong becomes a panic, not a wrong number:

  let x = big_val.to<i8>()!

     If it might not fit, pick what to do about it:

  let x = big_val.wrap<i8>()          // keep the low bits
  let x = big_val.clamp<i8>()         // pin to -128..=127
  let x = big_val.to<i8>() catch _ => 0
```

The order matters. `to<T>()!` goes first because it's what most narrowing code
means, and a suggestion list that opens with `wrap` teaches people to reach for
bit-truncation when what they meant was "this fits" — silently wrong at the one
moment they were asserting couldn't happen.

**Inexact int→float via `as` [CV1]:**
```
ERROR [type.primitives/CV1]: cannot convert i64 to f32 with `as`
   |
7  |  let ratio = total as f32
   |              ^^^^^^^^^^^^ an f32 can't hold every i64 exactly

WHY: past 2^24 an f32 only lands on multiples of 128, so a billion-scale count
     comes back wrong by hundreds. `as` promises nothing was lost.

FIX: let ratio = total.round<f32>()   // nearest f32, rounding accepted
```

**Direct u32-to-char cast [CH5]:**
```
ERROR [type.primitives/CH5]: cannot cast u32 to char with `as`
   |
3  |  let c = n as char
   |            ^^^^^^^^^ not all u32 values are valid Unicode scalars

WHY: char must be a valid Unicode scalar value. Use runtime validation.

FIX: let c = char.from_u32(n)   // returns char?
```

**Implicit int↔bool [BL3]:**
```
ERROR [type.primitives/BL3]: no implicit conversion between bool and integer
   |
4  |  let flag: bool = 1
   |                      ^ expected bool, found i32

FIX: let flag: bool = n != 0
```

---

## Appendix (non-normative)

### Rationale

**P5 (char as dedicated type):** A dedicated `char` type guarantees validity at the type level. Without it, every function taking a "character" would need runtime validation. The compiler knows the value is always a valid Unicode scalar, enabling better optimization and clearer APIs (`c.is_alphabetic()` makes sense on a char, not on an arbitrary `u32`).

**CV1–CV4 (as = lossless only):** `as` being lossless-only means you can read `x as i64` and know nothing was lost. Lossy conversions name what they give up. Consistent with the overflow philosophy in `type.integer-overflow`.

**CV5–CV8 (methods, not syntax).** These were once phrase verbs — `x truncate to u8`, `x saturate to u8`, and a separate `x float to int T (saturating)` for floats, with a parenthesized modifier that existed nowhere else in the language. Methods replace all of it, for a reason that isn't token count: **the policy set is open and grammar can't be.** Float→int alone wants four rounding modes crossed with three out-of-range behaviours. Every one of those is a name in a namespace and a line in a table; as syntax, each is a parse rule. `floor<i32>()` and `ceil<i32>()` can be added the day someone needs them — and until then they cost nothing, which is why the shipped set is four and not seven.

The float sub-family disappears in the move. There was never a second set of policies for floats — converting means the same thing there, on values that happen to have a fraction.

**CV6/CV7 (`wrap` and `clamp` are integers-only).** An earlier draft had `wrap` mean bit-truncation on integers and toward-zero on floats — one name, two mechanics, defended as both being the domain-standard reading of "throw away what doesn't fit". It doesn't survive the question *what would a float actually wrap?* Bit-truncating an IEEE pattern into an integer is a reinterpretation nobody wants, and toward-zero isn't wrapping at all — it's a rounding mode wearing the wrong word.

`clamp` went the same way for a different reason. It could be defined on floats, but to stay total it would have to pick a fraction policy silently, and it's the one member whose name can't say which — `(pos / cell).clamp<i32>()` gives 3 from 2.99 under nearest and 2 under toward-zero, with nothing at the site to tell you. The case that justified keeping it was colour quantization, `(channel * 255.0).clamp<u8>()`, and no such code exists in the tree: the single float→int site is `json.rk` asking whether a float is a whole number, which is `to`. A member kept for a hypothetical is a member the reader still has to learn. It comes back as one line the day something needs it.

Both restrictions leave float→int with exactly two entries, `to` and `round`, both fallible, each naming its fraction policy. Nothing about a float conversion is decided quietly.

**CV5 (`to` yields a result).** The common case by a wide margin is a length or a count moving into a smaller slot where the author knows it fits. That deserves the shortest name, and `to` is it. Yielding `T or ConvertError` rather than panicking directly is what lets one method serve every downstream: `!` to assert, `try` to propagate, `catch` to handle. The panic is spelled — one character, at the site, in the marker the language already uses for it — instead of being something you have to know about `to`.

It also removes the need for a separate optional-shaped member. An earlier draft had `check<T>() -> T?` alongside; once `to` fails rather than panics, `x.to<u8>() catch _ => 0` covers it, and a second spelling of the same test is a member the reader has to disambiguate for nothing.

That settles a question the old design couldn't answer. `truncate to` was the only form that read like "just do it", so it collected every "I know this fits" site in the tree and quietly wrapped them. The policy names now describe cases that are genuinely expected, and the assertion has its own word.

**CH3 (runtime construction returns `T?`):** `char.from_u32(n)` returning `char?` forces handling of invalid code points. The unsafe `char.from_u32_unchecked(n)` exists for performance-critical paths where validity is known.

**E1–E3 (endian types):** Endian-explicit types make byte order visible in struct definitions without runtime overhead. The type system handles conversion at parse/build boundaries, so application code works with native types.

### Patterns & Guidance

**Case conversion:** `to_lowercase()`/`to_uppercase()` use simple (1:1) Unicode mappings. For full case mapping (e.g., 'ß' → "SS"), use string methods. ASCII shortcuts (`to_ascii_lowercase()`) are faster when you know input is ASCII.

### See Also

- [Integer Overflow](integer-overflow.md) — Overflow behavior (`type.integer-overflow`)
- [Binary Structs](binary.md) — Endian-explicit types in binary parsing (`type.binary`)
- [SIMD Types](simd.md) — `Vec[T, N]` and shorthand `f32x4` etc. (`type.simd`)
- C interop: primitives have C-compatible layout (`struct.c-interop`)
