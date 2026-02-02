# Optionals

## The Question

How do optional values (values that may be absent) work in Rask?

## Decision

`Option<T>` is a standard enum with syntax sugar for ergonomic handling: `T?` for the type, `none` for absence, `?.` for chaining, `??` for defaults, `if x?` for smart unwrap.

## Rationale

Optional handling is extremely common. Rask provides both uniformity (Option is a normal enum) and ergonomics (syntax sugar eliminates ceremony). This matches Swift's approach: the type exists, but sugar makes it pleasant to use.

## Specification

### The Option Type

```rask
enum Option<T> {
    Some(T),
    None,
}
```

`Option<T>` is a standard enum with pattern matching, traits, and generics.

### Syntax Sugar

| Sugar | Meaning |
|-------|---------|
| `T?` | `Option<T>` |
| `none` | `Option.None` (type inferred) |
| `x?.field` | Access field if present, else none |
| `x ?? y` | x if present, else y |
| `x!` | Force unwrap (panic if none) |
| `if x?` | Check + smart unwrap in block |

### The `none` Literal

`none` is a literal representing absence:

```rask
let x: User? = none
func find(id: i64) -> User? { none }
```

Type is inferred from context. Equivalent to `Option.None`.

### Auto-wrapping

`T` coerces to `Option<T>` automatically:

```rask
let user: User? = load_user()    // wraps to Some(user)
```

`Option<T>` does NOT coerce to `T` — must unwrap explicitly.

### Optional Chaining: `?.`

```rask
user?.profile?.settings?.theme
```

| x is | `x?.field` |
|------|------------|
| Some(v) | Some(v.field) or v.field wrapped |
| None | none |

### Nil-Coalescing: `??`

```rask
const name = user?.name ?? "Anonymous"
```

| x is | `x ?? y` |
|------|----------|
| Some(v) | v (unwrapped) |
| None | y |

Short-circuits: `y` only evaluated if `x` is none.

### Force Unwrap: `!`

```rask
const user = get_user()!    // panics if none
```

Use sparingly. Prefer `??` or `if x?`.

### Conditional Check: `if x?`

```rask
let user: User? = get_user(id)

if user? {
    // user is User here (smart unwrapped)
    process(user)
}
// user is User? again
```

**Combined conditions:**
```rask
if user? && user.active {
    // user unwrapped, active checked
}
```

**Negation:** `if !x?` does NOT smart-unwrap in else (too error-prone).

### Methods

| Method | Behavior |
|--------|----------|
| `map(f)` | Transform if present |
| `filter(pred)` | Keep if predicate true |
| `ok_or(err)` | Convert to Result |
| `unwrap()` | Extract or panic |
| `unwrap_or(default)` | Extract or default |
| `is_some()` | Has value? |
| `is_none()` | Is absent? |

### Pattern Matching

Standard enum matching works:

```rask
match user {
    Some(u) => process(u),
    None => handle_missing(),
}
```

Rarely needed — prefer `if x?` and `??`.

### Linear Resources

If `T` is linear, `T?` is linear. Must handle both paths:

```rask
let file: File? = open("data.txt")
if file? {
    file.close()
}
```

### Comparison

```rask
x == none       // is absent
x != none       // has value
x == y          // compare inner values or both none
```

## Integration

- **Result:** Use `opt.ok_or(err)` to convert Option to Result when error context is needed.
- **Control Flow:** `if x?` integrates with expression-oriented design.

### The `?` Family

The `?` operators work on `Option<T>` and `Result<T, E>`:

| Syntax | Option | Result |
|--------|--------|--------|
| `x?` | Propagate None | Propagate Err (with union widening) |
| `x ?? y` | Value or default | — |
| `x!` | Force (panic: "None") | Force (panic: "Err: ...") |
| `x! "msg"` | Force (panic with message) | Force (panic with message) |

**Why `??` doesn't work on Result:** Silently discarding errors masks real problems. Use `.on_err(default)` to explicitly acknowledge you're ignoring the error.

**Type-specific syntax:**

| Syntax | Works On | Meaning |
|--------|----------|---------|
| `T?` | Types | `Option<T>` |
| `x?.field` | Option | Access if present |
| `if x?` | Option | Smart unwrap in block |

**Propagation rules:**
- `x?` on Option only valid in function returning `Option<U>`
- `x?` on Result only valid in function returning `Result<U, E>` where error types are compatible
- Mixing requires explicit conversion: `.ok_or(err)` or `.ok()`

See [Error Types](error-types.md) for Result handling and [Union Types](union-types.md) for error composition.

---

## See Also

- [Control Flow - Pattern Matching with `is`](../control/control-flow.md#pattern-matching-in-conditions-is) — `if opt is Some(x)` works for Option (and all enums), though `if opt?` is preferred for Option.
