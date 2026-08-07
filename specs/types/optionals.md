<!-- id: type.optionals -->
<!-- status: decided -->
<!-- summary: T? is sugar for T or none. none is a built-in zero-field type. The ?-family (?, ?., `is none`) tests and projects — as plain booleans, no narrowing; payload access is always the `as v` bind. `??` supplies the other branch — a value or an exit; bare `try` propagates the absence to a T?-returning caller. No Some/None constructors. Optionals nest: T?? keeps both layers distinct, operators act on the outer one, a bare none literal means the outer absent. -->
<!-- depends: types/types.md, types/union-types.md, types/error-types.md, control/control-flow.md -->

# Optionals

`T?` is shorthand for `T or none`. `none` is a built-in zero-field type — lowercase, like `void`. There is no `Option<T>` enum and no `Some`/`None` constructors; present values are bare, `none` is the absent sentinel.

Optionals aren't a separate kind of type. They're a particular union shape with dedicated operator surface. The `?`-family tests and projects through absence, and everything else (auto-wrap, linearity, equality) falls out of the general union rules.

Three words, three jobs, one glance: `try` means something **leaves**, a `?` means something is **missing**, `catch` means something **failed**. `try x` propagates the bad branch of either shape to the caller (`type.errors/ER16`); `x ?? <expr>` is the absence fallback — a value or an exit written out; failures are handled with `catch e =>`, which never appears on an optional (`type.errors/ER14`). The fallbacks are deliberately split per shape: an absent miss carries no information, so its fallback is terse — a discarded *error* is a real cost, so its fallback always names or explicitly drops the payload.

## The Type

| Rule | Description |
|------|-------------|
| **OPT1: `T?` is sugar for `T or none`** | The parser desugars `T?` to `T or none` before type checking; the rest of the compiler sees a regular union |
| **OPT2: `none` is a built-in zero-field type** | Lowercase, follows the primitive convention. One inhabitant, also spelled `none`. Not user-definable |
| **OPT3: `?`-family restricted to `T or none`** | `?`, `?.` and `??` apply only when the operand is a two-variant union with one variant `none` — never on a `T or E` (`type.errors/ER12`; failures use `catch`). `try`, `!` and `match` work on both shapes. Wider shapes (`T or E or none`) are a compile error pointing at the layering pattern |
| **OPT4: No user wrapper** | No `Some` keyword, constructor, or pattern. Bare values on the present path |

<!-- test: skip -->
```rask
let user: User? = load()       // present value, widens to User or none
let missing: User? = none      // absent sentinel
```

`T??` is `(T or none) or none`. It's legal and the two layers stay distinct — see [Nesting](#nesting) below.

## Construction

Construction follows the general union widening rule: a value of type `A` widens to `A or B or …` at any position expecting the union (return, assignment, field, argument). For optionals specifically:

| Rule | Description |
|------|-------------|
| **OPT5: No auto-unwrap** | `T?` does not coerce to `T`. Unwrap explicitly via `if x? as v`, `x!`, `x ?? <expr>`, or `try x` |
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
| **OPT11: Other branch** | `x ?? <expr>` | unwraps `x` if present, else evaluates the right side — lazily, only on the miss. The right side is a **value** (a `T` collapses to `T`, another `T?` stays wrapped and keeps chaining, `type.errors/ER14a`) **or any divergence** — `return`, `break`, `continue`, `panic(…)`. `x ?? return Token.Eof`, `x ?? break` are ordinary |
| — | `try x` | unwraps if present, else `none` **leaves to the caller** — so the enclosing function must return a `T?` (`type.errors/ER16`, ER47). The shape rule is the whole constraint; there is no clause |
| **OPT13: Force** | `x!` | extracts if present; panics with `"none"` or `x! "msg"` custom message |
| **OPT15: Absent check** | `x is none` | tests the absent branch. As an `is` test it participates in the general union narrowing rules (mechanism, not idiom — the canonical forms bind or use `??`). Presence is `x?` — there is no `is not none`. `x == none` still typechecks as ordinary equality on a zero-field type, but lints to `is none` (`tool.lint/I5`) |
| **OPT16: `!x?` forbidden** | `!x?` is a parse error suggesting `x is none` |

OPT12 (the `try x else <diverge>` absence-exit construct) is deleted — `try`'s clause is gone language-wide. Propagating absence is bare `try x`; leaving with anything else is `??` with the exit written out.

`??` chains while the left side stays wrapped:

<!-- test: skip -->
```rask
let name = user?.display_name
    ?? user?.email
    ?? "anon"
```

As soon as a right side is bare `T`, the chain collapses to `T` and a further `??` is a type error. The chain works flat because the right side sets the result type — see [error-types.md](error-types.md) ER14a. (The compiler doesn't implement the still-wrapped case yet, so a flat chain needs parentheses until [#578](https://github.com/rask-lang/rask/issues/578) lands.)

<!-- test: skip -->
```rask
let theme = config.theme ?? "default"             // a value — carries on
let home = env("HOME")! "HOME must be set"            // assert — `!` is shorter than ?? panic

// absence leaving, propagated or named — the exit is written where it happens
let prof = try load_user(id)                          // to the caller, as none
let user = load_user(id) ?? return ApiError.NoUser
let item = queue.pop() ?? break
let name = entry.as_string() ?? continue
```

There is no `?? e =>` form — `none` carries no payload to bind. Naming (or explicitly dropping) the payload is `catch e =>` / `catch _ =>`, on the failure shape (`type.errors/ER14`).

## Taking Out of a Mutable Slot

| Rule | Description |
|------|-------------|
| **OPT32: `take <place>`** | `take slot` on a mutable place of type `T?` moves the payload out and leaves `none` behind. The expression yields `T?` — present if the slot was, absent if it already was. The place must be mutable (`mut` binding, or a field path with mutable access); `take` on a `let` place is a compile error |

<!-- test: skip -->
```rask
struct Connection {
    pending: Request?,
}

// move the request out, leave the slot empty — one step, no clone
let req = take conn.pending
if req? as r {
    dispatch(r)
}
```

This is the swap-out idiom — state machines moving a waker, a buffer, or a queued item out of a `mut` field they'll refill later. It's a *mutation*, so neither `match` nor the operators can express it: every other read of an optional either copies, borrows, or consumes the whole slot. Without `take`, moving a non-Copy payload out of a field means dancing around the ownership rules or cloning for no reason.

The keyword is the parameter mode's word on purpose — `take` means "ownership moves out of here" in both positions (`mem.parameters`). For linear payloads the usual rules apply: the taken value must be consumed; the slot itself is left `none` and owes nothing.

Measured need: the move-out-and-leave-none idiom runs at 11 sites per 10k lines in tokio (wakers and futures in `mut` slots) — see [docs/rust-corpus-census.md](../../docs/rust-corpus-census.md). Implementation tracked in [#586](https://github.com/rask-lang/rask/issues/586). `take` on a wider union (`T or E`) is rejected — an error is not a slot you empty; use `match`.

## Conditions and Binding

The `?`-tests don't narrow — they're plain booleans. Getting at the payload is always a **binding**, and there is one way to write it: `as v`.

| Rule | Description |
|------|-------------|
| **OPT19: `if x? as v` binds** | Binds a let `v: T` in the block. Works on any scrutinee — `let`, `mut`, field paths, call results — with no restrictions to remember |
| **OPT18/OPT20/OPT23 deleted** | `if x?` used to narrow `x` in place on let scrutinees (with an else-narrow, and a rule excluding `mut`-rooted paths). Cut: it was a second spelling of test-and-use next to `as v`, and its restrictions were the seam. `x?` is now side-effect-free |
| **OPT21 deleted (with ER21/ER24)** | Early-exit narrowing after a diverging `is none` arm is gone along with all scrutinee narrowing (`type.errors`, Conditions and Binding). `is none` is a plain bool; the guard is `?? <exit>`, and it binds |
| **OPT22: Compounds are just bools** | `x? && y?` is a legal bool expression; to use the payloads, bind — nested `if x? as a`, or restructure |

<!-- test: skip -->
```rask
let user: User? = load()
if user? as u {
    greet(u)                 // the binding is the payload
}
if user? {
    hits += 1                // test-only — no payload touched; user: User? throughout
}

mut cache: Cache? = try_load()
if cache? as c {
    c.sweep()                // c: Cache, immutable, in the block
    // cache still Cache? — may be reassigned below
}
```

`use(x)` inside `if x? { … }` is a compile error (`x` is still `T?`) whose FIX line writes the `as` bind.

The guard idiom — check absence, leave, use the value flat — is the fallback operator, not a narrowing if:

<!-- test: skip -->
```rask
let user = load() ?? return
greet(user)
```

(`if x is none { return }` followed by `use(x)` at `T` is a compile error — `x` is still `T?`; no test changes a type. The guard that binds is `let v = x ?? return`.)

**Anonymous expressions and binding.** `if compute()? as v` binds the result at the check site — no double call, no intermediate binding needed.

## Nesting

Optionals nest. `T??` is `(T or none) or none` and the layers do **not** collapse — the outer one answers one question, the inner one answers another.

This isn't a corner case you have to go looking for. Any generic that returns `T?` produces it the moment `T` is itself optional:

<!-- test: skip -->
```rask
let slots: Vec<Config?> = load_slots()      // a slot may be empty
let first = slots.first()                    // Config??
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
| **OPT30: Operators act on the outer layer** | `?`, `??`, `try`, `!`, `is none` and `match` all see the outer layer only. `if x? as v` binds `v` at the inner type; unwrap again to reach the value. `??`'s value right side must therefore have the inner *optional* type, not the payload type |
| **OPT31: Depth is part of the type** | `T?`, `T??` and `T???` are three different types. Widening adds layers (a `T` reaches a `T??` position, an inner absent stays inner); nothing ever removes one implicitly |

<!-- test: skip -->
```rask
let outer_absent: Config?? = none            // vec was empty        [OPT29]

let empty_slot: Config? = none
let inner_absent: Config?? = empty_slot      // slot was empty       [OPT29]

let present: Config?? = load_config()        // widens through both layers
```

**Spelling.** Write `T??` for two layers — two optional markers, only ever in type position. Nothing else in the language spells two question marks together.

**Linear payloads.** Each layer binds separately, so a linear `T` is consumed on the innermost present path (OPT24 applies at that layer). `?.` still can't reach through a linear payload (OPT25).

**Depth beyond two** is legal and falls out of the same rules, but it's almost always a sign that two questions got layered where one nominal type would read better — `enum SlotState { Missing, Empty, Filled(Config) }`.

## Methods

None. `T?` has no methods at all — the operator surface is the whole API, and the same is true of `T or E` (`type.errors`).

`map`, `filter` and `and_then` used to be here. They exist to thread a *wrapped* value through a pipeline, which is the shape the operators deliberately replace: `try` gets the value or leaves now, `??` gets the value or a substitute now. Once nothing threads wrapped values, a combinator has no job — and measurement agreed, with **zero** uses of any of the three on an optional across stdlib, examples, projects and tests. The same census run over expert Rust (tokio, ripgrep) found `and_then` at ≤2 uses per 10k lines — the combinator style is rare even where the language offers it.

Lifting into a result is `x ?? return MyError`; the reverse is `r catch _ => none` — the discard is acknowledged, because an error is being dropped (`type.errors/ER14`). Both directions are operators that already exist, so neither shape needs a conversion method.

## Linear Resources

A union is linear if any variant is linear (general union rule). For `T or none` where `T` is linear:

| Rule | Description |
|------|-------------|
| **OPT24: Bind consumes on present path** | `if x? as v` treats the present path as a resource site — the bound payload must be consumed on that branch |
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
| `match x { none => a, v => f(v) }` | `if x? as v { f(v) } else { a }` |
| `match x { none => default, u => u.name }` | `x?.name ?? default` |
| `match x { none => return, v => v }` | `x ?? return` |
| `match x { none => panic("…"), v => v }` | `x! "…"` |

The lint is non-fatal. Match earns its keep on multi-error unions where the dispatch genuinely has more than two outcomes.

## Equality

Equality on `T or none` follows the general union equality rule:

- `x is none` — the absent check (canonical). Presence is `x?`
- `x == y` where both are `T?` — true if both absent, or both present and inner values equal

No optional-specific equality rule.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested optionals (`T??`) | OPT28 | Legal — layers stay distinct |
| Bare `none` at a `T??` position | OPT29 | Outer absent |
| `x ?? default` on `T??` | OPT30 | `default` must be `T?`, not `T` |
| `x?.field` on `T??` | OPT3/OPT30 | Compile error — the outer payload is `T?`, not a struct. Bind through the layers |
| `Vec<T?>.first()` | OPT28 | `T??` — outer says "vec empty", inner says "slot empty" |
| `?.` on `T or E or none` | OPT3 | Compile error suggesting layering: `(T or E)?` or `T or (E?)` |
| `x ?? return E` | OPT11 | Legal — the exit is written where it happens |
| `x ?? break` / `?? continue` | OPT11 | Legal — any divergence |
| `x ?? panic(…)` | OPT11 | Legal, but `x! "…"` is shorter; lint suggests it |
| `x ?? e => f(e)` | ER14 | Compile error — no payload to bind. That form is `catch e =>`, on the failure shape |
| `x catch e => …` on an optional | ER14 | Compile error — `catch` handles failures; absence has nothing to catch. Use `??` |
| `try x` in a `U?`-returning function | ER16/ER47 | Legal — propagates `none`; the ordinary spelling |
| `try x` in a `T or E`-returning function | ER47 | Compile error — `none` doesn't fit an error branch. Use `x ?? return <error>` |
| `use(x)` inside `if x? { … }` | OPT19 | Compile error — `x` is still `T?`; the test doesn't narrow. FIX writes `if x? as v` |
| `if compute()? as v` | OPT19 | Legal — binds the call result at the check site, no intermediate binding |
| `!x?` syntax | OPT16 | Parse error suggesting `x is none` |
| Linear `?.field` | OPT25 | Compile error — cannot partially move |
| `x ?? return MyError` where `MyError` isn't in the function's return type | ER9 | Compile error — normal `return` rules apply |
| `match` on `T?` with two arms | OPT27 | Legal; style lint suggests operators |
| `let x = none` | OPT8 | Legal. `x: none`. Widens at later use site |
| `none == none` | equality | `true`. Standard equality on a zero-field type |

## Error Messages

**Operator on wider union [OPT3]:**
```
ERROR [type.optionals/OPT3]: `?.` requires a two-variant union with `none`
   |
5  |  let name = result?.display_name
   |               ^^^^^^^ `result` is `User or DatabaseError or none` — three variants

WHY: The `?`-family operators handle the absent-or-present case. For unions
     with multiple non-absent variants, layer the types or use `match`.

FIX: Layer them — error on the inside, optionality on the outside:

  func find(id: UserId) -> (User or DatabaseError)? { ... }

  let outer = find(id)
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

FIX: if user is none { return }
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

**OPT11/OPT12 (the fallback settled in three rounds; the absence-exit construct is deleted).** Round one had `??` on both shapes; round two split per shape; round three briefly merged both into one word (`orelse`) on the argument that fallback is the same operation on both shapes. Reading real code killed the merge: at every fallback site the reader's first question is *"was there an error just now, and did it survive?"* — and the merged word couldn't answer it without a signature lookup. The resolution keys on what the operation does to information. An absent miss carries no payload — nothing is lost, so the absence fallback is terse: `??`, restored, right side a value **or any divergence** (`x ?? return Token.Eof` — the exit written where it happens; that latitude is round three's keeper). A discarded *error* destroys information, which Transparency of Cost says must be visible — so the failure fallback (`catch`, `type.errors/ER14`) always carries a binder and has no bare-value form at all.

The `try x else <diverge>` construct died with round three and stays dead: once `??`'s right side may diverge, the clause bought nothing but an extra keyword. Of the 72 absence-exits in the tree, 65 name a specific target (`?? return Token.Eof`, `?? break`) and 7 propagate `none` unchanged — those are bare `try x`, Rust's `?`-on-`Option`, which production Rust uses everywhere.

The flagship measured the split's cost at zero: of its 34 fallback sites, 28 are absence and 6 are failure — the shapes were never symmetric in practice, and marking the rare, information-destroying side costs six binders.

**OPT15 (`is none`, not `== none`).** The absent check used to be spelled with equality, which meant asking a *shape* question with the *value* verb. `is` tests a branch everywhere else in the language — `r is IoError`, `shape is Circle` — and `T?` is `T or none`, so `x is none` is that same test on the same machinery, not a new form. It also puts the absent check inside the family: before this, the memorable set was `?`, `?.`, `??`, `!`, and absence was outside it in a different register, with `!x?` banned (OPT16) and nothing obvious to reach for instead.

Presence stays `x?` and there is no `is not none`. Two spellings of presence (`x?` and `x != none`) collapse to one, and negation was the direction OPT16 already rules out.

`x == none` isn't made illegal. `none` is a zero-field type with one inhabitant, so equality on it is ordinary and `none == none` is `true` — banning the comparison in one position while it works in another would be a special case earning nothing. A lint carries the preference instead.

**Why `x? as v` survives (and `x is T as v` doesn't replace it).** `is` and `as` compose on results because `E` can be a union: `r is ParseError as e` genuinely *selects* among alternatives. An optional has exactly two branches, so naming the non-`none` one carries no information — and it costs real noise when the payload is generic:

<!-- test: skip -->
```rask
if player_ent.target? as handle { … }                      // T? is binary; the type adds nothing
if player_ent.target is Handle<Entity> as handle { … }     // same test, spelled out
```

So test-and-bind is the one row where the two shapes don't share a construct, and that's the honest outcome rather than a gap: `is` needs a branch name where branches are open, and can't use one where there are only two.

**OPT16 (`!x?` forbidden).** `!x?` parses right-to-left but reads left-to-right as "not present" — the directions fight. `x is none` is unambiguous. The rule is specific to `!` directly applied to a `?`-suffixed expression; other uses of `!` on booleans stay normal.

**OPT27 (match is a lint, not an error).** Hard errors should enforce safety or correctness, not style. Match on a two-arm union is perfectly safe; it's just verbose. A lint catches the common case.

**OPT28 (nesting is allowed; it used to be an error).** The earlier rule — `T??` is a duplicate-variant error — was written for optionals you spell out by hand, where nobody wants two layers anyway. It doesn't survive generics. `func head<T>(v: Vec<T>) -> T?` is about as ordinary as generic code gets, and under the old rule it silently failed to compile for every optional `T`. Rejecting it is the C++-template failure mode in the most boring code imaginable, and there's no bound the author could have written to warn you.

The alternatives were worse. Collapsing (Kotlin) throws away the distinction between "the vec was empty" and "the first slot was empty" — the caller of `first()` can no longer recover what happened. Erroring at instantiation makes every `-> T?` generic partial over `T`, which is precisely the non-compositionality being fixed for `T or E` (`type.errors/ER3a`).

The reason nesting works here and not for `T or E` is that branch selection stays decidable at every layer. Producing: `return v` picks by the declared type *before* substitution, so instantiation never changes which branch a `return` chose. Consuming: the outer operators act on the outer layer and the inner layer is only reachable by narrowing through it. The single leftover ambiguity — a bare `none` literal, which could mean either layer — is closed by OPT29.

**OPT29 (`none` binds outermost).** Both readings are defensible; picking one and stating it is what matters. Outermost wins because it matches how the layers get built: the outer layer is the one the immediate context added (`first()` may fail to find anything), so `none` at that position means "the thing right here is absent". Reaching an inner absent means you already have an inner-typed value in hand, and widening it is explicit.

**Why `x? as v` and not a let-in-condition.** The bind looks like Rust's `if let Some(v) = x` only functionally — syntactically it's the C# shape (`if (x is string s)`): scrutinee first, test, then name, reading forward. What makes let-Some unintuitive is exactly what this form doesn't do: the name appearing before the value it comes from, the wrapper noise, and a declaration smuggled into a condition. A Swift-style let-in-condition (`if let v = x`) was the alternative; it buys familiarity with a new grammar position, reads value-last again, and breaks the language's one binding habit — **`as name` comes after whatever just proved a value exists**, uniformly: `if x? as v`, `if r is Timeout as t`, `match { User as u => … }`, `else as e`. One rule, four positions. The honest seam — `as v` sits after a boolean-looking expression but binds the *payload* — is resolved by that same rule: `as` always names what the test proved, and `is E as e` trains the reading. `while queue.pop()? as item` falls out for free. And the no-rename convenience the narrowing cut removed is recoverable per-site with ordinary shadowing: `if x? as x { … }` — no fine print.

**The `?`-tests don't narrow (OPT18/OPT20/OPT23 deleted).** An earlier revision let `if x?` narrow `x` in place on let scrutinees, with an else-narrow and a rule excluding `mut`-rooted field paths. Cut, for the one-way principle: `if x? { use(x) }` and `if x? as v { use(v) }` were two spellings of test-and-use, and only one of them worked everywhere. The restrictions were the tell — "let scrutinees only", "not through a `mut` path", "anonymous expressions don't narrow" were three rules whose whole job was propping up the redundant spelling; the `as v` bind has none of them. A follow-up pass cut the `is`-side scrutinee narrowing too (ER21/ER24/OPT21) — no test of any kind changes a type now, and the checker has zero flow typing. Payload access is bindings, everywhere.

### Patterns & Guidance

**Absent as default input.** For an Option-valued field with a sensible default, read with `??`:

<!-- test: skip -->
```rask
let theme = config.theme ?? "default"
```

**Early exit on absence.** The guard is the fallback operator with the exit (or the default) written out — one line, and the binding is the payload from then on:

<!-- test: skip -->
```rask
func greet(id: UserId) -> string {
    let user = load_user(id) ?? return "Hello, guest"
    return "Hello, {user.name}"
}
```

When absence should leave rather than produce a value, same shape:

<!-- test: skip -->
```rask
func greet(id: UserId) -> string or ApiError {
    let user = load_user(id) ?? return ApiError.NotFound(id)
    return "Hello, {user.name}"
}
```

And when it should leave *as absence*, in a function that returns an optional itself, `try` is the one-word spelling:

<!-- test: skip -->
```rask
func lookup(id: UserId) -> Profile? {
    let user = try find_user(id)        // absent → this function returns none
    return try user.profile               // same again
}
```

**Binding from a `mut` scrutinee.** The bind works the same on `mut` — the let `c` is a stable name for the payload while the slot stays reassignable:

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

- Ghost text shows the bound type on hover for `as v` bindings.
- Quick action "Convert `match` to operator form" for the two-arm none/value case.
- Quick action "Convert `try … else <diverge>` to `?? <diverge>`", "Convert `orelse` to `??`/`catch` by shape", and "Add `as v` bind" for `use(x)` inside `if x?` — code written against the older rules.
- Quick action "Convert `?? return none` to `try`" when the enclosing function returns an optional.

### See Also

- [Union Types](union-types.md) — general union rules (`type.unions`)
- [Error Types](error-types.md) — `T or E`, `try`, `catch`, the union narrowing mechanism (`type.errors`)
- [Control Flow](../control/control-flow.md) — if/match/narrowing (`ctrl.flow`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
