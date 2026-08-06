<!-- id: type.operators -->
<!-- status: decided -->
<!-- summary: Operator precedence, Equal/Comparable traits, operator trait list -->
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
| 15 | `()` `[]` `.` `?` `?.` `!` (postfix) | Grouping, indexing, field, absence, force | Left |
| 14 | `!` `~` `-` (unary) | NOT, bitwise NOT, negate | Right |
| 13 | `*` `/` `%` | Mul, div, remainder | Left |
| 12 | `+` `-` | Add, subtract | Left |
| 11 | `<<` `>>` | Bit shifts | Left |
| 10 | `&` | Bitwise AND | Left |
| 9 | `^` | Bitwise XOR | Left |
| 8 | `\|` | Bitwise OR | Left |
| 7 | `orelse` | Other branch, both shapes (`type.optionals/OPT11`, `type.errors/ER14`) | Left |
| 6 | `==` `!=` `<` `>` `<=` `>=` | Comparison | None |
| 5 | `&&` | Logical AND | Left |
| 4 | `\|\|` | Logical OR | Left |
| 3 | `..` `..=` | Range | None |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | Assignment | Right |

Postfix `?`, `?.` and `!` bind with field access and calls at 15 — tighter than everything below. `x! == y` is `(x!) == y`; `x?.f + 1` is `(x?.f) + 1`. Note the two `!`s: postfix force at 15, prefix boolean NOT at 14.

`orelse` binds tighter than comparison, looser than the bitwise operators, so `port orelse 8080 == want` is `(port orelse 8080) == want` — the reading you want, without parens. It is **left**-associative, which is the correct grouping for a chain: the right side sets the result type, so `a orelse b orelse fallback` stays wrapped through `b` and collapses at `fallback` (`type.errors/ER14a`). The compiler doesn't implement the still-wrapped case yet — [#578](https://github.com/rask-lang/rask/issues/578).

`try` is a prefix keyword and sits outside the numeric table — its placement follows two rules. **It binds tighter than `orelse`** (`type.errors/ER16b`), so `try f() orelse v` is `(try f()) orelse v` with no parens — `try` peels the error, `orelse` handles the absence. That order is the common composite (Zig's stdlib has ~160 of it against ~4 reversed, and Zig's reversed precedence forces parens on it — their issue #5436).

The `orelse` right side may be a value or any divergence — `return`, `break`, `continue`, `panic(…)` — written where it happens (`type.errors/ER14`). Inside a comma list a diverging right side needs parens (`type.errors/ER45a`).

`try` attaches to the fallible step of the postfix chain rather than to the whole of it (`type.errors/ER16a`): `try store.get(id)` is `try (store.get(id))`, while `try read_file(p).len()` is `(try read_file(p)).len()`. A wrapped value has no payload methods, so normally only one placement type-checks; when two do it is an error asking for parens.

## Indexing

`c[i]` is type-checked against what the container accepts. The index type is not inferred and discarded — a wrong index type is a compile error (`E0819`).

| Rule | Description |
|------|-------------|
| **IX1: Sequence index** | Vec, arrays, slices, and strings take **any integer type** as index — no `as usize` ceremony. The value is range-checked at access (a negative or too-large index panics, `std.collections/V1`); there is no wraparound and no negative-from-end indexing |
| **IX2: Map index** | `Map<K, V>` is indexed by `K`. An unsuffixed integer literal adapts to an integer key type |
| **IX3: Pool index** | `Pool<T>` is indexed by `Handle<T>` (`mem.pools/PL4`). A handle whose element type differs from the pool's is rejected when statically known; same-type handles from a different pool are caught at runtime by the pool id |
| **IX4: Range slices sequences** | `c[a..b]` produces a slice and is valid only on Vec, arrays, slices, and strings — not Map or Pool |

## Bitwise Operators

| Rule | Description |
|------|-------------|
| **BW1: Integer only** | `&`, `\|`, `^`, `~`, `<<`, `>>` apply to integer types only |
| **BW2: Shift bounds** | Shift exceeding bit width panics |
| **BW3: Shift semantics** | `>>` is arithmetic on signed types, logical on unsigned |

## Compound Assignment

| Rule | Description |
|------|-------------|
| **CA1: Evaluates to void** | `+=`, `-=`, `*=`, `/=`, `%=`, `&=`, `\|=`, `^=`, `<<=`, `>>=` evaluate to `void` |

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

## Comparable Trait

| Rule | Description |
|------|-------------|
| **ORD1: Comparable trait** | `<`, `>`, `<=`, `>=` derived from `compare()` returning `Ordering` |
| **ORD2: Derivable** | Structs and enums auto-derive lexicographic ordering (first field, then second, etc.). Override with explicit `extend Type with Comparable` |
| **ORD3: Float exclusion** | `f32`/`f64` don't implement `Comparable` (NaN breaks totality); use `.total_cmp()` |

<!-- test: skip -->
```rask
trait Comparable: Equal {
    func compare(self, other: Self) -> Ordering
}

enum Ordering { Less, Equal, Greater }
```

**Programmer must ensure:** total (exactly one of `<`, `==`, `>` true), transitive, antisymmetric.

## Type Support Summary

| Type | `Equal` | `Comparable` | Notes |
|------|---------|--------------|-------|
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
| `NaN < 1.0` | ORD3 | Compile error (floats don't implement `Comparable`) |
| Shift exceeding bit width | BW2 | Panic |
| Comparison chaining | P2 | Compile error |
| Struct with float field | ORD2 | Cannot auto-derive `Comparable`; implement manually with `.total_cmp()` |

---

## Appendix (non-normative)

### Rationale

**EQ3 (float semantics):** IEEE 754 compliance means `NaN == NaN` is false, breaking reflexivity. Rather than silently deviating from IEEE or forbidding equality on floats, we provide `.total_eq()` and `.total_cmp()` as explicit opt-ins for total ordering.

**ORD3 (float exclusion):** Excluding floats from `Comparable` prevents subtle sorting bugs. If you need to sort floats, `.total_cmp()` makes the choice explicit.

**P2 (no chaining):** Chained comparisons (`a < b < c`) are ambiguous in most languages. Requiring `&&` is explicit and matches user expectation.

### See Also

- `type.overflow` — Integer overflow behavior
- `type.primitives` — Primitive types
- `type.traits` — Trait system
