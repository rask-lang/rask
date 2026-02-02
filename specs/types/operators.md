# Operators

## Operator Precedence

Higher precedence binds tighter. Left-to-right associativity unless noted.

| Prec | Operators | Description | Assoc |
|------|-----------|-------------|-------|
| 14 | `()` `[]` `.` | Grouping, indexing, field | Left |
| 13 | `!` `~` `-` (unary) | NOT, bitwise NOT, negate | Right |
| 12 | `*` `/` `%` | Mul, div, remainder | Left |
| 11 | `+` `-` | Add, subtract | Left |
| 10 | `<<` `>>` | Bit shifts | Left |
| 9 | `&` | Bitwise AND | Left |
| 8 | `^` | Bitwise XOR | Left |
| 7 | `|` | Bitwise OR | Left |
| 6 | `==` `!=` `<` `>` `<=` `>=` | Comparison | None |
| 5 | `&&` | Logical AND | Left |
| 4 | `||` | Logical OR | Left |
| 3 | `..` `..=` | Range | None |
| 2 | `?` `??` `!` (postfix) | Optional ops | Left |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `|=` `^=` `<<=` `>>=` | Assignment | Right |

## Bitwise Operators

`&` (AND), `|` (OR), `^` (XOR), `~` (NOT), `<<` (left shift), `>>` (right shift).

Integer operands only. Shift exceeding bit width panics. `>>` is arithmetic on signed, logical on unsigned.

## Comparison

`==` `!=` `<` `>` `<=` `>=` return `bool`. Chaining disallowed: use `a < b && b < c`.

## Compound Assignment

`+=` `-=` `*=` `/=` `%=` `&=` `|=` `^=` `<<=` `>>=`. Evaluates to `()`.

## Equality and Ordering Traits

### Equal

```
trait Equal {
    fn eq(self, other: Self) -> bool
}
```

`==` calls `eq()`. `!=` is `!eq()`.

**Requirements (compiler does NOT verify, programmer must ensure):**
- Reflexive: `a == a` is true
- Symmetric: `a == b` implies `b == a`
- Transitive: `a == b` and `b == c` implies `a == c`

**Derivable:** Structs and enums can derive `Equal` if all fields implement `Equal`.

**Floating-point:** `f32` and `f64` implement `Equal` with IEEE 754 semantics:
- `NaN == NaN` is `false` (breaks reflexivity)
- Use `.total_eq()` for reflexive comparison where `NaN.total_eq(NaN)` is `true`

### Ordered

```
trait Ordered: Equal {
    fn cmp(self, other: Self) -> Ordering
}

enum Ordering { Less, Equal, Greater }
```

`<` `>` `<=` `>=` are derived from `cmp()`.

**Requirements:**
- Total: exactly one of `a < b`, `a == b`, `a > b` is true
- Transitive: `a < b` and `b < c` implies `a < c`
- Antisymmetric: `a < b` implies `!(b < a)`

**Derivable:** Structs derive lexicographic ordering (first field, then second, etc.).

**Floating-point:** `f32` and `f64` do NOT implement `Ordered` (NaN breaks totality).
- Use `.total_cmp()` for total ordering where `NaN` sorts after all values

### Comparison Summary

| Type | `Equal` | `Ordered` | Notes |
|------|---------|-----------|-------|
| Integers | Yes | Yes | Natural ordering |
| `bool` | Yes | Yes | `false < true` |
| `char` | Yes | Yes | Unicode scalar order |
| `f32`, `f64` | Yes* | No | *NaN breaks reflexivity |
| Structs | Derive | Derive | All fields must impl |
| Enums | Derive | Derive | Variant order, then payload |

## Other Operator Traits

`Add` `Sub` `Mul` `Div` `Rem` `Neg` `BitAnd` `BitOr` `BitXor` `BitNot` `Shl` `Shr`.

## Integration

- Arithmetic overflow: [Integer Overflow](integer-overflow.md)
- Optionals: [Optionals](optionals.md)
- Ranges: [Ranges](../control/ranges.md)
- Primitives: [Primitives](primitives.md)
