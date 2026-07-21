<!-- id: type.trait-review -->
<!-- status: proposal -->
<!-- summary: Scenario-driven review of the trait system after the structural-to-nominal flip; proposed rules for the holes it opened -->
<!-- depends: types/generics.md, types/traits.md, types/gradual-constraints.md -->

# Trait System Review

The flip to nominal conformance (G1, issue #283) was decided on a sound argument — conformance is a semantic claim, not a shape. But a default this deep touches everything around it, and several rules written for the structural world weren't re-derived. This review walks the system scenario by scenario: what the user writes, where it bites, and what rule fixes it. The lens throughout: **common case zero-ceremony, special case opt-in and visible.**

Nothing here re-opens the flip itself. It stands.

---

## How traits actually get used

Ranked by how often they appear in the validation programs (HTTP server, grep, editor, game loop, sensor):

| # | Scenario | Frequency | Ceremony today |
|---|----------|-----------|----------------|
| 1 | Compare/sort/Map-key a user struct | Constant | Zero (auto-derive) ✓ |
| 2 | Print a user struct | Constant | Zero via `{x:debug}` ✓ |
| 3 | Encode/Decode | Common | Zero (auto-derive markers) ✓ |
| 4 | Private helper over "whatever has these methods" | Common | Zero (inferred bounds) ✓ |
| 5 | Public generic function with bounds | Common in libraries | One bound ✓ |
| 6 | Heterogeneous collection (`any Handler`) | Occasional | Cast per element ✓ |
| 7 | Operator overload on a math type | Occasional | Method definitions ✓ |
| 8 | Custom equality/hash/order | Rare | One extend block ⚠ hole |
| 9 | Conform a generic container to a trait | Rare but library-critical | ✗ no rule |
| 10 | Two traits sharing a method name | Rare | ✗ no rule |
| 11 | Conformance for a third-party type | Rare | #312 |

The top of the table is in good shape — auto-derive and gradual constraints carry the common cases with zero ceremony. Every hole is in the bottom half: rare cases that today have *no defined behavior* instead of an opt-in. The fixes below give each one a rule without adding ceremony to the top half.

---

## Finding 1: Overriding one auto-derived trait silently breaks its family

**Scenario 8.** The Equal/Hashable/Comparable family carries cross-trait contracts: `a == b` implies `hash(a) == hash(b)`, and `compare` must agree with `eq`. Auto-derive keeps them consistent by construction. Overriding *one* breaks the others silently:

<!-- test: skip -->
```rask
struct Username {
    name: string
}

extend Username with Equal {
    func eq(self, other: Username) -> bool {
        return self.name.lower() == other.name.lower()   // case-insensitive
    }
}

// Hashable is still auto-derived — hashes the raw string.
// "Bob" == "bob" but hash("Bob") != hash("bob").
// A Map<Username, T> now silently loses entries.
```

This is the same corruption class the #312 core-trait carve-out exists to prevent, arising *within* one package. Pre-flip this couldn't happen — there were no declared overrides.

**Proposed rules:**

| Rule | Description |
|------|-------------|
| **OC1: Override cancels dependents** | Overriding `Equal` cancels auto-derived `Hashable` and `Comparable` for that type. Overriding `Comparable` alone is checked against `Equal` (auto or manual) at the declaration |
| **OC2: Loud, with the fix** | Using a cancelled conformance is a compile error at the use site: "Username overrides Equal, so Hashable is no longer auto-derived — declare `extend Username with Hashable` consistent with your eq" |

Common case (no override) unaffected. The rare case gets an error instead of a haunted Map.

## Finding 2: Method-name collisions across traits have no rule

**Scenario 10.** Under structural matching this was a non-question — one `greet` method satisfied every greet-shaped trait. Under nominal, conformance blocks can carry bodies, so two traits can in principle demand two different `greet`s. There is no qualified-call syntax to disambiguate, and nothing says whether a method defined inside `extend T with Trait { }` joins T's ordinary method namespace.

**Proposed rules — one type, one method name, one meaning:**

| Rule | Description |
|------|-------------|
| **MN1: Single namespace** | Methods defined in `extend T with Trait { }` are ordinary methods of T, same namespace as plain `extend T` blocks |
| **MN2: Shared implementation** | Two conformances requiring the same method name share the one implementation — legal iff both signatures match it |
| **MN3: Conflict is an error** | If the signatures disagree, the second conformance declaration is a compile error naming both traits. Workaround: a wrapper type |

No qualified-call syntax, no per-trait method tables. This matches the flip's own logic: a method on a type is one semantic claim, and a type that means two different things by `greet` is two types. MN1 also answers a question users will hit in week one: yes, `dog.greet()` works even though `greet` was defined inside the conformance block.

## Finding 3: Generic containers can't conform conditionally

**Scenario 9.** `Stack<T>` is Displayable only when `T` is. Auto-derive handles this for the core five (a `Vec<T>` of Cloneable is Cloneable), which hides the gap: user traits on user generics have no rule at all. Structural matching used to give this for free — the shape either compiled per-instantiation or didn't.

**Proposed rule — reuse `where`, no new syntax:**

| Rule | Description |
|------|-------------|
| **CC1: Conditional conformance** | `extend Type<T> with Trait where T: Bound { ... }` — conformance holds exactly for instantiations satisfying the clause, checked at monomorphization like every other bound (G2/G6) |

<!-- test: skip -->
```rask
extend Stack<T> with Displayable where T: Displayable {
    func to_string(self) -> string {
        return self.items.map(|x| x.to_string()).join(", ")
    }
}
```

Blocked on `where` parsing (#313). No global analysis: the declaration is checked where the instantiation is, same as today's bounds.

## Finding 4: The inference seam needs its edges specced

**Scenarios 4→5, the prototype-to-production pipeline.** Gradual constraints deliberately keep private inference structural (GC3 collects method-requirements, not trait names). That's the right call — prototyping glue, invisible in APIs. But the promotion step got three unexamined edges:

1. **Bound propagation is mixed, not structural.** A private inferred function that passes its parameter to `sort<T: Comparable>` must carry the *nominal* bound onward — post-flip, a `compare` method no longer implies `Comparable`. GC3 as written only collects method-requirements.
2. **Promotion can hit a wall.** "Make public" must name traits. If the body's method-requirements match no trait, there is nothing to name — the honest fix is defining a trait, and the tooling should say so instead of pretending a signature exists. If they match *two* traits, promotion is ambiguous.
3. **The GC5 error message oversells.** It displays an inferred signature with a named trait (`<T: Validatable>`) recovered from shape — the exact resolution the flip removed. Display must be honest about which case it's in.

**Proposed rules:**

| Rule | Description |
|------|-------------|
| **IS1: Mixed inference** | Inferred bounds are the union of (a) nominal bounds propagated from called functions and (b) structural method-requirements from direct calls. A nominal bound subsumes the methods it provides |
| **IS2: Promotion is exact** | "Make public"/"make explicit" fills in a named trait only when exactly one visible trait covers the residual method-requirements. Zero matches: report the methods and offer to generate a trait definition. Two+: list candidates, user picks |
| **IS3: Honest ghost text** | Inferred-signature display distinguishes propagated nominal bounds from raw method-requirements (`T: Comparable` vs `T: {frobnicate}`) |

Also worth one line in gradual-constraints.md's gotcha section: annotating a working private function can make a working call fail, because the bound's meaning flips from shape to declaration. That's by design — say it out loud.

## Finding 5: Operators stayed structural — keep them, but on purpose

**Scenario 7.** G4 expands `a + b` to `a.add(b)` and checks only that the method exists. The flip's rationale ("existing isn't a semantic claim") seems to apply — but doesn't, and the reason is worth recording:

On **concrete** types, `a + b` resolves to an `add` method someone deliberately wrote on that type. There is no accidental-conformance risk — nothing is being *matched*, just called. The risk the flip addressed only exists where code accepts *unknown* types against a contract — and that path is already nominal: `func sum<T: Numeric>` requires declared conformance.

**Proposed rule (documents the status quo as a decision):**

| Rule | Description |
|------|-------------|
| **OP1: Concrete operators are authored sugar** | Operator expansion on concrete types is method-call sugar, no conformance involved. Generic operator use goes through nominal bounds (`Numeric`, `Comparable`) like any other generic call |

No fine-grained `Add`/`Sub`/`Mul` trait zoo. A math type defines the methods it supports and gets exactly those operators.

## Finding 6: Small inconsistencies

| Item | Problem | Fix |
|------|---------|-----|
| TD1 trait visibility | "Public by default" contradicts the language-wide package-private default (`struct.modules/V1`) | Traits default package-visible, `public trait` exports — same as everything else |
| Composite conformance | `extend T with HashKey {}` — unstated whether it checks/implies the supertrait chain | It checks the full chain (TD3 already collects it); auto-derived supertraits satisfy automatically, missing ones error at the declaration |
| Declaring conformance to a `structural trait` | Unstated | Allowed and harmless — it's documentation plus a signature check at the declaration instead of the use site |
| Trait evolution | Adding a required method breaks every conformer downstream | Non-normative note: adding a method **with a default body** (TD2) is non-breaking; without one is a major-version change |
| Conformance visibility | `min(trait, type)` inference — API surface changes with no syntax | Already bundled in #283 (`public extend`); resolve there |

## Cross-package conformance

Designed in #312; summary for completeness: core five never third-party (auto-derive already provides them), everything else freely retroactive, duplicates are a use-site error naming both packages. The ceremony lands only on the actual collision — same philosophy as this whole review.

---

## Ergonomics check: what the user writes, before and after

| Task | Ceremony |
|------|----------|
| Sort a struct, use it as Map key | nothing (auto-derive) |
| Print a struct | `{x:debug}` |
| Private helper over ad-hoc shapes | nothing (inferred) |
| Publish that helper | name the contract — a bound, or define the trait (IS2 assists) |
| Custom equality | one extend block; family stays consistent or errors (OC1) |
| Container conformance | one `where` clause (CC1) |
| Operator on a math type | write the methods (OP1) |
| Trait for someone else's type | one extend block; collision errors loudly (#312) |
| Two traits fighting over a name | wrapper type (MN3) — the one place ceremony lands |

Every row above the last two is the common case and stays at zero-or-one lines. The bottom rows are the special cases, opt-in and loud.

## Decision list

Needs sign-off, in dependency order:

1. **MN1–MN3** — single method namespace, conflict = error. Shapes how conformance blocks are compiled; decide before #314 is fixed.
2. **OC1–OC2** — override cancels dependent auto-derives. Small, prevents silent corruption.
3. **IS1–IS3** — inference seam edges. Affects #314's checker work and the LSP.
4. **CC1** — conditional conformance via `where`. Blocked on #313.
5. **OP1, TD1 fix, composite/structural/evolution notes** — spec edits, no compiler impact yet.

Then fold the accepted rules into generics.md / gradual-constraints.md and retire this file.
