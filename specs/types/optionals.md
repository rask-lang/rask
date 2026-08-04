<!-- id: type.optionals -->
<!-- status: decided -->
<!-- summary: T? is sugar for T or none. none is a built-in zero-field type. The ?-family (?, ?., ??, == none) handles absence and never transfers control; `try` is the exit marker on both shapes, and its else clause says where control goes. No Some/None constructors. Narrowing rides on const. Optionals nest: T?? keeps both layers distinct, operators act on the outer one, a bare none literal means the outer absent. -->
<!-- depends: types/types.md, types/union-types.md, types/error-types.md, control/control-flow.md -->

# Optionals

`T?` is shorthand for `T or none`. `none` is a built-in zero-field type — lowercase, like `void`. There is no `Option<T>` enum and no `Some`/`None` constructors; present values are bare, `none` is the absent sentinel.

Optionals aren't a separate kind of type. They're a particular union shape with dedicated operator surface. The `?`-family handles absence, and everything else (auto-wrap, linearity, equality) falls out of the general union rules.

One operator per shape for the value-instead case: `??` on an optional, `or` on a result ([error-types.md](error-types.md) ER14). Neither transfers control — they supply the value being bound. `try` is the one that can, and its `else` clause says where to (ER45).

## The Type

| Rule | Description |
|------|-------------|
| **OPT1: `T?` is sugar for `T or none`** | The parser desugars `T?` to `T or none` before type checking; the rest of the compiler sees a regular union |
| **OPT2: `none` is a built-in zero-field type** | Lowercase, follows the primitive convention. One inhabitant, also spelled `none`. Not user-definable |
| **OPT3: `?`-family restricted to `T or none`** | `?`, `?.`, `??`, `== none` apply only when the operand is a two-variant union with one variant `none` — never on a `T or E` (`type.errors/ER12`). `!` and `try` work on both shapes; `or` is failure-only. Wider shapes (`T or E or none`) are a compile error pointing at the layering pattern |
| **OPT4: No user wrapper** | No `Some` keyword, constructor, or pattern. Bare values on the present path |

<!-- test: skip -->
```rask
const user: User? = load()       // present value, widens to User or none
const missing: User? = none      // absent sentinel
```

`T??` is `(T or none) or none`. It's legal and the two layers stay distinct — see [Nesting](#nesting) below.

## Construction

Construction follows the general union widening rule: a value of type `A` widens to `A or B or …` at any position expecting the union (return, assignment, field, argument). For optionals specifically:

| Rule | Description |
|------|-------------|
| **OPT5: No auto-unwrap** | `T?` does not coerce to `T`. Unwrap explicitly via `if x?`, `x!`, `x ?? default`, `try x`, or `try x else <diverge>` |
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
| **OPT11: Other branch** | `x ?? default` | unwraps `x` if present, else evaluates `default`. The right side is a **value** — a `T` collapses to `T`, another `T?` stays wrapped and keeps chaining (`type.errors/ER14a`). It never transfers control: `return`, `break`, `continue` and `panic(…)` are rejected (ER48) |
| **OPT12: Extract or leave** | `try x` | unwraps if present, else **control leaves here**. Bare, it returns `none` — so it needs a `U?`-returning function. An `else` clause redirects it anywhere that diverges: `try x else return MyError`, `try x else break` (`type.errors/ER45`) |
| **OPT13: Force** | `x!` | extracts if present; panics with `"none"` or `x! "msg"` custom message |
| **OPT15: Absent check** | `x == none` / `x != none` | plain equality; `x?` and `x == none` narrow identically |
| **OPT16: `!x?` forbidden** | `!x?` is a parse error suggesting `x == none` |

`??` chains while the left side stays wrapped:

<!-- test: skip -->
```rask
const name = user?.display_name
    ?? user?.email
    ?? "anon"
```

As soon as a right side is bare `T`, the chain collapses to `T` and a further `??` is a type error. The chain works flat because the right side sets the result type — see [error-types.md](error-types.md) ER14a. (The compiler doesn't implement the still-wrapped case yet, so a flat chain needs parentheses until [#578](https://github.com/rask-lang/rask/issues/578) lands.)

The right side supplies the value that gets bound, so it can only be a value. Control flow doesn't produce one — that's `try`'s job, and its `else` clause says where control goes (`type.errors/ER45`, ER48).

<!-- test: skip -->
```rask
const theme = config.theme ?? "default"                 // a value — no control flow
const home = env("HOME")! "HOME must be set"            // assert — `!`, not `??`

// control leaving carries `try`; the clause says where it goes
const prof = try load_user(id)                          // to the caller, as `none`
const user = try load_user(id) else return ApiError.NoUser
const item = try queue.pop() else break
const name = try entry.as_string() else continue
```

There is no `?? |e|` form — `none` carries no payload to bind. That's `or |e|`, on the failure shape (ER44).

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

## Nesting

Optionals nest. `T??` is `(T or none) or none` and the layers do **not** collapse — the outer one answers one question, the inner one answers another.

This isn't a corner case you have to go looking for. Any generic that returns `T?` produces it the moment `T` is itself optional:

<!-- test: skip -->
```rask
const slots: Vec<Config?> = load_slots()      // a slot may be empty
const first = slots.first()                    // Config??
```

`Vec.first()` returns `T?` because the vec may be empty. With `T = Config?` the result carries both facts, and the caller can tell them apart:

<!-- test: skip -->
```rask
if slots.first()? as slot {          // outer: the vec was not empty
    if slot? as config {             // inner: that slot was filled
        apply(config)
    } else {
        println("first slot is empty")
    }
} else {
    println("no slots at all")
}
```

Collapsing the layers would throw the distinction away — an empty vec and an empty first slot would both read as `none`. That's Kotlin's nested-nullable bug, and it's exactly the information a caller of `first()` needs.

| Rule | Description |
|------|-------------|
| **OPT28: Layers stay distinct** | `T?` where `T` is itself `U or none` is a two-layer optional. `none` is exempt from the duplicate-variant rule ([union-types.md](union-types.md) U5) for this reason: it carries no payload, so the layers are told apart by position, not by type |
| **OPT29: `none` binds outermost** | A bare `none` literal at a `T??` position means the *outer* absent. To produce an inner absent, widen a value that already has the inner optional type |
| **OPT30: Operators act on the outer layer** | `?`, `??`, `!`, `== none` and `match` all see the outer layer only. `if x? as v` binds `v` at the inner type; unwrap again to reach the value. `??`'s right side must therefore have the inner *optional* type, not the payload type |
| **OPT31: Depth is part of the type** | `T?`, `T??` and `T???` are three different types. Widening adds layers (a `T` reaches a `T??` position, an inner absent stays inner); nothing ever removes one implicitly |

<!-- test: skip -->
```rask
const outer_absent: Config?? = none            // vec was empty        [OPT29]

const empty_slot: Config? = none
const inner_absent: Config?? = empty_slot      // slot was empty       [OPT29]

const present: Config?? = load_config()        // widens through both layers
```

**Spelling.** Write `T??` for two layers — two optional markers. The `??` *operator* only appears in expression position, never inside a type, so nothing is ambiguous.

**Linear payloads.** Each layer narrows separately, so a linear `T` is consumed on the innermost present path (OPT24 applies at that layer). `?.` still can't reach through a linear payload (OPT25).

**Depth beyond two** is legal and falls out of the same rules, but it's almost always a sign that two questions got layered where one nominal type would read better — `enum SlotState { Missing, Empty, Filled(Config) }`.

## Methods

None. `T?` has no methods at all — the operator surface is the whole API, and the same is true of `T or E` (`type.errors`).

`map`, `filter` and `and_then` used to be here. They exist to thread a *wrapped* value through a pipeline, which is the shape the operators deliberately replace: `try` gets the value or leaves now, `??` gets the value or a substitute now. Once nothing threads wrapped values, a combinator has no job — and measurement agreed, with **zero** uses of any of the three on an optional across stdlib, examples, projects and tests.

Lifting into a result is `try x else return MyError` (`type.errors/ER45`); the reverse is `r or none` (`type.errors/ER14a`). Both directions are operators that already exist, so neither shape needs a conversion method.

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
| `match x { none => return, v => v }` | `if x == none { return }` then use `x` |
| `match x { none => panic("…"), v => v }` | `x! "…"` |

The lint is non-fatal. Match earns its keep on multi-error unions where the dispatch genuinely has more than two outcomes.

## Equality

Equality on `T or none` follows the general union equality rule:

- `x == none` / `x != none` — present/absent predicate (canonical form for the absent check)
- `x == y` where both are `T?` — true if both absent, or both present and inner values equal

No optional-specific equality rule.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested optionals (`T??`) | OPT28 | Legal — layers stay distinct |
| Bare `none` at a `T??` position | OPT29 | Outer absent |
| `x ?? default` on `T??` | OPT30 | `default` must be `T?`, not `T` |
| `x?.field` on `T??` | OPT3/OPT30 | Compile error — the outer payload is `T?`, not a struct. Narrow first |
| `Vec<T?>.first()` | OPT28 | `T??` — outer says "vec empty", inner says "slot empty" |
| `?.` on `T or E or none` | OPT3 | Compile error suggesting layering: `(T or E)?` or `T or (E?)` |
| `x ?? return E` | ER48 | Compile error pointing at `try x else return E` |
| `x ?? break` / `?? continue` | ER48 | Compile error — use `try x else break` |
| `x ?? panic(…)` | ER48 | Compile error pointing at `x! "…"` |
| `x ?? \|e\| f(e)` | OPT11 | Compile error — no payload to bind. That form is `or \|e\|`, on the failure shape |
| `x or v` on an optional | OPT3/ER14 | Compile error — `or` is failure-only. Use `??` |
| `try x` in a `U?`-returning function | OPT12 | Legal — absence propagates as absence |
| `try x` elsewhere, no clause | OPT12/ER47 | Compile error naming the fix: `try x else return MyError` |
| `try x else break` / `else continue` | OPT12/ER45 | Legal — the clause takes any divergence |
| `x` is `mut` in `if x?` | OPT18 | No narrow; use `if x? as v` |
| Anonymous expression in condition | OPT18 | `if compute()?` does not narrow — no name to refine. Use `const v = compute()` or `if compute()? as v` |
| `!x?` syntax | OPT16 | Parse error suggesting `x == none` |
| Linear `?.field` | OPT25 | Compile error — cannot partially move |
| `try x else return MyError` where `MyError` isn't in the function's return type | ER9 | Compile error — normal `return` rules apply |
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

**OPT28 (nesting is allowed; it used to be an error).** The earlier rule — `T??` is a duplicate-variant error — was written for optionals you spell out by hand, where nobody wants two layers anyway. It doesn't survive generics. `func head<T>(v: Vec<T>) -> T?` is about as ordinary as generic code gets, and under the old rule it silently failed to compile for every optional `T`. Rejecting it is the C++-template failure mode in the most boring code imaginable, and there's no bound the author could have written to warn you.

The alternatives were worse. Collapsing (Kotlin) throws away the distinction between "the vec was empty" and "the first slot was empty" — the caller of `first()` can no longer recover what happened. Erroring at instantiation makes every `-> T?` generic partial over `T`, which is precisely the non-compositionality being fixed for `T or E` (`type.errors/ER3a`).

The reason nesting works here and not for `T or E` is that branch selection stays decidable at every layer. Producing: `return v` picks by the declared type *before* substitution, so instantiation never changes which branch a `return` chose. Consuming: the outer operators act on the outer layer and the inner layer is only reachable by narrowing through it. The single leftover ambiguity — a bare `none` literal, which could mean either layer — is closed by OPT29.

**OPT29 (`none` binds outermost).** Both readings are defensible; picking one and stating it is what matters. Outermost wins because it matches how the layers get built: the outer layer is the one the immediate context added (`first()` may fail to find anything), so `none` at that position means "the thing right here is absent". Reaching an inner absent means you already have an inner-typed value in hand, and widening it is explicit.

**Narrowing rides on `const`.** The usual flow-typing complications (mutation, intervening calls, closure capture, field paths) collapse into one structural fact the language already enforces: const bindings cannot be reassigned. Narrowing on a const scrutinee is trivially stable; `mut` requires an explicit `as v` bind. No flow analysis beyond "is this const?"

### Patterns & Guidance

**Absent as default input.** For an Option-valued field with a sensible default, read with `??`:

<!-- test: skip -->
```rask
const theme = config.theme ?? "default"
```

**Early exit on absence.** Bind, then narrow. The binding keeps its name, so nothing has to be renamed after the check:

<!-- test: skip -->
```rask
func greet(id: UserId) -> string {
    const user = load_user(id)
    if user == none { return "Hello, guest" }
    return "Hello, {user.name}"          // user: User here
}
```

When absence should leave rather than produce a value, that's one line:

<!-- test: skip -->
```rask
func greet(id: UserId) -> string or ApiError {
    const user = try load_user(id) else return ApiError.NotFound(id)
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
- Quick action "Convert `?? return` to `try … else return`" for code written against the older rule.
- Quick action "Convert `or` to `??`" for optional-shaped code written against the merged draft.

### See Also

- [Union Types](union-types.md) — general union rules (`type.unions`)
- [Error Types](error-types.md) — `T or E`, `try`, narrowing rules shared with optionals (`type.errors`)
- [Control Flow](../control/control-flow.md) — if/match/narrowing (`ctrl.flow`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
