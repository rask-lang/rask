<!-- id: type.option-cleanup -->
<!-- status: proposed -->
<!-- summary: Option is a builtin status type, not an enum. No Some wrapper at construction, no match patterns, no some keyword anywhere. Surface is operators only: T?, none, ?, ?., ??, !, try, == none, if x?, if x? as v. Narrowing rides on const. -->
<!-- depends: types/optionals.md -->

# Option Handling — Cleanup Proposal

The current Option surface carries Rust-legacy wrapping (`Some(x)`), five forms for the "check present and name the value" operation, and an "Option is just an enum" framing that fights with the dedicated sugar (`T?`, `?.`, `??`, `!`, auto-wrap, `try`) the language already has. This proposal makes Option genuinely builtin: no `Some` constructor, no `some` keyword, no Option-specific match patterns. The surface is operators only.

Result is handled in a separate proposal. The `is <variant>` narrowing rule described here is written to generalise to user enums, but Option itself does not use `is`.

## Problems with the current design

**P1 — `Some(x)` at construction is pure ceremony.** Auto-wrap (OPT8) already makes `T` coerce to `T?`. The wrapper adds a tag that is always the same tag.

**P2 — Five rebind forms for one operation.** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind. Each says "check present and name the value" with slightly different rules.

**P3 — Magic rebind is invisible.** `if x is Some { use(x) }` silently rewrites `x`'s type with no syntactic marker.

**P4 — "Invent a new name" is noise.** `if x is Some(u) { use(u) }` forces a rename even when `x` already describes the thing. `u` is a trivial alias that shows up constantly.

**P5 — "Option is just an enum" is a lie.** Option has more dedicated surface than any other type in the language (sugar, auto-wrap, propagation, linear propagation, sentinel layout). Treating it as an enum forces duplication between the enum pattern machinery and the operator surface.

## Proposed design

### The rule

**Option is a builtin status type. The present value is always unmarked; `none` is the only sentinel. Everything about Option is expressed through operators, not through patterns. Enums get `match`; Option gets operators.**

### Surface

| Need | Syntax |
|------|--------|
| Type | `T?` |
| Construct present | bare value (auto-wrap via OPT8) |
| Construct / test absent | `none` (literal, used with `==` and as rvalue) |
| Present check + narrow (const) | `if x? { use(x) }` (x: T in block) |
| Present + destructure bind (any) | `if x? as v { use(v) }` (v: const T in block) |
| Absent check | `if x == none { … }` or `if !x? { … }` |
| Chain | `x?.field` |
| Fallback value | `x ?? default` |
| Diverging fallback | `x ?? return none` (also `?? break`, `?? continue`, `?? panic("…")`) |
| Force (panic on none) | `x!` |
| Propagate | `try x` / `try { … }` |

That's the complete surface. There is no `some` keyword, no `is some`, no `match` arm for `none`, no Option-specific pattern.

### Narrowing rides on `const`

The usual flow-typing complications — does mutation invalidate the narrow, do calls touch the binding, does the narrow survive across closures, what about field paths — all collapse into one structural fact Rask already enforces:

**`const` bindings cannot be reassigned. Narrowing works on them for free. `mut` bindings require `if x? as v` to get a stable binding.**

| Scrutinee | `if x? { … }` | `if x? as v { … }` |
|-----------|----------------|---------------------|
| `const x: T?` | narrows `x` to `T` in the block | also binds `v: T`; `v` is a redundant alias |
| `mut x: T?` | predicate is legal, `x` stays `T?` (no narrow) | binds const `v: T`; `x` stays `T?` |

That's the whole rule. No flow analysis, no tracking of intervening calls, no closure-capture exceptions. Field paths narrow iff the full path is rooted in a `const` binding.

### Examples

```rask
const user: User? = load()
if user? {
    greet(user)              // user: User (const → narrows)
}

mut cache: Cache? = try_load_cache()
if cache? as c {
    c.sweep()                // c: Cache (const in block)
}

// two-armed branching
const name = user?.name ?? "Anonymous"

// or when chaining doesn't fit:
const action = if user? { user.greeting() } else { "hi" }

// propagate
func lookup(id: i64) -> User? {
    const user = try fetch(id)   // bails on none
    use(user)                    // user: User
    return user                  // auto-wraps to User?
}
```

### Why no `match` on Option

Match earns its keep on types with multiple shapes, guards, complex destructure, or non-trivial exhaustiveness. Option has two states. Everything match does on Option factors through `if`/`else` + the `?`-family, usually shorter:

| Match form | Operator form |
|------------|---------------|
| `match x { none => a, v => f(v) }` | `if x? { f(x) } else { a }` |
| `match x { none => default, u => u.name }` | `x?.name ?? default` |
| `match x { none => return, v => v }` | `x ?? return none` (or `try x`) |
| `match x { none => panic("…"), v => v }` | `x!` (or `x ?? panic("…")`) |

Keeping match on Option would mean reintroducing `some` and `none` as pattern keywords, which is exactly what the builtin framing is trying to remove. Enums get match; Option gets operators. Clean split.

## What gets deleted

- **`Some` / `None` (PascalCase).** Gone entirely. `none` is a literal (like `true`, `false`); there is no "Some variant."
- **`some(x)` at construction.** Auto-wrap handles the present path. The keyword `some` does not exist.
- **All rebind forms:** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`. Replaced by `if x?` (narrow) and `if x? as v` (destructure bind).
- **Magic rebind.** Replaced by explicit `if x?`.
- **`match` on Option.** The `?`-family covers every case more concisely.
- **`is some` / `is none` predicates.** Use `x?` / `x == none`. The `is <variant>` machinery stays for enums; Option is not an enum.
- **Nested optionals (`T??`).** Compile error. If you need "explicitly none vs. absent value," use a named enum like `T or NotFound`.

## What survives

- `T?` sugar, `none` literal, `?.`, `??`, `!`, auto-wrap (OPT8), `try`, linear propagation (OPT11), `x == none` comparison.
- Methods on `T?`: `map`, `filter`, `is_some`, `is_none`, `to_result` — compiler-provided on the builtin type.
- User-defined enums are unaffected. They still use `match` and the `is <variant>` narrowing rule. Option is no longer one of them.

## Migration map

| Current | Proposed |
|---------|----------|
| `Some(x)` at return / intermediate | `x` (auto-wrap) |
| `None` | `none` |
| `if x is Some { use(x) }` | `if x? { use(x) }` |
| `if x is Some(u) { use(u) }` | `if x? { use(x) }` (if no rename) or `if x? as u { use(u) }` |
| `if x is None { … }` | `if x == none { … }` or `if !x? { … }` |
| `const Some(u) = x else { return none }` | `const u = x ?? return none` |
| `match x { Some(v) => f(v), None => g() }` | `if x? { f(x) } else { g() }` |
| `match x { Some(v) => v.name, None => "anon" }` | `x?.name ?? "anon"` |
| `x.is_some()` / `x.is_none()` | unchanged (methods on builtin `T?`) |

## Open questions

**Q1 — `if x? as v` grammar.** `as` is used elsewhere for casts (`x as i64`). Parser must disambiguate on position. Alternative spellings: `if const v = x?` (Swift/Kotlin style). Preference: `as` is shorter and consistent with the "introduce a name at the check site" role. Confirm.

**Q2 — Interior mutability through const.** `const x: Shared<U?>` holds a shared cell; contents can change via box access. Narrowing applies to `x` itself (the box, const), not to contents accessed through it. Box access uses `with`-scoped const names, which narrow normally inside the `with` block.

**Q3 — Coordination with Result.** Result is a real enum with `Ok`/`Err` variants and keeps `match` / `is <variant>` narrowing. `try` and `x ?? y` mixing across Option/Result need to line up with whatever the Result proposal decides.

**Q4 — Migration scope.** Need to grep sources and stdlib for `Some(`, `None`, `match … { Some`, `match … { None` to size the rewrite. Mechanical but wide.

## Cost

- Mechanical migration across Rask source and stdlib. Tooling can automate `Some(x) → x`, `None → none`, and rewrite `match` on Option into `if`/`else` + `?`-family. The match rewrite is the largest single change.
- Users coming from Rust unlearn `Some(x)` wrapping and Option-in-match. One sentence: "Option is not an enum; use `?` operators, not match."
- `T??` becoming illegal may surprise generic code. Lint with a clear error.
- Documentation rewrite: optionals.md, SYNTAX.md, canonical-patterns.md, and any control-flow doc that mentions Option matching.

## Rationale summary

Every dropped piece had one of two causes: the `Some` wrapper (P1, P2, P3, P4) or the "Option is an enum" framing (P2, P5). Remove both and the cloud of rebind forms, magic rules, and pattern-site duplication evaporates. The remaining surface is what was already load-bearing in practice: the `?`-family plus `try` plus auto-wrap.

Framing Option as a builtin status type rather than an enum also draws a clean line. **Enums get `match`; Option gets operators.** That line justifies keeping the `?`-family special (only Option has it) and keeps user enums uniform (they don't need to special-case the most-used enum).

Narrowing on `const` bindings does the work of flow typing for free. The const/mut split was introduced for ownership discipline; narrowing reuses its invariant instead of duplicating it. Two features that reinforce each other.

## See Also

- [Optionals](optionals.md) — current Option spec (to be rewritten against this proposal)
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
- [Canonical Patterns](../canonical-patterns.md) — existing idioms
