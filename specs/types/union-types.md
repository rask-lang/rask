# Union Types

## Overview

Union types (`A | B`) provide type-safe error composition. They are **restricted to error position** in `Result<T, E>`—use explicit enums for data modeling.

## Decision

Error unions are compiler-generated anonymous enums, enabling ergonomic error composition without manual wrapper types. General-purpose union types are not supported—use explicit enums instead.

## Rationale

Error composition is extremely common. Requiring explicit error enums for every combination would violate Ergonomic Simplicity (ES). However, general union types add significant type system complexity (subtyping, variance, method resolution). Restricting unions to error position gives the ergonomic benefit where it matters most while keeping the type system simple.

## Specification

### Syntax

Union types are only valid in error position of `Result<T, E>`:

```rask
// Valid: error unions
func load() -> Config or (IoError | ParseError)
func process() -> Output or (IoError | ParseError | ValidationError)

// Invalid: general unions not allowed
let x: int | string = ...              // Compile error
func foo(input: A | B) -> C              // Compile error
```

### Semantics

`A | B | C` compiles to an anonymous enum:

```rask
// IoError | ParseError compiles to:
enum __ErrorUnion_IoError_ParseError {
    IoError(IoError),
    ParseError(ParseError),
}
```

The generated name is internal—users interact via the union syntax.

### Canonical Ordering

Union types are normalized alphabetically:

| Written | Canonical Form |
|---------|----------------|
| `ParseError \| IoError` | `IoError \| ParseError` |
| `C \| A \| B` | `A \| B \| C` |
| `IoError \| IoError` | `IoError` (deduplicated) |

Two union types are equal if their canonical forms are equal.

### Subtyping

For `try` propagation, error types widen automatically:

| Expression Error | Return Error | Valid? |
|------------------|--------------|--------|
| `IoError` | `IoError \| ParseError` | Yes |
| `IoError \| ParseError` | `IoError \| ParseError \| ValidationError` | Yes |
| `IoError \| ParseError` | `IoError` | No (ParseError not in target) |

**Rule:** `try` succeeds if expression error type ⊆ return error union.

```rask
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError ⊆ union: OK
    const config = try parse(content)     // ParseError ⊆ union: OK
    config
}

func process() -> Output or (IoError | ParseError | ValidationError) {
    const config = try load()             // IoError | ParseError ⊆ union: OK
    try validate(config)
}
```

### Memory Layout

| Component | Size |
|-----------|------|
| Discriminant | u8 (supports up to 256 error types) |
| Payload | max(sizeof(A), sizeof(B), ...) |
| Alignment | max alignment of all members |

Storage is inline (no heap allocation).

### Pattern Matching

Match on union errors by type name:

```rask
match result {
    Ok(config) => use(config),
    Err(IoError.NotFound(p)) => println("not found: {}", p),
    Err(IoError.PermissionDenied(p)) => retry_elevated(p),
    Err(ParseError.Syntax(l, c)) => println("syntax error at {}:{}", l, c),
    Err(_) => println("other error"),
}
```

Exhaustiveness checking works because all variants are known from the union definition.

### Interaction with Generics

Unions can extend generic error types:

```rask
func transform<T, E>(result: Result<T, E>) -> U or (E | TransformError)
```

The union extends E with additional variants.

### No General Union Types

For data modeling, use explicit enums:

```rask
// Instead of: let value: int | string = ...
// Use:
enum IntOrString { Int(i32), String(string) }
let value: IntOrString = IntOrString.Int(42)
```

Explicit enums are:
- Self-documenting (meaningful variant names)
- Extensible (add methods)
- Clear at call sites

## Examples

### Layered Error Handling

```rask
// Low-level
func read_file(path: string) -> string or IoError

// Mid-level
func parse_config(path: string) -> Config or (IoError | ParseError) {
    const content = try read_file(path)
    try parse(content)
}

// High-level
func load_app() -> App or (IoError | ParseError | ValidationError) {
    const config = try parse_config("app.toml")
    const valid = try validate(config)
    App.new(valid)
}
```

### Handling Specific Errors

```rask
match load_app() {
    Ok(app) => app.run(),
    Err(IoError.NotFound(_)) => create_default_config(),
    Err(ParseError.Syntax(l, c)) => {
        println("Fix syntax error at line {}", l)
        exit(1)
    }
    Err(e) => {
        println("Error: {}", e.message())
        exit(1)
    }
}
```

## Integration

- **Error propagation:** `try` auto-widens to return union
- **Pattern matching:** Match by type name, exhaustiveness checked
- **Enums:** Union members are typically enums with `message()` method
- **Result methods:** `.map_err()` can transform union errors
