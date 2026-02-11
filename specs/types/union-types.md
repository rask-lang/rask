<!-- id: type.unions -->
<!-- status: decided -->
<!-- summary: Error unions (A | B) for type-safe error composition, restricted to error position -->
<!-- depends: types/error-types.md, types/enums.md -->

# Union Types

Union types (`A | B`) provide type-safe error composition. Restricted to error position in `T or E` — use explicit enums for data modeling.

## Union Syntax and Semantics

| Rule | Description |
|------|-------------|
| **U1: Error position only** | Union types valid only in error position of `T or E` |
| **U2: Anonymous enum** | `A \| B \| C` compiles to a compiler-generated anonymous enum |
| **U3: Canonical ordering** | Union types normalized alphabetically; duplicates deduplicated |
| **U4: Equality** | Two union types equal if their canonical forms are equal |

<!-- test: skip -->
```rask
// Valid: error unions
func load() -> Config or (IoError | ParseError)
func process() -> Output or (IoError | ParseError | ValidationError)

// Invalid: general unions not allowed
let x: int | string = ...              // Compile error
func foo(input: A | B) -> C           // Compile error
```

## Subtyping and Propagation

| Rule | Description |
|------|-------------|
| **S1: Subset widening** | `try` succeeds if expression error type is a subset of the return error union |
| **S2: Auto-widen** | Error types widen automatically during `try` propagation |

| Expression Error | Return Error | Valid? |
|------------------|--------------|--------|
| `IoError` | `IoError \| ParseError` | Yes |
| `IoError \| ParseError` | `IoError \| ParseError \| ValidationError` | Yes |
| `IoError \| ParseError` | `IoError` | No (ParseError not in target) |

<!-- test: skip -->
```rask
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError ⊆ union: OK
    const config = try parse(content)     // ParseError ⊆ union: OK
    return config
}
```

## Memory Layout

| Rule | Description |
|------|-------------|
| **L1: Inline storage** | Union errors stored inline, no heap allocation |
| **L2: Discriminant** | u8 discriminant (up to 256 error types) |
| **L3: Payload** | Payload sized to max(sizeof(A), sizeof(B), ...) |
| **L4: Alignment** | Max alignment of all members |

## Pattern Matching

| Rule | Description |
|------|-------------|
| **M1: Match by type** | Match union errors by type name |
| **M2: Exhaustiveness** | All variants known from definition — exhaustiveness checked |

<!-- test: skip -->
```rask
match result {
    Ok(config) => use(config),
    Err(IoError.NotFound(p)) => println("not found: {}", p),
    Err(ParseError.Syntax(l, c)) => println("syntax error at {}:{}", l, c),
    Err(_) => println("other error"),
}
```

## Generics

| Rule | Description |
|------|-------------|
| **G1: Extend generic errors** | Unions can extend generic error types: `U or (E \| TransformError)` |

<!-- test: skip -->
```rask
func transform<T, E>(result: T or E) -> U or (E | TransformError)
```

## Error Messages

```
ERROR [type.unions/U1]: union types only allowed in error position
   |
3  |  let x: int | string = ...
   |         ^^^^^^^^^^^^ use an explicit enum for data modeling

WHY: General union types add subtyping complexity. Use enums instead.

FIX: enum IntOrString { Int(i32), String(string) }
```

```
ERROR [type.unions/S1]: error type not subset of return union
   |
5  |  const config = try load()
   |                 ^^^ load() can return ParseError
6  |  // but this function returns IoError only

WHY: All error types from try must be covered by the return union.

FIX: Add ParseError to the return type:
  func process() -> Output or (IoError | ParseError)
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Duplicate types in union | U3 | Deduplicated (`IoError \| IoError` = `IoError`) |
| Single type in union | U3 | Equivalent to bare error type |
| Order differences | U3 | Normalized alphabetically |
| Union in non-error position | U1 | Compile error |
| Generic error extension | G1 | Union extends E with additional variants |

---

## Appendix (non-normative)

### Rationale

**U1 (error position only):** The actual need for ad-hoc unions is almost entirely error composition — calling `read_file()` then `parse()` means expressing "fails with IoError or ParseError" without a boilerplate enum every time. That's a real, frequent pain point.

For data modeling, enums are strictly better. `i32 | string` is anonymous — it tells you *what* the possibilities are but not *why*. `enum ConfigValue { Int(i32), Text(string) }` is self-documenting, extensible, and forces you to name the concept.

General unions would significantly complicate the type system:

- **Subtyping.** Is `i32 | string` a subtype of `i32 | string | bool`? Now you need variance rules everywhere, not just in error position.
- **Method resolution.** What methods exist on `A | B`? Intersection? That requires cross-type analysis and gets weird fast.
- **Type narrowing.** TypeScript needs an entire flow-typing system (`typeof`, type guards) to make general unions usable. That's a massive complexity budget.
- **Local analysis.** Subtyping makes type inference harder and can push toward whole-program reasoning.

Rask already covers the common "general union" cases: `T?` for nullable, `T or (A | B)` for error composition, and explicit enums for data variants. The one case where general unions feel lighter — throwaway "X or Y" types — is exactly where an explicit enum forces you to name the concept, which makes the code better.

I chose to restrict unions to error position because it gives the benefit where it matters while avoiding the subtyping complexity that would undermine local analysis and simplicity.

**U2 (anonymous enum):** The generated name is internal — users interact via union syntax. This avoids polluting the namespace with boilerplate error enum definitions.

### Patterns & Guidance

**Layered error handling:**

<!-- test: skip -->
```rask
// Low-level
func read_file(path: string) -> string or IoError

// Mid-level: composes errors from lower layers
func parse_config(path: string) -> Config or (IoError | ParseError) {
    const content = try read_file(path)
    return try parse(content)
}

// High-level: composes further
func load_app() -> App or (IoError | ParseError | ValidationError) {
    const config = try parse_config("app.toml")
    const valid = try validate(config)
    return App.new(valid)
}
```

**For data modeling, use explicit enums:**

<!-- test: skip -->
```rask
// Instead of: let value: int | string = ...
enum IntOrString { Int(i32), String(string) }
let value: IntOrString = IntOrString.Int(42)
```

Explicit enums are self-documenting (meaningful variant names), extensible (add methods), and clear at call sites.

### See Also

- `type.enums` — Enum definitions
- `type.errors` — Error types and `try` propagation
- `type.generics` — Generic type parameters
