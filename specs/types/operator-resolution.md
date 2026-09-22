<!-- id: type.operator-resolution -->
<!-- status: decided -->
<!-- summary: Operators resolve on the ordered pair of operand types against declared operator traits, instead of as a method lookup on the left operand -->
<!-- depends: types/operators.md, types/traits.md, types/generics.md, types/associated-types.md -->
<!-- implemented-by: compiler/crates/rask-types/, stdlib/ops.rk -->

# Operator Resolution

`a + b` used to be rewritten to `a.add(b)` and resolved as an ordinary method call on `a`. The right operand never participated in choosing what ran — it was only checked against whatever signature the left operand happened to offer.

Resolution is on the **ordered pair** `(typeof a, typeof b)` against declared operator traits. It stays entirely a compile-time question: the pair is known when the program is built, nothing is looked up at runtime, and nothing lands in the binary.

## Motivation

Three separate problems shared one cause.

**The left operand decided everything.** `Meters * 2.0` works — `Meters` writes `mul(self, k: f64) -> Meters` and heterogeneous right operands were always fine. `2.0 * Meters` was unreachable: it means `(2.0).mul(Meters)`, and a primitive's methods come from a closed table in the compiler with no extension point. Units, currency, scalar-times-vector and matrix-times-vector all want both directions, and half of each pair could not be written at all.

**The compiler was the extension point.** When the stdlib needed an operator whose result type depends on the right operand, there was nowhere to say it, so it went into the type checker instead (`rask-types/src/checker/resolve.rs`):

```rust
// Instant subtraction: overloaded on argument type
//   instant - instant -> Duration
//   instant - duration -> Instant
("Instant", "sub") if args.len() == 1 => {
```

That comment is two-argument resolution, hand-written for one type because the language could not express it. Three rows of `std.time`'s own table — `duration * n`, `n * duration`, `duration / duration` — were simply missing, because each would have been another arm in the compiler.

**The check that should reject a bad pair was the check that got dropped.** The float operator path discarded its own argument unification, so `f64 * <struct>` type-checked and native codegen multiplied the struct's address ([#978](https://github.com/rask-lang/rask/issues/978)).

## Design

### Operator traits are real, and take both sides

```rask
public trait Mul<Rhs = Self> {
    type Out = Self

    func mul(self, rhs: Rhs) -> Self.Out
}
```

`Out` is an associated type: it is a consequence of the pair, not something a caller chooses. Writing it as a third parameter would let a call site ask for `Mul<f64, string>` and get a "no conformance" error instead of the truth, which is that the result type was never up for negotiation.

| Rule | Description |
|------|-------------|
| **OR1: Resolution on the ordered pair** | `a OP b` selects the operator-trait conformance registered for `(typeof a, typeof b)`, in that order. It is not a method lookup on `a` |
| **OR2: Declared operator traits** | `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Shl`, `Shr` are declared traits taking `<Rhs>` and carrying an associated `Out`. `Neg` and `BitNot` are unary — no `Rhs`, `Out` only. They live in [`stdlib/ops.rk`](../../stdlib/ops.rk) |
| **OR3: Both default to `Self`** | The operator traits are declared `trait Mul<Rhs = Self> { type Out = Self … }`, so this is `type.generics/GT4` and `type.associated-types/AT4` rather than an operator rule. `extend Point with Add` is `Add<Point>` answering in `Point`; `extend Meters with Mul<f64>` answers in `Meters` |
| **OR4: One conformance per pair** | At most one conformance of a given operator trait for a given `(Self, Rhs)` in a build. A second is a use-site error naming both packages — the same collision rule retroactive conformance already carries (#312). Two conformances of one operator to *different* pairs are fine and are what OR1 tells apart |
| **OR5: `Out` is read, not inferred** | OR4 makes the conformance unique, so `Out` is read off it — `type.associated-types/AT6`, which holds for every associated type for the same reason. No inference search and no ambiguity |
| **OR6: Primitives take conformances only** | `extend f64 with Mul<Meters>` is legal. `extend f64 { … }` — an inherent method on a primitive — remains illegal |
| **OR7: No implicit symmetry, pending `@commutative`** | Defining `Meters * f64` does not by itself generate `f64 * Meters`. Whether `@commutative` may generate the flip is open — see below |
| **OR8: A missing pair is a compile error** | Naming both operand types and the operator as it was written, at check time. The left operand having a method of that name is not a conformance — `extend Meters { func mul(…) }` leaves `m * 2.0` undefined, and the error says the header is what's missing |
| **OR8a: Method syntax is not the operator** | `a.mul(b)` written out is an ordinary method call. It reaches the conformance when there is one, and an inherent `mul` when there isn't — so a type is free to have a `mul`, an `add` or a `div` that means something else. Only the operator requires the conformance |
| **OR9: Comparison stays same-type** | `Equal` and `Comparable` keep `Self` on both sides and are not resolved on the pair. Mixed-signedness integer comparison remains the builtin exception (`type.operators/ORD4`) |
| **OR10: Resolution is static** | The pair comes from static types only. No runtime component, no dispatch table in the binary, no cost at the call site |
| **OR11: Compound assignment needs `Out == Self`** | `a *= b` requires the `(A, B)` conformance to answer in `A`. Otherwise the assignment would change the variable's type |
| **OR12: The builtin pairs stay the compiler's** | `i64 + i64`, `u64 << u32` and the rest resolve to machine instructions, and a conformance written on a primitive does not take them away. A stdlib type whose pair is likewise the compiler's declares it with `@builtin`: the conformance says what the pair answers with, and the backends keep their own lowering |

### What it looks like

```rask
struct Meters { v: f64 }

extend Meters with Mul<f64> {          // Out defaults to Meters
    func mul(self, k: f64) -> Meters {
        return Meters { v: self.v * k }
    }
}

extend f64 with Mul<Meters> {          // the direction that was impossible
    type Out = Meters

    func mul(self, m: Meters) -> Meters {
        return Meters { v: self * m.v }
    }
}
```

Both may be written by a third package that owns neither `f64` nor `Meters`, because #312 already allows retroactive conformance. That is the composability win, and it arrives without any runtime machinery.

`Out` differing from `Self` is expressible, and so is a second conformance on the same type:

```rask
extend Meters with Mul<Meters> {
    type Out = SquareMeters

    func mul(self, other: Meters) -> SquareMeters {
        return SquareMeters { v: self.v * other.v }
    }
}
```

`Meters` now answers both `Mul<f64>` and `Mul<Meters>`, and the argument is what picks. `type.associated-types/AT8` recorded that pair as unwritable and named this rule as what would fix it.

### Shifts fall out

`type.operators` notes that shifts sit outside the homogeneous operators because the shift amount is its own type. Under OR2 that is not a special case — `Shl<u32>` answering in `Self` says it directly.

### Where the argument hasn't landed yet

An unsuffixed literal has no type until defaulting runs, and a conformance is chosen from types. Three rules keep the common shapes working without guessing:

- **One conformance is not a choice.** `meters * 2.0` where `Meters` carries only `Mul<f64>` types the call against it, and that is what settles the literal.
- **A literal still says something.** `2` can only be an integer and `2.0` only a float, so against `Div<i64>` and `Div<Duration>` a `duration / 2` has one candidate of the right kind.
- **A literal on the *left* takes the primitive that forms a pair.** `3 * duration` has nothing tying the `3` to anything — the right operand isn't a number — so it takes the type of the one primitive that forms a pair with `Duration`. The candidate set is the primitives, so this is a lookup over a fixed list, not a search.

Anything still ambiguous waits for literal defaulting rather than picking.

## What doesn't change

- Source for the common case. `extend Point with Add` reads the same as `extend Point { func add(…) }` did and means the same thing, because of OR3.
- Precedence, associativity, newline continuation, `try`/`??`/`catch` placement — all of `type.operators` P1–P4 is untouched.
- Indexing, `Equal`, `Comparable`, division and remainder semantics, overflow.
- Method-call syntax. `a.mul(b)` still works and resolves the same conformance.
- Generic bounds. `func scale<T: Mul>(…)` means `Mul<T>` by OR3.

## What it changed

- **The hardcoded stdlib pairs are gone.** `("Instant", "add")`, `("Instant", "sub")` and their neighbours are ordinary conformances in `stdlib/time.rk`, and the three rows of the arithmetic table that had never been implemented came with them.
- **Primitives gained a conformance surface.** Their method tables are still closed to inherent methods (OR6), but a conformance can be written on one.
- **Existing inherent operator methods stopped serving operators.** `extend Meters { func mul(…) }` is still a method and `m.mul(2.0)` still calls it; what it no longer does is answer `*`. The rewrite is mechanical and the compiler prints the header (E0894).
- **A type can carry two conformances of one operator.** Each one's method is filed under the applied argument, so the two keep separate symbols.
- **`operators.md`'s "Operator traits: `Add`, `Sub`, …" line describes something that exists.**

## Error messages

The error OR8 requires:

```
error[E0382]: cannot apply `*` to `f64` and `Meters`
  --> src/main.rk:6:13
    |
  6 |     let r = 2.0 * d
    |             ^^^^^^^ `f64` on the left, `Meters` on the right
    = fix: extend f64 with Mul<Meters> { type Out = Meters … }
    = why: an operator is resolved from both operand types, in order
```

The collision error OR4 requires:

```
error: two definitions of `*` between `f64` and `Meters`
    = note: `units` defines it at units/scale.rk:12
    = note: `physics` defines it at physics/units.rk:40
    = fix: depend on one of them, or ask the packages to agree on which owns it
```

Both are the messages that decide whether the feature is trusted, so they are normative, not decoration.

## Non-goals

- **Not multiple dispatch.** The pair is read from static types. Nothing is chosen while the program runs.
- **Not an open world.** No definition may be added after the program is built.
- **Not sealing.** Rask's world is already closed at build time; there is nothing to seal.
- **Not comparison.** OR9 leaves `Equal`/`Comparable` alone.

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| `2.0 * meters`, with `extend f64 with Mul<Meters>` | OR1, OR6 | `Meters` |
| `2.0 * meters`, without it | OR8 | Compile error naming both operands |
| `meters * meters` and `meters * 2.0` on one type | OR4 | Two conformances, told apart by the argument |
| `f64 * f64` where `f64` also carries `Mul<Meters>` | OR12 | The builtin pair — a conformance doesn't take it away |
| `a *= b` where `(A, B)` answers in `C` | OR11 | Compile error: the assignment would change `a`'s type |
| `instant - instant` | OR12 | `Duration`, declared `@builtin` in `stdlib/time.rk` |
| `3 * duration` | OR1 | `Duration` — the literal takes the `i64` the pair was written for |
| `x.mul(2.0)` inside `func f<T: Mul<f64>>` | OR1, AT6 | The bound names the pair; `T.Out` is read off it |
| `m * 2.0` where `Meters` has an inherent `mul` | OR8 | Compile error (E0894) naming both operands and the header the method wants |
| `m.mul(2.0)` where `Meters` has an inherent `mul` | OR8a | Legal — an ordinary method call, which is all it ever was |
| `extend Holder { func add(mutate self, v: i64) }` | OR8a | Legal; `holder + v` is not |

---

## Appendix (non-normative)

### Rationale

This came out of asking why multiple dispatch and a JIT are a bad fit for a systems language, using Julia as the reference.

The finding was that "multiple dispatch" bundles three separable things:

1. Choosing a method from the types of several arguments.
2. Letting anyone define a method for a combination of types they do not own.
3. Choosing from the types the arguments turn out to have *while the program runs*, over a set that is never finished.

Only the third requires a compiler inside the running program — and it is the third that makes Julia unshippable as a static binary, that makes its cost model invisible, and that makes traits unanswerable (you cannot check a promise about a list that never ends).

The first two are ordinary compile-time work. Rask already had the second: #312 allows a third package to write a conformance for types it does not own, with collisions caught at the use site. This adds the first. There is no reason the two need the third.

The Julia comparison also produced a caution worth recording: part of why Julia composes so well is that nothing can reject you. A checked system buys early errors and pays for them with errors in cases Julia would simply have run. OR8 is that bill. I think it is the right trade for a language that has to run on a sensor, but it is a real cost and not a free win.

### Where the operator/method distinction is kept

Desugaring rewrites `a * b` to `a.mul(b)`, so by the time anything can act on it the two are one node kind — and only one of them has to resolve against a conformance. The first draft of the implementation answered that by reserving the twelve method names at the declaration: a method called `mul` had to come from a `with Mul` block, whatever the call site did.

That works and it costs a language restriction nobody asked for. A registry's `add`, a buffer's, a builder's `div` — none of them is an operator, and all of them would have had to be renamed.

So desugar hands the fact over instead: it records which calls it rewrote, and the checker reads that. Six signatures carry it — desugar's three entry points, the checker's three — and in exchange OR1 is true as written, the names stay free, and the error lands on the operator with both operand types in it.

It's the same shape as every other thing the front end knows and the back end would otherwise re-derive: `operator_targets` for which pairs resolved to a call, `call_targets` for dispatch. A pass that erases a distinction says what it erased.

### What was considered and rejected

**`Out` as a third type parameter, to dodge associated types.** An earlier draft did this, on the grounds that OR4's uniqueness makes `Out` a lookup anyway and promoting associated types for one feature is expensive. Rejected: it puts the result type in the caller's hands syntactically when it is never the caller's to choose, and it means a wrong guess reports "no conformance" instead of the real answer. Holding out was right and cheaper than it looked — the promotion came in narrow (`type.associated-types`), and it took two of this spec's rules off its hands rather than adding to them.

**Julia's model wholesale.** Runtime pair selection over an open set. Rejected — it is exactly the third item above, and it costs a compiler in the process, unpredictable pauses mid-run, and any hope of a small static binary.

**Leaving it alone and correcting `operators.md` instead.** A real option: delete the sentence claiming operator traits exist, keep left-operand method lookup, and accept that `f64 * Meters` is not expressible. It is less work and it is honest. Rejected because the `("Instant", "sub")` special cases showed the limit was already being hit inside the stdlib, and each future library that hits it has no recourse.

### Open questions

**`@commutative` (OR7).** The first draft rejected auto-deriving the flipped direction on the grounds that the compiler would have to be told which instances commute, "at which point the annotation costs as much as the second conformance". That arithmetic is wrong. One line:

<!-- test: skip -->
```rask
@commutative
extend Meters with Mul<f64> { … }        // and f64 * Meters, for free
```

against roughly five for the hand-written flip. Rask has user annotations already, so this is not new machinery.

Two objections survive, and both are answerable:

- *The compiler cannot check that `a * b` really equals `b * a`.* True, and the same is already true of `Equal`'s reflexivity and `Comparable`'s transitivity, which the spec records as "programmer must ensure". Precedent exists.
- *It writes a conformance on a type the annotation's author may not own* — `@commutative` on `Meters: Mul<f64>` registers something about `f64`. That needs a precedence rule against OR4: an explicitly written conformance beats a generated one, and two generated ones that land on the same pair are the ordinary collision error.

What the transparency principle actually objects to is a definition appearing with nothing in the source to point at. `@commutative` is one word at the definition site saying "and the other way round" — arguably more legible than two near-identical blocks that a reader has to diff.

Leaning yes. Scope note if it lands: `Add` and `Mul` are the commutative cases worth covering. `Sub` is anti-commutative and `Div` is neither — do not build a family.

### See Also

- `type.operators` — precedence, `Equal`/`Comparable`, the operator trait list this makes real
- `type.associated-types` — `Out`, and AT8's two-conformances case that OR4 answers
- `type.generics` — conformance rules, MN3 conflict scoping, the #312 retroactive-conformance design
- [#978](https://github.com/rask-lang/rask/issues/978) — the discarded unification, the prerequisite that landed first
- [#399](https://github.com/rask-lang/rask/issues/399) — operator overloads computing on struct addresses
