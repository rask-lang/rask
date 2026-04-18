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
- **Error bound.** Every `E` in `T or E` must implement the structural `ErrorMessage` trait (`message(self) -> string`). Enforced at type formation. Primitives like `i32`, `f64` don't qualify unless wrapped in a nominal type. This bound is what makes `r!` format a useful message and removes the "is this literal an error?" ambiguity at construction.
- **Auto-wrap rules (asymmetric):**
  - **`T or E`:** auto-wrap fires **only at `return`**. Elsewhere (assignment, field, argument) requires the value to already have the union type. This keeps the error-branch coercion visible — you can only produce a `T or E` by returning from a function declared to return one.
  - **`T?`:** auto-wrap fires at return **and** assignment (OPT8 unchanged). Bare `T` becomes `T?` wherever a `T?` is expected. Absence-via-sentinel is unambiguous and the pattern is too common for ceremony.

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
| Construct present | bare value (auto-wrap at return and assignment) |
| Absent literal | `none` |
| Present bool expression (anywhere) | `x?` (returns `bool`) |
| Present check + narrow (const) | `if x? { use(x) }` |
| Present + destructure bind (any) | `if x? as v { use(v) }` |
| Absent check | `if x == none { … }` or `!x?` |
| Early-exit narrow | `if x == none { return } … use(x)` (x: T after) |
| Chain | `x?.field` |
| Fallback value | `x ?? default` |
| Diverging fallback | `x ?? return none` / `?? break` / `?? panic("…")` |
| Force | `x!` (panics with "none" or `x! "custom {ctx}"` override) |
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
try { compute(r) } else |e| context(e)     // block form for error-recovery-with-context
r ?? 0.0                                   // value fallback (value only)
r!                                         // force (panic on err, uses E's message())
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

**Option `T?`** — four methods:
- `.map(f: |T| -> U) -> U?` — transform present without unwrapping
- `.filter(pred: |T| -> bool) -> T?` — keep if predicate holds
- `.and_then(f: |T| -> U?) -> U?` — chain Option-returning operations
- `.to_result(err: E) -> T or E` — lift to Result. Needed because `??` does not widen; `o ?? err_value` is a type error when `err_value`'s type doesn't match `T`.

**Result `T or E`** — four methods:
- `.map(f: |T| -> U) -> U or E` — transform success
- `.map_err(f: |E| -> E2) -> T or E2` — translate error
- `.and_then(f: |T| -> U or E) -> U or E` — chain Result-returning
- `.ok() -> T?` — drop error, lift to Option

Eight methods total. Compiler-provided on the builtin types — no `impl` blocks for users to discover or replicate.

**Cut from today's surface:**

| Method | Replacement |
|--------|-------------|
| `.is_some()` / `.is_none()` | `x?` / `x == none` |
| `.is_ok()` / `.is_err()` | `r?` / `r is E` |
| `.unwrap()` | `x!` / `r!` |
| `.unwrap_or(default)` | `x ?? default` |
| `.unwrap_or_else(f)` | `try { … } else \|e\| f(e)` block form |
| `.to_option()` | `.ok()` (single survivor) |
| `.or(other)` | `x ?? other` already returns `T?` |
| `.or_else(f)` | `try { … } else \|e\| …` or `match` |

Each removed method either duplicated an operator or can be reconstructed trivially. The retained eight are precisely the ones that keep a value in wrapper-land for the next chain step, plus the two explicit conversion paths (`.ok()`, `.to_result(err)`).

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

`T or E` requires T ≠ E. Uses Rask's existing nominal-vs-alias distinction (see [type-aliases.md](type-aliases.md)):

- `type Score = i32` (nominal) — `i32 or Score` is **legal**; `Score` is a distinct type.
- `type alias Score = i32` (transparent) — `i32 or Score` = `i32 or i32`, **illegal**.
- Generic instantiations like `Vec<i32>` and `Vec<string>` are distinct (different type constructors applied).
- `T or Never` (where `Never` is uninhabited) collapses to `T` — the E branch can't exist at runtime.
- References vs values: `T or &T` is legal (distinct types).

Enforcement happens at:

- **Type formation** for concrete types (compile error)
- **Instantiation** for generics (compile error at the use site, not the definition)
- **Signature parse** for trivially-equal forms like `func id<T>(x: T) -> T or T`

The escape hatch is **newtypes**, not language syntax. `type ParseError = i32 with (…)` lets you express what would have been `i32 or i32`. No special-case `err` keyword for the same-type case.

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

**`try` cross-shape.** `try` never crosses shapes. Legal combinations:
- `try r` (`T or E`) in fn returning `U or E2`: requires `E ⊆ E2`.
- `try o` (`T?`) in fn returning `U?`: propagates `none`.

Illegal (compile error, not silent conversion):
- `try o` (`T?`) in fn returning `U or E`: fabricating an error is a footgun.
- `try r` (`T or E`) in fn returning `U?`: dropping the error is a footgun.

Use `.ok()` or `.to_result(err)` for explicit conversions.

**Linear resources.** OPT11 stays: if `T` is linear, `T?` is linear, and `T or E` is linear if either branch is. Operators must consume the resource exactly once.

- `if x? as v { consume(v) }` — `v` binds the payload; the linear resource moves into `v` at the bind site. `x` is no longer usable in the block (standard move semantics).
- `if x? { consume(x) }` — same, except `x` is consumed directly. After the block, `x` is consumed regardless of branch taken (the `none` branch has no resource to consume).
- `x ?? default` — consumes whichever branch evaluates. Both paths produce exactly one `T`.
- `try x` — consumes `x` by moving the payload into the current function's flow (success path) or returning `x` to the caller (absent path).
- `x?.field` — **not supported on linear `T?`**. Projecting a field can't partially move out of `T`. Use `if x? as v { … v.field … }`.
- `x!` — moves payload on success; panics on absence.

**`else as` is Result-only.** `if r? { … } else as e { log(e) }` binds the error value. Option has no error to name in the else branch — `else as n { … }` would bind "none" which has no payload. Only `T or E` supports `else as`.

**`ErrorMessage` trait.** Structural; requires a single `message(self) -> string` method. Implemented by writing an `extend` block with that method — no explicit trait declaration needed. Examples:

```rask
enum DivError { ByZero, Overflow }
extend DivError {
    func message(self) -> string {
        match self {
            DivError.ByZero => "division by zero",
            DivError.Overflow => "overflow",
        }
    }
}

struct NotFound { key: string }
extend NotFound {
    func message(self) -> string { "not found: {self.key}" }
}
```

The bound is enforced at type formation: `T or E` where `E` doesn't implement `ErrorMessage` is a compile error pointing at the missing method. Primitives (`i32`, `f64`, `string`) don't qualify — wrap them in a nominal type that does.

**Layered error traits.** `ErrorMessage` is the minimum. Richer capabilities live in opt-in traits on top — `LinedError` (source line), `ContextualError` (key/value context map), `CodedError` (numeric code), etc. Libraries choose which they implement. Operators and `match` don't require the richer traits; they're for diagnostics/logging pipelines that want more than a string.

**`??` is strictly extract, never widens.** `x ?? y` requires `y` to be compatible with the inner type of `x` (`T` for `x: T?`, `T` for `x: T or E`). Never produces a wider type. If you have `o: T?` and want `T or E`, use `o.to_result(err)`.

**`x?` as a boolean.** `x?` / `r?` is a bool expression anywhere. Narrowing is the special behaviour gated to condition position over a const scrutinee; the expression itself is always a bool. `!x?`, `x? && y?`, `const b: bool = x?` all legal.

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

1. Write `specs/types/error-types.md` and `specs/types/optionals.md` (full rewrites against the decisions in this proposal)
2. Update CORE_DESIGN, SYNTAX, GLOSSARY
3. Implement compiler changes (parser → types → MIR)
4. Build migration tool
5. Migrate stdlib
6. Migrate examples
7. Update canonical-patterns; write the prior-art comparison page

## What "done" looks like

A new Rask user reading the error-handling section sees: type-based wrap, four operators, no constructors. They can write a fallible function in three lines without learning any wrapper types. They can read existing code and understand it without consulting an enum-variant reference. The model fits on one page.

## See Also

- [Optionals](optionals.md) — current Option spec (to be rewritten)
- [Error Types](error-types.md) — current Result spec (to be rewritten)
- [Union Types](union-types.md) — disjointness rule extension target
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
