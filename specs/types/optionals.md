<!-- id: type.optionals -->
<!-- status: decided -->
<!-- summary: T? is a builtin status type — bare values on the present path, none as sentinel. Operator-only surface (?, ?., ??, !, try, == none). No Some/None wrappers, no match arms. Narrowing rides on const. -->
<!-- depends: types/types.md, control/control-flow.md -->

# Optionals

`T?` is a builtin "present or absent" status type. Not an enum — the language provides the status directly, with a dedicated operator family (`?`, `?.`, `??`, `!`, `try`, `== none`) and no user-visible constructor. A value of type `T` is already a `T?` at the boundaries where auto-wrap applies; `none` is the absent sentinel.

Enums get `match`; Option gets operators. That line is load-bearing — it's why Option has `T?` sugar, auto-wrap, and the `?`-family while user enums use pattern matching.

## The Type

| Rule | Description |
|------|-------------|
| **OPT1: Builtin status** | `T?` is a compiler-generated tagged union of "present `T`" or "absent," not a user-definable enum |
| **OPT2: No user wrapper** | There is no `Some` constructor, keyword, or pattern. Present values are bare |
| **OPT3: Absent sentinel** | `none` is a literal denoting absence; context infers its type |
| **OPT4: Nested optionals forbidden** | `T??` is a compile error; use a named enum like `T or NotFound` if you need to distinguish two flavours of absence |

<!-- test: skip -->
```rask
const user: User? = load()       // present value, auto-wrapped
const missing: User? = none       // absent sentinel
```

## Construction

| Rule | Description |
|------|-------------|
| **OPT5: Auto-wrap at return** | In a function with return type `T?`, returning a value of type `T` is wrapped automatically |
| **OPT6: Auto-wrap at assignment** | Assigning a value of type `T` to a binding of type `T?` wraps automatically. Extends to field initialisers and `const`/`mut` declarations with explicit `T?` type |
| **OPT7: No auto-unwrap** | `T?` does not coerce to `T` — unwrap explicitly via `if x?`, `x!`, `x ?? default`, or `try x` |

<!-- test: skip -->
```rask
func load_user() -> User? { … }          // bare User return auto-wraps

mut cache: User? = none                  // literal; stays absent
cache = get_current_user()                // User → User? at assignment
```

## Operators

| Rule | Syntax | Meaning |
|------|--------|---------|
| **OPT8: Type shorthand** | `T?` | the Option type |
| **OPT9: Absent literal** | `none` | absent value; type inferred |
| **OPT10: Boolean present** | `x?` | `true` when present, `false` when absent; `bool` expression |
| **OPT11: Optional chain** | `x?.field` | accesses `field` when present, else `none`; short-circuits |
| **OPT12: Value fallback** | `x ?? default` | unwraps `x` if present, else yields `default`; both operands must have compatible inner types |
| **OPT13: Diverging fallback** | `x ?? return none` (or `break`, `continue`, `panic(…)`) | unwraps `x` if present, else diverges |
| **OPT14: Force** | `x!` | extracts if present; panics with "none" or `x! "msg"` custom message |
| **OPT15: Propagate** | `try x` | in a `T?`-returning function, unwraps if present, else returns `none` |
| **OPT16: Absent check** | `x == none` / `x != none` | plain equality; `x?` and `x == none` narrow identically |
| **OPT17: `!x?` forbidden** | `!x?` is a parse error suggesting `x == none` |
| **OPT18: Propagate block** | `try { … }` | unwraps each `try` inside; propagates `none` on the first absent scrutinee |

<!-- test: skip -->
```rask
// Chain and fallback
const name = user?.profile?.display_name ?? "Anonymous"

// Force (asserts present)
const first = list.first()!

// Propagate inside a T?-returning function
func find_admin() -> User? {
    const user = try lookup("root")
    return user
}
```

## Conditions and Narrowing

| Rule | Description |
|------|-------------|
| **OPT19: `if x?` narrows** | In a `const` scrutinee, `if x?` narrows `x` to `T` inside the block |
| **OPT20: `if x? as v` binds** | Binds a const `v: T` in the block; works for `mut` scrutinees, and for renaming |
| **OPT21: Both branches narrow** | On a const scrutinee, the `else` branch is narrowed to "absent" (information-only) |
| **OPT22: Early-exit narrow** | If a branch of `if x == none { … }` diverges, `x` is `T` in the fall-through |
| **OPT23: No compound narrowing** | `x? && y?` is a legal bool expression but does not narrow either side — use nested `if` or `as v` bind |
| **OPT24: No field-path narrow through mut** | `player.weapon` narrows iff the full path is rooted in a `const` binding. With `mut` anywhere in the path, use `if player.weapon? as w` |

<!-- test: skip -->
```rask
const user: User? = load()
if user? {
    greet(user)              // user: User here
}

mut cache: Cache? = try_load()
if cache? as c {
    c.sweep()                 // c: Cache (const) in the block
    // cache still Cache? — may be reassigned below
}

// Early-exit guard
const user: User? = load()
if user == none {
    return
}
greet(user)                   // user: User after the guard
```

## Methods

Four compiler-provided methods on `T?`. Each preserves the wrapper for chaining; operators always extract or panic.

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

| Rule | Description |
|------|-------------|
| **OPT25: Linearity propagates** | If `T` is linear, `T?` is linear. Both paths (present and absent) must be handled |
| **OPT26: Narrow consumes on present path** | `if x?` / `if x? as v` treats the present path as a resource site — the payload must be consumed on that branch |
| **OPT27: `?.` forbidden on linear** | Optional chaining cannot partially move out of a linear `T`. Use `if x? as v { … v.field … }` |
| **OPT28: `??` consumes one branch** | Short-circuits; exactly one `T` is produced and must be consumed |

<!-- test: skip -->
```rask
mut file: File? = open("data.txt")
if file? as f {
    try f.write(content)
    try f.close()             // consumed on present path
}
// absent path has no resource to consume
```

## No Match on Option

Match dispatches over multi-branch types. Option has two states — every useful pattern factors through operators, usually shorter.

| Match form (rejected) | Operator form |
|----------------------|---------------|
| `match x { none => a, v => f(v) }` | `if x? { f(x) } else { a }` |
| `match x { none => default, u => u.name }` | `x?.name ?? default` |
| `match x { none => return, v => v }` | `x ?? return none` (or `try x`) |
| `match x { none => panic("…"), v => v }` | `x!` (or `x ?? panic("…")`) |

The compiler rejects `match` on `T?` with a first-class diagnostic (see Error Messages below).

## Comparison

| Rule | Description |
|------|-------------|
| **OPT29: Equality with `none`** | `x == none` / `x != none` are the canonical absent/present predicates |
| **OPT30: Inner equality** | `x == y` when both are `T?` compares inner values (present-present) or returns true for absent-absent |

<!-- test: skip -->
```rask
if user == none { return default_profile() }
if a == b { … }   // compares inner or both-absent
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Nested optionals (`T??`) | OPT4 | Compile error |
| `match x { … }` on `T?` | — | Compile error with operator suggestions |
| `x` is `mut` in `if x?` | OPT19 | No narrow; use `if x? as v` |
| Anonymous expression in condition | OPT19 | `if compute()?` does not narrow — no name to refine. Use `const v = compute()` or `if compute()? as v` |
| Auto-wrap assignment of non-`T?`-typed binding | OPT6 | Type mismatch — annotation `: T?` is what triggers the wrap |
| `!x?` syntax | OPT17 | Parse error suggesting `x == none` |
| Linear `?.field` | OPT27 | Compile error — cannot partially move |
| `try x` outside a `T?`-returning function | OPT15 | Compile error — propagation target mismatch |

## Error Messages

**Match on Option [migration]:**
```
ERROR [type.optionals/NO_MATCH]: Option cannot be matched
   |
5  |  match user { Some(u) => …, None => … }
   |  ^^^^^ Option is a builtin status type, not an enum

WHY: Option has two states — present and absent — and the ?-family
covers both more concisely than a match.

FIX: use operators instead:

  if user? { … } else { … }                   // branching
  if user? as u { use(u) } else { default() } // with a fresh name
  user?.name ?? "Anonymous"                   // chain + fallback
  if user == none { return }                  // early-exit
  greet(user)                                 // user: User here

Match is for enums with three or more branches.
```

**`Some(v)` / `None` at construction [migration]:**
```
ERROR [type.optionals/NO_WRAPPER]: Some/None are not valid in Rask
   |
3  |  return Some(user)
   |         ^^^^^^^^^^ bare value auto-wraps to User? at return

FIX: return user   (or none for absent)
```

**`!x?` forbidden [OPT17]:**
```
ERROR [type.optionals/OPT17]: cannot negate `x?` with prefix `!`
   |
8  |  if !user? { return }
   |     ^^^^^^ mixes prefix ! with suffix ? ; fights the parse

FIX: if user == none { return }
```

**Nested optional [OPT4]:**
```
ERROR [type.optionals/OPT4]: nested optional type `User??` is not allowed
   |
2  |  const x: User?? = …
   |           ^^^^^^ T?? cannot distinguish "absent" from "explicitly-none inner"

FIX: Use a named enum if you need two flavours of absence:
     type LookupResult = User or NotFound
```

---

## Appendix (non-normative)

### Rationale

**OPT1 (builtin status).** The original spec had Option as an enum with dedicated sugar (`T?`, `?.`, `??`, `!`) bolted on top. Calling it "just an enum" is a fiction — Option has more dedicated surface than any other type in the language. The proposal makes the spec agree with the language: Option is builtin, operators are its interface, enums are a different thing.

**OPT5/OPT6 (auto-wrap).** Writing `return Some(user)` when the function returns `User?` adds a tag that is always the same tag. The wrapper is redundant. Auto-wrap at return and assignment lets the type system do the work; the source stays clean.

**OPT17 (`!x?` forbidden).** Mixing prefix `!` with suffix `?` mixes reading directions (`!x?` parses right-to-left but reads left-to-right as "not present"). `x == none` is unambiguous. The rule is specific to `!` applied to a `?`-suffixed expression; other uses of `!` on booleans stay normal.

**No match on Option.** Keeping match would require `some`/`none` as pattern keywords with no construction counterparts. That asymmetry is exactly the Rust-legacy ceremony the redesign removes. The rejection is the pedagogical move that makes "Option is not an enum" true in practice.

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

**Chains with multiple fallbacks.** `??` composes while the left side stays wrapped:

<!-- test: skip -->
```rask
const name = user?.display_name
    ?? user?.email
    ?? "anon"
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

### IDE Integration

- Ghost text shows the narrowed type on hover inside `if x?` blocks.
- Quick action "Convert `match` to operator form" for migrated code.
- Ghost text for the diverging `??` fallback shows the return type of the enclosing function.

### See Also

- [Error Types](error-types.md) — `T or E`, `try`, union errors (`type.errors`)
- [Control Flow](../control/control-flow.md) — if/match/narrowing (`ctrl.flow`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — decision record for the operator-only surface
