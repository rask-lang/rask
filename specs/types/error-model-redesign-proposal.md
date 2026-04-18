<!-- id: type.error-model-redesign -->
<!-- status: proposed -->
<!-- summary: Full error-model redesign. T or E and T? become compiler-generated tagged unions with type-based branch disambiguation. No Ok/Err/Some/None constructors. Disjointness rule (T != E) enforces unambiguous construction. Operator family covers both shapes; match is for multi-branch unions only. Narrowing rides on the const/mut distinction — no flow typing. -->
<!-- depends: types/optionals.md, types/error-types.md, types/union-types.md -->

# Rask Error Model Redesign

Rask currently uses `Result<T, E>` and `Option<T>` as standard enums with constructor sugar (`Ok(v)`, `Err(e)`, `Some(v)`, `None`) and operator sugar (`T or E`, `T?`, `??`, `?.`, `!`, `try`). The constructors are Rust-legacy ceremony on top of sugar that already does the work. This proposal removes the constructor wrappers entirely, collapses both shapes onto a single operator family, and lets narrowing ride on the existing `const`/`mut` distinction instead of introducing flow typing.

## Problems with the current design

**P1 — Constructor ceremony.** `Some(x)` and `Ok(x)` add a tag that is always the same tag. Auto-wrap (OPT8, ER7) already makes `T` coerce at function boundaries; the wrapper survives only at intermediate construction sites.

**P2 — Five rebind forms for one operation.** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind. Each says "check present and name the value" with slightly different rules.

**P3 — Magic rebind is invisible.** `if x is Some { use(x) }` silently rewrites `x`'s type with no syntactic marker.

**P4 — "Invent a new name" is noise.** `if x is Some(u) { use(u) }` forces a rename when `x` already describes the thing.

**P5 — "Option is just an enum" is a lie.** Option has more dedicated surface than any other type (sugar, auto-wrap, propagation, linear propagation, sentinel layout). Treating it as an enum forces duplication between pattern machinery and operator surface.

**P6 — Result carries the same duplication.** `Ok`/`Err` constructors for a sum type whose branch is already determined by the payload's type; `match` on Result for what is almost always a two-branch split already covered by operators.

## Final design

### Core model

- **`T or E` is a language-level sum type** (compiler-generated tagged union), not a user-definable enum. `Result<T, E>` as a named user-facing type is gone.
- **`T?` is a language-level nullable** (compiler-generated tagged union), not a user-definable enum. `Option<T>` as a named user-facing type is gone.
- **No constructor keywords or wrappers.** No `Ok`, `Err`, `Some`, `None`, `ok`, `err`, `some` keywords or constructors anywhere.
- **Type-based branch disambiguation.** `T or E` requires T and E to be distinct nominal types. The compiler picks the branch from the value's type at construction.
- **Universal auto-wrap.** Any value of type T or E auto-wraps into the corresponding `T or E` branch in any context where the target type is known (return, assignment, collection literal, struct field, function argument).

### Option surface

```rask
const user: User? = load_user()       // bare value, auto-wraps
const missing: User? = none           // absence literal

if user? { greet(user) }              // const → narrows user to User
if user? as c { c.sweep() }           // bind for mut, or to rename
if user == none { return }            // absent guard

user?.name ?? "Anonymous"             // chain + fallback
user!                                 // force (panic on none)
try user                              // propagate (current fn must return U?)
```

| Need | Syntax |
|------|--------|
| Type | `T?` |
| Construct present | bare value (auto-wrap) |
| Absent literal | `none` |
| Present check + narrow (const) | `if x? { use(x) }` |
| Present + destructure bind (any) | `if x? as v { use(v) }` |
| Absent check | `if x == none { … }` |
| Early-exit narrow | `if x == none { return } … use(x)` (x: T after) |
| Chain | `x?.field` |
| Fallback value | `x ?? default` |
| Diverging fallback | `x ?? return none` / `?? break` / `?? panic("…")` |
| Force | `x!` |
| Propagate | `try x` / `try { … }` |

No `some` keyword, no `is some`, no `match` arm for `none`, no Option-specific pattern.

### Result surface

```rask
func divide(a: f64, b: f64) -> f64 or DivError {
    if b == 0 { return DivError.ByZero }  // type → E branch
    return a / b                           // type → T branch
}

const r = divide(a, b)

if r? { use(r) }                           // const → narrows to T
if r? as v { use(v) }                      // explicit bind (mut or rename)
if r? { use(r) } else as e { log(e) }      // bind error in else branch
if r is DivError as e { log(e); return }   // narrow-to-error via type pattern

try r                                      // propagate (E ⊆ caller's E2)
r ?? 0.0                                   // value fallback
r ?? |e| fallback_from(e)                  // closure fallback (sees error)
r!                                         // force (panic on err)
r?.field                                   // chain (propagates err)

// match kept for multi-error unions
match r {
    f64 => use(r),                    // r: f64 in this arm (const narrow)
    IoError => log(r),                // r: IoError in this arm
    ParseError as e => handle(e),     // optional rename with `as`
}
```

Match arms dispatch on type and narrow the scrutinee in each arm. No forced rename — `r` just narrows to the arm's type, the same way `if r? { use(r) }` narrows. Use `Type as name` when a fresh name reads better or when the scrutinee is `mut`.

### Methods

The operator family covers most cases. A small set of combinators stays as methods because they compose in ways operators can't (they preserve the wrapper type for chaining; operators always extract or panic).

**Option `T?`** — three methods:
- `.map(f: |T| -> U) -> U?` — transform present without unwrapping
- `.filter(pred: |T| -> bool) -> T?` — keep if predicate holds
- `.and_then(f: |T| -> U?) -> U?` — chain Option-returning operations

**Result `T or E`** — four methods:
- `.map(f: |T| -> U) -> U or E` — transform success
- `.map_err(f: |E| -> E2) -> T or E2` — translate error
- `.and_then(f: |T| -> U or E) -> U or E` — chain Result-returning
- `.ok() -> T?` — drop error, lift to Option

Seven methods total. Compiler-provided on the builtin types — no `impl` blocks for users to discover or replicate.

**Cut from today's surface:**

| Method | Replacement |
|--------|-------------|
| `.is_some()` / `.is_none()` | `x?` / `x == none` |
| `.is_ok()` / `.is_err()` | `r?` / `r is E` |
| `.unwrap()` | `x!` / `r!` |
| `.unwrap_or(default)` | `x ?? default` |
| `.unwrap_or_else(f)` | `r ?? \|e\| f(e)` |
| `.to_result(err)` | `o ?? err` (auto-wrap handles the E branch) |
| `.to_option()` | `.ok()` (single survivor) |
| `.or(other)` | `x ?? other` already returns `T?` |
| `.or_else(f)` | `match` or chain |

Each removed method either duplicated an operator or can be reconstructed trivially. The retained seven are precisely the ones that keep a value in wrapper-land for the next chain step.

### Narrowing rides on `const`

All the usual flow-typing complications — mutation, intervening calls, closure capture, field paths — collapse into one structural fact the language already enforces:

**`const` bindings cannot be reassigned. Narrowing works on them for free. `mut` bindings require `if x? as v` to get a stable binding.**

| Scrutinee | `if x? { … }` | `if x? as v { … }` | `if x == none { return } …` |
|-----------|----------------|---------------------|------------------------------|
| `const x` | narrows `x` in both branches | binds `v`; also narrows `x` | narrows `x` after the guard |
| `mut x` | predicate legal, no narrowing | binds `v`; `x` unchanged | no narrowing |

Same rule for `T or E`.

**Both branches narrow symmetrically.** When the condition is a recognised predicate over a const scrutinee, the then-branch narrows to the positive variant and the else-branch to the negative. For Option, `x?`, `!x?`, `x == none`, `x != none` all narrow equivalently. For Result, `r?`, `!r?`, `r is E` all narrow equivalently. Compound predicates (`&&`, `||`) do **not** narrow — use nested `if` or `as v` bind.

**Early-exit narrows the fall-through.** If a branch diverges (`return`, `break`, `continue`, `panic`, `loop { … }`), the code after the `if` is narrowed as if the other branch had run.

**Field paths narrow iff the full path is rooted in a `const` binding.** `player.weapon` narrows if `player` is `const`. If `player` is `mut`, use `if player.weapon? as w` to bind.

### Why no `match` on Option

Match earns its keep on types with multiple shapes, guards, complex destructure, or non-trivial exhaustiveness. Option has two states — everything match does factors through operators, usually shorter:

| Match form | Operator form |
|------------|---------------|
| `match x { none => a, v => f(v) }` | `if x? { f(x) } else { a }` |
| `match x { none => default, u => u.name }` | `x?.name ?? default` |
| `match x { none => return, v => v }` | `x ?? return none` (or `try x`) |
| `match x { none => panic("…"), v => v }` | `x!` (or `x ?? panic("…")`) |

Match on `T or E` is kept because multi-error unions (`T or (A | B | C)`) genuinely need multi-arm dispatch. Two-branch `T or E` matches are still written with operators.

### Naming: `:` vs `as`

`:` annotates in declarations. `as` renames in usage positions. They never compete for the same job.

| Position | Operator | Example |
|----------|----------|---------|
| Declaration (binding with type) | `:` | `const x: i64 = 1`, `func f(x: i64)`, `struct P { x: i64 }` |
| Cast | `as` | `x as i64` |
| Narrow with rename | `as` | `if x? as v { … }` |
| Branch rename | `as` | `if r? { … } else as e { … }` |
| Type-pattern narrow with rename | `as` | `if r is DivError as e { … }` |
| Match arm rename | `as` | `match r { Type as name => … }` |

Anywhere you introduce a name for an existing value, `as` is the operator. Anywhere you annotate a declaration with a type, `:` is the operator. Match arms without `as` simply narrow the scrutinee in place — no rename is forced.

### Disjointness rule

`T or E` requires T ≠ E. Enforced at:

- **Type formation** for concrete types (compile error)
- **Instantiation** for generics (compile error at the use site, not the definition)
- **Signature parse** for trivially-equal forms like `func id<T>(x: T) -> T or T`

The escape hatch is **newtypes**, not language syntax. `struct ParseError(i32)` lets you express what would have been `i32 or i32`. No special-case `err` keyword for the same-type case.

### Migration diagnostic

The biggest ergonomic cliff is a user typing `match user { Some(u) => …, None => … }` from Rust or old-Rask habit. The diagnostic must be first-class:

```
ERROR [type.error-model/NO_MATCH_OPTION]: Option cannot be matched
   |
5  |  match user { Some(u) => …, None => … }
   |  ^^^^^ Option is a builtin status type, not an enum

WHY: Option has two states — present and absent — and the ?-family
covers both more concisely than a match.

FIX: use operators instead:

  if user? { … } else { … }                 // branching
  if user? as u { use(u) } else { default() }  // with a fresh name
  user?.name ?? "Anonymous"                  // chained + fallback
  if user == none { return }; greet(user)    // early exit

Use match for enums with three or more branches, or multi-error unions.
```

Analogous diagnostics fire for `Some(v)`, `Ok(v)`, `Err(e)`, `None`.

## Details and edge cases

**Linear resources.** OPT11 stays: if `T` is linear, `T?` is linear, and `T or E` is linear if either branch is. Operators must consume the resource exactly once.

- `if x? as v { consume(v) }` — `v` binds the payload; the linear resource moves into `v` at the bind site. `x` is no longer usable in the block (standard move semantics).
- `if x? { consume(x) }` — same, except `x` is consumed directly. After the block, `x` is consumed regardless of branch taken (the `none` branch has no resource to consume).
- `x ?? default` — consumes whichever branch evaluates. Both paths produce exactly one `T`.
- `try x` — consumes `x` by moving the payload into the current function's flow (success path) or returning `x` to the caller (absent path).
- `x?.field` — **not supported on linear `T?`**. Projecting a field can't partially move out of `T`. Use `if x? as v { … v.field … }`.
- `x!` — moves payload on success; panics on absence.

**`else as` is Result-only.** `if r? { … } else as e { log(e) }` binds the error value. Option has no error to name in the else branch — `else as n { … }` would bind "none" which has no payload. Only `T or E` supports `else as`.

**Error bound on `E`.** `T or E` accepts any nominal `E` — numeric status codes (`f64 or i32`), enums, structs, all legal. Operators that print error information (`r!` without a literal message) require `E` to implement `ErrorMessage` (Q6). `try r`, `match r`, `r ?? default`, and the methods impose no trait bounds beyond disjointness.

**Anonymous expressions don't narrow.** The narrowing rule applies to const bindings. `if compute()? { use(compute()) }` calls `compute()` twice and does not narrow either call. Use `const v = compute()` first, then `if v? { use(v) }`, or use `if compute()? as v { use(v) }` to bind at the check site.

**Nesting is shape-specific.** `T??` and `(T or E) or E` are forbidden (same-shape nesting is ambiguous). All cross-shape nesting is fine:
- `(T?) or E` — a Result whose success is an Option. Distinct compiler-generated type.
- `T or (U?)` — Result with Option error side.
- `(T or E)?` — Option holding a Result.

**`??` chaining.** Works while the left side remains wrapped:
```rask
const x: T? = a ?? b ?? c              // ok if a, b are T?; c is T or T?
const r: T or E = a ?? b ?? handle_e   // ok if a, b are T or E
```
As soon as an RHS is bare `T`, the chain collapses to `T` and further `??` is a type error.

**Match pattern families.** `match` accepts two pattern styles depending on the scrutinee:
- **Type patterns** for `T or E`: `f64 => …`, `IoError as e => …`
- **Variant patterns** for user enums: `Token.Plus => …`, `Token.Number(n) => …`, `Token.Ident as t => …`
Both narrow the scrutinee in the arm. Wildcard `_ => …` is available in either style.

**Exhaustiveness.** `match r` on `T or E` must cover every branch. For a widened error side (`T or (A | B | C)`), each error variant is its own arm, or `_ => …` catches the rest. Compiler diagnoses missing arms with the same error used for user enums.

**Shadowing works normally inside narrowed blocks.** `if x? { const x = upgrade(x); use(x) }` — the outer `x` narrows to `T`, the inner `const x` shadows with a new binding. Standard scoping rules; the narrow doesn't prevent shadowing.

## Rejected alternatives

Don't reintroduce without strong reason.

1. **`ok`/`err` as production keywords.** Disjointness makes them unnecessary. Rust baggage.
2. **`throw e` / `fail e`.** Exception baggage (`throw`, `raise`, `bail`, `fail` all carry implications of stack unwinding or fatal failure).
3. **Asymmetric auto-wrap (success only).** Replaced by symmetric type-based wrap once disjointness was added.
4. **Allowing `T or T`.** Disjointness is the move that eliminates `err`. Newtype workaround is idiomatic and self-documenting.
5. **Flow typing for mut bindings.** The `const`/`mut` split makes flow typing unnecessary. Mut narrowing requires explicit `as` bind.
6. **Type-theoretic union (with disjointness in the union sense).** Terminology error early in the design. `T or E` is a sum type with a runtime tag, not a type-theoretic union. The disjointness rule is for *branch disambiguation at construction*, not for union soundness.
7. **`is some` / `is ok` keywords.** `some` and `ok` would be destructure-only keywords with no construction counterpart. Inconsistent.
8. **`x == none` and `is none` both available.** Kept `== none` as the absent form. `is none` not pursued — `is <variant>` is enum-only.
9. **Matching on Option.** Covered above — operators suffice and keep the builtin framing honest.

## Open spec questions

Each needs a one-paragraph answer before the Option/Result specs can be rewritten.

**Q1 — Define disjointness precisely.**
- Type aliases (`type Score = i32`): is `i32 or Score` legal? *Recommendation: no, same nominal type.*
- Empty types (`enum Never {}`): does `T or Never` collapse to T? *Recommendation: yes.*
- Generic instantiations: `Vec<i32>` vs `Vec<string>` — different types? *Recommendation: yes.*
- Newtypes: confirmed disjoint from their wrapped type.

**Q2 — Auto-wrap scope.**
- Universal (return, assignment, literal, field, argument)? *Recommendation: universal.*
- Or positional (return only, like Rust)? Document the call.

**Q3 — `r?` / `x?` in expression position.**
- Forbidden outside `if`/`while` conditions? *Recommendation: forbidden.* Use `try r` for propagation, `r!` for force, `if r?` for test+narrow, `x != none` / `r is E` for a plain bool.
- If permitted as bool outside conditions, document explicitly — otherwise the Option surface table should read "in conditions only" for `x?`.

**Q4 — `??` with closure form.**
- Single operator overloaded on RHS type (`T` value vs `|E| -> T` closure)? *Recommendation: yes, single overloaded.*
- Closure form is Result-only (Option has no error value to pass).

**Q5 — `try` cross-type rules.**
- `try r` (`T or E`) in fn returning `U or E2`: requires E ⊆ E2.
- `try o` (`T?`) in fn returning `U?`: propagates `none`.
- `try o` (`T?`) in fn returning `U or E`: *Recommendation: ill-typed.*
- `try r` (`T or E`) in fn returning `U?`: *Recommendation: ill-typed.*
- Spec the error message explicitly so cross-shape is diagnostic, not a generic type mismatch.

**Q6 — `r!` / `x!` panic message.**
- Generic ("Result was error") — useless.
- Required `ErrorMessage` trait, structurally checked — *recommended*.
- Literal-message override (`x! "msg"`, with interpolation) stays as today.
- Same trait applies to `x!` on Option (defaults to "none").

**Q7 — Result ↔ Option conversion.**
- `r: T or E` → `T?`: drop error. Method `.ok()`.
- `o: T?` → `T or E`: needs an error. `o ?? Error.NotFound` works under universal auto-wrap (the RHS auto-wraps into the E branch).

**Q8 — `T or T` rejection timing.**
- Reject at signature parse for trivially-equal forms.
- Reject at instantiation for generic cases (`map<i32, i32, i32>`).

**Q9 — Auto-wrap inference with numeric literals.**
- `func f(x: i32 or i64) -> …` called as `f(5)`: both branches accept the literal. *Recommendation: require explicit annotation or a cast (`f(5 as i32)`), since the disjointness rule doesn't help when both types accept the value.* Most real code uses a newtype for the error side, so this is rare — but it needs to be diagnosed, not silently defaulted.

**Q10 — Panic-message trait scope.**
- Q6 proposes `ErrorMessage` as the trait for `r!`'s default panic message.
- Does `try r` also require `ErrorMessage` for origin tracking / diagnostic messages? *Recommendation: no — `try r` propagates the error unchanged, no formatting needed at the propagation site.*
- Does the trait apply to `match` arms' default printing? *Recommendation: no — match is explicit, user decides whether to format.*
- Net: `ErrorMessage` is only for `r!` / `x!` without a literal message override.

## Migration scope

Breaking change. Affected:

**Spec files:** `specs/types/error-types.md` (full rewrite), `specs/types/optionals.md` (full rewrite), `specs/types/union-types.md` (extend disjointness rule), `specs/types/gradual-constraints.md` (error union inference GC7 update), `specs/control/ensure.md` (try interaction), `specs/SYNTAX.md`, `specs/CORE_DESIGN.md`, `specs/GLOSSARY.md`, `specs/rejected-features.md`, `specs/canonical-patterns.md`.

**Stdlib:** `stdlib/result.rk` and `stdlib/option.rk` likely deleted (combinators move to free functions or compiler builtins). Every stdlib file using `Result`/`Option` migrates.

**Compiler:** `rask-parser` (universal auto-wrap, type patterns in match), `rask-types` (disjointness check at formation and instantiation), `rask-resolve` (operator forms), `rask-mir` (operator lowering), `rask-diagnostics` (migration errors for `Some(v)`, `Ok(v)`, `match on Option`).

**Examples and tests:** all `.rk` files using `Result`/`Option`/`Some`/`None`/`Ok`/`Err`.

**Tooling:** `rask fmt --migrate-errors` for mechanical conversion of old code.

## Validation criteria

- Spec passes `rask test-specs`
- All `examples/*.rk` compile and run under the new model
- `package_manager.rk` (1248 lines, the biggest example) migrates cleanly
- LSP works with new patterns
- Migration tool successfully converts existing Rask code

## Out of scope

- Changes to `try` / `??` / `!` / `?.` operator behavior beyond what's needed for the new model
- `ensure` / linear resource handling
- Custom user-defined "result-shaped" types (Rask deliberately doesn't have a `Try` trait; this stays)
- Effect tracking (`comp.effects`) — separate system

## Prior art

| Language | Construction | Destructure | Notes |
|----------|--------------|-------------|-------|
| Rust | `Ok(v)` / `Err(e)` | `match` / `?` | Good payload expressiveness; verbose at construction |
| Zig | `error.X` literal | `try` / `catch` | Language-level error union; no rich error payloads |
| Swift | `throw` | `try` / `catch` | Function coloring |
| Go | `return v, err` | `if err != nil` | Uniformly noisy |
| **Rask (new)** | bare value, type-disambiguated | `?` / `??` / `!` / `try` / match | Rust's payload expressiveness, Zig's language-level treatment, plus type-disambiguated auto-wrap and an operator family neither offers |

**Framing:** Rust's payload expressiveness, Zig's language-level treatment, with type-disambiguated auto-wrap and an operator family neither offers. The cost is a disjointness rule on T and E; newtype is the workaround.

## Recommended order of work

1. Answer Q1–Q8 in spec form
2. Write `specs/types/error-types.md` and `specs/types/optionals.md` (full rewrites)
3. Update CORE_DESIGN, SYNTAX, GLOSSARY
4. Implement compiler changes (parser → types → MIR)
5. Build migration tool
6. Migrate stdlib
7. Migrate examples
8. Update canonical-patterns; write the prior-art comparison page

## What "done" looks like

A new Rask user reading the error-handling section sees: type-based wrap, four operators, no constructors. They can write a fallible function in three lines without learning any wrapper types. They can read existing code and understand it without consulting an enum-variant reference. The model fits on one page.

## See Also

- [Optionals](optionals.md) — current Option spec (to be rewritten)
- [Error Types](error-types.md) — current Result spec (to be rewritten)
- [Union Types](union-types.md) — disjointness rule extension target
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
