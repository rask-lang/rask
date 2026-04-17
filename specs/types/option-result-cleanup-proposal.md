<!-- id: type.option-result-cleanup -->
<!-- status: proposed -->
<!-- summary: Shrink the Option/Result surface: drop magic rebind, collapse five rebind forms to one destructure, keep try as the propagation keyword -->
<!-- depends: types/optionals.md, types/error-types.md -->

# Option/Result Handling — Cleanup Proposal

The current Option/Result surface has accumulated overlap and ambiguity. `is Some` narrows implicitly inside `if` but not in guards. `if x is Some { use(x) }` silently rebinds `x` from `Option<T>` to `T` with no syntactic marker. The same intent can be written five different ways, each with slightly different rules. This proposal shrinks the surface to a set of non-overlapping constructs that each do one thing, without inventing new keywords.

## Problems with the current design

**P1 — Magic rebind in `if x is Some { use(x) }`.** The scrutinee silently changes from `Option<T>` to `T` inside the block, with no syntactic marker. It's the only place in the language where pattern matching retroactively rewrites an outer-scope variable's type invisibly.

**P2 — `is Some` behaves differently in `if` vs. guards.** In `if x is Some { use(x) }` it narrows-and-unwraps. In `const u = x is Some else { return }` it narrows-and-unwraps (guard case). In `if x is Some(u)` it destructures. Three constructs, three rulesets for the same prefix.

**P3 — Redundant rebinding forms.** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, and magic rebind all express "check Some and name the payload" with slightly different rules and readings.

## Proposed design

### Core vocabulary

| Need | Syntax |
|------|--------|
| Propagate failure (Option or Result), single call | `const v = try fetch(url)` |
| Propagate failure, chain | `const v = try { fetch(url).parse().validate() }` |
| Propagation with error transform | `const v = try { … } else \|e\| context(e)` |
| Option chain | `x?.field` |
| Option value fallback | `x ?? default` |
| Option diverging fallback | `x ?? return none` (also `?? continue`, `?? break`, `?? panic("…")`) |
| Option force (panic on none) | `x!` |
| Variant check (narrows scrutinee) | `if x is Variant { … }` |
| Variant destructure (bind payload) | `if x is Variant(a, b) { … }` |
| Variant guard (narrows, early exit on mismatch) | `const v = x is Variant else { … }` |
| Multi-arm | `match x { … }` |

### The two load-bearing rules

**R1 — `is Variant` narrows the scrutinee's type in the true branch.** General flow-typing rule, applied uniformly to `if`, `match`, and guards. After `if x is Click`, `x` is typed `Click` in the block. Narrowing does **not** unwrap positional payloads — to bind a payload, destructure with `is Variant(u)`. This replaces the magic rebind.

**R2 — Payload binding happens in exactly one place: destructure.** `is Variant(a, b)` binds payload fields. `as u` binding, `const Pattern = x`, and magic unwrap all go. If you want the whole narrowed value under a different name, write `const c = event` inside the block.

`try` stays as the propagation keyword. It already covers both Option and Result, already works in prefix and block form, and adding a new keyword just moves the friction rather than removing it. The existing OPT13 rule ("`try x` on Option requires the enclosing function to return `Option<U>`") stays.

### Mental model

One sentence: **subject first, predicate second, bind payloads via destructure.**

- "Is this enum this variant?" → `x is Variant`
- "I want the payload" → `x is Variant(a, b)` (in `if`, `match`, or guard)
- "This must succeed or I propagate" → `try x`
- "What happens if it's absent?" → `?? <value>` (fallback), `?? <diverge>` (early exit)
- "Miss = programmer bug" → `x!`

## What gets deleted

- **Magic rebind** (`if x is Some { use(x) }` silently unwrapping `x` to `T`). Replaced by R1 narrowing — `x` is typed `Some<T>` inside the block. To use the payload, destructure: `if x is Some(u) { use(u) }`.
- **`is Some as u` and `as u` whole-variant binding.** Narrowing preserves the original name. For a rename, `const c = event` inside the block.
- **`const Some(u) = x else { … }` guard form.** Option case becomes `const u = x ?? return none`. For general enum destructure-with-guard, use `match` or narrow + destructure in an `if/else`.
- **`if const Some(u) = x { … }` conditional bind.** Use `if x is Some(u)` — same effect, universal syntax.
- **Magic unwrap in guard** (`const v = x is Some else { return }` giving `v: T`). After R1, `v: Some<T>`. For Option early-exit-with-unwrap, use `const v = x ?? return none`.

## What survives from the current spec

- The `?`-family: `T?`, `x?.field`, `x ?? y`, `x!`.
- `none` literal and auto-Some wrapping (OPT8).
- `try` keyword, including block form and `else |e|` transform.
- `match` semantics.
- Result methods (`on_err`, `map`, `map_err`, `is_ok`, `is_err`, `to_option`, `to_error`, `to_result`).
- Auto-Ok wrapping (ER7), implicit `Ok(())` (ER8), union widening (ER9), `any Error` boxing (ER10).
- Error origin tracking (ER15, ER16).
- `@message` annotation for error enums.
- Custom error types via structural `message()` method (ER1).

## Migration map

| Current | Proposed |
|---------|----------|
| `try result` | unchanged |
| `try result else \|e\| context(e)` | unchanged |
| `try opt` (in `T?`-returning function) | unchanged (or `opt ?? return none` if you prefer control flow visible) |
| `if x is Some { use(x) }` (magic unwrap) | `if x is Some(u) { use(u) }` |
| `if x is Some(u) { use(u) }` | unchanged |
| `if x is Some as u { use(u) }` | `if x is Some(u) { use(u) }` |
| `const Some(u) = x else { return }` | `const u = x ?? return none` |
| `if const Some(u) = x { use(u) }` | `if x is Some(u) { use(u) }` |
| `const u = x is Some else { return }` (magic unwrap) | `const u = x ?? return none` |
| `if result is Ok(v) { use(v) }` | unchanged |
| `const v = result is Ok else { return }` (narrow-only) | unchanged |
| `if event is Click { use(event.field) }` (magic, non-Option) | `if event is Click(c) { use(c.field) }` — or keep `is Click` if you don't need the payload; narrowing gives you the variant type |

## Three ways to handle a missing Option — orthogonal by intent

| Intent | Syntax |
|--------|--------|
| Propagate absence to caller | `try x` (requires `T?` return) |
| Custom control flow on absence | `x ?? return err(…)`, `?? break`, `?? continue`, `?? panic(…)` |
| Programmer invariant: must be present | `x!` |

All three survive. They differ in intent, and each one's shape makes the intent obvious at the call site.

## Open questions

**Q1 — `try` block error-type.** Inside `try { a().b().c() }` where `a()` returns `Result<X, E1>` and `b()` returns `Result<Y, E2>`, the block's error type is `E1 | E2` (union widening, same as today). Mixing Option and Result inside the block requires explicit `.to_result(err)` or `.to_option()`. **Proposal: confirm this is already the rule in error-types.md; if not, spell it out.**

**Q2 — Narrowing on positional tuple variants.** After `if x is Some`, `x` is typed `Some<T>`. How is the payload reached without destructure? In Rask's current enums spec, positional payloads are accessed by pattern, not by `.0`. So the answer is: you don't — destructure. This is the intended discipline, but worth spelling out explicitly in optionals.md so users don't hunt for `x.0`.

**Q3 — Non-Option/Result enums with magic unwrap.** If today's compiler applies magic unwrap for any single-field tuple variant (not just `Some`), that behaviour also goes. Check the current implementation to confirm the scope of the change.

## Cost

- Migration of existing Rask code using magic rebind — mechanical (add `(u)` to every `is Some` that uses the scrutinee afterward, rename uses from `x` to `u`). Tooling can automate.
- Learning cost: destructure is now mandatory when you want the payload. Four extra characters `(u)` in exchange for one narrowing rule across the whole language.
- One-shot documentation pass across optionals.md, error-types.md, SYNTAX.md, canonical-patterns.md.

## Rationale summary

The current design accumulated overlap because each feature was added locally — `try` for propagation, `is Some` for checks, magic rebind for ergonomics, guards for early exit. Each made sense in isolation; together they produce a surface where the same intent has three or four valid spellings with different rules.

The proposal collapses the surface around two rules (`is Variant` narrows; payload binding happens in destructure only) and keeps the existing keywords and operators. Nothing new is invented. Every remaining construct does one thing, has one reading, and carries its meaning on its face. The magic rebind goes because invisible type rewriting belongs nowhere in the language. The rebinding forms collapse to destructure because having five ways to name a payload is five times the cognitive tax for one operation.

`try` stays. It was already doing the job, works the same on Option and Result, and swapping it for `must` would trade a cosmetic concern (Swift/Rust readers associating `try` with exceptions) for a real problem (`must x` and `x!` both reading as strong assertions with different behaviour).

## See Also

- [Optionals](optionals.md) — current Option spec
- [Error Types](error-types.md) — current Result spec
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
- [Canonical Patterns](../canonical-patterns.md) — existing idioms
