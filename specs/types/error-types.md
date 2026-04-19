<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: T or E is a builtin sum type with type-based branch disambiguation. No Ok/Err wrappers. Disjointness rule (T ≠ E) via the nominal/alias split. E must implement ErrorMessage. Auto-wrap fires only at return. Operator family + match for multi-error unions. -->
<!-- depends: types/types.md, types/optionals.md, types/union-types.md, types/type-aliases.md -->

# Error Types

Errors are values. `T or E` is a builtin sum type — compiler-generated tagged union — with type-based branch disambiguation. No `Ok` or `Err` constructors; the compiler picks the branch from the value's type at the return site. Every `E` implements the structural `ErrorMessage` trait.

Libraries use union errors (`T or (A | B | C)`), applications use `any Error` (type-erased boxing). Match dispatches on type; operators cover the two-branch case.

## The Type

| Rule | Description |
|------|-------------|
| **ER1: Builtin sum** | `T or E` is a compiler-generated tagged union, not a user-definable enum |
| **ER2: No user wrapper** | There is no `Ok` or `Err` constructor, keyword, or pattern. Success values are bare; error values are the error type's own constructor (e.g. `DivError.ByZero`) |
| **ER3: Disjointness** | `T or E` requires T ≠ E using Rask's nominal-vs-alias distinction (see [type-aliases.md](type-aliases.md)). Violation is a compile error at type formation |
| **ER4: Error bound** | Every `E` must implement `ErrorMessage` — a structural trait requiring `func message(self) -> string`. Enforced at type formation. Primitives (`i32`, `f64`, `string`) don't qualify; newtype them |
| **ER5: No `Result<T, E>` name** | The generic `Result<T, E>` type is gone. Use `T or E` directly |

<!-- test: skip -->
```rask
func read_file(path: string) -> string or IoError        // two-branch
func load() -> Config or (IoError | ParseError)          // union error
func save(data: Data) -> () or IoError                   // unit success
```

`T or E` is valid in return types, bindings (inferred or explicit), fields, generics — same positions as any type.

**Precedence:** `?` (tightest) > `|` (error union) > `or` (loosest). `string? or IoError | ParseError` parses as `(string?) or (IoError | ParseError)`.

### The `ErrorMessage` Trait

| Rule | Description |
|------|-------------|
| **ER6: Structural matching** | Any type with `func message(self) -> string` satisfies `ErrorMessage` — no explicit `impl Trait` needed |
| **ER7: Auto-Displayable** | Error types auto-satisfy `Displayable`; `to_string()` delegates to `message()` |
| **ER8: Layered traits** | Richer capabilities (`LinedError`, `ContextualError`, `CodedError`) are opt-in traits on top of `ErrorMessage`. The minimum bound is just `message() -> string` |

<!-- test: skip -->
```rask
enum DivError { ByZero, Overflow }
extend DivError {
    func message(self) -> string {
        match self {
            DivError.ByZero   => "division by zero",
            DivError.Overflow => "overflow",
        }
    }
}

struct NotFound { key: string }
extend NotFound {
    func message(self) -> string { "not found: {self.key}" }
}
```

## Construction

| Rule | Description |
|------|-------------|
| **ER9: Auto-wrap at return only** | In a function returning `T or E`, a `return` with a value of type `T` wraps to the success branch; a value of type `E` wraps to the error branch. The branch is picked by type; disjointness makes this unambiguous |
| **ER10: Implicit unit success** | In a function returning `() or E` reaching the end without explicit `return`, the unit success path is implied |
| **ER11: No auto-wrap elsewhere** | Assignment, field initialisers, function arguments, and collection literals do **not** auto-wrap into `T or E`. The value must already have the union type (typically from a function call) |

<!-- test: skip -->
```rask
func divide(a: f64, b: f64) -> f64 or DivError {
    if b == 0.0 { return DivError.ByZero }     // E branch, by type
    return a / b                                // T branch, by type
}

func save(data: Data) -> () or IoError {
    try file.write(data)
    // implicit unit success at end
}
```

Why return-only? Construction in assignment/field positions makes the error-branch coercion invisible at use sites. Keeping it at `return` means "this function produced a result"; branches are always visible at the site that produces them.

## Operators

| Rule | Syntax | Meaning |
|------|--------|---------|
| **ER12: Boolean ok** | `r?` | `true` when in the T branch, `false` in the E branch; `bool` expression |
| **ER13: Chain** | `r?.field` | Projects `field` when T; propagates E otherwise |
| **ER14: Value fallback** | `r ?? default` | Yields T if present, else `default`. `??` is strictly extract — does not widen; `default` must have type T |
| **ER15: Force** | `r!` | Extracts T, or panics using `E.message()`; `r! "msg"` overrides with a custom message |
| **ER16: Propagate** | `try r` | Extracts T, or returns early with E widened to the current function's error type |
| **ER17: Propagate block** | `try { … }` | Each `try` inside propagates; the first E short-circuits out |
| **ER18: Error-context block** | `try { … } else \|e\| transform(e)` | Catches any E from the block, applies `transform`, then returns the result |

<!-- test: skip -->
```rask
// Single-call propagation
const data = try read_file(path)

// Chain with propagation
const size = try read_file(path)?.len()

// Force
const config = load_config()!

// Error-context block (replaces r ?? |e| f(e))
const content = try {
    try fs.read_file(path)
} else |e| context("reading {path}", e)
```

`??` is value-only; there is no closure form. Error-recovery-with-context uses the `try … else |e|` block form.

### try-else

`try expr else |e| error_expr` is sugar for the block form when the body is a single expression. Desugars to:

<!-- test: skip -->
```rask
match expr {
    T as v => v,
    E as e => return error_expr,
}
```

Example:

<!-- test: skip -->
```rask
const text = try fs.read_file(path) else |e| context("reading {path}", e)
```

`try`, `map_err`, and `try … else`:
- `map_err` transforms without propagating
- `try` propagates without transforming
- `try … else` transforms and propagates in one step

## Conditions and Narrowing

Narrowing rides on `const` — the same rule as Option. See [optionals.md](optionals.md) for the full semantics; the rules below apply identically to `T or E`.

| Rule | Description |
|------|-------------|
| **ER19: `if r?` narrows** | On a const scrutinee, `if r?` narrows `r` to `T` inside the block |
| **ER20: `if r? as v` binds** | Binds a const `v: T` in the block; works for `mut` scrutinees and for renaming |
| **ER21: else branch narrows** | On a const scrutinee, the `else` branch narrows `r` to `E` |
| **ER22: Bind error in else** | `if r? { … } else as e { … }` binds the error value in the else branch |
| **ER23: Type pattern narrow** | `if r is ErrType as e { … }` narrows and binds when `r`'s error side is (or contains) `ErrType`. Works for widened unions: `if r is IoError as io { … }` |
| **ER24: Early-exit narrow** | If a branch diverges, the fall-through is narrowed to the opposite variant |
| **ER25: Compound does not narrow** | `r? && s?` is a legal bool but does not narrow either side |
| **ER26: `!r?` forbidden** | Parse error suggesting `r is E` or a type-pattern predicate |

<!-- test: skip -->
```rask
const r = divide(a, b)

if r? {
    use(r)                        // r: f64
}

if r? as v {
    use(v)                        // v: f64
}

if r? { use(r) }
else as e { log(e.message()) }    // e: DivError

if r is DivError as e {
    log(e.message())              // e: DivError
    return
}
// r: f64 here (early-exit narrow)
```

## Match

Match arms dispatch on type and narrow the scrutinee in the arm. Two pattern families:

| Rule | Description |
|------|-------------|
| **ER27: Type patterns** | `Type => …` narrows the scrutinee to that type in the arm. `Type as name => …` additionally binds |
| **ER28: Variant patterns** | Enum variants use normal variant destructure (`IoError.NotFound(p)`, `ParseError.Syntax(line, col)`) — narrows and destructures |
| **ER29: Wildcard** | `_ => …` matches anything not covered |
| **ER30: Exhaustiveness** | Match on `T or E` must cover T and every variant of E (or use `_`) |

<!-- test: skip -->
```rask
match divide(a, b) {
    f64 => use(divide(a, b)!),                    // narrow + force (for the demo)
    DivError.ByZero   => log("divided by zero"),
    DivError.Overflow => log("overflow"),
}

// With rename and union errors
match load() {
    Config as config              => use(config),
    IoError.NotFound(p)           => println("not found: {p}"),
    ParseError.Syntax(line, col)  => println("syntax at {line}:{col}"),
    _                             => println("other error"),
}
```

Match earns its keep on multi-error unions. Two-branch matches usually read better as operator form (`if r? { … } else as e { … }`).

## Methods

Four compiler-provided methods on `T or E`. Each preserves the wrapper for chaining; operators always extract or panic.

| Method | Signature | Behavior |
|--------|-----------|----------|
| `map` | `func<U>(take self, f: \|T\| -> U) -> U or E` | Transform success |
| `map_err` | `func<E2: ErrorMessage>(take self, f: \|E\| -> E2) -> T or E2` | Translate error |
| `and_then` | `func<U>(take self, f: \|T\| -> U or E) -> U or E` | Chain Result-returning |
| `ok` | `func(take self) -> T?` | Drop error, lift to Option |

<!-- test: skip -->
```rask
const translated = parse(input).map_err(|e| AppError.Parse(e))
const profile = load_user(id).and_then(|u| load_profile(u.id))
const maybe_v = compute().ok()
```

Methods removed from the old spec: `.unwrap_or`, `.unwrap_or_else`, `.is_ok`, `.is_err`, `.to_option`, `.to_error`, `.on_err`. Operators and the four surviving methods cover every case; see the [redesign proposal](error-model-redesign-proposal.md) for the full migration map.
