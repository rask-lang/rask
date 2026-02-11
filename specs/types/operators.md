<!-- id: type.operators -->
<!-- status: decided -->
<!-- summary: Operator precedence, comparison/equality traits, operator trait list -->
<!-- depends: types/primitives.md, types/traits.md -->

# Operators

Operators follow standard precedence. Equality and ordering are trait-based. Comparison chaining disallowed.

## Precedence

| Rule | Description |
|------|-------------|
| **P1: Left-to-right** | All operators associate left-to-right unless noted |
| **P2: No chaining comparisons** | `a < b < c` is disallowed; use `a < b && b < c` |

| Prec | Operators | Description | Assoc |
|------|-----------|-------------|-------|
| 14 | `()` `[]` `.` | Grouping, indexing, field | Left |
| 13 | `!` `~` `-` (unary) | NOT, bitwise NOT, negate | Right |
| 12 | `*` `/` `%` | Mul, div, remainder | Left |
| 11 | `+` `-` | Add, subtract | Left |
| 10 | `<<` `>>` | Bit shifts | Left |
| 9 | `&` | Bitwise AND | Left |
| 8 | `^` | Bitwise XOR | Left |
| 7 | `\|` | Bitwise OR | Left |
| 6 | `==` `!=` `<` `>` `<=` `>=` | Comparison | None |
| 5 | `&&` | Logical AND | Left |
| 4 | `\|\|` | Logical OR | Left |
| 3 | `..` `..=` | Range | None |
| 2 | `try` (prefix) `??` `!` (postfix) | Propagation, optional ops | Left |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | Assignment | Right |

## Bitwise Operators

| Rule | Description |
|------|-------------|
| **BW1: Integer only** | `&`, `\|`, `^`, `~`, `<<`, `>>` apply to integer types only |
| **BW2: Shift bounds** | Shift exceeding bit width panics |
| **BW3: Shift semantics** | `>>` is arithmetic on signed types, logical on unsigned |

## Compound Assignment

| Rule | Description |
|------|-------------|
| **CA1: Evaluates to unit** | `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `\|=`, `^=`, `<<=`, `>>=` evaluate to `()` |

## Equality Trait

| Rule | Description |
|------|-------------|
| **EQ1: Equal trait** | `==` calls `eq()`; `!=` is `!eq()` |
| **EQ2: Derivable** | Structs and enums can derive if all fields implement `Equal` |
| **EQ3: Float semantics** | `f32`/`f64` use IEEE 754: `NaN == NaN` is `false`; use `.total_eq()` for reflexive equality |

<!-- test: skip -->
```rask
trait Equal {
    func eq(self, other: Self) -> bool
}
```

**Programmer must ensure:** reflexive (`a == a`), symmetric (`a == b` implies `b == a`), transitive.

## Ordering Trait

| Rule | Description |
|------|-------------|
| **ORD1: Ordered trait** | `<`, `>`, `<=`, `>=` derived from `cmp()` returning `Ordering` |
| **ORD2: Derivable** | Structs derive lexicographic ordering (first field, then second, etc.) |
| **ORD3: Float exclusion** | `f32`/`f64` don't implement `Ordered` (NaN breaks totality); use `.total_cmp()` |

<!-- test: skip -->
```rask
trait Ordered: Equal {
    func cmp(self, other: Self) -> Ordering
}

enum Ordering { Less, Equal, Greater }
```

**Programmer must ensure:** total (exactly one of `<`, `==`, `>` true), transitive, antisymmetric.

## Type Support Summary

| Type | `Equal` | `Ordered` | Notes |
|------|---------|-----------|-------|
| Integers | Yes | Yes | Natural ordering |
| `bool` | Yes | Yes | `false < true` |
| `char` | Yes | Yes | Unicode scalar order |
| `f32`, `f64` | Yes* | No | *NaN breaks reflexivity |
| Structs | Derive | Derive | All fields must implement |
| Enums | Derive | Derive | Variant order, then payload |

## Arithmetic Traits

Operator traits: `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `BitAnd`, `BitOr`, `BitXor`, `BitNot`, `Shl`, `Shr`.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| `NaN == NaN` | EQ3 | `false` (IEEE 754) |
| `NaN < 1.0` | ORD3 | Compile error (floats don't implement `Ordered`) |
| Shift exceeding bit width | BW2 | Panic |
| Comparison chaining | P2 | Compile error |
| Struct with float field | ORD2 | Cannot auto-derive `Ordered`; implement manually with `.total_cmp()` |

---

## Appendix (non-normative)

### Rationale

**EQ3 (float semantics):** IEEE 754 compliance means `NaN == NaN` is false, breaking reflexivity. Rather than silently deviating from IEEE or forbidding equality on floats, we provide `.total_eq()` and `.total_cmp()` as explicit opt-ins for total ordering.

**ORD3 (float exclusion):** Excluding floats from `Ordered` prevents subtle sorting bugs. If you need to sort floats, `.total_cmp()` makes the choice explicit.

**P2 (no chaining):** Chained comparisons (`a < b < c`) are ambiguous in most languages. Requiring `&&` is explicit and matches user expectation.

### See Also

- `type.overflow` — Integer overflow behavior
- `type.primitives` — Primitive types
- `type.traits` — Trait system
