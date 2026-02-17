<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: Errors are values with try propagation, union composition, and auto-Ok wrapping -->
<!-- depends: types/enums.md, types/optionals.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Error Types

Errors are values. Any type with `message()` can be an error. Composition uses union types for type-safe propagation.

## Error Trait

| Rule | Description |
|------|-------------|
| **ER1: Structural matching** | Any type with `func message(self) -> string` satisfies the Error trait |

<!-- test: parse -->
```rask
trait Error {
    func message(self) -> string
}
```

## Result Type

| Rule | Description |
|------|-------------|
| **ER2: Result enum** | `Result<T, E>` is a built-in enum with `Ok(T)` and `Err(E)` variants |
| **ER3: Shorthand** | `T or E` is identical to `Result<T, E>` |

<!-- test: parse -->
```rask
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

| Shorthand | Full type | Meaning |
|-----------|-----------|---------|
| `T?` | `Option<T>` | might be absent |
| `T or E` | `Result<T, E>` | might fail with E |

<!-- test: parse -->
```rask
func read_file(path: string) -> string or IoError        // Result<string, IoError>
func load() -> Config or (IoError | ParseError)           // Result<Config, IoError | ParseError>
func save(data: Data) -> () or IoError                    // Result<(), IoError>
```

**Precedence:** `?` (tightest) > `|` (error union) > `or` (loosest). So `string? or IoError | ParseError` = `Result<Option<string>, IoError | ParseError>`.

Both notations interchangeable. `or` works in return types, variables, fields, generics.

For `Option<T>`, see [Optionals](optionals.md).

### Result Methods

| Method | Signature | Behavior |
|--------|-----------|----------|
| `on_err` | `func(take self, default: T) -> T` | Returns T or default (discards error) |
| `ok` | `func(take self) -> T?` | `Ok(t)` → `Some(t)`, `Err(_)` → `None` |
| `err` | `func(take self) -> E?` | `Err(e)` → `Some(e)`, `Ok(_)` → `None` |
| `is_ok` | `func(self) -> bool` | True if Ok |
| `is_err` | `func(self) -> bool` | True if Err |
| `map` | `func<U>(take self, f: \|T\| -> U) -> Result<U, E>` | Transform Ok value |
| `map_err` | `func<F>(take self, f: \|E\| -> F) -> Result<T, F>` | Transform Err value |

Force unwrap uses operators, not methods:
- `x!` — panic with auto message (includes error info)
- `x! "msg"` — panic with custom message

## Error Propagation

| Rule | Description |
|------|-------------|
| **ER4: try extracts** | `try` extracts `Ok` or returns early with `Err` |
| **ER5: try binding** | `try` binds to full following expression including chains |
| **ER6: try on Option** | `try` also works on `Option` — propagates `None` |

<!-- test: parse -->
```rask
func process() -> Data or IoError {
    const file = try open(path)
    const data = try file.read_all()
    data  // auto-wrapped to Ok(data)
}
```

`try` works on both `Result` and `Option`—uniformly means "propagate failure." `?` reserved for Option sugar only (`T?` type, `x?.field` chaining, `x ?? y` default).

Use parens for chaining after: `(try file.read()).trim()`.

## Auto-Ok Wrapping

| Rule | Description |
|------|-------------|
| **ER7: Auto-wrap T** | When return type is `T or E`, returning a value of type `T` is automatically wrapped in `Ok` |
| **ER8: Implicit unit Ok** | When return type is `() or E` and execution reaches end, returns `Ok(())` |

<!-- test: skip -->
```rask
func load() -> Config or IoError {
    const content = try read_file(path)
    return parse(content)   // Returns Config, auto-wrapped to Ok(Config)
}

func might_fail() -> i32 or Error {
    if bad_condition {
        return Err(Error.Bad)  // Explicit Err still works
    }
    return 42  // Auto-wrapped to Ok(42)
}
```

<!-- test: skip -->
```rask
func save(data: Data) -> () or IoError {
    const file = try File.create(path)
    try file.write(data)
    // No explicit return needed - implicit Ok(())
}

func main() -> () or Error {
    println("Starting...")
    try run_app()
    println("Done!")
    // implicit Ok(())
}
```

## Error Type Widening

| Rule | Description |
|------|-------------|
| **ER9: Auto-widen** | `try` auto-widens when return type is a union — succeeds if expression error type ⊆ return error union |

<!-- test: skip -->
```rask
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError widens to union
    const config = try parse(content)     // ParseError widens to union
    config
}
```

See [Union Types](union-types.md) for union type semantics.

## Custom Error Types

| Rule | Description |
|------|-------------|
| **ER10: Enum errors** | Errors are defined as enums with a `message()` method |

<!-- test: parse -->
```rask
enum AppError {
    NotFound(path: string),
    InvalidFormat(line: i32, col: i32),
    Timeout,
}

extend AppError {
    func message(self) -> string {
        match self {
            NotFound(p) => format("not found: {}", p),
            InvalidFormat(l, c) => format("invalid format at {}:{}", l, c),
            Timeout => "operation timed out",
        }
    }
}
```

### Built-in IoError

<!-- test: skip -->
```rask
enum IoError {
    NotFound(path: string),
    PermissionDenied(path: string),
    ConnectionRefused(addr: string),
    Timeout,
    Interrupted,
    Other(message: string),
}

extend IoError {
    func message(self) -> string { ... }
}
```

## Error Composition

| Rule | Description |
|------|-------------|
| **ER11: Same type** | Same error type propagates directly |
| **ER12: Union** | Different error types compose via union (`A \| B`) |
| **ER13: Union compose** | Union return types accept any subset union via `try` |

### Same error type — direct propagation

<!-- test: skip -->
```rask
func read_both() -> Data or IoError {
    const a = try read_file(x)   // IoError
    const b = try read_file(y)   // IoError
    combine(a, b)
}
```

### Different error types — union

<!-- test: skip -->
```rask
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError ⊆ union
    const config = try parse(content)     // ParseError ⊆ union
    config
}
```

### Composing unions

<!-- test: skip -->
```rask
func process() -> Output or (IoError | ParseError | ValidationError) {
    const config = try load()           // IoError | ParseError ⊆ union
    const valid = try validate(config)  // ValidationError ⊆ union
    transform(valid)
}
```

## Pattern Matching Errors

<!-- test: skip -->
```rask
match load() {
    Ok(config) => use(config),
    Err(IoError.NotFound(p)) => println("file not found: {}", p),
    Err(IoError.PermissionDenied(p)) => retry_with_sudo(p),
    Err(ParseError.Syntax(l, c)) => println("syntax error at {}:{}", l, c),
    Err(_) => println("unexpected error"),
}
```

## Linear Resources in Errors

| Rule | Description |
|------|-------------|
| **ER14: Linear payloads** | Errors can contain linear resources; wildcard on linear payloads is a compile error |

<!-- test: skip -->
```rask
enum FileError {
    ReadFailed(file: File, reason: string),
}

match result {
    Ok(data) => process(data),
    Err(FileError.ReadFailed(file, msg)) => {
        try file.close()   // MUST consume
        log(msg)
    }
}
```

## Operator Family

| Syntax | Option | Result |
|--------|--------|--------|
| `try x` | Propagate None | Propagate Err |
| `x ?? y` | Value or default | — |
| `x!` | Force (panic) | Force (panic with error info) |
| `x! "msg"` | Force (panic with message) | Force (panic with message) |

`??` doesn't work on Result — silently discarding errors masks real problems. Use `.on_err(default)` to explicitly acknowledge you're ignoring the error.

Optional sugar (`T?`, `x?.field`, `x ?? y`) is distinct from `try` propagation — `?` is never used for propagation.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Return `T` from `T or E` function | ER7 | Auto-wrapped to `Ok(T)` |
| Reach end of `() or E` function | ER8 | Implicit `Ok(())` |
| `try` on error type not in return union | ER9 | Compile error — type not subset |
| `try` on `Option` in `Result` function | ER6 | `None` maps to `Err` (types must align) |
| Wildcard on linear error payload | ER14 | Compile error — must consume |
| Nested `try` in closures | ER4 | Propagates to closure's return, not enclosing function |
| `try` binding with method chain | ER5 | Binds to full expression; use parens to chain after |

---

## Appendix (non-normative)

### Rationale

**ER1 (structural matching):** Structural matching means you don't need to import a trait to make your type an error. If it has `message()`, it works.

**ER7/ER8 (auto-Ok):** If you wanted an error, you'd use `return Err(...)` or `try`. Reaching the end means success. Eliminates noisy `Ok(())` at function ends.

**ER9 (auto-widen):** Without auto-widening, every `try` on a narrower error type would need an explicit conversion. The subset check keeps it type-safe without boilerplate.

**Operator split (`try` vs `?`):** `try` is for propagation (both Result and Option). `?` is reserved for Option sugar only — type suffix, chaining, defaults, smart unwrap. This avoids Rust's overloading where `?` means different things in different contexts.

**`??` not on Result:** Silently discarding errors masks real problems. `.on_err(default)` makes the intent explicit.

### Patterns & Guidance

#### Panic vs Error: When to Use Which

Simple rule: **panic for programmer errors, return errors for expected failures.**

| Situation | Mechanism | Rationale |
|-----------|-----------|-----------|
| **Bug in the code** | `panic` | Continuing is meaningless — the program is wrong |
| **Bad input / environment** | `return Err(...)` | Caller can recover, retry, or report |

**Panic (programmer error):**

<!-- test: skip -->
```rask
// Array out of bounds — programmer miscalculated
const item = arr[arr.len()]   // panic: index out of bounds

// Invariant violated — internal state is corrupt
func withdraw(self, amount: u64) {
    if amount > self.balance {
        panic("withdraw called with amount > balance — caller must check")
    }
    self.balance = self.balance - amount
}

// Unwrap on None/Err — programmer asserted it can't fail
const config = load_config()!   // panic if None/Err
```

**Return Error (expected failure):**

<!-- test: skip -->
```rask
// File might not exist — that's not a bug
func read_config(path: string) -> Config or IoError {
    const content = try fs.read(path)   // IoError propagated
    return try parse(content)
}

// Network might be down — expected in production
func fetch(url: string) -> Response or (IoError | HttpError) {
    const conn = try net.connect(url)
    return try conn.get("/")
}

// User input might be invalid — not our bug
func parse_age(input: string) -> u32 or ParseError {
    const n = try input.parse_int()
    if n < 0 || n > 150: return Err(ParseError.OutOfRange)
    return n as u32
}
```

#### The Grey Area

| Case | Choice | Why |
|------|--------|-----|
| Division by zero | Panic | Caller should have checked — this is a logic error |
| Integer overflow | Panic (debug), wrap (release) | See [integer-overflow.md](integer-overflow.md) |
| Stack overflow | Panic | Can't meaningfully recover |
| Out of memory | Panic | Allocation failure is nearly unrecoverable |
| Missing required config | Error if loading, panic if already validated | Depends on where you are |
| Unreachable match arm | Panic | If it's reached, the code is wrong |

Rule of thumb: If adding error handling makes the caller's code strictly worse (more complex, no meaningful recovery), the callee should panic. If the caller has a reasonable recovery path, return an error.

#### Panic Messages

Panic messages should explain the invariant that was violated:

<!-- test: skip -->
```rask
// Good: explains what went wrong
panic("buffer.len() must be >= header_size, got {buffer.len()}")

// Bad: unhelpful
panic("invalid state")
```

#### IDE Integration

IDE shows `→ returns Err` as ghost text after `try` for visibility.

### Remaining Issues

#### Low Priority
1. **Stack traces** — Debug builds could capture (not specified)

#### Dependencies
- **Union types** — See [Union Types](union-types.md) (TODO: create spec)

### See Also

- [Optionals](optionals.md) — `T?` sugar, `??` default (`type.optionals`)
- [Enums](enums.md) — Enum definitions and pattern matching (`type.enums`)
- [Union Types](union-types.md) — Union type semantics for error composition
