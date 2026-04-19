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

## Union Widening and Boxing

| Rule | Description |
|------|-------------|
| **ER31: Auto-widen** | `try` succeeds when the expression's error type is a subset of the current function's error union |
| **ER32: Auto-box to `any Error`** | `try` auto-boxes when the current function's error type is `any Error` — any `E` satisfying `ErrorMessage` widens by boxing |

<!-- test: skip -->
```rask
// Library: precise union
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError ⊆ union
    const config = try parse(content)     // ParseError ⊆ union
    return config
}

// Application: boxed any Error
func start_app() -> App or any Error {
    const config = try read_config(path)  // IoError | ParseError → boxed
    const db = try connect(config.db_url) // DbError → boxed
    return App.new(config, db)
}
```

Libraries use union errors (precise, matchable). Applications use `any Error` (ergonomic, sufficient for logging). Downcast with `if r is IoError as e` for recovery.

## Error Origin Tracking

| Rule | Description |
|------|-------------|
| **ER33: Origin capture** | `try` records `(file, line)` on the error at the first propagation site, in both debug and release builds |
| **ER34: Origin access** | All errors expose `.origin` — always available, ~16 bytes |

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or (IoError | ParseError) {
    const content = try read_file(path)    // origin: "config.rk:2"
    const config = try parse(content)      // origin: "config.rk:3"
    return config
}

if start_app() is any Error as e {
    log("{e.origin}: {e.message()}")
    // "config.rk:2: file not found: /etc/app.conf"
}
```

Cost: ~16 bytes per error (file pointer + line). Negligible on the exceptional path. For full propagation chains, add context with `try … else` at key call sites.

## @message Annotation

`@message` generates the `message()` method from per-variant templates — eliminates the match boilerplate for error enums.

| Rule | Description |
|------|-------------|
| **ER35: @message opt-in** | `@message` on an enum generates `func message(self) -> string`. Compile error if the enum already defines `message()` manually |
| **ER36: Variant template** | `@message("template")` on a variant provides the format string. `{name}` for named payloads, `{0}` / `{1}` for positional |
| **ER37: Auto-delegate** | A variant with a single payload that itself satisfies `ErrorMessage`, and no `@message` annotation, delegates to `inner.message()` |
| **ER38: Coverage required** | Every variant must have either an annotation or an auto-delegatable payload. Missing coverage is a compile error |

<!-- test: skip -->
```rask
@message
enum RegistryError {
    @message("package not found: {name}")
    PackageNotFound(name: string),

    @message("network error: {0}")
    NetworkError(string),

    @message("checksum mismatch for {pkg}: expected {expected}, got {got}")
    ChecksumMismatch(pkg: string, expected: string, got: string),
}

// Wrapper enum — auto-delegates
@message
enum FetchError {
    Manifest(ManifestError),    // delegates to ManifestError.message()
    Version(VersionError),      // delegates
    @message("I/O: {0}")
    Io(string),                 // needs explicit template
}
```

Manual `message()` is always available. `@message` is pure convenience over ER6.

## Inferred Error Unions (Private Functions)

Private functions can omit error return types entirely, or use `or _` to state the success type while letting the compiler infer the error union. Same local-analysis pattern as [Gradual Constraints](gradual-constraints.md).

| Rule | Description |
|------|-------------|
| **ER39: Error union inference** | Private functions may omit error types or use `or _`. The compiler computes the union from all error-producing expressions in the body |
| **ER40: Public must be explicit** | `public` functions must declare error types explicitly — `or _` is rejected (API stability, same as `type.gradual/GC5`) |
| **ER41: Recursive annotation** | Mutually recursive functions where the error type is ambiguous require annotation on at least one function in the cycle |

Three annotation levels:

<!-- test: skip -->
```rask
// 1. Fully omitted — both success and error inferred
func load_config(path: string) {
    const text = try read_file(path)       // IoError
    const config = try parse(text)          // ParseError
    return config
}
// Inferred: -> Config or (IoError | ParseError)

// 2. Partial: `or _` — success explicit, error inferred
func load_config(path: string) -> Config or _ {
    const text = try read_file(path)
    return try parse(text)
}

// 3. Public — must be explicit
public func load_config(path: string) -> Config or (IoError | ParseError) {
    const text = try read_file(path)
    return try parse(text)
}
```

Each `try expr` where `expr` returns `T or E` contributes `E`. Each bare error return in the body contributes that error's type. `try … else |e| transform(e)` contributes the type of `transform(e)`, not the original. The inferred union is deduplicated and sorted alphabetically for deterministic output.

## Linear Resources in Errors

| Rule | Description |
|------|-------------|
| **ER42: Linear payloads** | Errors may carry linear resources; both branches of `T or E` must handle the resource |
| **ER43: Wildcard forbidden on linear** | `_` in a match arm or destructure that would discard a linear payload is a compile error |

<!-- test: skip -->
```rask
enum FileError {
    ReadFailed(file: File, reason: string),
}

match result {
    data: Data => process(data),
    FileError.ReadFailed(file, msg) => {
        try file.close()   // linear file MUST be consumed
        log(msg)
    }
}
```

## Development Panics

| Rule | Description |
|------|-------------|
| **DP1: todo()** | Panics with "not yet implemented" and source location |
| **DP2: unreachable()** | Panics with "entered unreachable code" and source location |
| **DP3: Optional message** | Both accept an optional string: `todo("auth flow")`, `unreachable("invalid state")` |
| **DP4: Never type** | Both return `Never`, coercible to any type |
| **DP5: Lint warning** | `rask lint` warns on `todo()` in non-test code |

<!-- test: skip -->
```rask
func handle(event: Event) -> Response {
    match event {
        Click(pos) => handle_click(pos),
        Key(k)     => todo("keyboard handling"),
    }
}
```

**`todo()` output:**
```
thread panicked at 'not yet implemented: keyboard handling', src/handler.rk:4:19
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Return bare `T` from `T or E` function | ER9 | Wraps to T branch |
| Return bare `E` from `T or E` function | ER9 | Wraps to E branch |
| `const x: T or E = 5` (assignment) | ER11 | Type error — auto-wrap is return-only |
| `T or T` | ER3 | Compile error; newtype one side |
| `T or i32` (primitive E) | ER4 | Compile error — E lacks `ErrorMessage` |
| `try r` in `fn -> T?` | — | Cross-shape, ill-typed. Use `r.ok()` then `try` |
| `try o` in `fn -> T or E` | — | Cross-shape, ill-typed. Use `o.to_result(err)` then `try` |
| `try` on narrower E into wider union | ER31 | Auto-widen succeeds |
| `try` into `any Error` | ER32 | Auto-box succeeds |
| `r ?? err_value` where `err_value: E` | ER14 | Type error — `??` does not widen. Use `.to_result(err)` or match |
| `!r?` | ER26 | Parse error suggesting `r is E` |
| `r? && s?` in condition | ER25 | Legal bool; neither narrows |
| Wildcard on linear error payload | ER43 | Compile error |
| `.origin` in release build | ER33 | Always available |
| Nested `try` in closure | ER16 | Propagates to closure's return, not the enclosing function |
| `@message` + manual `message()` | ER35 | Compile error — pick one |
| `@message` variant without template or delegatable payload | ER38 | Compile error |

## Error Messages

**`Ok(v)` / `Err(e)` at construction [migration]:**
```
ERROR [type.errors/NO_WRAPPER]: Ok/Err are not valid in Rask
   |
3  |  return Ok(config)
   |         ^^^^^^^^^^ bare value auto-wraps to the success branch at return

FIX: return config    (for success)
     return MyError.Failed   (for error — type picks the branch)
```

**Disjointness violation [ER3]:**
```
ERROR [type.errors/ER3]: T and E must be distinct in `T or E`
   |
2  |  func f() -> i32 or i32
   |              ^^^^^^^^^^ both branches have the same type

WHY: The compiler picks the branch from the value's type at return.
     Two branches of the same type are ambiguous.

FIX: Newtype one side:
     type ParseError = i32 with (…)
     func f() -> i32 or ParseError
```

**Missing ErrorMessage [ER4]:**
```
ERROR [type.errors/ER4]: i32 cannot be an error type
   |
2  |  func f() -> string or i32
   |                        ^^^ i32 does not implement ErrorMessage

WHY: Every error type must provide `func message(self) -> string`.

FIX: Newtype it and implement message():
     type StatusCode = i32
     extend StatusCode {
         func message(self) -> string { "status {self.value}" }
     }
```

**Auto-wrap outside return [ER11]:**
```
ERROR [type.errors/ER11]: cannot assign value of type `i32` to `i32 or MyError`
   |
3  |  const r: i32 or MyError = 5
   |                            ^ auto-wrap only fires at `return`

WHY: Construction at assignment hides the branch choice. Only `return`
     triggers auto-wrap for T or E — elsewhere the value must already
     have the union type (typically from a function call).

FIX: Construct via a function that returns T or E, or use
     explicit branch construction helpers.
```

**Cross-shape try [migration]:**
```
ERROR [type.errors/CROSS_SHAPE]: cannot `try` Option in Result-returning function
   |
4  |  const x = try maybe_value
   |            ^^^ maybe_value: T?
   |
   |  current function returns T or E

WHY: Cross-shape propagation silently fabricates or drops errors.

FIX: Convert explicitly:
     const x = try maybe_value.to_result(MyError.NotFound)
```

**Match on Option [migration]:**
See [optionals.md#error-messages](optionals.md). Same diagnostic fires for `match x { Some(…) => …, None => … }`.

---

## Appendix (non-normative)

### Rationale

**ER1 (builtin sum).** The old spec said `Result<T, E>` was a normal enum with `T or E` as sugar. In practice Result had dedicated sugar, auto-Ok wrapping, `try` propagation, `any Error` boxing, origin tracking, and union widening — more bespoke surface than any user enum. Making `T or E` a builtin lets the spec stop pretending.

**ER3 (disjointness).** Type-based branch disambiguation at construction (no `Ok`/`Err` wrappers) only works if T ≠ E. Rask's existing nominal-vs-alias split gives this for free: nominal types are distinct, aliases are transparent. The escape hatch is newtype, not a wrapper keyword.

**ER4 (ErrorMessage bound).** A minimum bound on E solves three problems at once: (1) `r!` can always produce a useful panic message; (2) primitives can't accidentally be error types, so `i32 or i32` style ambiguities don't arise; (3) richer capabilities (context, codes, stack traces) layer opt-in on top without forcing complexity on simple errors.

**ER9 (auto-wrap return-only).** Auto-wrap at assignment/field/argument positions makes the branch choice invisible at the use site. Restricting it to `return` keeps the error branch visible — you can only produce a `T or E` by returning from a function declared to return one.

**ER14 (no `??` widening).** `??` that widens into `T or E` when the RHS doesn't match T would be a second type rule for one operator. Keeping `??` as strict-extract means one mental model ("fallback to an inner value"). Option→Result lifting uses the explicit `.to_result(err)` method.

**ER31/ER32 (libraries vs applications).** Libraries should expose precise union errors so callers can match and recover. Application code calling 5 libraries shouldn't re-declare every error on every function — `any Error` is the escape hatch, type-erased, with `is` downcast for recovery. Same split as Rust's thiserror + anyhow, built into the language.

**No `match` on Option.** See [optionals.md Appendix](optionals.md). Match for `T or E` is kept because multi-error unions genuinely need multi-arm dispatch; Option doesn't.

**`try … else` over `r ?? |e| f(e)`.** Closure-form `??` overloads one operator on two distinct shapes (value vs. `|E| -> T`). Splitting the two cases — `??` for strict value fallback, `try … else` for error-recovery-with-context — keeps each form's meaning crisp.

### Patterns & Guidance

**Panic vs Error.** Panic for programmer bugs (invariant violations, unreachable branches, unwrap assertions). Return errors for expected failures (I/O, parsing, user input, network). Adding error handling for programmer bugs makes the caller strictly worse; adding panics for user-facing failures makes the app unrecoverable.

| Situation | Mechanism |
|-----------|-----------|
| Bug / invariant violation | `panic(…)` |
| `todo()` / `unreachable()` | panics with source location |
| I/O, parse, auth, network | return `T or E` |
| Programmer asserts present | `x!` / `r!` |

**Context chains.** For application-level errors, add string context at each layer boundary:

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or ContextError {
    const text = try fs.read_file(path) else |e| context("reading {path}", e)
    return try Config.parse(text) else |e| context("parsing {path}", e)
}
```

**Typed domain errors.** For library-level errors, wrap in domain-specific types:

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_file(path) else |e| ConfigError.Io { path, source: e }
    return try Config.parse(text) else |e| ConfigError.Parse { path, source: e }
}
```

**Recovery with downcast.** In application code catching `any Error`:

<!-- test: skip -->
```rask
if start_app() is any Error as e {
    if e is IoError { retry() }
    else            { log("fatal: {e.origin}: {e.message()}") }
}
```

### IDE Integration

- Ghost text shows `→ returns E` after `try` for visibility.
- Ghost text shows inferred error union inline for `or _` and fully-omitted private functions.
- Quick action: "Make error type explicit" fills in the inferred union.
- Quick action: "Make public" adds `public` and the full explicit signature.
- `.origin` hover shows the capture site.

### See Also

- [Optionals](optionals.md) — `T?`, operator family, narrowing (`type.optionals`)
- [Union Types](union-types.md) — `A | B` error composition (`type.unions`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
- [Gradual Constraints](gradual-constraints.md) — inferred signatures (`type.gradual`)
- [Ensure](../control/ensure.md) — `ensure … else |e|` pattern (`ctrl.ensure`)
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — decision record for the no-wrappers surface
