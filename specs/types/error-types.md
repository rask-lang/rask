# Error Types Specification

## Overview

Errors are values. Any type with a `message()` method can be used as an error. Error composition uses union types for type-safe propagation.

## The Error Trait

```rask
trait Error {
    func message(self) -> string
}
```

Structural matching — any type with `func message(self) -> string` satisfies `Error`.

## Result Type

```rask
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

For `Option<T>`, see [Optionals](optionals.md).

### Result Methods

| Method | Signature | Behavior |
|--------|-----------|----------|
| `on_err` | `func(take self, default: T) -> T` | Returns T or default (discards error) |
| `ok` | `func(take self) -> T?` | `Ok(t)` → `Some(t)`, `Err(_)` → `None` |
| `err` | `func(take self) -> E?` | `Err(e)` → `Some(e)`, `Ok(_)` → `None` |
| `is_ok` | `func(self) -> bool` | True if Ok |
| `is_err` | `func(self) -> bool` | True if Err |
| `map` | `func<U>(take self, f: func(T) -> U) -> Result<U, E>` | Transform Ok value |
| `map_err` | `func<F>(take self, f: func(E) -> F) -> Result<T, F>` | Transform Err value |

Force unwrap uses operators, not methods:
- `x!` — panic with auto message (includes error info)
- `x! "msg"` — panic with custom message

## Error Propagation: `try`

Extracts `Ok` or returns early with `Err`. Prefix keyword.

```rask
func process() -> Result<Data, IoError> {
    const file = try open(path)
    const data = try file.read_all()
    data  // auto-wrapped to Ok(data)
}
```

`try` works on both `Result` and `Option` — it uniformly means "propagate failure." The `?` character is reserved for Option sugar only (`T?` type, `x?.field` chaining, `x ?? y` default, `if x?` smart unwrap).

**Binding:** `try` binds to the full following expression including method chains. Use parens or line-splitting when chaining after: `(try file.read()).trim()`.

**IDE support:** Per Principle 7, the IDE shows `→ returns Err` as ghost text after `try` expressions to make control flow visible.

### Auto-Ok Wrapping

If a function returns `Result<T, E>` and the final expression is of type `T`, it's automatically wrapped in `Ok`:

```rask
func load() -> Result<Config, IoError> {
    const content = try read_file(path)
    try parse(content)   // Returns Config, auto-wrapped to Ok(Config)
}

func might_fail() -> Result<i32, Error> {
    if bad_condition {
        return Err(Error.Bad)  // Explicit Err still works
    }
    42  // Auto-wrapped to Ok(42)
}
```

### Implicit Ok(()) for Unit Results

When a function returns `Result<(), E>`, reaching the end of the function without an explicit return automatically returns `Ok(())`:

```rask
func save(data: Data) -> Result<(), IoError> {
    const file = try File.create(path)
    try file.write(data)
    // implicit Ok(()) — no need to write it
}

@entry
func main() -> Result<(), Error> {
    println("Starting...")
    try run_app()
    println("Done!")
    // implicit Ok(())
}
```

**Rationale:** If you wanted to return an error, you would have used `return Err(...)` or `try`. Reaching the end of a `Result<(), E>` function means success. This eliminates the noisy `Ok(())` that would otherwise appear at the end of most side-effecting functions.

### Error Type Widening

When return type is a union, `try` auto-widens:

```rask
func load() -> Result<Config, IoError | ParseError> {
    const content = try read_file(path)   // IoError widens to union
    const config = try parse(content)     // ParseError widens to union
    config
}
```

**Rule:** `try` succeeds if expression error type ⊆ return error union.

See [Union Types](union-types.md) for union type semantics.

## Custom Error Types

Define errors as enums:

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

## Built-in IoError

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

### Same error type — direct propagation

```rask
func read_both() -> Result<Data, IoError> {
    const a = try read_file(x)   // IoError
    const b = try read_file(y)   // IoError
    combine(a, b)
}
```

### Different error types — union

```rask
func load() -> Result<Config, IoError | ParseError> {
    const content = try read_file(path)   // IoError ⊆ union
    const config = try parse(content)     // ParseError ⊆ union
    config
}
```

### Composing unions

```rask
func process() -> Result<Output, IoError | ParseError | ValidationError> {
    const config = try load()           // IoError | ParseError ⊆ union
    const valid = try validate(config)  // ValidationError ⊆ union
    transform(valid)
}
```

## Pattern Matching Errors

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

Errors can contain linear resources. Wildcards on linear payloads are compile errors.

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

## Summary

| Feature | Mechanism |
|---------|-----------|
| Error trait | `func message(self) -> string` |
| Result/Option | Built-in enums |
| Propagation | `try` keyword (works on both) |
| Composition | Union types (`A | B`) |
| Custom errors | Enums with `message()` |
| Auto-Ok | Final expression auto-wrapped |

### The Operator Family

| Syntax | Option | Result |
|--------|--------|--------|
| `try x` | Propagate None | Propagate Err |
| `x ?? y` | Value or default | — |
| `x!` | Force (panic) | Force (panic with error info) |
| `x! "msg"` | Force (panic with message) | Force (panic with message) |

**Why `??` doesn't work on Result:** Silently discarding errors masks real problems. Use `.on_err(default)` to explicitly acknowledge you're ignoring the error.

**Optional sugar** (`T?`, `x?.field`, `x ?? y`, `if x?`) is distinct from `try` propagation — `?` is never used for propagation.

---

## Remaining Issues

### Medium Priority
1. **Panic vs Error** — Guidelines for when to panic vs return Result

### Low Priority
2. **Stack traces** — Debug builds could capture (not specified)

### Dependencies
- **Union types** — See [Union Types](union-types.md) (TODO: create spec)
