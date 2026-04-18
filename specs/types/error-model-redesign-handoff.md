<!-- id: type.error-model-redesign -->
<!-- status: proposed -->
<!-- summary: Full error-model redesign handoff. T or E and T? become compiler-generated tagged unions with type-based branch disambiguation. No Ok/Err/Some/None constructors. Disjointness rule enforces T != E. Option cleanup proposal is subsumed. -->
<!-- depends: types/optionals.md, types/error-types.md, types/union-types.md -->

# Rask Error Model Redesign — Handoff

## Context

Rask currently uses `Result<T, E>` and `Option<T>` as standard enums with constructor sugar (`Ok(v)`, `Err(e)`, `Some(v)`, `None`) and operator sugar (`T or E`, `T?`, `??`, `?.`, `!`, `try`). Through three rounds of design discussion, we converged on a redesign that eliminates the constructor wrappers entirely. This document captures the final shape, the rationale for each move, and the spec-level questions still open.

## Final design (decided)

### Core model

- **`T or E` is a language-level sum type** (compiler-generated tagged union), not a user-definable enum. `Result<T, E>` as a named user-facing type is gone.
- **`T?` is a language-level nullable** (compiler-generated tagged union), not a user-definable enum. `Option<T>` as a named user-facing type is gone.
- **No constructor keywords or wrappers.** No `Ok`, `Err`, `Some`, `None`, `ok`, `err`, `some` keywords or constructors.
- **Type-based branch disambiguation.** `T or E` requires T and E to be distinct nominal types. The compiler picks the branch from the value's type at construction.
- **Universal auto-wrap.** Any value of type T or E auto-wraps into the corresponding `T or E` branch in any context where the target type is known (return, assignment, collection literal, struct field, function argument).

### Option surface (complete)

```rask
const user: User? = load_user()       // bare value, auto-wraps
const missing: User? = none           // absence literal

if user? { greet(user) }              // const → narrows user to User in block
if user? as c { c.sweep() }           // bind for mut, or to rename
if user == none { return }            // absent guard

user?.name ?? "Anonymous"             // chain + fallback
user!                                 // force (panic on none)
try user                              // propagate (current fn must return U?)
```

**Removed from Option:** `Some`/`None`, `some(x)` construction, `is some`, `is none`, `match` on Option (use operators), `T??` (nested optionals), all rebind forms (`is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind).

### Result surface (complete)

```rask
func divide(a, b) -> f64 or DivError {
    if b == 0: return DivError.ByZero    // type disambiguates → E branch
    return a / b                          // type disambiguates → T branch
}

const r = divide(a, b)

if r? { use(r) }                          // narrow to T (const)
if r? as v { use(v) }                     // explicit bind
if r? { use(r) } else as e { log(e) }     // bind error in else
if r is DivError as e { log(e); return }  // narrow-to-error pattern

// Operators
try r                                     // propagate (E ⊆ caller's E2 required)
r ?? 0.0                                  // value fallback on err
r ?? |e| fallback_from(e)                 // closure fallback (sees error)
r!                                        // force (panic on err)
r?.field                                  // chain (propagates err)

// Match — kept for multi-error unions
match r {
    v: f64 => use(v),
    e: IoError => handle_io(e),
    e: ParseError => handle_parse(e),
}
```

**Removed from Result:** `Ok`/`Err` constructors, `Result<T, E>` as a named type, `ok`/`err` keywords (production and pattern). All branch resolution is by value type, enforced by the disjointness rule.

### Const/mut interaction

The narrowing semantics ride on the existing `const`/`mut` distinction. **No flow-typing subsystem is added.**

- `const x: T?` — `if x?` narrows `x` to `T` in the block. Stable because `x` cannot be reassigned.
- `mut x: T?` — `if x?` does not narrow. User writes `if x? as v { use(v) }` to bind a const inner value.
- Same rule for `T or E`: `if r?` narrows for const, `if r? as v` binds for mut.

### Match policy

- **Option:** match removed. Two-state, operators suffice.
- **Result:** match kept, but only useful for multi-error unions (`T or (A | B | C)`). Two-branch `T or E` matches are written with operators.
- **Match arms use type-based patterns**: `v: T => …`, `e: SomeError => …`. No keyword wrappers.

### Disjointness rule

`T or E` requires T ≠ E. Enforced at:
- **Type formation** for concrete types (compile error)
- **Instantiation** for generics (compile error at the use site, not the definition)
- **Signature parse** for trivially-equal forms like `func id<T>(x: T) -> T or T`

The escape hatch is **newtypes**, not language syntax. `struct ParseError(i32)` lets you express what would have been `i32 or i32`. No special-case `err` keyword for the same-type case.

## Why each move (rejected alternatives)

For the new session: these were considered and rejected. Don't reintroduce without strong reason.

1. **`ok`/`err` as production keywords.** Rejected — disjointness makes them unnecessary. Rust baggage.
2. **`throw e` / `fail e`.** Rejected — exception baggage (`throw`, `raise`, `bail`, `fail` all carry implications of stack unwinding or fatal failure).
3. **Asymmetric auto-wrap (success only).** Initially considered. Replaced by symmetric type-based wrap once disjointness was added.
4. **Allowing `T or T`.** Rejected — disjointness is the move that eliminates `err`. Newtype workaround is idiomatic and self-documenting.
5. **Flow typing for mut bindings.** Rejected — `const`/`mut` distinction makes flow typing unnecessary. Mut narrowing requires explicit `as` bind.
6. **Type-theoretic union (with disjointness in the union sense).** This was a terminology error early in the design. `T or E` is a sum type with a runtime tag, not a type-theoretic union. The disjointness rule is for *branch disambiguation at construction*, not for type-theoretic union soundness.
7. **`is some` / `is ok` keywords.** Rejected — `some` and `ok` would be destructure-only keywords with no construction counterpart. Inconsistent.
8. **`x == none` and `is none` both available.** Kept `== none` as the absent guard form. Did not pursue `is none` separately.

## Open spec questions (must answer before ship)

These are not blockers for the design direction but are loadbearing for compiler behavior. Each one needs a one-paragraph answer in the spec:

**Q1. Define disjointness precisely.**
- Type aliases (`type Score = i32`): is `i32 or Score` legal? (Recommendation: no, same nominal type.)
- Empty types (`enum Never {}`): does `T or Never` collapse to T? (Recommendation: yes.)
- Generic instantiations: `Vec<i32>` vs `Vec<string>` — different types? (Recommendation: yes.)
- Newtypes: confirmed disjoint from their wrapped type.

**Q2. Auto-wrap scope.**
- Universal (return, assignment, literal, field, argument)? Recommendation: universal.
- Or positional (return only, like Rust)? Document the call.

**Q3. `r?` in expression position.**
- Forbidden outside `if`/`while` conditions?
- Or overloaded (returns `bool`, `T`, or propagates)?
- Recommendation: forbidden outside conditions. Use `try r` for propagation, `r!` for force, `if r?` for test+narrow.

**Q4. `??` with closure form.**
- Single operator overloaded on RHS type (`T` value vs `|E| -> T` closure)?
- Or two operators?
- Recommendation: single overloaded operator. Compiler picks based on RHS type.

**Q5. `try` cross-type rules.**
- `try r` (T or E) in fn returning `U or E2`: requires E ⊆ E2. (Already in current spec.)
- `try o` (T?) in fn returning `U?`: propagates none.
- `try o` (T?) in fn returning `U or E`: ill-typed (recommendation) or auto-converts?
- `try r` (T or E) in fn returning `U?`: ill-typed (recommendation) or drops error?
- Recommendation: cross-shape `try` is ill-typed; require explicit conversion.

**Q6. `r!` panic message.**
- Generic ("Result was error") — useless.
- Required `ErrorMessage` trait, structurally checked — recommended.
- Same trait applies to Option's `o!` (prints "None" or similar).

**Q7. Result ↔ Option conversion.**
- `r: T or E` → `T?`: drop error. Method `r.ok()` or operator?
- `o: T?` → `T or E`: needs an error. Verify `o ?? Error.NotFound` works under universal auto-wrap (Error.NotFound auto-wraps to E branch).
- Recommendation: explicit method `.ok()` for the lossy direction; `??` works for the lifting direction.

**Q8. `T or T` rejection timing.**
- Reject at signature parse for trivial cases (`func id<T>(x) -> T or T`).
- Reject at instantiation for generic cases (`map<i32, i32, i32>`).

## Migration scope

This is a breaking change. Affected:

**Spec files:**
- `specs/types/error-types.md` — full rewrite
- `specs/types/optionals.md` — full rewrite
- `specs/types/union-types.md` — extend disjointness rule from error position to general `T or E`
- `specs/types/gradual-constraints.md` — error union inference (GC7) needs update
- `specs/control/ensure.md` — try interaction
- `specs/SYNTAX.md` — new operator surface
- `specs/CORE_DESIGN.md` — error handling description
- `specs/GLOSSARY.md` — terminology
- `specs/rejected-features.md` — note the move
- `specs/canonical-patterns.md` — new patterns

**Stdlib:**
- `stdlib/result.rk` — likely deleted (combinators move to free functions or compiler builtins)
- `stdlib/option.rk` — same
- Every stdlib file using `Result`/`Option` — migrate

**Compiler:**
- `rask-parser` — universal auto-wrap, new pattern syntax
- `rask-types` — disjointness check at type formation and instantiation
- `rask-resolve` — operator forms
- `rask-mir` — lowering of new operators
- `rask-diagnostics` — migration error messages (catch `Some(v)`, `Ok(v)`, etc., suggest new form)

**Examples and tests:**
- All `.rk` files using `Result`/`Option`/`Some`/`None`/`Ok`/`Err`

**Tooling:**
- `rask fmt --migrate-errors` to mechanically convert old code

## Validation criteria

- Spec passes `rask test-specs`
- All `examples/*.rk` compile and run under new model
- `package_manager.rk` (1248 lines, the biggest example) migrates cleanly
- LSP works with new patterns
- Migration tool successfully converts existing Rask code

## Out of scope for this work

- Changes to `try`/`??` /`!`/`?.` operator behavior beyond what's needed to support the new model
- Changes to `ensure` / linear resource handling
- Custom user-defined "result-shaped" types (Rask deliberately doesn't have a `Try` trait; this stays)
- Effect tracking (`comp.effects`) — separate system

## Comparison to prior art (for the spec's "rationale" section)

| Language | Construction | Destructure | Notes |
|----------|--------------|-------------|-------|
| Rust | `Ok(v)` / `Err(e)` | `match`/`?` | Verbose at construction |
| Zig | `error.X` literal | `try`/`catch` | Closest sibling; uses `!` for union |
| Swift | `throw` | `try`/`catch` | Function coloring |
| Go | `return v, err` | `if err != nil` | Uniformly noisy |
| **Rask (new)** | bare value, type-disambiguated | `?`/`??`/`!`/`try`/match | Zig-class tightness, cleaner consumer ergonomics |

The honest framing: "Zig-class tightness with better consumer ergonomics, at the cost of disjointness on T and E."

## Recommended order of work

1. Answer Q1–Q8 in spec form
2. Write `specs/types/error-types.md` and `specs/types/optionals.md` (full rewrites)
3. Update CORE_DESIGN, SYNTAX, GLOSSARY
4. Implement compiler changes (parser → types → MIR)
5. Build migration tool
6. Migrate stdlib
7. Migrate examples
8. Update canonical-patterns and write the comparison page

## What "done" looks like

A new Rask user reading the error-handling section sees: type-based wrap, four operators, no constructors. They can write a fallible function in three lines without learning any wrapper types. They can read existing code and understand it without consulting an enum-variant reference. The model fits on one page.

## Reconciliation with the Option cleanup proposal

The Option cleanup proposal (`option-cleanup-proposal.md`) was drafted before the Result side was in scope. Several of its detail decisions need revisiting once Q1–Q8 are answered:

**R1 — `x?` in expression position.** Option proposal says `x?` is "a plain boolean expression." Handoff Q3 recommends forbidding `x?` outside `if`/`while` conditions. **If Q3 lands as recommended, amend the Option proposal: `x?` is a branching-construct form, not a standalone bool. Use `x != none` for a bool expression.**

**R2 — Auto-wrap scope.** Option proposal keeps OPT8 (return/assignment coercion). Handoff Q2 proposes universal auto-wrap (also literals, fields, arguments). **If universal, Option's OPT8 expands correspondingly.** No surface change at call sites — just more positions where the coercion fires.

**R3 — `??` closure form.** Handoff Q4 adds `r ?? |e| fallback(e)` for Result. Option has no error value to pass to a closure. **Decide: does `x ?? |_| fallback()` exist for Option, or is the closure form Result-only?** Recommendation: Result-only. Option's fallback is already value-only; a closure without an argument adds nothing.

**R4 — `!` panic message.** Option proposal keeps the literal-message sugar (`x! "msg"`, with interpolation). Handoff Q6 proposes a structural `ErrorMessage` trait as the default. **If the trait lands, both Option and Result get a generic default message ("none" for Option, trait-driven for Result). The literal-message sugar likely stays as an override.** Spec both.

**R5 — Symmetric narrow + early-exit.** Option proposal specs both-branch narrowing and early-exit fall-through narrowing. Handoff says "same rule for T or E" for const/mut narrowing. **Extend the symmetric-narrow and early-exit-narrow rules to Result explicitly in the Result spec.** Not a change to the Option proposal, just cross-application.

**R6 — Match diagnostic scope.** Option proposal specs a first-class "cannot match on Option" diagnostic. Result keeps match. **The diagnostic fires only when the scrutinee is `T?`, not `T or E`.** Already implicit; worth stating in the diagnostic spec.

**R7 — `try` cross-shape.** Handoff Q5 recommends cross-shape `try` (Option-in-Result-returning fn, or vice versa) is ill-typed. Option proposal inherits today's OPT13. **Either way, spec must state cross-shape explicitly so the error message is specific, not a generic type mismatch.**

Net: the Option proposal's final surface stands, but `x?`-as-bool, auto-wrap scope, closure-`??`, and `!`-message need the handoff's answers before the Option spec is rewritten. The narrowing rules (symmetric, early-exit, const-rides-narrow) carry over to Result unchanged.

## See Also

- [Option Cleanup Proposal](option-cleanup-proposal.md) — detailed Option narrowing rules and migration diagnostic (subsumed by this handoff)
- [Optionals](optionals.md) — current Option spec (to be rewritten)
- [Error Types](error-types.md) — current Result spec (to be rewritten)
- [Union Types](union-types.md) — disjointness rule extension target
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
