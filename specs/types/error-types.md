<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: Errors are values with try propagation, try-else context, union composition, auto-Ok wrapping, origin tracking, and any Error auto-boxing -->
<!-- depends: types/enums.md, types/optionals.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Error Types

Errors are values. Any type with `message()` can be an error. Composition uses union types for type-safe propagation.

## Error Trait

| Rule | Description |
|------|-------------|
| **ER1: Structural matching** | Any type with `func message(self) -> string` satisfies the Error trait |
| **ER1a: Auto-Displayable** | Error types auto-satisfy `Displayable` — `to_string()` calls `message()`. See `std.fmt/D5` |

<!-- test: parse -->
```rask
trait Error {
    func message(self) -> string
}
```

Error types work directly in `format("{}", err)` and string interpolation without implementing `Displayable` separately.

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
| `to_option` | `func(take self) -> T?` | `Ok(t)` → `Some(t)`, `Err(_)` → `None` |
| `to_error` | `func(take self) -> E?` | `Err(e)` → `Some(e)`, `Ok(_)` → `None` |
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
| **ER10: Auto-box to `any Trait`** | `try` auto-boxes when return error type is `any Error` (or any `any Trait`) — succeeds if expression error type satisfies the trait |

<!-- test: skip -->
```rask
// Union widening — library code with precise types
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError widens to union
    const config = try parse(content)     // ParseError widens to union
    config
}

// Auto-boxing — application code with type-erased errors
func start_app() -> App or any Error {
    const config = try read_config(path)     // IoError | ParseError → boxed to any Error
    const db = try connect(config.db_url)    // DbError → boxed to any Error
    const schema = try validate(db)          // ValidationError → boxed to any Error
    App.new(config, db, schema)
}
```

The pattern: libraries use union types (precise, matchable). Applications use `any Error` (ergonomic, sufficient for logging/reporting). Downcast with `is` for recovery:

<!-- test: skip -->
```rask
match start_app() {
    Ok(app) => app.run(),
    Err(e) if e is IoError => retry(),
    Err(e) => log("fatal: {} at {}", e.message(), e.origin),
}
```

See [Union Types](union-types.md) for union type semantics. See [Traits](traits.md) for `any Trait` semantics.

## Custom Error Types

| Rule | Description |
|------|-------------|
| **ER11: Enum errors** | Errors are defined as enums with a `message()` method |

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

### @message Annotation

`@message` on an enum auto-generates the `message()` method from per-variant templates, replacing the manual match boilerplate.

| Rule | Description |
|------|-------------|
| **ER23: @message opt-in** | `@message` on an enum generates `func message(self) -> string`. Compile error if the enum already has a manual `message()` |
| **ER24: Variant template** | `@message("template")` on a variant provides the format string. Uses `{name}` for named payloads, `{0}` `{1}` for positional |
| **ER25: Auto-delegate** | Variant with a single payload satisfying Error and no `@message` annotation delegates to `inner.message()` |
| **ER26: Coverage required** | Every variant must have either a `@message` annotation or an auto-delegatable payload. Missing coverage is a compile error |

<!-- test: skip -->
```rask
// Before: manual match boilerplate
enum RegistryError {
    PackageNotFound(name: string),
    NetworkError(string),
    ChecksumMismatch(pkg: string, expected: string, got: string),
}

extend RegistryError {
    func message(self) -> string {
        match self {
            PackageNotFound(name) => format("package not found: {}", name),
            NetworkError(msg) => format("network error: {}", msg),
            ChecksumMismatch(pkg, expected, got) => {
                format("checksum mismatch for {}: expected {}, got {}", pkg, expected, got)
            }
        }
    }
}

// After: @message annotation
@message
enum RegistryError {
    @message("package not found: {name}")
    PackageNotFound(name: string),

    @message("network error: {0}")
    NetworkError(string),

    @message("checksum mismatch for {pkg}: expected {expected}, got {got}")
    ChecksumMismatch(pkg: string, expected: string, got: string),
}
```

Auto-delegation for wrapper enums — the most common pattern for composed error types:

<!-- test: skip -->
```rask
@message
enum FetchError {
    Manifest(ManifestError),          // auto-delegates: ManifestError has message()
    Version(VersionError),            // auto-delegates
    Registry(RegistryError),          // auto-delegates
    Resolve(ResolveError),            // auto-delegates
    @message("I/O error: {0}")
    Io(string),                       // needs explicit template
}
```

The compiler generates the equivalent match:

<!-- test: skip -->
```rask
// Generated by @message
extend FetchError {
    func message(self) -> string {
        match self {
            Manifest(e) => e.message(),
            Version(e) => e.message(),
            Registry(e) => e.message(),
            Resolve(e) => e.message(),
            Io(msg) => "I/O error: {msg}",
        }
    }
}
```

Manual `message()` is always available — `@message` is pure convenience, not a new concept. Both approaches satisfy ER1 the same way.

## Error Composition

| Rule | Description |
|------|-------------|
| **ER12: Same type** | Same error type propagates directly |
| **ER13: Union** | Different error types compose via union (`A \| B`) |
| **ER14: Union compose** | Union return types accept any subset union via `try` |

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
| **ER19: Linear payloads** | Errors can contain linear resources; wildcard on linear payloads is a compile error |

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

## Error Origin Tracking

| Rule | Description |
|------|-------------|
| **ER15: Origin capture** | `try` records `(file, line)` on the error at the first propagation site. Available in both debug and release builds |
| **ER16: Origin access** | All errors have `.origin` — always available in both debug and release |

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or (IoError | ParseError) {
    const content = try read_file(path)    // origin set: "config.rk:2"
    const config = try parse(content)      // origin set: "config.rk:3"
    config
}

func start() -> () or any Error {
    const config = try load_config(path)
    try run(config)
}

// In the error handler:
match start() {
    Err(e) => {
        log("{}: {}", e.origin, e.message())
        // "config.rk:2: file not found: /etc/app.conf"
    }
    Ok(_) => {}
}
```

Cost: `origin` is ~16 bytes per error (file pointer + line number) — negligible on the exceptional path.

## Error Context

Real-world error handling needs context. "IoError: file not found" is useless in production — you need "loading config from /app.toml: file not found". `map_err` handles this but is verbose enough that people skip it. `origin` tells you *where* it failed; context tells you *what you were trying to do*.

| Rule | Description |
|------|-------------|
| **ER18: try-else** | `try expr else \|e\| error_expr` extracts `Ok` or transforms the error and returns `Err(error_expr)` from the current function |

### try-else

`try...else` extends `try` with an error transformation clause. Follows the same `else |e|` pattern established by `ensure` ([ctrl.ensure/ER2](../control/ensure.md)).

<!-- test: skip -->
```rask
// ensure's else (existing) — same pattern
ensure file.close() else |e| log(e)

// try's else (new) — transforms error, then propagates
const text = try fs.read_file(path) else |e| context("reading {path}", e)
```

Desugars to:

<!-- test: skip -->
```rask
const text = match fs.read_file(path) {
    Ok(v) => v,
    Err(e) => return Err(context("reading {path}", e)),
}
```

Both expression and block forms work, matching Rask's block semantics (last expression = value):

<!-- test: skip -->
```rask
// Expression form
const text = try fs.read_file(path) else |e| context("reading {path}", e)

// Block form — when you need multiple statements
const text = try fs.read_file(path) else |e| {
    log("failed to read {path}: {e.message()}")
    context("reading {path}", e)
}
```

`try...else` is general-purpose — works for string context AND typed error wrapping:

<!-- test: skip -->
```rask
// Application code — string context chains
func load_config(path: string) -> Config or ContextError {
    const text = try fs.read_file(path) else |e| context("reading {path}", e)
    return try Config.parse(text) else |e| context("parsing {path}", e)
}

// Library code — typed domain errors
func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_file(path) else |e| ConfigError.Io { path, source: e }
    return try Config.parse(text) else |e| ConfigError.Parse { path, source: e }
}
```

`try`, `map_err`, and `try...else` are complementary:
- `map_err` — transforms the error type without propagating
- `try` — propagates without transforming
- `try...else` — transforms AND propagates in one step

Stdlib provides `ContextError` and `context()` for application-level string context chains — see `std.errors`.

## Operator Family

| Syntax | Option | Result |
|--------|--------|--------|
| `try x` | Propagate None | Propagate Err |
| `x ?? y` | Value or default | — |
| `x!` | Force (panic) | Force (panic with error info) |
| `x! "msg"` | Force (panic with message) | Force (panic with message) |

`??` doesn't work on Result — silently discarding errors masks real problems. Use `.on_err(default)` to explicitly acknowledge you're ignoring the error.

Optional sugar (`T?`, `x?.field`, `x ?? y`) is distinct from `try` propagation — `?` is never used for propagation.

## Development Panics

| Rule | Description |
|------|-------------|
| **DP1: todo()** | Panics with "not yet implemented" and source location |
| **DP2: unreachable()** | Panics with "entered unreachable code" and source location |
| **DP3: Optional message** | Both accept an optional string: `todo("auth flow")`, `unreachable("invalid state")` |
| **DP4: Never type** | Both return `Never` (coerces to any type, same as `panic`) |
| **DP5: Lint warning** | `rask lint` warns on `todo()` in non-test code |

<!-- test: parse -->
```rask
func handle(event: Event) -> Response {
    match event {
        Click(pos) => handle_click(pos),
        Key(k) => todo("keyboard handling"),
    }
}

func process(status: Status) -> i32 {
    match status {
        Active => 1,
        Inactive => 0,
        Deleted => unreachable(),
    }
}
```

**`todo()` panic output:**
```
thread panicked at 'not yet implemented: keyboard handling', src/handler.rk:4:19
```

**`unreachable()` panic output:**
```
thread panicked at 'entered unreachable code', src/handler.rk:12:21
```

## Inferred Error Unions for Private Functions

Private functions can omit error return types entirely. The compiler infers the error union from all `try` calls and explicit `Err()` returns in the body — same local analysis pattern as [Gradual Constraints](gradual-constraints.md).

| Rule | Description |
|------|-------------|
| **ER20: Error union inference** | Private functions may omit error types in their return signature. The compiler computes the union from all error-producing expressions in the body |
| **ER21: Public must be explicit** | `public` functions must declare error types explicitly (API stability, same as `type.gradual/GC5`) |
| **ER22: Recursive annotation** | Mutually recursive functions where error type is ambiguous require annotation on at least one function in the cycle (same as `type.gradual/GC2`) |

<!-- test: skip -->
```rask
// Private function — error union inferred from body
func load_config(path: string) {
    const text = try read_file(path)       // IoError
    const config = try parse(text)         // ParseError
    return config
}
// Compiler infers: -> Config or (IoError | ParseError)
// IDE ghost text shows the full signature

// Explicit error type still works — merges with inferred (GC4)
func load_config(path: string) -> Config or (IoError | ParseError) {
    const text = try read_file(path)
    return try parse(text)
}

// Public — must be explicit (ER21)
public func load_config(path: string) -> Config or (IoError | ParseError) {
    const text = try read_file(path)
    return try parse(text)
}
```

Inference rules:
- Each `try expr` where `expr` returns `T or E` contributes `E` to the inferred union
- Each `return Err(e)` where `e: E` contributes `E` to the inferred union
- `try...else |e| transform(e)` contributes the type of `transform(e)`, not the original error
- If no error-producing expressions exist, the return type has no error component
- Inferred union is deduplicated and sorted alphabetically for deterministic output

IDE integration:
- Ghost text shows inferred error union after return type
- Quick action: "Make error type explicit" fills in the full union
- Quick action: "Make public" adds `public` and the explicit error union

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Return `T` from `T or E` function | ER7 | Auto-wrapped to `Ok(T)` |
| Reach end of `() or E` function | ER8 | Implicit `Ok(())` |
| `try` on error type not in return union | ER9 | Compile error — type not subset |
| `try` on `Option` in `Result` function | ER6 | `None` maps to `Err` (types must align) |
| Wildcard on linear error payload | ER19 | Compile error — must consume |
| `try` when return type is `any Error` | ER10 | Auto-box concrete error to `any Error` |
| `.origin` in release build | ER15 | Always available (~16 bytes per error) |
| Nested `try` in closures | ER4 | Propagates to closure's return, not enclosing function |
| `try` binding with method chain | ER5 | Binds to full expression; use parens to chain after |
| `try...else` error type mismatch | ER18 | Compile error — else expression must match function's error return type |
| Private function, inferred error | ER20 | Union computed from all `try` and `Err()` in body |
| Private function, no `try` calls | ER20 | No error component — return type is plain `T` |
| Recursive private function | ER22 | Annotation required on at least one function in cycle |
| `@message` + manual `message()` | ER23 | Compile error — can't have both |
| `@message` variant, no annotation, non-Error payload | ER26 | Compile error — must annotate or use Error payload |
| `@message` with named payload | ER24 | Use `{name}` in template |
| `@message` with positional payload | ER24 | Use `{0}`, `{1}` in template |
| `@message` auto-delegate, payload is Error | ER25 | Calls `inner.message()` |

---

## Appendix (non-normative)

### Rationale

**ER1 (structural matching):** Structural matching means you don't need to import a trait to make your type an error. If it has `message()`, it works.

**ER7/ER8 (auto-Ok):** If you wanted an error, you'd use `return Err(...)` or `try`. Reaching the end means success. Eliminates noisy `Ok(())` at function ends.

**ER9 (auto-widen):** Without auto-widening, every `try` on a narrower error type would need an explicit conversion. The subset check keeps it type-safe without boilerplate.

**ER10 (auto-box):** Libraries should use precise union error types — callers can match on them. But application code that calls 5 libraries shouldn't need `-> T or (IoError | ParseError | DbError | ValidationError | AuthError)` on every function. `any Error` is the escape hatch: type-erased, sufficient for logging/reporting, with `is` downcast for recovery. This mirrors Rust's thiserror (libraries) + anyhow (applications) split, but built into the language.

**ER15 (error origin):** When an `IoError` propagates through 10 functions, "file not found" tells you nothing. `origin` captures where the error first surfaced — always available, ~16 bytes, negligible on the exceptional path. If you need the full propagation chain, add context with `try...else` at key call sites.

**ER18 (try-else):** `try + map_err` is the most common error handling pattern — nearly every `try` in real code transforms the error. Fusing them into `try...else` reduces ceremony. The `else |e|` pattern already exists in `ensure` (ctrl.ensure/ER2), so no new concepts needed.

**Operator split (`try` vs `?`):** `try` is for propagation (both Result and Option). `?` is reserved for Option sugar only — type suffix, chaining, defaults, smart unwrap. This avoids Rust's overloading where `?` means different things in different contexts.

**`??` not on Result:** Silently discarding errors masks real problems. `.on_err(default)` makes the intent explicit.

**ER23–ER26 (@message):** Error enums with 3+ variants accumulate mechanical `match self { Variant(x) => format(...) }` boilerplate — the package manager stress test has ~90 lines across 5 error types. `@message` eliminates this without new concepts: it's an annotation (like `@rename`, `@skip` in encoding) that generates code you'd write anyway. I kept it opt-in because: (1) simple errors with 1–2 variants don't need it, (2) complex formatting that can't fit a template should stay manual, (3) auto-deriving from variant names produces bad messages ("checksum mismatch" doesn't tell you *which* checksum).

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
| Integer overflow | Panic | See [integer-overflow.md](integer-overflow.md) — panic in all builds |
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

#### Dependencies
- **Union types** — See [Union Types](union-types.md) (TODO: create spec)

### See Also

- [Optionals](optionals.md) — `T?` sugar, `??` default (`type.optionals`)
- [Enums](enums.md) — Enum definitions and pattern matching (`type.enums`)
- [Union Types](union-types.md) — Union type semantics for error composition
