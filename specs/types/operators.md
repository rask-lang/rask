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
| 7 | `??` `catch` | Fallbacks — `??` for absence (`type.optionals/OPT11`), `catch <binder> =>` for failure (`type.errors/ER14`) | Left |
| 6 | `==` `!=` `<` `>` `<=` `>=` | Comparison | None |
| 5 | `&&` | Logical AND | Left |
| 4 | `\|\|` | Logical OR | Left |
| 3 | `..` `..=` | Range | None |
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `&=` `\|=` `^=` `<<=` `>>=` | Assignment | Right |

Postfix `?`, `?.` and `!` bind with field access and calls at 15 — tighter than everything below. `x! == y` is `(x!) == y`; `x?.f + 1` is `(x?.f) + 1`. Note the two `!`s: postfix force at 15, prefix boolean NOT at 14.

The fallbacks bind tighter than comparison, looser than the bitwise operators, so `port ?? 8080 == want` is `(port ?? 8080) == want` — the reading you want, without parens. `??` is **left**-associative, which is the correct grouping for a chain: the right side sets the result type, so `a ?? b ?? fallback` stays wrapped through `b` and collapses at `fallback` (`type.errors/ER14a`). The compiler doesn't implement the still-wrapped case yet — [#578](https://github.com/rask-lang/rask/issues/578).

`catch` needs one more ruling than `??`, because it has two sides with different behavior:

- **Left of `catch`, level 7, left-associative** — same as `??`. Mixed chains group left on their left operands: `x ?? y catch e => z` is `(x ?? y) catch e => z` — which parses fine and then rarely type-checks, since `??` produced an optional or a `T` and `catch` needs a result; the mistake surfaces as a type error naming the shapes.
- **Right of `=>`, the body is greedy** — a full expression extending as far right as it can, exactly like a match-arm body. `a catch _ => b ?? c` is `a catch _ => (b ?? c)`; `a catch _ => b catch _ => c` right-nests, which is the wanted chain; and `r catch _ => a == b` is `r catch _ => (a == b)`. The body ends at a comma, a closing bracket, or the statement end — parenthesize the `catch` expression to end it earlier.

So the two rules a parser needs: the *operand* chain is level-7-left, and everything after `=>` belongs to the body until a list or statement boundary.

`try` is a prefix keyword and sits outside the numeric table — its placement follows two rules. **It binds tighter than `??`** (`type.errors/ER16b`), so `try f() ?? v` is `(try f()) ?? v` with no parens — `try` peels the error, `??` handles the absence. That order is the common composite (Zig's stdlib has ~160 of it against ~4 reversed, and Zig's reversed precedence forces parens on it — their issue #5436).

The `??` right side and the `catch` body may be a value or any divergence — `return`, `break`, `continue`, `panic(…)` — written where it happens (`type.optionals/OPT11`, `type.errors/ER14`). `catch`'s binder (`e =>` or `_ =>`) is mandatory. Inside a comma list a diverging right side needs parens (`type.errors/ER45a`).

`try` attaches to the fallible step of the postfix chain rather than to the whole of it (`type.errors/ER16a`): `try store.get(id)` is `try (store.get(id))`, while `try read_file(p).len()` is `(try read_file(p)).len()`. The wrappers have no methods or fields at all, so exactly one placement type-checks — the compiler finds it, and no parens are ever needed.

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
| **ORD4: Mixed-signedness comparison** | `==`, `!=`, `<`, `<=`, `>`, `>=` work between any two integer primitives, answered by **value** — a negative signed operand is below every unsigned one, so `5u64 > -1i32` is true and `u64::MAX > 1i32` is true. Comparison operators only: mixed-type *arithmetic* is a type error, because `u64 + i32` has no obviously-correct result type while the comparison has an obviously-correct answer. The bitwise operators and the shifts go with arithmetic, not with comparison. Integer primitives only — not floats, not user types — and `Comparable` itself is unchanged and stays same-type |
| **ORD3: Float ordering** | `f32`/`f64` implement `Comparable`. `compare()` is a **total** order so sorting is well-defined; the operators `<`, `>`, `<=`, `>=` stay IEEE, so every comparison against `NaN` is `false` |

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
| `f32`, `f64` | Yes* | Yes** | *NaN breaks reflexivity for `==`. **`compare()` is total (NaN sorts last); the operators stay IEEE |
| Structs | Derive | Derive | All fields must implement |
| Enums | Derive | Derive | Variant order, then payload |

## Arithmetic Traits

Operator traits: `Add`, `Sub`, `Mul`, `Div`, `Rem`, `Neg`, `BitAnd`, `BitOr`, `BitXor`, `BitNot`, `Shl`, `Shr`.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| `NaN == NaN` | EQ3 | `false` (IEEE 754) |
| `NaN < 1.0` | ORD3 | `false` — the operator is IEEE |
| `NaN > 1.0` | ORD3 | `false` — IEEE, *not* derived from `compare()` |
| `NaN.compare(1.0)` | ORD3 | `Greater` — the total order puts a positive NaN last |
| `[3.0, NaN, 1.0].sort()` | ORD3 | `[1.0, 3.0, NaN]` — total order, no lost elements. A *negative* NaN sorts first, ahead of `-inf`, per IEEE totalOrder |
| `5u64 > -1i32` | ORD4 | `true` — by value, not by reinterpreting either operand |
| `u64::MAX > 1i32` | ORD4 | `true` — the bit pattern is not read as a negative number |
| `5u64 + 1i32` | ORD4 | Compile error (E0371) — comparison is the exception, arithmetic isn't |
| `5u64 << 1i32` | ORD4 | Compile error (E0371) — a shift count's signedness isn't decoration; a negative one is a bug |
| Shift exceeding bit width | BW2 | Panic |
| Comparison chaining | P2 | Compile error |
| Struct with float field | ORD2 | Auto-derives — the float field compares by the total order |

---

## Appendix (non-normative)

### Rationale

**EQ3 (float semantics):** IEEE 754 compliance means `NaN == NaN` is false, breaking reflexivity. Rather than silently deviating from IEEE or forbidding equality on floats, we provide `.total_eq()` and `.total_cmp()` as explicit opt-ins for total ordering.

**ORD3 (float ordering):** Floats were originally excluded from `Comparable` on the grounds that NaN breaks totality, with a `.total_cmp()` opt-in for sorting. That reading traded one subtle bug for a louder one: `Vec<f64>.sort()`, `min`, `max` and every `T: Comparable` helper stopped working on the most numeric type in the language, and the workaround was a method name nobody would guess.

Splitting the two questions costs nothing and answers both. The **operators** are IEEE — `NaN < 1.0` and `NaN > 1.0` are both `false`, which is what `F1` promises and what anyone reading `a < b` expects of floats. **`compare()` is a total order** — IEEE totalOrder, so a positive NaN sorts last and a negative NaN first — which is what a sort needs to terminate with every element still present. This is the one place where ORD1's "operators derive from `compare()`" doesn't hold, and it's deliberate: the alternative is a `>` that answers `true` for NaN, which no float user wants.

`==` stays IEEE per `EQ3`, so `NaN.compare(NaN)` is `Equal` while `NaN == NaN` is `false`. That inconsistency is inherent to IEEE, not to this rule — it's the reason `total_cmp` exists in other languages as a separate method. Rask puts it in `compare()` because `compare()` has exactly one job here: order things for a sort.

**P2 (no chaining):** Chained comparisons (`a < b < c`) are ambiguous in most languages. Requiring `&&` is explicit and matches user expectation.

### See Also

- `type.overflow` — Integer overflow behavior
- `type.primitives` — Primitive types
- `type.traits` — Trait system
