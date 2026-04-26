<!-- id: type.optionals -->
<!-- status: decided -->
<!-- summary: T? is sugar for T or none. none is a built-in zero-field type. The ?-family operators (?, ?., ??, !, try, == none) apply to any two-variant union where one variant is none. No Some/None constructors. Narrowing rides on const. -->
<!-- depends: types/types.md, types/union-types.md, types/error-types.md, control/control-flow.md -->

# Optionals

`T?` is shorthand for `T or none`. `none` is a built-in zero-field type — lowercase, like `void`. There is no `Option<T>` enum and no `Some`/`None` constructors; present values are bare, `none` is the absent sentinel.

Optionals aren't a separate kind of type. They're a particular union shape with dedicated operator surface. The `?`-family covers the absent-or-present case; everything else (auto-wrap, linearity, equality) falls out of the general union rules.

## The Type

| Rule | Description |
|------|-------------|
| **OPT1: `T?` is sugar for `T or none`** | The parser desugars `T?` to `T or none` before type checking; the rest of the compiler sees a regular union |
| **OPT2: `none` is a built-in zero-field type** | Lowercase, follows the primitive convention. One inhabitant, also spelled `none`. Not user-definable |
| **OPT3: `?`-family restricted to `T or none`** | `?`, `?.`, `??`, `!`, `try`, `== none` apply only when the operand is a two-variant union with one variant `none`. Wider shapes (`T or E or none`) are a compile error pointing at the layering pattern |
| **OPT4: No user wrapper** | No `Some` keyword, constructor, or pattern. Bare values on the present path |

<!-- test: skip -->
```rask
const user: User? = load()       // present value, widens to User or none
const missing: User? = none      // absent sentinel
```

`T??` is `(T or none) or none` — rejected by the union duplicate-variant rule (see [union-types.md](union-types.md)). No optional-specific rule needed.

## Construction

Construction follows the general union widening rule: a value of type `A` widens to `A or B or …` at any position expecting the union (return, assignment, field, argument). For optionals specifically:

| Rule | Description |
|------|-------------|
| **OPT5: No auto-unwrap** | `T?` does not coerce to `T`. Unwrap explicitly via `if x?`, `x!`, `x ?? default`, or `try x` |
| **OPT6: `none` widens at use** | `none` has type `none` on its own; widens to `T or none` at any position with a target union type |

<!-- test: skip -->
```rask
func load_user() -> User? { … }         // bare User return widens
mut cache: User? = none                  // none widens to User or none
cache = get_current_user()               // User widens at assignment
```

## Operators

| Rule | Syntax | Meaning |
|------|--------|---------|
| **OPT7: Type shorthand** | `T?` | sugar for `T or none` |
| **OPT8: Absent literal** | `none` | absent value; type widens at use |
| **OPT9: Boolean present** | `x?` | `true` when present, `false` when absent; `bool` expression |
| **OPT10: Optional chain** | `x?.field` | accesses `field` when present, else `none`; short-circuits |
| **OPT11: Value fallback** | `x ?? default` | unwraps `x` if present, else yields `default`. `??` is strict-extract — `default`'s type must match the inner `T` |
| **OPT12: Diverging fallback** | `x ?? return none` (or `break`, `continue`, `panic(…)`) | unwraps if present, else diverges |
| **OPT13: Force** | `x!` | extracts if present; panics with `"none"` or `x! "msg"` custom message |
| **OPT14: Propagate** | `try x` | in a `T?`-returning function, unwraps if present, else returns `none` |
| **OPT15: Absent check** | `x == none` / `x != none` | plain equality; `x?` and `x == none` narrow identically |
| **OPT16: `!x?` forbidden** | `!x?` is a parse error suggesting `x == none` |
| **OPT17: Propagate block** | `try { … }` | each `try` inside propagates `none` on the first absent scrutinee |

`??` chains while the left side stays wrapped:

<!-- test: skip -->
```rask
const name = user?.display_name
    ?? user?.email
    ?? "anon"
```

As soon as an RHS is bare `T`, the chain collapses to `T` and further `??` is a type error.

## Conditions and Narrowing

Narrowing rides on `const` — the same rule for any union with a recognised predicate. See [error-types.md](error-types.md) for the shared semantics; the rules below apply identically to `T or none`.

| Rule | Description |
|------|-------------|
| **OPT18: `if x?` narrows** | On a const scrutinee, `if x?` narrows `x` to `T` inside the block |
| **OPT19: `if x? as v` binds** | Binds a const `v: T` in the block; works for `mut` scrutinees, and for renaming |
| **OPT20: Both branches narrow** | On a const scrutinee, the `else` branch narrows `x` to `none` |
| **OPT21: Early-exit narrow** | If a branch of `if x == none { … }` diverges, `x` is `T` in the fall-through |
| **OPT22: No compound narrowing** | `x? && y?` is a legal bool expression but does not narrow either side — use nested `if` or `as v` bind |
| **OPT23: No field-path narrow through mut** | `player.weapon` narrows iff the full path is rooted in a `const` binding. With `mut` anywhere in the path, use `if player.weapon? as w` |

<!-- test: skip -->
```rask
const user: User? = load()
if user? {
    greet(user)              // user: User here
}

mut cache: Cache? = try_load()
if cache? as c {
    c.sweep()                // c: Cache (const) in the block
    // cache still Cache? — may be reassigned below
}

// Early-exit guard
const user: User? = load()
if user == none {
    return
}
greet(user)                   // user: User after the guard
```

**Anonymous expressions don't narrow.** `if compute()? { use(compute()) }` calls `compute()` twice and does not narrow either call. Use `const v = compute()` then `if v?`, or `if compute()? as v` to bind at the check site.

## Methods

Four compiler-provided methods on `T or none`. Each preserves the wrapper for chaining; operators always extract or panic.

| Method | Signature | Behavior |
|--------|-----------|----------|
| `map` | `func<U>(take self, f: \|T\| -> U) -> U?` | Transform if present; absent stays absent |
| `filter` | `func(take self, pred: \|T\| -> bool) -> T?` | Keep if predicate true; else absent |
| `and_then` | `func<U>(take self, f: \|T\| -> U?) -> U?` | Chain Option-returning operations |
| `to_result` | `func<E>(take self, err: E) -> T or E` | Lift to Result; absent becomes `err`. Required because `??` does not widen |

<!-- test: skip -->
```rask
const valid_email = lookup_user(id)
    .filter(|u| u.is_active)
    .map(|u| u.email)

const profile = load_user(id).and_then(|u| load_profile(u.id))
```

## Linear Resources

A union is linear if any variant is linear (general union rule). For `T or none` where `T` is linear:

| Rule | Description |
|------|-------------|
| **OPT24: Narrow consumes on present path** | `if x?` / `if x? as v` treats the present path as a resource site — the payload must be consumed on that branch |
| **OPT25: `?.` forbidden on linear** | Optional chaining cannot partially move out of a linear `T`. Use `if x? as v { … v.field … }` |
| **OPT26: `??` consumes one branch** | Short-circuits; exactly one `T` is produced and must be consumed |

<!-- test: skip -->
```rask
mut file: File? = open("data.txt")
if file? as f {
    try f.write(content)
    try f.close()             // consumed on present path
}
// absent path has no resource to consume
```

## Match on `T?`

| Rule | Description |
|------|-------------|
| **OPT27: Match is legal but linted** | `match` on `T or none` follows the general match rules. A style lint suggests operators when the match is two-arm and one arm is `none`, since the operator form is shorter |

Match on `T or none` is legal — it's a union, the general match rules apply. The lint catches the common two-arm case:

<!-- test: skip -->
```rask
// Legal, but lint suggests operators
match user {
    none => "guest",
    u    => u.name,
}

// Preferred — operators are shorter
user?.name ?? "guest"
```

| Match form | Operator form |
|------------|---------------|
| `match x { none => a, v => f(v) }` | `if x? { f(x) } else { a }` |
| `match x { none => default, u => u.name }` | `x?.name ?? default` |
| `match x { none => return, v => v }` | `x ?? return none` (or `try x`) |
| `match x { none => panic("…"), v => v }` | `x!` (or `x ?? panic("…")`) |

The lint is non-fatal. Match earns its keep on multi-error unions where the dispatch genuinely has more than two outcomes.

## Equality

Equality on `T or none` follows the general union equality rule:

- `x == none` / `x != none` — present/absent predicate (canonical form for the absent check)
- `x == y` where both are `T?` — true if both absent, or both present and inner values equal

No optional-specific equality rule.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested optionals (`T??`) | union duplicate-variant | Compile error |
| `?.` on `T or E or none` | OPT3 | Compile error suggesting layering: `(T or E)?` or `T or (E?)` |
| `x` is `mut` in `if x?` | OPT18 | No narrow; use `if x? as v` |
| Anonymous expression in condition | OPT18 | `if compute()?` does not narrow — no name to refine. Use `const v = compute()` or `if compute()? as v` |
| `!x?` syntax | OPT16 | Parse error suggesting `x == none` |
| Linear `?.field` | OPT25 | Compile error — cannot partially move |
| `try x` outside a `T?`-returning function | OPT14 | Compile error — propagation target mismatch |
| `match` on `T?` with two arms | OPT27 | Legal; style lint suggests operators |
| `const x = none` | OPT8 | Legal. `x: none`. Widens at later use site |
| `none == none` | equality | `true`. Standard equality on a zero-field type |

## Error Messages

**Operator on wider union [OPT3]:**
```
ERROR [type.optionals/OPT3]: `?.` requires a two-variant union with `none`
   |
5  |  const name = result?.display_name
   |               ^^^^^^^ `result` is `User or DatabaseError or none` — three variants

WHY: The `?`-family operators handle the absent-or-present case. For unions
     with multiple non-absent variants, layer the types or use `match`.

FIX: Layer them — error on the inside, optionality on the outside:

  func find(id: UserId) -> (User or DatabaseError)? { ... }

  const outer = find(id)
  if outer? as r {
      match r {
          User       as u => use(u),
          DatabaseError as e => log(e),
      }
  }
```

**`Some(v)` / `None` at construction [migration]:**
```
ERROR [type.optionals/NO_WRAPPER]: Some/None are not valid in Rask
   |
3  |  return Some(user)
   |         ^^^^^^^^^^ bare value widens to User? at return

FIX: return user   (or none for absent)
```

**`!x?` forbidden [OPT16]:**
```
ERROR [type.optionals/OPT16]: cannot negate `x?` with prefix `!`
   |
8  |  if !user? { return }
   |     ^^^^^^ mixes prefix ! with suffix ? ; fights the parse

FIX: if user == none { return }
```

**Match on `T or none` with two arms [style lint, non-fatal]:**
```
LINT [type.optionals/lint-match]: prefer operators over `match` on optional
   |
5  |  match user {
6  |      none => default_name(),
7  |      u    => u.name,
8  |  }

SUGGEST: user?.name ?? default_name()
```

---

## Appendix (non-normative)

### Rationale

**OPT1 (sugar, not a distinct kind).** Earlier drafts treated Option as a builtin "status type" — different from enums and unions, with its own construction rules, auto-wrap rules, linearity propagation, and ban on nesting. That framing carried more teaching burden than the language earned. The new framing: "`T?` is shorthand for `T or none`, and the `?`-operators handle that shape." Shorter to teach, fewer rules to remember. The dedicated surface is on the *operators*, not on the type — the type itself is just a particular union shape.

**OPT2 (lowercase `none`).** Rask's primitives are lowercase (`i32`, `bool`, `string`, `void`); user-facing types are capitalized (`User`, `Vec`). `none` is builtin, not a user type, so it follows the primitive convention. Uppercase `None` would read like an enum variant you have to import — exactly the framing this design moves away from.

**OPT3 (restrict operators to two-variant unions).** Generalising `?.` to pass through other variants makes result types unreadable — `user?.profile?.name` on `User or DBError or none` returns `string or DBError or DBError or none`. Coherent but unteachable. Layering is the cleaner discipline; operators stay simple.

**OPT16 (`!x?` forbidden).** `!x?` parses right-to-left but reads left-to-right as "not present" — the directions fight. `x == none` is unambiguous. The rule is specific to `!` directly applied to a `?`-suffixed expression; other uses of `!` on booleans stay normal.

**OPT27 (match is a lint, not an error).** Hard errors should enforce safety or correctness, not style. Match on a two-arm union is perfectly safe; it's just verbose. A lint catches the common case.

**Narrowing rides on `const`.** The usual flow-typing complications (mutation, intervening calls, closure capture, field paths) collapse into one structural fact the language already enforces: const bindings cannot be reassigned. Narrowing on a const scrutinee is trivially stable; `mut` requires an explicit `as v` bind. No flow analysis beyond "is this const?"

### Patterns & Guidance

**Absent as default input.** For an Option-valued field with a sensible default, read with `??`:

<!-- test: skip -->
```rask
const theme = config.theme ?? "default"
```

**Guard-style early exit.** Common for top-of-function absent checks:

<!-- test: skip -->
```rask
func greet(user: User?) -> string {
    if user == none { return "Hello, guest" }
    return "Hello, {user.name}"
}
```

**Mutation inside a narrow.** `mut` needs explicit bind; the const `v` inside the block is safely narrowed:

<!-- test: skip -->
```rask
mut cache: Cache? = try_load_cache()
if cache? as c {
    c.sweep()
    // cache itself still Cache? — may be reassigned
}
```

**Layered with errors.** When a function can both fail and return absence, layer them — outer optional, inner result, or vice versa:

<!-- test: skip -->
```rask
func find(id: UserId) -> (User or DatabaseError)? {
    // outer ? indicates "not found"; inner union indicates DB error
}
```

### IDE Integration

- Ghost text shows the narrowed type on hover inside `if x?` blocks.
- Quick action "Convert `match` to operator form" for the two-arm none/value case.
- Ghost text for the diverging `??` fallback shows the return type of the enclosing function.

### See Also

- [Union Types](union-types.md) — general union rules (`type.unions`)
- [Error Types](error-types.md) — `T or E`, `try`, narrowing rules shared with optionals (`type.errors`)
- [Control Flow](../control/control-flow.md) — if/match/narrowing (`ctrl.flow`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
