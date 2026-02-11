<!-- id: type.optionals -->
<!-- status: decided -->
<!-- summary: Option<T> enum with T?, none, ?., ?? sugar -->
<!-- depends: types/enums.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Optionals

`Option<T>` is a standard enum with syntax sugar: `T?` for type, `none` for absence, `?.` for chaining, `??` for defaults.

## The Option Type

| Rule | Description |
|------|-------------|
| **OPT1: Standard enum** | `Option<T>` is a normal enum with `Some(T)` and `None` variants |
| **OPT2: Full enum support** | Pattern matching, traits, and generics all work on `Option<T>` |

```rask
enum Option<T> {
    Some(T),
    None,
}
```

## Syntax Sugar

| Rule | Sugar | Meaning |
|------|-------|---------|
| **OPT3: Type shorthand** | `T?` | `Option<T>` |
| **OPT4: Absence literal** | `none` | `Option.None` (type inferred from context) |
| **OPT5: Optional chaining** | `x?.field` | Access field if present, else `none` |
| **OPT6: Nil-coalescing** | `x ?? y` | `x` if present, else `y` (short-circuits) |
| **OPT7: Force unwrap** | `x!` | Extract value or panic if `none` |

## Auto-wrapping

| Rule | Description |
|------|-------------|
| **OPT8: T to Option coercion** | `T` coerces to `Option<T>` automatically |
| **OPT9: No reverse coercion** | `Option<T>` does NOT coerce to `T` — must unwrap explicitly |

```rask
let user: User? = load_user()    // wraps to Some(user)
```

## The `none` Literal

```rask
let x: User? = none
func find(id: i64) -> User? { none }
```

Type inferred from context. Equivalent to `Option.None`.

## Optional Chaining: `?.`

```rask
user?.profile?.settings?.theme
```

| x is | `x?.field` |
|------|------------|
| Some(v) | Some(v.field) or v.field wrapped |
| None | none |

## Nil-Coalescing: `??`

```rask
const name = user?.name ?? "Anonymous"
```

| x is | `x ?? y` |
|------|----------|
| Some(v) | v (unwrapped) |
| None | y |

Short-circuits: `y` evaluated only if `x` is none.

## Force Unwrap: `!`

```rask
const user = get_user()!    // panics if none
```

Use sparingly. Prefer `??` or pattern matching.

## Methods

| Method | Behavior |
|--------|----------|
| `map(f)` | Transform if present |
| `filter(pred)` | Keep if predicate true |
| `ok_or(err)` | Convert to Result |
| `unwrap()` | Extract or panic |
| `unwrap_or(default)` | Extract or default |
| `is_some()` | Has value? |
| `is_none()` | Is absent? |

## Pattern Matching

Standard enum matching:

```rask
match user {
    Some(u) => process(u),
    None => handle_missing(),
}
```

Single-variant check with `if is`:

```rask
// Explicit binding
if user is Some(u) {
    process(u)
}

// Implicit unwrap (reuses outer name)
if user is Some {
    process(user)  // user unwrapped automatically
}
```

Guard pattern for early exit:

```rask
const user = get_user(id) is Some else { return None }
// user available here (unwrapped)
```

## Linear Resources

| Rule | Description |
|------|-------------|
| **OPT11: Linear propagation** | If `T` is linear, `T?` is linear — must handle both paths |

```rask
let file: File? = open("data.txt")
if file is Some(f) {
    f.close()
}
```

## Comparison

```rask
x == none       // is absent
x != none       // has value
x == y          // compare inner values or both none
```

## Propagation with `try`

| Rule | Description |
|------|-------------|
| **OPT12: try propagates None** | `try x` on `Option` propagates `None` to caller |
| **OPT13: Return type required** | `try x` on `Option` only valid in function returning `Option<U>` |
| **OPT14: Explicit conversion** | Mixing `Option` and `Result` requires `.ok_or(err)` or `.ok()` |

| Syntax | Option | Result |
|--------|--------|--------|
| `try x` | Propagate None | Propagate Err (with union widening) |
| `x ?? y` | Value or default | — |
| `x!` | Force (panic: "None") | Force (panic: "Err: ...") |
| `x! "msg"` | Force (panic with message) | Force (panic with message) |

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `T` auto-wraps to `Option<T>` | OPT8 | Implicit coercion |
| `Option<T>` cannot auto-unwrap | OPT9 | Must use `??` or pattern match |
| `??` on Result | — | Not supported — use `.on_err(default)` |
| Linear `T` in `Option` | OPT11 | Both `Some` and `None` paths must be handled |
| `try` in non-Option function | OPT13 | Compile error |

## Error Messages

**Force unwrap on none [OPT7]:**
```
ERROR [type.optionals/OPT7]: force unwrap on none value
   |
5  |  const user = get_user()!
   |               ^^^^^^^^^^^ value is none

WHY: Force unwrap panics when the value is absent.

FIX: Use ?? for a default, or if x is Some for conditional access:

  const user = get_user() ?? default_user()

  if user is Some {
      process(user)
  }
```

**try in non-Option function [OPT13]:**
```
ERROR [type.optionals/OPT13]: cannot use try on Option in non-Option function
   |
3  |  const val = try lookup(key)
   |              ^^^ function returns i32, not Option<i32>

WHY: try propagates None to the caller, which requires an Option return type.

FIX: Convert to Result, or change the return type:

  func find(key: string) -> i32? {
      const val = try lookup(key)
      return val
  }
```

---

## Appendix (non-normative)

### Rationale

**OPT1 (standard enum):** Option is just an enum — no special compiler magic. This means pattern matching, traits, and generics all work uniformly. Sugar makes it pleasant without making it special.

**OPT6 vs Result:** `??` doesn't work on Result because silently discarding errors masks real problems. Use `.on_err(default)` to explicitly acknowledge you're ignoring the error.

**OPT8 (auto-wrapping):** Returning `T` from a function that returns `T?` should just work. The reverse is intentionally forbidden — unwrapping must be explicit.

**Pattern matching:** Use `if x is Some` for conditional checks with implicit unwrap, or `if x is Some(v)` for explicit binding. Uniform syntax across all sum types.

### Patterns & Guidance

**Option sugar (uses `?`):**

| Syntax | Works On | Meaning |
|--------|----------|---------|
| `T?` | Types | `Option<T>` |
| `x?.field` | Option | Access if present |
| `x ?? y` | Option | Value or default |

`?` is for Option sugar only — never for propagation. `try` handles propagation for both Option and Result.

**When to use what:**

| Scenario | Use |
|----------|-----|
| Provide a default | `x ?? default` |
| Access a field | `x?.field` |
| Conditional logic (same name) | `if x is Some { ... }` |
| Conditional logic (new name) | `if x is Some(v) { ... }` |
| Early exit if absent | `const v = x is Some else { return }` |
| Transform the value | `x.map(f)` |
| Convert to Result | `x.ok_or(err)` |
| You're sure it's present | `x!` (use sparingly) |
| Full pattern match | `match x { Some(v) => ..., None => ... }` |

### See Also

- [Error Types](error-types.md) — Result handling and `try` propagation (`type.error-types`)
- [Union Types](union-types.md) — Error composition (`type.union-types`)
- [Enums](enums.md) — Underlying enum type (`type.enums`)
- [Control Flow - Pattern Matching with `is`](../control/control-flow.md) — `if opt is Some(x)` for conditional checks (`ctrl.flow`)
