<!-- id: type.optional-unification -->
<!-- status: proposed -->
<!-- summary: T? becomes sugar for T or none. none is a built-in zero-field type. The ?-family operators work on any two-variant union where one variant is none. No source-level changes; spec shrinks and the "Option is a special kind of type" rule goes away -->
<!-- depends: types/optionals.md, types/error-types.md, types/union-types.md -->

# Option Unification

`T?` today is a builtin "status type" with its own rulebook: its own construction rules, its own auto-wrap rules, its own linearity propagation, its own ban on nesting, its own "match is forbidden" rule. It lives parallel to `T or E`, which is the regular union machinery.

This proposal collapses the two. `T?` becomes syntactic sugar for `T or none`. `none` is a built-in zero-field type (lowercase, like `void`). The `?`-family operators (`?`, `?.`, `??`, `!`, `try`, `== none`) apply to any two-variant union where one variant is `none`. Every other rule about optionals falls out of the general union rules.

Source code stays identical. The spec shrinks.

## Motivation

`type.optionals` today has 30 rules (OPT1–OPT30). Most of them restate, for optionals, rules that already exist for unions:

| Optional rule | General equivalent |
|---------------|-------------------|
| OPT5/OPT6 — auto-wrap `T` → `T?` at return/assignment | Union widening at coercion sites |
| OPT4 — `T??` forbidden | Unions can't have duplicate variants |
| OPT25 — linearity propagates through `T?` | A union is linear if any variant is linear |
| OPT29 — `x == none` as present check | Plain equality against a value of type `none` |
| OPT30 — inner equality on `T?` | Union equality: same-variant AND inner equal |

The current spec acknowledges this tension in OPT1's rationale: "Option has more dedicated surface than any other type." I think that framing is backwards — the dedicated surface is on the *operators*, not on the type. Let the operators remain; let the type be an ordinary union.

## Design

### Core change

| Rule | Description |
|------|-------------|
| **OU1: `none` is a built-in zero-field type** | Lowercase (matches `void`, `bool`, `i32`). One value, also spelled `none`. Not user-definable |
| **OU2: `T?` is sugar for `T or none`** | Parser desugars before type checking. No separate "optional" type kind |
| **OU3: `?`-family operators restricted to `T or none`** | `?`, `?.`, `??`, `!`, `try`, `== none` only apply where the operand type is exactly a two-variant union with one variant `none`. Wider unions (e.g. `T or E or none`) get a compile error suggesting the layering pattern |
| **OU4: Widening subsumes auto-wrap** | `T` → `T or none` at return, assignment, field init, argument — the same widening rule that coerces `T` → `T or E` at return. No optional-specific auto-wrap |
| **OU5: `match` on `T or none` is legal** | No special prohibition. A style lint suggests operators when the match is two-arm and one arm is `none` |

### What the operators mean

Operators dispatch on the structural shape "two-variant union with one variant `none`." The compiler checks the operand type at the operator site.

<!-- test: skip -->
```rask
func load() -> User? { ... }                    // desugars to User or none

const user = load()                             // user: User or none
if user? { greet(user) }                        // narrows to User
const name = user?.display_name ?? "Anonymous"  // chain + fallback
const first = list.first()!                     // force
try user                                         // propagate in a ?-returning fn
```

Every line above is identical to what you'd write today. Only the desugaring target differs.

### Multi-variant unions with `none`

`T or E or none` is a legal union type. The `?`-family does *not* apply to it (OU3). This keeps the operators teachable ("absent-or-present") and keeps result types from spiralling through chained `?.` calls.

If you need both an error and absence, layer them:

<!-- test: skip -->
```rask
func lookup(id: UserId) -> User or DatabaseError {
    // returns DatabaseError on connection failure, User on success
    // returns User-with-absent-fields, not absence
}

func find(id: UserId) -> (User or DatabaseError)? {
    // outer ? indicates "not found"; inner union indicates DB error on lookup
}
```

Error-handling on the inside, optionality on the outside. The `?`-family works on the outer layer; `try` and match handle the inner.

### Nested optionals

`T??` is `(T or none) or none`. The union duplicate-variant rule rejects this — no special case needed. OPT4 deletes.

<!-- test: compile-fail -->
```rask
const x: User?? = ...   // ERROR: duplicate variant `none` in union
```

### Auto-wrap becomes widening

Today's OPT5/OPT6 say `T` auto-wraps to `T?` at return and assignment. `type.errors` has the equivalent rule ER7 for `T or E`. Both become instances of one widening rule:

> When a value of type `A` is used in a position expecting a union type `A or B or ...`, the value widens to the `A` variant.

No optional-specific auto-wrap rule remains. `return user` in a `User or none`-returning function widens by the same mechanism as `return user` in a `User or DatabaseError`-returning function.

### Linearity

`mem.linear` already has to handle unions with linear variants for `T or E` where `T` is linear. Extend the rule uniformly: a union is linear if any variant is linear. `T?` where `T` is linear is covered by this — OPT25 deletes.

### Narrowing

OPT19–OPT24 stay, restated over `T or none`. `if x?` narrows a `const x: T or none` to `T` inside the block. The narrowing rules are the same; they just describe a specific union shape instead of a distinct type.

### Inference

`none` is a value of type `none`. Use sites have a target type (return type, parameter type, annotated binding), and widening covers them all. No special "what T?" inference is needed:

<!-- test: skip -->
```rask
const cache: User? = none        // widens at assignment: target is User or none
return none                       // widens at return: target is return type
some_fn(none)                     // widens at argument: target is parameter type
const x = none                    // x has type `none` (legal, just not useful)
```

The last line isn't an error. `x` has type `none`. It can later be assigned into a `T or none` slot where it widens.

## What doesn't change

Every line of Rask source code compiles identically. The changes are all inside the spec and compiler.

| Surface | Today | After |
|---------|-------|-------|
| Type annotation | `User?` | `User?` (sugar for `User or none`) |
| Absent literal | `none` | `none` |
| Chain | `x?.field` | `x?.field` |
| Fallback | `x ?? default` | `x ?? default` |
| Force | `x!` | `x!` |
| Propagate | `try x` | `try x` |
| Present check | `x?`, `x == none` | `x?`, `x == none` |
| Narrow | `if x?`, `if x? as v` | `if x?`, `if x? as v` |
| Auto-wrap at return | works | works (by widening) |
| Auto-wrap at assignment | works | works (by widening) |

## What does change

**`match` on `T?` compiles.** Today it's a hard error with a migration diagnostic. After: it's legal. Operators are still shorter, so a style lint nudges users toward them. The compiler doesn't enforce.

<!-- test: skip -->
```rask
// Legal after unification, but the lint suggests operators
match user {
    none => "guest",
    u => u.name,
}

// Preferred — operators are shorter
user?.name ?? "guest"
```

**Spec surface.** OPT1, OPT4, OPT5, OPT6, OPT25, OPT29, OPT30 collapse into general union rules. `type.optionals` becomes a shorter file that documents the `?`-family operators and their restriction to two-variant unions with `none`. The "status type" framing retires.

**Compiler.** One fewer type kind. The parser desugars `T?` → `T or none` before type checking; the rest of the pipeline sees a regular union.

## Migration

None. This is a source-compatible reformulation. The `NO_MATCH` diagnostic becomes a style lint instead of a hard error — that's the one behavior change, and it relaxes a restriction rather than adding one.

Existing `.rk` code continues to compile. Existing documentation continues to describe the language correctly at the source level. Spec files that cite OPT* rules need updating to cite the general union rules instead.

## Edge cases

| Case | Rule | Handling |
|------|------|----------|
| `T??` | OU3 + union rules | Compile error: duplicate variant `none` |
| `match` on `T?` | OU5 | Legal; style lint suggests operators for two-arm matches with `none` |
| `T or E or none` with `?.` | OU3 | Compile error: `?.` requires a two-variant union with `none`. Suggest layering |
| `const x = none` | OU4 | Legal. `x` has type `none`. Widens at later use |
| `none == none` | OU1 | `true`. Same rule as equality on any zero-field type |
| `none` in generic position | OU4 | Widens to the target union type if the context provides one. Otherwise `x: none` |
| Linear `T?` with `T: @resource` | Union linearity | Both variants must be handled; present path consumes the resource |

## Error messages

**Restricted operator on wider union [OU3]:**
```
ERROR [type.optional-unification/OU3]: `?.` requires a two-variant union with `none`
   |
5  |  const name = result?.display_name
   |               ^^^^^^^ `result` is `User or DatabaseError or none` — three variants

WHY: The `?`-family operators handle the absent-or-present case. For unions
     with multiple non-absent variants, use `match` or `try` to disambiguate.

FIX: Layer the types. Error on the inside, optionality on the outside:

  func find(id: UserId) -> (User or DatabaseError)? { ... }

  const outer = find(id)
  if outer? as r {
      match r {
          .Ok(user) => use(user),
          .Err(e)   => log(e),
      }
  }
```

**Match on `T or none` with two arms [style lint, non-fatal]:**
```
LINT [type.optional-unification/lint-match]: prefer operators over `match` on optional
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

**OU1 (lowercase `none`).** Rask's primitives are lowercase (`i32`, `bool`, `string`, `void`); user-facing types are capitalized (`User`, `Vec`, `Error`). `none` is builtin, not a user type, so it follows the primitive convention. Uppercase `None` would read like an enum variant you have to import — exactly the framing this proposal moves away from.

**OU2 (sugar, not a distinct kind).** The teaching burden today is "Option is a builtin status type, different from enums and unions, with its own operators and its own construction rules." After: "`T?` is shorthand for `T or none`, and the `?`-operators handle that shape." Shorter to teach, fewer rules to remember.

**OU3 (restrict operators to two-variant unions).** I considered generalizing `?.` to pass through other variants on a multi-variant union. The result types get hairy fast — `user?.profile?.name` on `User or DBError or none` returns something like `string or DBError or DBError or none`. Coherent but unreadable. Restriction is the simpler rule and it matches today's behavior; users who mix absence and errors layer the types, which is the cleaner pattern anyway.

**OU4 (widening subsumes auto-wrap).** OPT5/OPT6 and ER7 are the same rule stated twice. Merging them removes the duplication. The widening rule is already needed for `T or E` to behave sensibly at function boundaries — extending it to cover `T or none` is free.

**OU5 (match becomes legal).** The "no match on Option" rule in the current spec is pedagogical, not structural. Nothing breaks if someone writes a two-arm match; it just reads longer than operators. A lint catches the common case. I prefer a lint here over a hard error — the language stays regular, and the style guidance lives where style guidance belongs.

**Why not keep `T?` as a distinct kind "for clarity"?** The clarity was always about the operators, not about the type representation. Users never see the "tagged union" internals; they see `User?`, `none`, and the operator family. Making the type a union underneath doesn't change what users see — it just stops the spec from lying about what the compiler does.

### What was considered and rejected

**Generalize `?.` to any union with `none`.** Rejected as OU3 — the result types get unwieldy and the operator stops being teachable. Layering is the cleaner discipline.

**Keep `Option<T>` as a user-facing enum name.** Rejected. The point of the unification is to stop treating optional as special. Reintroducing a user-facing name would split the surface again.

**Retain the "match forbidden" rule as a hard error.** Rejected. Hard errors should enforce safety or correctness, not style. Match on a two-arm union is perfectly safe; it's just verbose.

### Patterns & Guidance

**Optional inside a struct field — same as today:**

<!-- test: skip -->
```rask
struct User {
    name: string
    nickname: string?       // sugar for (string or none)
}

const u = User { name: "Tove", nickname: none }
```

**Error and optional layered:**

<!-- test: skip -->
```rask
func find_user(id: UserId) -> (User or DatabaseError)? {
    try { db.query(id) }    // DB errors propagate via inner union
                             // outer `?` wraps the success case
}

const outer = find_user(id)
if outer? as result {        // narrow: present
    match result {
        .Ok(user) => greet(user),
        .Err(err) => log(err),
    }
} else {
    log("not found")
}
```

### See Also

- [Optionals (current)](optionals.md) — Existing `T?` spec; this proposal supersedes
- [Error Types](error-types.md) — `T or E`, `try`, widening rules shared by this proposal
- [Union Types](union-types.md) — Duplicate-variant rule that subsumes OPT4
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — Companion proposal on the wider error model
