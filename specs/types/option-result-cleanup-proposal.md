<!-- id: type.option-result-cleanup -->
<!-- status: proposed -->
<!-- summary: Shrink the Option/Result surface: drop try, magic rebind, is-Some(u) binding; adopt must, flow narrowing, universal ?-family -->
<!-- depends: types/optionals.md, types/error-types.md -->

# Option/Result Handling — Cleanup Proposal

The current Option/Result surface has accumulated overlap and ambiguity. `try` works on both but carries exception baggage. `is Some` means different things in different positions. `is Some(u)` binding looks like comparison. The guard, if, and match forms have inconsistent binding rules. This proposal shrinks the surface to a set of non-overlapping constructs that each do one thing.

## Problems with the current design

**P1 — `try` on Option has the wrong vibe.** `const u = try get_user(id)` reads as "try an operation that might throw," but `get_user` just returns `Option<User>`. None isn't a thrown error, it's absence. The keyword fights the semantics.

**P2 — Magic rebind in `if x is Some { use(x) }`.** The scrutinee silently narrows to `T` inside the block, with no syntactic marker. It's the only place in the language where pattern matching retroactively narrows an outer-scope variable invisibly.

**P3 — `is Some` means two things depending on position.** In a guard (`const u = x is Some else { ... }`) it's a variant check where binding is done by the outer `const`. In an `if` (`if x is Some { use(x) }`) it both checks and invisibly rebinds. Same tokens, different semantics.

**P4 — `is Some(u)` binding looks like comparison.** Pattern-as-construction-dual is ML convention, but the ambiguity is real for readers who haven't internalized it. You read it as "is x equal to Some(u)" until you learn otherwise.

**P5 — Redundant rebinding forms.** We've accumulated `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, and magic rebind. All express "check Some and name the payload" with slightly different rules and readings.

**P6 — Stacked `?` symbols are hard to parse.** Mixing optional chaining (`?.`), nil-coalescing (`??`), and any proposed postfix-`?` propagation (Rust-style) produces `x?.foo()?.bar ?? baz()?` which is genuinely difficult at a glance.

## Proposed design

### Core vocabulary

| Need | Syntax |
|------|--------|
| Propagate failure (Option or Result), single call | `const v = must fetch(url)` |
| Propagate failure (Option or Result), chain | `const v = must { fetch(url).parse().validate() }` |
| Propagation with error transform | `const v = must { … } else \|e\| context(e)` |
| Option present check with narrowing | `if x? { use(x) }` |
| Option chain | `x?.field` |
| Option value fallback | `x ?? default` |
| Option diverging fallback | `x ?? return none` (also `?? continue`, `?? break`, `?? panic("…")`) |
| Option force | `x!` |
| Enum variant check (narrows scrutinee) | `if x is Variant { … }` |
| Enum destructure (bind payload) | `if x is Variant(a, b) { … }` |
| Enum guard | `const v = x is Variant else { … }` |
| Multi-arm | `match x { … }` |

### The two load-bearing rules

**R1 — `must` is the universal propagation keyword.** Prefix for single calls (`must fetch(url)`), block for chains (`must { ... }`). Works uniformly on Option and Result. Replaces `try`.

**R2 — `is Variant` narrows the scrutinee's type in the true branch.** General flow typing rule, not a special case. After `if x is Click`, `x` is typed `Click` in the block. The narrowing is explicit because it's a universal language rule, not hidden behind a specific syntactic form. This replaces the magic rebind.

### Mental model

One sentence: **subject first, predicate second, bind on the left of `=` or via destructure.**

- "Is this Option present?" → `x?`
- "Is this enum this variant?" → `x is Variant`
- "I want the payload" → `x is Variant(a, b)`
- "This must succeed or I bail" → `must x`
- "What happens if it fails?" → `else { ... }` (guards), `?? diverge` (Option shortcut), `must` (propagate)

## What gets deleted

- **`try` as a keyword.** Replaced by `must`.
- **Magic rebind (`if x is Some { use(x) }` silent narrowing).** Replaced by explicit narrowing rule (`if x is Some` narrows x, applied uniformly).
- **`is Some(u)` binding form on Option.** Use `if x? { use(x) }` with narrowing, or destructure a general enum with `if x is Some(u)` only when you specifically want the payload as a new name.
- **`is Some as u` and `as u` whole-variant binding.** Use narrowing on the original name. For a genuine rename, `const c = event` inside the block.
- **`const Some(u) = x else { … }` guard form.** Option case goes to `x ?? return none`. Enum case stays on `const v = x is Variant else { … }`.
- **`if const Some(u) = x { … }` conditional bind.** Option case goes to `if x?`. Enum case goes to `if x is Variant(u)` when destructuring is needed.

## What survives from the current spec

- The `?` family: `T?`, `x?.field`, `x ?? y`, `x!`.
- `none` literal, auto-Some wrapping (OPT8).
- `match` semantics.
- Result methods (`on_err`, `map`, `map_err`, `is_ok`, `is_err`, `to_option`, `to_error`, `to_result`).
- Auto-Ok wrapping (ER7), implicit `Ok(())` (ER8), union widening (ER9), `any Error` boxing (ER10).
- Error origin tracking (ER15, ER16).
- `@message` annotation for error enums.
- Custom error types via structural `message()` method (ER1).

## Migration map

| Current | Proposed |
|---------|----------|
| `try result` | `must result` |
| `try result else \|e\| context(e)` | `must result else \|e\| context(e)` |
| `try opt` (in `T?` function) | `opt ?? return none` (or `must opt`) |
| `if x is Some { use(x) }` (magic rebind) | `if x? { use(x) }` |
| `if x is Some(u) { use(u) }` | `if x? { const u = x; use(u) }` or `if x is Some(u)` if payload destructure is what's actually meant |
| `const u = x is Some else { return }` | `const u = x ?? return none` |
| `if result is Ok(v) { use(v) }` | unchanged (destructure is fine) |
| `const v = result is Ok else { return }` | unchanged |

## Open questions

**Q1 — Does `must` work for Option in a non-Option-returning function?** Today `try x` on Option requires the enclosing function to return `T?`. `must x` should have the same constraint (and same compile error if not). Need to confirm this reads well across both types.

**Q2 — `must` block error type.** Inside `must { a().b().c() }`, when `a()` returns `Result<X, E1>` and `b()` returns `Result<Y, E2>`, the block's error type is `E1 | E2` (union widening, same as today's `try`). Any mixing with Option inside the block requires explicit `.to_result(err)`.

**Q3 — `if x?` versus `if x is Some`.** Both narrow Option. Is `is Some` available for Option at all, or reserved for non-Option enums? Proposal: `if x?` is canonical for Option (fits the `?` family); `if x is Some` is also legal (general enum rule applies) but stylistically discouraged. Consider linting.

**Q4 — Whole-variant rebind escape hatch.** Proposal eliminates `as` binding. The rare case of "I want the whole narrowed variant under a new name" uses a manual `const c = event` inside the block. Is this acceptable, or do we need an `as` escape hatch for readability in rare cases?

**Q5 — `must` outside propagation contexts.** If the enclosing function doesn't return `T?` or `T or E`, what does `must x` do? Proposal: compile error, same as today's `try`.

**Q6 — Interaction with `x!`.** `x!` panics on None/Err. `must x` propagates. Both are "extract or fail." Keep `x!` for "miss = programmer bug" and `must` for "miss = propagate to caller." Proposal: keep both, they express different intents.

## Cost

- Migration of existing Rask code that uses `try`. Mechanical (`try` → `must`) but touches many files. Tooling can automate.
- Learning cost: designers familiar with Rust's `?` or Swift's `try` need to internalize `must`. One new word.
- One-shot documentation pass across optionals.md, error-types.md, SYNTAX.md, canonical-patterns.md.

## Rationale summary

The current design accumulated overlap because each feature was added locally — `try` for propagation, `is Some` for checks, magic rebind for ergonomics, `x?` for sugar, guards for early exit. Each made sense in isolation; together they produce a surface where users (Rask's own designers) forget which form to use.

The proposal collapses the surface around two rules (`must` for propagation, `is Variant` narrows) and a tight `?` family for Option. Every remaining construct does one thing, has one reading, and carries its meaning on its face. `try` was always a mistake because the word doesn't match the semantics for Option. Magic rebind was always a mistake because it hid type narrowing behind no syntactic marker. Both go.

## See Also

- [Optionals](optionals.md) — current Option spec
- [Error Types](error-types.md) — current Result spec
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
- [Canonical Patterns](../canonical-patterns.md) — existing idioms
