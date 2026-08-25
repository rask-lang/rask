<!-- id: type.operator-resolution -->
<!-- status: proposed -->
<!-- summary: Operators resolve on the ordered pair of operand types against declared operator traits, instead of as a method lookup on the left operand -->
<!-- depends: types/operators.md, types/traits.md, types/generics.md -->

# Operator Resolution

`a + b` today is rewritten to `a.add(b)` and resolved as an ordinary method call on `a`. The right operand never participates in choosing what runs — it is only checked against whatever signature the left operand happens to offer.

This proposal replaces that with resolution on the **ordered pair** `(typeof a, typeof b)` against declared operator traits. It stays entirely a compile-time question: the pair is known when the program is built, nothing is looked up at runtime, and nothing lands in the binary.

## Motivation

Three separate problems share one cause.

**The left operand decides everything.** `Meters * 2.0` works — `Meters` writes `mul(self, k: f64) -> Meters` and heterogeneous right operands are fine today. `2.0 * Meters` is unreachable: it means `(2.0).mul(Meters)`, and a primitive's methods come from a closed table in the compiler with no extension point. Units, currency, scalar-times-vector and matrix-times-vector all want both directions, and half of each pair cannot be written at all.

**The compiler became the extension point.** When the stdlib needed an operator whose result type depends on the right operand, there was nowhere to say it, so it went into the type checker instead (`rask-types/src/checker/resolve.rs`):

```rust
// Instant subtraction: overloaded on argument type
//   instant - instant -> Duration
//   instant - duration -> Instant
("Instant", "sub") if args.len() == 1 => {
```

That comment is two-argument resolution, hand-written for one type because the language could not express it. Every units or vector library hits the same wall without a compiler to patch.

**The check that should reject a bad pair is the check that got dropped.** The float operator path discards its own argument unification, so `f64 * <struct>` type-checks and native codegen multiplies the struct's address ([#978](https://github.com/rask-lang/rask/issues/978)). The arm that should dispatch the pair and the arm that should reject it are the same arm, and right now it does neither.

There is also a documentation gap: `operators.md` lists `Add`, `Sub`, `Mul` and the rest as "operator traits", but no such traits exist anywhere in the stdlib or the compiler. Operators are duck-typed on the method name. This proposal makes that sentence true; the alternative is deleting it.

## Design

### Operator traits are real, and take both sides

<!-- test: skip -->
```rask
trait Mul<Rhs, Out> {
    func mul(self, rhs: Rhs) -> Out
}
```

`Out` is a plain type parameter, not an associated type — `type.generics` defers associated types out of the MVP, and OR5 below recovers the only thing they would have bought.

| Rule | Description |
|------|-------------|
| **OR1: Resolution on the ordered pair** | `a OP b` selects the operator-trait conformance registered for `(typeof a, typeof b)`, in that order. It is not a method lookup on `a` |
| **OR2: Declared operator traits** | `Add`, `Sub`, `Mul`, `Div`, `Rem`, `BitAnd`, `BitOr`, `BitXor`, `Shl`, `Shr` are declared traits with `<Rhs, Out>`. `Neg` and `BitNot` are unary and take `<Out>` only |
| **OR3: Both parameters default** | `Rhs` defaults to `Self` and `Out` defaults to `Self`. `extend Point with Add` means `Add<Point, Point>`; `extend Meters with Mul<f64>` means `Mul<f64, Meters>` |
| **OR4: One conformance per pair** | At most one conformance of a given operator trait for a given `(Self, Rhs)` in a build. A second is a use-site error naming both packages — the same collision rule retroactive conformance already carries (#312) |
| **OR5: `Out` is read, not inferred** | Because OR4 makes the conformance unique, `Out` is read off it directly. No inference search, and no associated types needed |
| **OR6: Primitives take conformances only** | `extend f64 with Mul<Meters, Meters>` is legal. `extend f64 { … }` — an inherent method on a primitive — remains illegal |
| **OR7: No implicit symmetry** | Defining `Meters * f64` does not generate `f64 * Meters`. Both directions are written where both are wanted |
| **OR8: A missing pair is a compile error** | Naming both operand types and the operator as it was written, at check time. This is [#978](https://github.com/rask-lang/rask/issues/978) stated normatively |
| **OR9: Comparison stays same-type** | `Equal` and `Comparable` keep `Self` on both sides. Mixed-signedness integer comparison remains the builtin exception (`type.operators/ORD4`) |
| **OR10: Resolution is static** | The pair comes from static types only. No runtime component, no dispatch table in the binary, no cost at the call site |
| **OR11: Compound assignment needs `Out == Self`** | `a *= b` requires the `(A, B)` conformance to answer in `A`. Otherwise the assignment would change the variable's type |

### What it looks like

<!-- test: skip -->
```rask
struct Meters { v: f64 }

extend Meters with Mul<f64> {          // Out defaults to Meters
    func mul(self, k: f64) -> Meters {
        return Meters { v: self.v * k }
    }
}

extend f64 with Mul<Meters, Meters> {  // the direction that is impossible today
    func mul(self, m: Meters) -> Meters {
        return Meters { v: self * m.v }
    }
}
```

Both may be written by a third package that owns neither `f64` nor `Meters`, because #312 already allows retroactive conformance. That is the composability win, and it arrives without any runtime machinery.

`Out` differing from `Self` becomes expressible for the first time:

<!-- test: skip -->
```rask
extend Meters with Mul<Meters, SquareMeters> {
    func mul(self, other: Meters) -> SquareMeters { … }
}
```

### Shifts fall out

`type.operators` notes that shifts sit outside the homogeneous operators because the shift amount is its own type. Under OR2 that is not a special case — `Shl<u32, Self>` says it directly.

## What doesn't change

- Source for the common case. `extend Point with Add` reads the same as today's `extend Point { func add(…) }` and means the same thing, because of OR3.
- Precedence, associativity, newline continuation, `try`/`??`/`catch` placement — all of `type.operators` P1–P4 is untouched.
- Indexing, `Equal`, `Comparable`, division and remainder semantics, overflow.
- Method-call syntax. `a.mul(b)` still works and resolves the same conformance.
- Generic bounds. `func scale<T: Mul>(…)` means `Mul<T, T>` by OR3.

## What does change

- **Existing inherent operator methods stop being found by operators.** `extend Meters { func mul(self, k: f64) -> Meters }` today serves `*`; under OR1 it does not, because there is no conformance. It becomes `extend Meters with Mul<f64> { … }`. The rewrite is mechanical and the compiler can emit it as a suggestion.
- **The hardcoded stdlib pairs are deleted.** `("Instant", "add")`, `("Instant", "sub")` and their neighbours become ordinary conformances written in `stdlib/time.rk`.
- **Primitives gain a conformance surface.** Their method tables stop being closed. This is the bulk of the implementation work.
- **`operators.md` becomes accurate.** The "Operator traits: `Add`, `Sub`, …" line describes something that exists.

## Migration

`#978` is a prerequisite and lands first, on its own: the discarded unification is a soundness bug in shipped behaviour and should not wait on a design change. Once a bad pair is rejected at check time, OR1 changes *which* pairs are accepted rather than changing an unchecked path into a checked one.

The inherent-to-conformance rewrite is a one-time mechanical edit. Both sides of it are visible to the compiler, so the diagnostic can print the replacement line.

## Error messages

The error OR8 requires:

```
error[E____]: no `*` between `f64` and `Meters`
  --> src/main.rk:6:13
    |
  6 |     let r = 2.0 * d
    |             ^^^^^^^
    = fix: extend f64 with Mul<Meters, Meters> { … }
    = why: an operator is resolved from both operand types, in order
```

The collision error OR4 requires:

```
error[E____]: two definitions of `*` between `f64` and `Meters`
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

---

## Appendix (non-normative)

### Rationale

This came out of asking why multiple dispatch and a JIT are a bad fit for a systems language, using Julia as the reference.

The finding was that "multiple dispatch" bundles three separable things:

1. Choosing a method from the types of several arguments.
2. Letting anyone define a method for a combination of types they do not own.
3. Choosing from the types the arguments turn out to have *while the program runs*, over a set that is never finished.

Only the third requires a compiler inside the running program — and it is the third that makes Julia unshippable as a static binary, that makes its cost model invisible, and that makes traits unanswerable (you cannot check a promise about a list that never ends).

The first two are ordinary compile-time work. Rask already has the second: #312 allows a third package to write a conformance for types it does not own, with collisions caught at the use site. This proposal adds the first. There is no reason the two need the third.

The Julia comparison also produced a caution worth recording: part of why Julia composes so well is that nothing can reject you. A checked system buys early errors and pays for them with errors in cases Julia would simply have run. OR8 is that bill. I think it is the right trade for a language that has to run on a sensor, but it is a real cost and not a free win.

### What was considered and rejected

**Associated types (`type Out`).** The natural shape, and unavailable — `type.generics` defers associated types out of the MVP. OR4 plus OR5 gets the same result: uniqueness per pair makes `Out` a lookup rather than an inference problem. Cheaper than promoting associated types for one use.

**Auto-deriving the flipped direction for "commutative" operators.** Tempting for `f64 * Meters`, and wrong. `Mul` is not commutative in general (matrices), so the compiler would have to be told which instances are — at which point the annotation costs as much as the second conformance. It also generates a definition nobody wrote, which the transparency principle argues against. Hence OR7.

**Leaving it alone and correcting `operators.md` instead.** A real option: delete the sentence claiming operator traits exist, keep left-operand method lookup, and accept that `f64 * Meters` is not expressible. It is less work and it is honest. Rejected because the `("Instant", "sub")` special cases show the limit is already being hit inside the stdlib, and each future library that hits it has no recourse.

**Julia's model wholesale.** Runtime pair selection over an open set. Rejected — it is exactly the third item above, and it costs a compiler in the process, unpredictable pauses mid-run, and any hope of a small static binary.

### See Also

- `type.operators` — precedence, `Equal`/`Comparable`, the operator trait list this makes real
- `type.generics` — conformance rules, MN3 conflict scoping, the #312 retroactive-conformance design
- [#978](https://github.com/rask-lang/rask/issues/978) — the discarded unification, prerequisite
- [#816](https://github.com/rask-lang/rask/issues/816) — the same discarded-unify pattern, fixed for one pair
- [#399](https://github.com/rask-lang/rask/issues/399) — operator overloads computing on struct addresses
