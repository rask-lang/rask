<!-- id: type.option-result-cleanup -->
<!-- status: proposed -->
<!-- summary: Remove Some. Option becomes a builtin "present or none" status type. Binding happens through the language's regular mechanisms (const=, match, narrowing), not through Option-specific destructure forms. -->
<!-- depends: types/optionals.md, types/error-types.md -->

# Option Handling — Cleanup Proposal

The current spec says "Option is just an enum" (OPT1). In practice Option is not just an enum — it has dedicated type sugar (`T?`), a dedicated literal (`none`), dedicated chain/fallback/force operators (`?.`, `??`, `!`), auto-wrapping (OPT8), and dedicated propagation via `try`. The "just an enum" framing is fiction we tell ourselves, and it's the source of the churn in the rebinding forms.

This proposal stops pretending. **Option becomes a builtin status type with no `Some` wrapper.** Everything that exists today because of `Some` — `is Some(u)`, `as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind — disappears, because there is nothing to wrap or destructure. Result stays exactly as it is: a regular enum with `Ok` and `Err` variants.

## Problems with the current design

**P1 — The `Some` wrapper is pure ceremony.** Auto-wrapping (OPT8) already makes `T` coerce to `T?` at function boundaries. Construction sites still have to write `Some(x)` manually when an intermediate expression needs to be `T?`. The wrapper adds a tag that's always the same tag.

**P2 — Every rebind form exists because of `Some`.** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`, magic rebind — five ways to say "check present and name the value." The entire category exists to work around the wrapper.

**P3 — Match on Option has a noise tag.** `match x { Some(v) => …, None => … }` — the `Some` on the left always matches the same shape. Compare `match x { v => …, none => … }`.

**P4 — The "Option is just an enum" line is a lie.** Option has more dedicated surface than any other type in the language. Treating it as a regular enum forces the language to smuggle specialness back in everywhere (sugar, auto-wrap, propagation, linear propagation, sentinel layout). Accepting Option as builtin lets us collapse the whole surface cleanly.

## Proposed design

### The rule

**Option is a builtin status type. `T?` means "a value of `T`, or `none`." There is no `Some`.** Binding happens through the language's regular mechanisms — `const =`, match arms, if-narrowing — with no Option-specific bind syntax.

### Surface

| Need | Syntax |
|------|--------|
| Type | `T?` |
| Absent literal | `none` |
| Construct present | just the value (auto-wrap via OPT8) |
| Present check + narrow | `if x? { use(x) }` (x is T in block) |
| Absent check | `if x == none { … }` |
| Chain | `x?.field` |
| Fallback value | `x ?? default` |
| Diverging fallback | `x ?? return none` (also `?? break`, `?? continue`, `?? panic("…")`) |
| Force (panic on none) | `x!` |
| Propagate | `try x` or `try { … }` or `try { … } else \|e\| …` |
| Multi-arm | `match x { none => …, v => … }` |

### One binding rule

All binding happens through the language's existing mechanisms. Option adds nothing.

- **const bind:** `const u = x ?? default`, `const u = try x`, `const u = x!` — regular `const =` with an Option expression on the right.
- **match arm:** `match x { none => …, v => use(v) }` — `v` is a regular identifier pattern that catches the present case and binds it.
- **narrowing in if:** `if x? { use(x) }` — `x` is typed `T` inside the block (flow-narrowing, same mechanism that exists for other narrowing in the language).

Nothing else. No `is Some(u)`, no `as u`, no `const Some(u) =`. Those forms can't exist because `Some` doesn't exist.

### `try` unchanged

`try x` propagates `none` to the caller, requires the enclosing function to return `T?`. Block form and `else |e|` transform work as today. Result propagation via `try` is unaffected.

### Result is unaffected

Result stays as it is: a regular enum with `Ok(T)` and `Err(E)` variants. All the existing patterns (`if r is Ok(v) { … }`, `match r { Ok(v) => …, Err(e) => … }`, `try`, `on_err`, `map_err`, union widening, etc.) keep working. Only the *Option* surface changes.

## What gets deleted

- **`Some` as a constructor and pattern.** It no longer exists.
- **`None` as a variant.** Replaced by the `none` literal (already exists).
- **All Option-specific rebind forms:** `is Some(u)`, `is Some as u`, `const Some(u) = x`, `if const Some(u) = x`.
- **Magic rebind** (`if x is Some { use(x) }` silently unwrapping). Replaced by explicit `if x? { use(x) }`.
- **`is none` as a predicate.** Use `== none`. The `is Variant` machinery is for enums, and Option is no longer an enum.
- **Nested optionals (`T??`).** Compile error. If you need to distinguish "explicitly none" from "value absent," use `T or NotFound` or a real enum.

## What survives

- `T?` type sugar, `none` literal, `?.`, `??`, `!`, auto-wrap (OPT8), `try`, linear propagation (OPT11), `x == none` comparison.
- All Result behaviour: `Ok`/`Err`, `try`, `try … else |e|`, `match r`, `is Ok(v)` destructure, methods, union widening, auto-Ok wrapping, error origin tracking, `@message`, custom error types.
- Match on Option remains exhaustive — `none` arm plus a catch-all identifier (or literal pattern) covers all cases.

## Migration map

| Current | Proposed |
|---------|----------|
| `Some(x)` | `x` (bare — auto-wrap handles it) |
| `None` | `none` |
| `if x is Some { use(x) }` | `if x? { use(x) }` |
| `if x is Some(u) { use(u) }` | `match x { none => …, u => use(u) }` or `if x? { const u = x; use(u) }` |
| `match x { Some(v) => f(v), None => g() }` | `match x { none => g(), v => f(v) }` |
| `const Some(u) = x else { return none }` | `const u = x ?? return none` |
| `x is none` | `x == none` |
| `x.is_some()`, `x.is_none()` | unchanged (compiler-provided methods on builtin `T?`) |
| `x.map(f)`, `x.filter(p)`, `x.to_result(err)` | unchanged |
| `try x` | unchanged |
| `x ?? y`, `x?.f`, `x!` | unchanged |
| Result patterns (`Ok`, `Err`, `try`, `is Ok(v)`, `match`, methods) | unchanged |

## Open questions

**Q1 — Representation.** `T?` where `T` is a pointer-like type uses null-pointer optimisation (same as today). For other `T`, a one-byte discriminant sits next to the payload. Implementation detail; no user-visible change.

**Q2 — Methods on builtin Option.** `map`, `filter`, `is_some`, `is_none`, `to_result` etc. become compiler-provided methods rather than user-defined `impl` blocks. Need to decide if these are written as `impl` in stdlib, or hard-coded. Leaning stdlib `extend` for grep-ability.

**Q3 — Match exhaustiveness.** `match x { none => …, v => … }` — the `v` arm is an identifier pattern that catches "any present value." Exhaustiveness checker must recognise this as covering the present case. Should be natural.

**Q4 — Linear `T?`.** If `T` is linear, `T?` is linear (OPT11 unchanged). Both paths must consume. Works with `if x? { consume(x) }` + implicit drop of `none` branch, and with match.

**Q5 — User-defined Maybe-like enums.** A user can still write `enum MyMaybe<T> { Present(T), Empty }`. It just doesn't get the `?`-family sugar. That's fine — the sugar is Option-specific.

**Q6 — Migration scope.** Need to grep Rask sources and the stdlib for `Some(` and `None` to size the change. Mechanical but wide.

## Cost

- Large mechanical migration across Rask source and stdlib. Tooling can automate `Some(x) → x`, `None → none`, pattern rewrites.
- Users coming from Rust need to unlearn `Some(x)`. One sentence: "Rask doesn't wrap; bare values are already optional when the type says so."
- `T??` becoming illegal may surprise generic code that was relying on it. In practice uncommon; add a lint with a clear error message.
- Documentation rewrite across optionals.md, SYNTAX.md, canonical-patterns.md. Error-types.md largely unaffected.

## Rationale summary

The original proposal tried to clean up the Option surface while preserving `Some`. That kept the root cause alive — every "ways to do one thing" complaint was a symptom of the wrapper. Remove the wrapper and the whole cloud of rebind forms evaporates, because there is no tag to check and no payload to name separately from the scrutinee.

Option was never a regular enum in spirit. This proposal makes the spec agree with the language.

Result stays an enum because error values are genuinely two-sided — `Ok(T)` and `Err(E)` are distinct shapes with distinct payloads, and both need destructuring. Option is one-sided: "present" is the value itself, "absent" is a sentinel. Treating them the same was the mistake.

## See Also

- [Optionals](optionals.md) — current Option spec (to be rewritten against this proposal)
- [Error Types](error-types.md) — Result spec, unaffected
- [Syntax Reference](../SYNTAX.md) — language-wide syntax
- [Canonical Patterns](../canonical-patterns.md) — existing idioms
