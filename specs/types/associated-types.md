<!-- id: type.associated-types -->
<!-- status: decided -->
<!-- summary: An interface may name a type its conformance supplies; projections are read off a conformance, never searched for -->
<!-- depends: types/generics.md, types/interfaces.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Associated Types

An interface can name a type it doesn't know yet and let each conformance fill it in:

```rask
interface Mul<Rhs> {
    type Out
    func mul(self, rhs: Rhs) -> Self.Out
}

extend Meters implements Mul<f64> {
    type Out = Meters
    func mul(self, k: f64) -> Meters {
        return Meters { v: self.v * k }
    }
}
```

`Out` is a consequence of the conformance, not a choice the caller makes. Writing it as a third type parameter would let a call site ask for `Mul<f64, string>` and be told "no conformance" when the truth is the result type was never negotiable.

## The line this draws

Every associated type here is a **lookup**. Given `Self` and the interface's arguments, there is one conformance, and `Out` is read off it. Nothing runs backwards — knowing `Out` never picks `Self`, and no constraint ties two projections together and asks the compiler to solve for both.

That is the whole reason this was affordable enough to promote. `rejected-features.md` justified associated types over HKT on exactly this ground: no kind polymorphism, no inference blowup. A design that reintroduces a search has lost its own argument, so the search cases are out — see AT7.

## Rules

| Rule | Description |
|------|-------------|
| **AT1: Declared in the interface** | `type Out` in an interface body declares an associated type. It is a member of the interface like a method — part of what a conformance owes |
| **AT2: Supplied by the conformance** | `type Out = Meters` inside the `extend T implements Interface` block. A conformance that leaves one unsupplied and undefaulted is an error at the block, naming the associated type |
| **AT3: Projection is `.`** | `Self.Out` inside the interface, `T.Out` in generic code where `T` carries the bound. A dot, like every other member access in Rask — not `::` |
| **AT4: Declared defaults** | `type Out = Self` in the *interface* gives a default; a conformance may then omit it. Without a default the conformance must state it |
| **AT5: Bounds** | `type Out: Comparable` requires every conformance's `Out` to satisfy `Comparable`, checked at the conformance against the concrete type it named. One check, no search |
| **AT6: Read, never inferred** | Resolving a projection means finding the conformance and reading the binding. It is never solved for: no candidate set, no backtracking, no ambiguity |
| **AT7: No equality constraints** | `where T.Out == U` is not in the language. That constraint is what turns a lookup into a search, and nothing needs it yet. A generic function names `T.Out` and uses it; it cannot demand that two projections agree |
| **AT8: One binding per applied interface** | The binding belongs to `(Self, interface with its arguments)`. `Mul<f64>` and `Mul<Meters>` are different conformances with different `Out`s, whether they're on one type or two |
| **AT9: Not through `any`** | A method whose signature mentions an associated type has no vtable slot, and calling it through `any Interface` is a compile error at the call site (`type.interfaces/TR4`). Creating the `any` value is still fine |
| **AT10: Conditional conformance** | The binding may name the block's type parameters (`extend Ring<T> implements Wrap { type Out = Ring<T> }`), resolved per instantiation like every other part of a conditional conformance (CC1) |

### Two of them on one type

AT8 keys the binding on the applied interface, so the two `Out`s in

<!-- test: skip -->
```rask
extend Meters implements Mul<f64>    { type Out = Meters }
extend Meters implements Mul<Meters> { type Out = SquareMeters }
```

never get confused for each other. `m.mul(x)` used to have no answer, though: `MN1` gives a type one `mul` and both conformances want it, so the second block was rejected where it was written.

`type.operator-resolution/OR1` is what settled it — the argument's type picks the conformance, and each one's `mul` is filed under the argument it takes, so the two bodies keep separate symbols. It holds for the operator interfaces, where the argument is something to go on. For every other generic interface `MN1` still applies, and `MN4`'s `scoped extend` is the general answer for a conformance whose methods stay out of the namespace.

One thing the pair doesn't reach is the projection: `T.Out` in a generic signature doesn't record which bound it came through, so on a type carrying two `Mul`s there are two `Out`s and no way to say which ([#1330](https://github.com/rask-lang/rask/issues/1330)).

## Using one from generic code

A bound gives you the projection:

<!-- test: skip -->
```rask
func doubled<T: Mul<f64>>(x: T) -> T.Out {
    return x.mul(2.0)
}
```

`T.Out` is written, never guessed. At each instantiation `T` is concrete, its `Mul<f64>` conformance is unique (G1 makes conformance declared, and `type.operator-resolution/OR4` makes the operator ones unique per pair), and `Out` is read off it. That is the same work monomorphization already does.

What you cannot write is the version that ties two of them together:

<!-- test: skip -->
```rask
// Not in the language (AT7)
func chain<A: Mul<f64>, B: Mul<f64>>(a: A, b: B) -> ... where A.Out == B.Out
```

`==` between projections is the constraint that needs a solver. If a real case for it turns up, it is its own decision with its own cost — not something to be smuggled in now.

## What this unblocks

**Operator resolution.** `type.operator-resolution` was blocked outright on this; `interface Mul<Rhs> { type Out ... }` is its first line, and it landed on top of this. Two of its own rules stopped being operator-specific and became the general feature:

- OR3's "`Out` defaults to `Self`" is AT4 — a declared default in the interface, not an operator convenience.
- OR3's "`Rhs` defaults to `Self`" is `type.generics/GT4`, the same rule for interface type parameters.
- OR5's "`Out` is read, not inferred" is AT6, and it holds for the same reason: the conformance is unique, so the lookup is total.

**Encoding.** `std.encoding` recorded wanting `Encode`/`Decode` as bounds and settling for marker interfaces because a `Serializer` hierarchy needs associated types. That's now writable. Whether to rewrite encoding around it is a separate call — the markers work and the rewrite is not free.

## What is deliberately left out

- **Equality constraints** (AT7). The search case.
- **Associated constants.** `const N: usize` in an interface body. No customer; comptime parameters cover the cases that came up.
- **Lifting the `any` restriction** (AT9). A projection has no single answer across the types behind an `any`, which is the whole point of `any`. TR4 was written with a "(MVP)" hedge; this is the revisit, and the answer is that the restriction is right on its merits, not provisional.
- **Generic associated types.** `type Out<T>` is HKT wearing a different hat, and `rejected-features.md` already ruled on that.

---

## Appendix (non-normative)

### Rationale

The scope question was the real decision: promote the narrow read-off-a-unique-conformance version, or the general one with equality constraints and a solver behind them.

I took the narrow one, and the test that decided it is the one `rejected-features.md` used to pick associated types over HKT in the first place — "no inference blowup." Equality constraints are the entire inference cost of the feature. Drop them and what's left is a table lookup that monomorphization was already doing. Keep them and the argument for promoting associated types over HKT stops being true of the thing being promoted.

That is not "the minimum to unblock operators." Defaults, bounds, projections in generic signatures and per-instantiation bindings are all in, and none of them was needed for operator resolution alone. They're in because each one is still a lookup. The line is drawn at the property, not at the customer.

The counter-argument — promote once, properly, rather than twice — is real, and the answer is that AT7 is additive. Nothing here has to be unsaid to add equality constraints later; there is no `Out` written as a parameter to migrate off, no syntax to change. The door stays real, which is what `design-horizon.md` asks of a deferral.

### Why `.` and not `::`

Rask has one member-access operator and `Token.Plus` already reads a type's member with it. `Self.Out` is the same gesture. A second sigil for "member, but of a type, at compile time" would be a distinction the reader has to learn to get back exactly what the dot already meant.

### Why the default lives in the interface

`type Out = Self` on the declaration says "most conformances answer in their own type," once, where the interface is designed. The alternative — a per-feature rule like OR3's — says it again in every spec that wants it, and the two spellings drift. One rule, declared where the contract is.

### See Also

- `type.generics` — GT1–GT4, interface type parameters; G1, declared conformance; CC1, conditional conformance
- `type.interfaces` — TR4, the `any` restriction AT9 confirms
- `type.operator-resolution` — the feature this unblocked
- [#1165](https://github.com/rask-lang/rask/issues/1165) — the promotion decision
- [#1164](https://github.com/rask-lang/rask/issues/1164) — interface type parameters, the prerequisite
