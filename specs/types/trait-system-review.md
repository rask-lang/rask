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

**Rules (accepted):**

| Rule | Description |
|------|-------------|
| **OC1: Override cancels dependents** | Overriding `Equal` cancels auto-derived `Hashable` and `Comparable` for that type. Overriding `Hashable` alone is safe (hashing fewer fields than eq compares only costs collisions, never correctness) and cancels nothing |
| **OC2: Loud, with the fix** | Using a cancelled conformance is a compile error at the use site: "Username overrides Equal, so Hashable is no longer auto-derived — declare `extend Username with Hashable` consistent with your eq" |
| **OC3: Canonical order only** | The OC error for `Comparable` should steer one-off orderings ("sort by salary") to `sort_by` — `Comparable` is the type's one canonical order, not a per-call-site choice |

Common case (no override) unaffected. The rare case gets an error instead of a haunted Map.

## Finding 2: Method-name collisions across traits have no rule

**Scenario 10.** Under structural matching this was a non-question — one `greet` method satisfied every greet-shaped trait. Under nominal, conformance blocks can carry bodies, so two traits can in principle demand two different `greet`s. There is no qualified-call syntax to disambiguate, and nothing says whether a method defined inside `extend T with Trait { }` joins T's ordinary method namespace.

**Rules (accepted, with opt-in scoping):** single namespace by default; collisions opt into scoping at the site of the collision.

| Rule | Description |
|------|-------------|
| **MN1: Single namespace** | Methods defined in `extend T with Trait { }` are ordinary methods of T, same namespace as plain `extend T` blocks |
| **MN2: Shared implementation** | Two conformances requiring the same method name share the one implementation — legal iff both signatures match it |
| **MN3: Conflict needs scoping** | If the signatures disagree, the second conformance declaration is a compile error naming both traits — unless it is declared `scoped` |
| **MN4: Scoped conformance** | `scoped extend T with Trait { ... }` — methods in a scoped conformance do not enter T's inherent namespace. Reachable through trait dispatch (generic bounds, `any Trait`) and trait-qualified calls |
| **MN5: Trait-qualified call** | `Trait.method(value, args)` — mirrors the existing `Type.method()` static-call syntax. Legal for any conformance, needed only for scoped ones |

<!-- test: skip -->
```rask
extend Dog with Greeter {
    func greet(self) -> string { ... }            // ordinary method: dog.greet()
}

scoped extend Dog with Announcer {
    func greet(self, volume: i32) -> string { ... }  // trait-only
}

dog.greet()                 // Greeter's — the inherent one
Announcer.greet(dog, 5)     // Announcer's — qualified
```

Common case: nothing to learn, `dog.greet()` works even when `greet` was defined inside a conformance block. Collision case: one keyword, at the declaration where the special case lives — the scoping is visible in source. (Exact spelling of `scoped` is bikesheddable; the prefix position parallels the planned `public extend` from #283.)

## Finding 3: Generic containers can't conform conditionally

**Scenario 9.** `Stack<T>` is Displayable only when `T` is. Auto-derive handles this for the core five (a `Vec<T>` of Cloneable is Cloneable), which hides the gap: user traits on user generics have no rule at all. Structural matching used to give this for free — the shape either compiled per-instantiation or didn't.

**Rules (accepted, with inferred clauses):** conditional conformance exists, and the condition is inferred — writing it out is for public API only. Precedent is already in the language twice: auto-derive's conditionality ("Vec of Cloneable is Cloneable") has always been implicit, and gradual constraints already infer bounds from bodies.

| Rule | Description |
|------|-------------|
| **CC1: Conditional conformance** | Conformance on a generic type holds exactly for instantiations satisfying its condition, checked at monomorphization like every other bound (G2/G6) |
| **CC2: Condition inferred** | Package-private conformances omit the clause; the compiler derives it from the conformance body (same machinery and same local-only scope as gradual constraints, GC6). IDE ghosts the inferred clause |
| **CC3: Public states it** | Public conformances (`public extend`, #283) declare the clause explicitly with `where` — same rule as public function signatures (GC5) |

<!-- test: skip -->
```rask
// Package-private: zero boilerplate — clause inferred as `where T: Displayable`
extend Ring<T> with Displayable {
    func to_string(self) -> string {
        return self.items.map(|x| x.to_string()).join(", ")
    }
}

// Public library API: the contract is spelled out
public extend Ring<T> with Displayable where T: Displayable { ... }
```

CC3 is blocked on `where` parsing (#313) and `public extend` (#283). No global analysis: instantiation-site checking, same as today's bounds.

## Finding 4: The inference seam needs its edges specced

**Scenarios 4→5, the prototype-to-production pipeline.** Gradual constraints deliberately keep private inference structural (GC3 collects method-requirements, not trait names). That's the right call — prototyping glue, invisible in APIs. But the promotion step got three unexamined edges:

1. **Bound propagation is mixed, not structural.** A private inferred function that passes its parameter to `sort<T: Comparable>` must carry the *nominal* bound onward — post-flip, a `compare` method no longer implies `Comparable`. GC3 as written only collects method-requirements.
2. **Promotion can hit a wall.** "Make public" must name traits. If the body's method-requirements match no trait, there is nothing to name — the honest fix is defining a trait, and the tooling should say so instead of pretending a signature exists. If they match *two* traits, promotion is ambiguous.
3. **The GC5 error message oversells.** It displays an inferred signature with a named trait (`<T: Validatable>`) recovered from shape — the exact resolution the flip removed. Display must be honest about which case it's in.

**Rules (accepted):**

| Rule | Description |
|------|-------------|
| **IS1: Mixed inference** | Inferred bounds are the union of (a) nominal bounds propagated from called functions and (b) structural method-requirements from direct calls. A nominal bound subsumes the methods it provides |
| **IS2: Promotion is exact** | "Make public"/"make explicit" fills in a named trait only when exactly one visible trait covers the residual method-requirements. Zero matches: report the methods and offer to generate a trait definition. Two+: list candidates, user picks |
| **IS3: Honest ghost text** | Inferred-signature display distinguishes propagated nominal bounds from raw method-requirements (`T: Comparable` vs `T: {frobnicate}`) |

Also worth one line in gradual-constraints.md's gotcha section: annotating a working private function can make a working call fail, because the bound's meaning flips from shape to declaration. That's by design — say it out loud.

### Prototyping with traits: the dial already exists

Two clarifications that came out of reviewing this seam, worth putting in the spec's guidance:

**Traits belong to the structuring/publishing phase, not the sketching phase.** Prototype code doesn't need them — private inference carries shapes, and that is unchanged. The promotion wall is not a defect in the prototype workflow; naming the contract *is* the publish step. The tooling's job (IS2) is to make naming it one action.

**When a trait is wanted during prototyping, `duck trait` is the prototype mode** (keyword decided below). Declare the trait duck while sketching: zero conformance declarations, methods move freely between types, nothing to keep in sync. To harden it, delete the `duck` keyword — the compiler knows every type currently matching by shape, so it lists them and a quick-fix inserts the `extend T with Trait {}` declarations mechanically. This is the same migration #283 describes for the global flip, available per-trait, permanently:

| Phase | What you write | What conformance costs |
|-------|----------------|------------------------|
| Sketching | `duck trait Frobber { ... }` | Nothing — shape matches |
| Hardening | delete `duck` | Accept the generated `extend ... with` lines |
| Published | nominal trait | One declaration per new conforming type |

Prototype-to-production for traits is: delete one word, accept the quick-fixes. Cheap to move things around while coding; explicit exactly when it becomes API.

### Renaming the `structural` keyword — decided: `duck trait`

`structural` is type-theory jargon. The replacement is `duck trait` — the established name for exactly this semantics (duck typing), pre-taught to the Python-first audience. The register is deliberate: the keyword *reading as unserious is the signal*. A `duck trait` in a diff announces "this contract is loose by design" — prototype-mode made visible in source, and lintable (`rask lint` warns on duck traits outside prototype contexts).

**Consequence (ruled): the stdlib ships zero duck traits.** `Reader`, `Writer`, and `ErrorMessage` go nominal; `duck` is purely the prototyping dial. The structural carve-out for them entered in commit 27c65f4 as implementation detail of the #283 migration, never as its own decision, and its stated rationale (ER6: "a conformance line on every error enum would tax the most common trait") is arithmetically wrong — `message()` is hand-written in an extend block regardless, so conformance is a header edit (`extend ConfigError with ErrorMessage { ... }`), zero marginal lines. Multi-trait types stay flat via CD1/CD2 (one block, header lists the claims). Retroactive conformance for third-party types is one line, priced by #312. Fold-in rewrites ER4/ER6 and the G1 rationale accordingly.

The candidate analysis, for the record:

| Candidate | Verdict |
|-----------|---------|
| `duck trait` | **Chosen.** The only candidate needing zero explanation; the unserious register doubles as the prototype-mode signal |
| `implied trait` | Runner-up — dignified literal pairing with *declared*; lost because dignity was the wrong goal: the keyword should mark looseness, not launder it |
| `matching trait` | Names the action, conjugates well in diagnostics — but teaches nothing by itself |
| `shape trait` | Names the criterion; same tier as `matching` |
| `automatching trait` | Fixes the auto-derive collision by fusing, but seven syllables next to `const`/`func`/`mut` |
| `auto trait` | Rejected: collides with **auto-derive** inside Rask itself (the core five are auto-derived *nominal* traits — "auto-derived but not auto" is a confusion factory), plus the unrelated Rust meaning |
| `lazy trait` | Rejected: "lazy" means deferred work everywhere else; nothing is deferred here |
| `magic trait` | Rejected: too cute, and the one word a transparency-first language can't use |
| `open trait` | Rejected: most overloaded word in PL (Kotlin `open`, open/closed principle, open unions) — invites confident misreading |
| `structural trait` | Retires to prose — docs say "known elsewhere as structural typing" for searchability |

## Finding 5: Operators stayed structural — keep them, but on purpose

**Scenario 7.** G4 expands `a + b` to `a.add(b)` and checks only that the method exists. The flip's rationale ("existing isn't a semantic claim") seems to apply — but doesn't, and the reason is worth recording:

On **concrete** types, `a + b` resolves to an `add` method someone deliberately wrote on that type. There is no accidental-conformance risk — nothing is being *matched*, just called. The risk the flip addressed only exists where code accepts *unknown* types against a contract — and that path is already nominal: `func sum<T: Numeric>` requires declared conformance.

**Rule (accepted — status quo, now with its rationale on record):**

| Rule | Description |
|------|-------------|
| **OP1: Concrete operators are authored sugar** | Operator expansion on concrete types is method-call sugar, no conformance involved. Generic operator use goes through nominal bounds (`Numeric`, `Comparable`) like any other generic call |

No fine-grained `Add`/`Sub`/`Mul` trait zoo. A math type defines the methods it supports and gets exactly those operators.

## Finding 6: Small inconsistencies (all accepted)

| Item | Problem | Fix |
|------|---------|-----|
| TD1 trait visibility | "Public by default" contradicts the language-wide package-private default (`struct.modules/V1`) | Traits default package-visible, `public trait` exports — same as everything else |
| Composite conformance | `extend T with HashKey {}` — unstated whether it checks/implies the supertrait chain | It checks the full chain (TD3 already collects it); auto-derived supertraits satisfy automatically, missing ones error at the declaration |
| Declaring conformance to a `structural trait` | Unstated | Allowed and harmless — it's documentation plus a signature check at the declaration instead of the use site |
| Trait evolution | Adding a required method breaks every conformer downstream | Non-normative note: adding a method **with a default body** (TD2) is non-breaking; without one is a major-version change |
| Conformance visibility | `min(trait, type)` inference — API surface changes with no syntax | Already bundled in #283 (`public extend`); resolve there |

## Trim: several conformances, one declaration

A type that satisfies several traits with methods it already has needs one line per trait today. The language already has a list form for exactly this on nominal type declarations — `type UserId = u64 with (Equal, Hashable, Debug)` — so mirror it:

| Rule | Description |
|------|-------------|
| **CD1: Conformance list** | `extend T with A, B, C { ... }` declares all listed conformances. Each trait's signature check runs independently against the block plus the type's existing methods. Composes with modifiers: `public extend`/`scoped extend` apply to every listed trait |
| **CD2: Block body unrestricted** | The block may mix methods for any of the listed traits and ordinary non-trait methods. The conformance list is a header on a normal extend block, not a per-trait container |
| **CD3: One condition per block** | On generic types, inferred conditions (CC2) are computed per listed trait independently. An explicit `where` clause (public, CC3) applies to the whole block — traits needing different conditions split into separate blocks |

<!-- test: skip -->
```rask
extend Ring<T> with Countable, Sizable {}       // two claims, one line

// The common shape for a trait-rich type: ONE block, header carries the claims
extend LogSource with Reader, Displayable, ErrorMessage {
    func read(mutate self, buf: Buffer) -> usize or IoError { ... }
    func to_string(self) -> string { ... }
    func message(self) -> string { ... }
    func rewind(mutate self) { ... }            // plain method, same block
}
```

Without CD2 this would be Rust's shape — one impl block per trait, stacked on every type. With it, conformance costs a header on the block you were writing anyway. Declaring conformance inline on the `struct` itself was considered and rejected — struct bodies stay pure data layout (`type.structs`).

## Cross-package conformance

Designed in #312; summary for completeness: core five never third-party (auto-derive already provides them), everything else freely retroactive, duplicates are a use-site error naming both packages. The ceremony lands only on the actual collision — same philosophy as this whole review.

---

## The auto-derive roster: corpus survey

Question: the core five are Equal, Hashable, Comparable, Cloneable, Default — should the set grow? Surveyed the repo's 105 `.rk` files (examples, test suite, stdlib) for trait-need signals. (Note the full auto-derive roster is already eight: Debug for all types, Encode/Decode markers. "Core five" names the invariant-carrying #312 carve-out family.)

| Signal | Count | Verdict |
|---|---|---|
| Hand-written `message()` impls | 48 in 23 files | **Largest ceremony source in the language** |
| `.clone()` sites | 32 | Cloneable earns its seat |
| `Map.` usage | 23 | Hashable/Equal earn theirs |
| `spawn` sites | 52 | Sendability: compiler property, zero declarations ever needed — not a trait |
| parse-from-string | 29 | Not derivable (format is a choice) — not core |
| `to_string` impls | 6 | Displayable opt-in confirmed correct |
| `.default()` | 0 | Default is the weakest member (see below) |
| User-declared traits | 14, all domain | No missing core trait |

**Proposed addition (the only one): auto-derive `ErrorMessage` for enums.** The sampled `message()` bodies are mechanical — match over variants, `"invalid nesting: {ctx}"`, wrappers delegating to `inner.message()`. Derivation: humanized variant name + payload interpolation; single-payload variants whose payload is itself ErrorMessage delegate. Overridable like EQ2; lint nudges public error types toward hand-written prose. Kills ~one impl per two files of ceremony, and keeps ErrorMessage nominal with compiler-provided conformance — consistent with the "stdlib ships zero duck traits" ruling at zero added cost.

**Default needs a rebase, not removal.** Zero corpus usage, and DF4's universal zeros ("0 for ints, false for bool") are Go zero-values by another name. Once #311 (struct field defaults) lands, re-derive: a struct is Default iff every field has a *declared* or derivable default — from your stated defaults, not universal zeros. Fold into #311.

Rejected after survey: `Copy` (16-byte threshold, not a trait), `Sendable` (compiler-checked property), `Parseable` (not derivable), `Iterator` (retired by Sequence protocol), `Displayable` promotion (user-facing strings are intentional).

### Cross-check against Rust's trait traffic

Rust's most-derived and most-implemented traits, mapped: `Debug`/`Clone`/`Eq`/`Hash`/`Ord`/`Default`/serde → all auto-derived here (and the `Partial*` splits collapse — they were float-driven; HA4/CO4 exclude floats instead of doubling every trait). Two findings with teeth:

- **thiserror validates the ErrorMessage proposal.** One of Rust's most popular crates exists solely to derive error messages for enums — the ecosystem already voted for this feature. The corpus survey (48 mechanical impls) and Rust's dependency graph point at the same gap independently.
- **`From`/`Into` — Rust's most hand-written trait — stays out, on record.** Its three jobs are dissolved at the language level: error conversion for `?` (Rask `try` widens error *unions* structurally — the `impl From<LibError> for MyError` ceremony class never exists), flexible string params (one `string` type, no `String`/`&str`/`Cow` to abstract), general conversion (residue covered by opt-in `Convert<From, To>`). Rust immigrants will ask; this is the answer.

Deliberate absences confirmed against Rust's remaining heavy hitters: `Deref` (no autoderef — boxes use `with`), `AsRef`/`Borrow` (no reference-flavor zoo), `Drop` (`ensure`/`@resource`), `Send`/`Sync` (compiler property). The Rust data surfaces no new core candidate beyond ErrorMessage — most of Rust's trait traffic compensates for design choices Rask didn't make.

## Ergonomics check: what the user writes, before and after

| Task | Ceremony |
|------|----------|
| Sort a struct, use it as Map key | nothing (auto-derive) |
| Print a struct | `{x:debug}` |
| Private helper over ad-hoc shapes | nothing (inferred) |
| Publish that helper | name the contract — a bound, or define the trait (IS2 assists) |
| Custom equality | one extend block; family stays consistent or errors (OC1) |
| Container conformance | nothing — condition inferred (CC2); public API spells it out (CC3) |
| Prototype a trait | `structural trait`; harden by deleting the keyword + accepting generated declarations |
| Operator on a math type | write the methods (OP1) |
| Trait for someone else's type | one extend block; collision errors loudly (#312) |
| Two traits fighting over a name | one `scoped` keyword on the second conformance (MN4) |

Every row is zero-or-one lines in the common case; the special cases are opt-in, and each opt-in is visible at the declaration that needs it.

## Status

All findings ruled on. Accepted: **MN1–MN5** (single namespace, `scoped` opt-in for collisions, trait-qualified calls), **OC1–OC3** (override cancels dependents, hard error), **IS1–IS3** (mixed inference, exact promotion with generate-trait assist, honest ghost text) plus the structural-as-prototype-dial guidance, **CC1–CC3** (conditional conformance, condition inferred, public states it), **OP1** (concrete operators are authored sugar), and the Finding 6 fixes.

Also accepted: **CD1–CD3** (comma-list conformance declarations, unrestricted block bodies, one condition per block).

Proposed, awaiting ruling: **auto-derived `ErrorMessage` for enums** and the **Default-from-field-defaults rebase** (corpus survey above).

Remaining open details (bikeshed-level, decide during spec fold-in):
- ~~Renaming `structural`~~ — **decided: `duck trait`**, and the stdlib ships zero of them: `Reader`/`Writer`/`ErrorMessage` go nominal (ER4/ER6 rewrite at fold-in). CD2 keeps multi-trait types at one block.
- Exact spelling of the `scoped` modifier (keyword prefix vs `@`-attribute).
- Whether IS2's generate-a-trait action lives in the compiler diagnostic or LSP-only.
- CC3 wording depends on #283's final `public extend` syntax.

Implementation order: MN and IS shape the #314 checker fix and should land in the specs first; CC is blocked on #313 (`where` parsing) and #283 (`public extend`); OC and the Finding 6 items are independent.

Next step: fold the accepted rules into generics.md / gradual-constraints.md / traits.md and retire this file.
