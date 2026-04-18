<!-- id: type.option-cleanup -->
<!-- status: proposed -->
<!-- summary: Option stays an enum but with lowercase builtin variants (some/none). Construction is keyword-free on the present path (auto-wrap); destructuring/narrowing uses the variant name. is <variant> narrows the scrutinee — general language rule, applies to all enums. Result is handled separately. -->
<!-- depends: types/optionals.md -->

# Option Handling — Cleanup Proposal

The current Option surface has accumulated five ways to say "check present and name the value," a magic-rebind rule hidden behind `is Some`, and a `Some(x)` wrapper that auto-wrapping (OPT8) already makes redundant at construction. This proposal collapses the surface around **one rule — `is <variant>` narrows the scrutinee** — and drops the `Some` ceremony at construction sites.

Result is handled in a separate proposal. Any narrowing/construction changes there should follow the same shape described here — but the specifics are out of scope for this document.

## Problems with the current design

**P1 — `Some(x)` at construction is pure ceremony.** Auto-wrap (OPT8) makes `T` coerce to `T?` at function boundaries. Intermediate construction still has to write `Some(x)` manually. The wrapper adds a tag that's always the same tag.

**P2 — Five rebind forms for one operation.** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind. All say "check present and name the value," each with slightly different rules.

**P3 — Magic rebind is invisible.** `if x is Some { use(x) }` silently rewrites `x`'s type with no syntactic marker. Unique in the language.

**P4 — "Invent a new name" is noise.** Today `if x is Some(u) { use(u) }` forces a rename even when the outer name (`x`, `user`, `file`) already describes the thing. `u` is a trivial alias that shows up constantly.

## Proposed design

### The rule

**`is <variant>` narrows the scrutinee.** General language rule — applies to Option and to user-defined enums. After `if user is some`, `user` has type `User` in the block. After `if event is Click`, `event` has type `Click` in the block. No other rebinding form is needed — the scrutinee already has a name. Explicit rename is still available via `is some(u)` when you genuinely want a different name.

Result should follow the same pattern when its proposal lands.

### Option

Option is a builtin enum with **lowercase variants `some` and `none`**. Auto-wrap (OPT8) means `some(x)` is never written by hand — bare values coerce. The keyword `some` appears only in patterns. `none` is both the absence literal and the variant pattern.

```rask
const user: User? = load()       // bare value, auto-wraps
const missing: User? = none      // absence literal

if user is some {
    greet(user)                  // user: User
}

if user is none {
    return
}

match user {
    some(u) => greet(u),         // explicit rename, available but not required
    none    => default_greet(),
}
```

### Construction asymmetry

**The present path is unmarked at construction because it's the default. At destructuring, every branch is named because you're choosing between them.** This is the entire mental model for the construction/match split.

### Narrowing rules

These are the edges any `is <variant>` narrowing mechanism has to pin down. Not Option-specific — same rules apply to user enums.

1. **Mutation invalidates the narrow.** If the scrutinee is reassigned inside the block (`user = load_again()`), it reverts to `T?` — the new value might be `none`.
2. **Calls that don't reassign preserve the narrow.** `greet(user)` doesn't touch the binding; narrow stays.
3. **Narrow doesn't cross closure boundaries.** Inside a closure captured from a narrowed scope, the name reverts to `T?` — the closure might run later when the narrow no longer holds.
4. **The `else` branch gets the negative narrow.** `if x is some { T } else { none }`. Symmetric.
5. **Plain identifiers only, not field paths.** `if player.weapon is some { use(player.weapon) }` does **not** narrow — `player.weapon` could be reassigned between check and use. Use a local: `const w = player.weapon; if w is some { use(w) }`. (Kotlin-style strict, not TypeScript-style loose.)

### Surface

| Need | Syntax |
|------|--------|
| Type | `T?` |
| Construct present | bare value (auto-wrap) |
| Construct absent | `none` |
| Narrow to present | `if x is some { use(x) }` |
| Narrow to absent | `if x is none { … }` |
| Bind with rename | `if x is some(u) { use(u) }` |
| Chain | `x?.field` |
| Fallback value | `x ?? default` |
| Diverging fallback | `x ?? return none` (also `?? break`, `?? continue`, `?? panic("…")`) |
| Force | `x!` |
| Propagate | `try x` / `try { … }` |
| Multi-arm | `match x { some(v) => …, none => … }` |

## What gets deleted

- **`Some` / `None` as PascalCase.** Replaced by lowercase `some` / `none`.
- **Explicit `some(x)` at construction.** Auto-wrap handles the present path. The keyword exists only in patterns.
- **All rebind forms except `is some(u)`:** `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`. The remaining form is kept only for the rename case; when no rename is needed, `is some` narrows and reuses the outer name.
- **Magic rebind.** Replaced by explicit `is some` narrowing (same ergonomics, visible marker).
- **Nested optionals (`T??`).** Compile error. If you need "explicitly none vs. absent value," use a named enum like `T or NotFound`.

## What survives

- `T?` sugar, `none` literal, `?.`, `??`, `!`, auto-wrap (OPT8), `try`, linear propagation (OPT11), `x == none` comparison, methods (`map`, `filter`, `is_some`, `is_none`, `to_result`).
- User-defined enums work exactly as before. The `is <variant>` narrowing rule applies to them too — it's general, not Option-specific.

## Migration map

| Current | Proposed |
|---------|----------|
| `Some(x)` at return / intermediate | `x` (auto-wrap) |
| `None` | `none` |
| `if x is Some { use(x) }` (magic rebind) | `if x is some { use(x) }` |
| `if x is Some(u) { use(u) }` | `if x is some { use(x) }` (if no rename), or `if x is some(u) { use(u) }` (if renamed) |
| `const Some(u) = x else { return none }` | `const u = x ?? return none` |
| `match x { Some(v) => …, None => … }` | `match x { some(v) => …, none => … }` |
| `if x is Some as u { … }` | `if x is some(u) { … }` |

## Open questions

**Q1 — Field-path narrowing.** Proposal says no, use a local. Confirm this is acceptable ergonomically. The escape hatch is one line.

**Q2 — Closure capture of narrow.** Proposal says narrow doesn't cross closures (conservative). An opt-in "closure captures the narrowed type if synchronous" escape hatch might be added later. Not in this proposal.

**Q3 — PascalCase in existing user code.** User enums stay PascalCase. Only Option builtin variants are lowercase. Confirm this split is acceptable, or decide whether all enum variants move to lowercase (separate, larger decision).

**Q4 — Coordination with Result proposal.** `try`, `x ?? y` on mixed Option/Result, and any narrowing behaviour for Result need to line up with whatever lands there. Narrowing rules above are written to be Option/Result-agnostic so they can be adopted wholesale.

**Q5 — Migration scope.** Need to grep sources and stdlib for `Some(`, `None` to size the rewrite. Mechanical but wide.

## Cost

- Mechanical migration across Rask source and stdlib. Tooling can automate `Some(x) → x`, `None → none`, and the pattern rewrites.
- Users coming from Rust unlearn `Some(x)` at construction. One sentence: "Rask doesn't wrap the present path — bare values are already optional when the type says so."
- `T??` becoming illegal may surprise generic code. Uncommon in practice; lint with a clear error.
- Documentation rewrite: optionals.md, SYNTAX.md, canonical-patterns.md, and any control-flow doc that mentions narrowing.

## Rationale summary

The original surface accumulated because each feature was added locally: `Some` as a wrapper, `is Some` as a predicate, magic rebind for ergonomics, five rebind forms for different contexts. Each made sense in isolation; together they produced duplication.

Collapsing around "`is <variant>` narrows the scrutinee" gives one rule that:

- Removes the magic-rebind footgun — narrowing is explicit at the `is some` call site.
- Removes the "invent a new name" cost — the scrutinee's existing name is the narrowed value.
- Extends to user-defined enums, so nothing is Option-specific except the `?`-family sugar and auto-wrap.
- Leaves a clean shape for Result to adopt when its own proposal lands.

Dropping `Some` at construction removes the last piece of Rust-legacy ceremony: auto-wrap was already doing the work; the wrapper was a redundant label. Keeping `none` and `some` as pattern keywords preserves readability at destructuring sites, where branch identity matters.

## See Also

- [Optionals](optionals.md) — current Option spec (to be rewritten against this proposal)
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
- [Canonical Patterns](../canonical-patterns.md) — existing idioms
