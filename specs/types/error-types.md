<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: T or E is a builtin sum type with type-based branch disambiguation. No Ok/Err wrappers. Disjointness rule (T ≠ E) via the nominal/alias split, checked at the call site once a generic's type argument is known. E must implement ErrorMessage. Auto-wrap fires only at return. Operator family + match for multi-error unions. -->
<!-- depends: types/types.md, types/optionals.md, types/union-types.md, types/type-aliases.md -->

# Error Types

Errors are values. `T or E` is a builtin sum type — compiler-generated tagged union — with type-based branch disambiguation. No `Ok` or `Err` constructors; the compiler picks the branch from the value's type at the return site. Every `E` implements the structural `ErrorMessage` trait.

Libraries use union errors (`T or (A | B | C)`), applications use `any Error` (type-erased boxing). Match dispatches on type; operators cover the two-branch case.

## The Type

| Rule | Description |
|------|-------------|
| **ER1: Builtin sum** | `T or E` is a compiler-generated tagged union, not a user-definable enum. Optionals (`T?`) are sugar for `T or none` and share the same machinery — see [optionals.md](optionals.md) |
| **ER2: No user wrapper** | There is no `Ok` or `Err` constructor, keyword, or pattern. Success values are bare; error values are the error type's own constructor (e.g. `DivError.ByZero`) |
| **ER3: Disjointness** | `T or E` requires T ≠ E using Rask's nominal-vs-alias distinction (see [type-aliases.md](type-aliases.md)). Checked where the type is written, and again after generic substitution (ER3a). Same rule as [union-types.md](union-types.md) U6. **Exception:** `none` — see ER3b |
| **ER3a: Disjointness is a use-site obligation** | A signature that writes `T or E` with a type parameter on either side *is* the disjointness bound — there's no separate syntax to declare it. The compiler reads the obligation off the signature and checks it at the call site, where the type argument is known. A generic caller that forwards its own parameter passes the obligation on to *its* call sites, same as a trait bound (GF3) |
| **ER3b: `none` layers instead of colliding** | `none` is exempt from disjointness and from the duplicate-variant rule. `T?` where `T` is itself optional is a legal two-layer optional, not a collision — see [optionals.md](optionals.md) |
| **ER4: Error bound** | Every `E` must implement `ErrorMessage` — `func message(self) -> string`, auto-derived for enums (ER6). Enforced at type formation. Primitives (`i32`, `f64`, `string`) don't qualify; newtype them. **Exception:** `none` is exempt — it's the absent sentinel for optionals (`T or none`), not an error type |
| **ER5: No `Result<T, E>` name** | The generic `Result<T, E>` type is gone. Use `T or E` directly |

<!-- test: skip -->
```rask
func read_file(path: string) -> string or IoError        // two-branch
func load() -> Config or (IoError | ParseError)          // union error
func save(data: Data) -> void or IoError                   // unit success
```

`T or E` is valid in return types, bindings (inferred or explicit), fields, generics — same positions as any type.

**Precedence:** `?` (tightest) > `|` (error union) > `or` (loosest). `string? or IoError | ParseError` parses as `(string?) or (IoError | ParseError)`.

### Disjointness Under Generics [ER3a]

A generic signature can be fine at the definition and broken at one instantiation:

<!-- test: skip -->
```rask
func cached<T>(f: || -> T or CacheError) -> T or CacheError    // fine: T and CacheError are different types here
```

Substitute `T = CacheError` and the return type becomes `CacheError or CacheError`. Nothing can be done at that point: the body's `return v` picked the success branch, but the only match arm the caller can write is `CacheError as e`, which reads as the error branch. The caller would be told an error happened when it didn't.

So the compiler reads `T or CacheError` as a requirement — "T may not be `CacheError`" — and checks it at the call site:

<!-- test: skip -->
```rask
cached(|| load())              // T = Config: fine
cached(|| CacheError.Miss)     // T = CacheError: error at this line
```

The error lands on the call, not inside `cached`. That's the point — a use-site failure with the type argument in hand, not a mystery error in someone else's generic body.

No new syntax. The signature already says which types can't collide; writing a separate `T: !CacheError` bound would just repeat it. When a caller genuinely wants both branches to carry a `CacheError`, newtype one side — the same escape hatch as the non-generic case.

### The `ErrorMessage` Trait

| Rule | Description |
|------|-------------|
| **ER6: Auto-derived for enums** | `ErrorMessage` is nominal, auto-derived for enums: `message()` is the humanized variant name plus payload interpolation (`UnexpectedEnd(ctx)` → `"unexpected end: {ctx}"`); a single-payload variant whose payload implements `ErrorMessage` delegates to it. Override with `extend E with ErrorMessage { ... }` for hand-written prose — `rask lint` nudges public error types toward it. Structs declare conformance (usually the header of the block defining `message()`) |
| **ER7: Auto-Displayable** | Error types auto-satisfy `Displayable`; `to_string()` delegates to `message()` |
| **ER8: Layered traits** | Richer capabilities (`LinedError`, `ContextualError`, `CodedError`) are opt-in traits on top of `ErrorMessage`. The minimum bound is just `message() -> string` |

<!-- test: skip -->
```rask
enum DivError { ByZero, Overflow }
// Nothing else needed — auto-derived (ER6):
//   ByZero → "by zero", Overflow → "overflow"

// Override for hand-written prose:
extend DivError with ErrorMessage {
    func message(self) -> string {
        match self {
            DivError.ByZero   => "division by zero",
            DivError.Overflow => "arithmetic overflow",
        }
    }
}

// Structs declare conformance in the block defining message():
struct NotFound { key: string }
extend NotFound with ErrorMessage {
    func message(self) -> string { "not found: {self.key}" }
}
```

## Construction

| Rule | Description |
|------|-------------|
| **ER9: Auto-wrap at return only** | In a function returning `T or E` (with `E ≠ none`), a `return` with a value of type `T` wraps to the success branch; a value of type `E` wraps to the error branch. The branch is picked by type; disjointness makes this unambiguous |
| **ER10: Implicit unit success** | In a function returning `void or E` reaching the end without explicit `return`, the unit success path is implied |
| **ER11: No auto-wrap elsewhere** | For `T or E` (E ≠ none), assignment, field initialisers, function arguments, and collection literals do **not** auto-wrap. The value must already have the union type (typically from a function call). **Optionals (`T or none`) are more permissive** — bare `T` and `none` widen at any position; see [optionals.md](optionals.md) |

<!-- test: skip -->
```rask
func divide(a: f64, b: f64) -> f64 or DivError {
    if b == 0.0 { return DivError.ByZero }     // E branch, by type
    return a / b                                // T branch, by type
}

func save(data: Data) -> void or IoError {
    try file.write(data)
    // implicit unit success at end
}
```

Why return-only for errors? Construction in assignment/field positions makes the error-branch coercion invisible at use sites. Keeping it at `return` means "this function produced a result"; branches are always visible at the site that produces them. Optionals don't have this concern — `none` is the absent sentinel, not a hidden failure — so the optional shape relaxes the rule.

## Operators

| Rule | Syntax | Meaning |
|------|--------|---------|
| **ER12: Boolean ok** | `r?` | `true` when in the T branch, `false` in the E branch; `bool` expression |
| **ER13: Chain** | `r?.field` | Projects `field` when T; propagates E otherwise |
| **ER14: Value fallback** | `r ?? default` | Yields T if present, else `default`. `??` is strictly extract — does not widen; `default` must have type T, and may not diverge (ER48) |
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
    try fs.read_text(path)
} else |e| context("reading {path}", e)
```

`??` is value-only; there is no closure form. Error-recovery-with-context uses the `try … else |e|` block form.

## The `else` Family

Every non-success branch is handled by `else`. Two keywords carry the whole territory: **`try` means the value may leave this function**, **`else` means the other branch is handled right here**. What follows `else` says how.

| Rule | Form | Meaning |
|------|------|---------|
| ER14 | `r ?? fallback` | fold with a constant — no error binding |
| **ER44** | `r else \|e\| f(e)` | **fold using the error** — produces `T`, nothing propagates |
| **ER45** | `const v = r else { diverge }` | bind or bail — presence guard |
| ER16 | `try r` | propagate |
| **ER46** | `try r else \|e\| f(e)` | transform, then propagate |
| **ER47** | `try r else err_expr` | replace, then propagate — binding omitted |

| Rule | Description |
|------|-------------|
| **ER44: Terminal fold** | Bare `r else \|e\| f(e)` — no `try` — collapses `T or E` into `T` by handling the error value. Binds `e: E`; `f(e)` must have type `T`. Nothing leaves the function. On a `T?` scrutinee this is a compile error pointing at `??` (no payload to bind) |
| **ER45: Presence guard** | `const v = expr else { … }` binds the success payload of a `T?` or `T or E` initialiser to `v` in the enclosing scope; the `else` block must diverge. Same rule as the pattern guard `const v = x is P else { … }` — see [control-flow.md](../control/control-flow.md) CF13–CF15. The error value is **not** available in the block; use `try … else \|e\|` when you need it |
| **ER46: Transform and propagate** | `try r else \|e\| f(e)` is the single-expression spelling of ER18. `f(e)` must be an error type reaching the current function's error union |
| **ER47: Binding omissible** | The `\|e\|` in try-else may be dropped when the replacement doesn't use the error: `try r else err_expr` ≡ `try r else \|_\| err_expr` |
| **ER48: `??` is value-only** | The right side of `??` must have type `T`. A diverging right side (`return`, `break`, `continue`, `panic(…)`) is a compile error pointing at ER45 — one operator, one reading. `Never` coercion is untouched everywhere else in the language |

<!-- test: skip -->
```rask
// Fold — the outermost boundary, where there's nothing left to propagate to
return dispatch(req) else |e| error_response(e)

// Presence guard — bind or bail
const ms = raw.parse() else { return ApiError.BadRequest("ms must be a non-negative integer") }
const sock = state is Connected else { return }          // pattern guard, same shape

// Transform, then propagate
const text = try fs.read_text(path) else |e| context("reading {path}", e)

// Replace, then propagate — the error carries nothing worth keeping
const dto = try json.decode(req.body) else ApiError.BadRequest("invalid JSON")
```

**Reading the forms.** `try` present means the error may leave the function, so the payload after `else` is an *error*. `try` absent means it may not, so the payload is either a `T` (fold) or a diverging block (guard). Getting it backwards is a type error on that line — `T ≠ E` by disjointness (ER3) — never a silent change of behavior.

`try`, `map_err`, and the two try-else forms:
- `map_err` transforms without propagating
- `try` propagates without transforming
- `try … else |e| f(e)` transforms and propagates in one step
- `try … else err_expr` replaces and propagates, discarding the original

### Presence Guard [ER45]

`const v = expr else { diverge }` desugars to:

<!-- test: skip -->
```rask
const v = match expr {
    T as inner => inner,
    _          => { diverge }      // block must not fall through
}
```

The initialiser must be a `T?` or a `T or E`; anything else is a type error. The binding escapes to the enclosing scope exactly like the pattern guard (CF14), and it is always valid after the statement because the `else` block cannot fall through (CF13). `mut` binds the same way, and a tuple destructure composes on top (`const (a, b) = pair_opt else { … }`).

The error value is discarded, so a guard on a `T or E` whose error carries a **linear** payload is a compile error — discarding a must-consume value is never implicit (ER43). Those cases want `try … else |e|` or a `match` that consumes the payload.

This is the form to reach for when the fallback is control flow rather than a value:

<!-- test: skip -->
```rask
// Guard — divert on absent
const ms = raw.parse() else { return ApiError.BadRequest("bad duration") }

// Fallback — a value on absent
const port = config.port ?? 8080
```

### Terminal Fold [ER44]

The outermost boundary of a program — a router, `main`, a task body — has nowhere left to propagate to. It has to turn `T or E` into `T`:

<!-- test: skip -->
```rask
// Before: four lines of if/else
const outcome = dispatch(req)
if outcome? as resp {
    return resp
} else as e {
    return error_response(e)
}

// After
return dispatch(req) else |e| error_response(e)
```

`??` covers the same shape when the replacement ignores the error. `rask lint` flags an unused binding in a bare fold and suggests `??` — the mirror of ER47 dropping the binding in try-else.

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

Methods removed from the old spec: `.unwrap_or`, `.unwrap_or_else`, `.is_ok`, `.is_err`, `.to_option`, `.to_error`, `.on_err`. Operators and the four surviving methods cover every case — `.unwrap_or` is `??` (ER14), `.unwrap_or_else` is the bare fold `else |e| f(e)` (ER44). See the [redesign proposal](error-model-redesign-proposal.md) for the full migration map.

There is deliberately no fold *method*. A fold ends the error's journey, and journey-endings belong to the operator family — a `.recover()` would be the first step back toward the method zoo the redesign removed (`std.api/SD4`).

## Union Widening, Wrapping, and Boxing

| Rule | Description |
|------|-------------|
| **ER31: Auto-widen** | `try` succeeds when the expression's error type is a subset of the current function's error union |
| **ER31a: Auto-wrap into a boundary enum** | `try` succeeds when the current function's error type is an enum with **exactly one** variant whose only payload is the propagated error type. `try f()` then means `try f() else \|e\| Outer.Variant(e)`. Two candidate variants is a compile error naming both — the wrap has to be unambiguous |
| **ER32: Auto-box to `any Error`** | `try` auto-boxes when the current function's error type is `any Error` — any `E` satisfying `ErrorMessage` widens by boxing |

<!-- test: skip -->
```rask
// Library: precise union
func load() -> Config or (IoError | ParseError) {
    const content = try read_file(path)   // IoError ⊆ union
    const config = try parse(content)     // ParseError ⊆ union
    return config
}

// Service boundary: one enum, one variant per wrapped error
enum ApiError {
    Store(StoreError),
    Validation(ValidationError),
    BadRequest(string),
}

func view(id: TaskId) -> TaskView or ApiError {
    const task = try store.view_task(id)  // → ApiError.Store(e)
    return task
}
```

Libraries use union errors (precise, matchable). Applications use `any Error` (ergonomic, sufficient for logging). Downcast with `if r is IoError as e` for recovery.

ER31a is the enum spelling of ER31's subset check. The union form composes error types structurally; the enum form gives the composition a name and a `match` at the boundary. Both should propagate without ceremony — writing `else |e| ApiError.Store(e)` at every call restates what the enum already says. The wrap is one hop: `StoreError` reaching an `ApiError` return is automatic, `StoreError` reaching a `TopError` that wraps `ApiError` is not.

Only a variant with a single payload of exactly the source type counts. `Store(StoreError, Context)` doesn't — the second field has no value to fill in. Neither does a variant whose payload is a union or a generic; those aren't boundary-enum wrappers.

## Error Origin Tracking

Origin tracking is **opt-in** — typed errors carry no metadata unless annotated. `any Error` boxes always track origin because they're already heap-allocated.

| Rule | Description |
|------|-------------|
| **ER33: Default no origin** | Typed errors (`IoError`, `ParseError`, user unions) carry no origin metadata. Zero overhead on the error value |
| **ER34: @traced opt-in** | `@traced` on an error type enables origin capture — `try` records `(file, line)` at the first propagation site. Adds ~16 bytes to the error value |
| **ER34a: any Error always tracks** | `any Error` carries origin unconditionally (already heap-boxed; the 16-byte cost is marginal relative to the box) |
| **ER34b: .origin access** | `.origin` is available on `@traced` types and on `any Error`. Accessing `.origin` on a non-traced typed error is a compile error |

<!-- test: skip -->
```rask
// Typed error — no origin, zero overhead
enum DivError { ByZero, Overflow }
extend DivError {
    func message(self) -> string { match self { … } }
}
// sizeof(DivError) = 1 byte (tag only)

// Traced error — opt-in, carries 16 bytes
@traced
@message
enum ConfigError {
    @message("not found: {0}") NotFound(string),
    @message("parse error at {line}") Parse(line: i32),
}
// sizeof(ConfigError) = sizeof(payload) + 16 bytes origin

func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_text(path)     // ConfigError.NotFound captures origin
    return try Config.parse(text)            // ConfigError.Parse captures origin
}

if load_config(path) is ConfigError as e {
    log("{e.origin}: {e.message()}")         // "config.rk:42: not found: app.conf"
}

// any Error — origin always available
func start_app() -> App or any Error {
    const config = try load_config(path)     // IoError auto-boxes, gets origin
    return App.new(config)
}
```

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

Each `try expr` where `expr` returns `T or E` contributes `E`. Each bare error return in the body contributes that error's type. `try … else |e| transform(e)` and `try … else err_expr` contribute the type of the replacement, not the original. A bare fold (`expr else |e| f(e)`) and a presence guard contribute **nothing** — neither one lets an error leave the function. The inferred union is deduplicated and sorted alphabetically for deterministic output.

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

**`todo()` output (`ctrl.panic/F1` format):**
```
panic at src/handler.rk:4:19: not yet implemented: keyboard handling
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Return bare `T` from `T or E` function | ER9 | Wraps to T branch |
| Return bare `E` from `T or E` function | ER9 | Wraps to E branch |
| `const x: T or E = 5` (assignment, E ≠ none) | ER11 | Type error — auto-wrap is return-only |
| `const x: T? = bare_t` (assignment) | ER11/optionals | Legal — `T or none` widens at any position |
| `T or T` | ER3 | Compile error; newtype one side |
| `f<T>() -> T or E` called with `T = E` | ER3a | Compile error at the call site, naming the parameter |
| Generic caller forwards its own `T` into `T or E` | ER3a | Obligation propagates to the caller's own call sites |
| `T?` where `T` is itself optional | ER3b | Legal — two-layer optional, see [optionals.md](optionals.md) |
| `T or i32` (primitive E) | ER4 | Compile error — E lacks `ErrorMessage` |
| `T or none` | ER4 | Legal — `none` is exempt from the `ErrorMessage` bound |
| `try r` in `fn -> T?` | — | Cross-shape, ill-typed. Use `r.ok()` then `try` |
| `try o` in `fn -> T or E` | — | Cross-shape, ill-typed. Use `o.to_result(err)` then `try` |
| `try` on narrower E into wider union | ER31 | Auto-widen succeeds |
| `try` on `E` into an enum with one `Variant(E)` | ER31a | Auto-wrap succeeds |
| `try` on `E` into an enum with two `Variant(E)`s | ER31a | Compile error — name the variant with `else \|e\|` |
| `try` on `E` into an enum with a `Variant(E, Context)` | ER31a | No wrap; falls through to the plain mismatch |
| `try` into `any Error` | ER32 | Auto-box succeeds |
| `r ?? err_value` where `err_value: E` | ER14 | Type error — `??` does not widen. Use `.to_result(err)` or match |
| `r ?? return X` | ER48 | Compile error — use `const v = r else { return X }` |
| `r else \|e\| f(e)` where `f(e): T` | ER44 | Folds to `T`; nothing propagates |
| `r else \|e\| f(e)` where `f(e): E2` | ER44 | Type error — an error type here means you wanted `try r else \|e\| …` |
| `o else \|e\| f(e)` on an optional | ER44 | Compile error — `none` carries nothing to bind. Use `??` or the guard |
| `const v = o else { … }` where the block falls through | ER45/CF13 | Compile error — the `else` block must diverge |
| `const v = x else { … }` where `x` is neither `T?` nor `T or E` | ER45 | Type error — the guard needs a two-branch initialiser |
| `try r else err_expr` (no binding) | ER47 | Legal — same as `else \|_\| err_expr` |
| Unused `e` in a bare fold | ER44 | Lint suggesting `??` |
| `mut v = r else { … }` | ER45 | Legal — binds a rebindable `v`, same as any initialiser |
| `const (a, b) = r else { … }` | ER45/DS7 | Legal — the guard binds, then the tuple destructures |
| `try r else { diverge }` | ER47 | Legal but redundant (the block's `Never` is the replacement error). Lint suggests dropping `try` for the guard |
| Guard on a `T or E` whose `E` carries a linear payload | ER43/ER45 | Compile error — the guard discards the error, and a linear payload may not be discarded. Use `try … else \|e\|` or `match` |
| `!r?` | ER26 | Parse error suggesting `r is E` |
| `r? && s?` in condition | ER25 | Legal bool; neither narrows |
| Wildcard on linear error payload | ER43 | Compile error |
| `.origin` on `@traced` error | ER34/ER34b | Available in debug and release |
| `.origin` on non-traced typed error | ER34b | Compile error |
| `.origin` on `any Error` | ER34a | Always available |
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

**Disjointness violation at instantiation [ER3a]:**
```
ERROR [type.errors/ER3a]: `T` may not be `CacheError` here
    |
12  |  const r = cached(|| CacheError.Miss)
    |            ^^^^^^ T = CacheError
    |
 4  |  func cached<T>(f: || -> T or CacheError) -> T or CacheError
    |                          ------------------ both branches become CacheError

WHY: `cached` returns `T or CacheError`. The compiler picks the branch from
     the value's type, so the two branches have to stay distinct. With
     T = CacheError the caller can't tell a cached value from a cache miss.

FIX: Newtype the success side at this call:
     type Cached = CacheError with (…)
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

**Diverging `??` right side [ER48]:**
```
ERROR [type.errors/ER48]: the right side of `??` must be a value
   |
7  |  const ms = raw.parse() ?? return ApiError.BadRequest("bad duration")
   |                            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this diverges
   |
WHY: `??` supplies a fallback value. Bailing out is a different operation,
     and reading it off the right side of `??` means the operator means two
     things depending on what follows it.

FIX: const ms = raw.parse() else { return ApiError.BadRequest("bad duration") }
```

**Bare fold on an optional [ER44]:**
```
ERROR [type.errors/ER44]: `else |e|` has nothing to bind on an optional
   |
4  |  const port = config.port else |e| default_port(e)
   |                                ^^^ `config.port` is `i32?` — `none` carries no value

WHY: The binding form folds an *error* into a value. Absence has no payload.

FIX: const port = config.port ?? default_port()      (a fallback value)
     const port = config.port else { return }        (bail out instead)
```

**Guard on a plain value [ER45]:**
```
ERROR [type.errors/ER45]: `else` guard needs an optional or a result
   |
3  |  const n = compute() else { return }
   |            ^^^^^^^^^ `compute()` returns `i32` — it always produces a value

WHY: The guard binds the success payload of a two-branch value. There's no
     failing branch here for the `else` block to handle.

FIX: const n = compute()
```

**Cross-shape try [special-case of subset mismatch]:**
```
ERROR [type.errors/CROSS_SHAPE]: cannot `try` Option in Result-returning function
   |
4  |  const x = try maybe_value
   |            ^^^ maybe_value: T?  (= T or none)
   |
   |  current function returns T or E  — `none` is not in E

WHY: `try` widens the inner error union into the function's error union.
     `none` is the absent sentinel, not an error — silently treating it
     as one would fabricate errors out of absence.

FIX: Convert explicitly:
     const x = try maybe_value.to_result(MyError.NotFound)
```

The reverse case — `try r` (a `T or E`) in a `T?`-returning function — fails the same subset check (`E ⊄ none`) and gets a parallel diagnostic suggesting `r.ok()`.

**Match on Option:**
`match x { Some(…) => …, None => … }` is rejected as `Some`/`None` are not valid Rask syntax (see the `Some(v)`/`None` diagnostic in [optionals.md](optionals.md)). The accepted form `match x { none => …, u => … }` is legal but emits a style lint suggesting the operator form.

---

## Appendix (non-normative)

### Rationale

**Why operators instead of combinator methods.** Rust handles the same territory with a method vocabulary — `unwrap`, `expect`, `unwrap_or`, `unwrap_or_else`, `map`, `map_err`, `and_then`, `or_else`, `ok`, `ok_or`, `ok_or_else`, and their siblings, doubled across `Option` and `Result`. Rask replaces the vocabulary with the operator family (ER12–ER18), and the replacement isn't cosmetic — three structural facts make the zoo unnecessary rather than merely renamed:

1. **The operators enforce extract-early style.** Rust's combinators exist to thread a *wrapped* value through a pipeline — transform the inside, stay wrapped, unwrap at the end. Rask's operators do the opposite: `try` gets the value or exits now, `??` gets the value or a default now. Once nothing threads wrapped values, `map`/`and_then`/`or_else` have no job. This is a style decision (handle it here, like Go) wearing concise syntax.
2. **One shape, not two.** Half of Rust's vocabulary is `Option`↔`Result` plumbing (`ok`, `ok_or`, `err`, …). `T?` being `T or none` on the same machinery deletes the category — `.ok()` is the one conversion that remains (cross-shape `try`, above).
3. **Operators can't breed.** Methods grow by one-line PR — that's how the zoo happened. An operator is a language change gated by the Ceremony Test (`CORE_DESIGN`). The family is *frozen by construction*: eight forms around one mental template (test, extract, fallback, force, propagate, chain), learned once as a unit. Swift's `?`/`!`/`??` and Kotlin's `?.`/`?:` are the precedent that a small closed operator set for this territory is learnable in a day and then disappears into fluency.

The corollary is a discipline: the family beats the method zoo exactly as long as it stays frozen. An ergonomic itch is answered by composing existing operators or writing a `match` — never by minting form nine. And no combinator methods through the back door: a `map_err` in the stdlib would be the zoo regrowing (`std.api/SD4`); the error-transform need is served by `try … else |e|` (ER18).

**ER1 (builtin sum).** The old spec said `Result<T, E>` was a normal enum with `T or E` as sugar. In practice Result had dedicated sugar, auto-Ok wrapping, `try` propagation, `any Error` boxing, origin tracking, and union widening — more bespoke surface than any user enum. Making `T or E` a builtin lets the spec stop pretending.

**ER3 (disjointness).** Type-based branch disambiguation at construction (no `Ok`/`Err` wrappers) only works if T ≠ E. Rask's existing nominal-vs-alias split gives this for free: nominal types are distinct, aliases are transparent. The escape hatch is newtype, not a wrapper keyword.

**ER3a (checked at the call site, not the definition).** Three ways to handle a generic whose `T or E` can collapse:

1. Reject the bad instantiation at the call site.
2. Add a negative bound, `T: !CacheError`.
3. Auto-newtype one branch behind the scenes.

I picked 1. Option 3 doesn't actually work: renaming a branch internally doesn't help the caller, who still has one type name and two branches to point it at. Option 2 buys nothing — the signature already says `T or CacheError`, so a separate `T: !CacheError` clause is the same fact written twice, and negative bounds drag in reasoning ("does any type *not* equal this one?") that the rest of the language doesn't need.

Rask already checks generics at the use site (`type.generics/G2`) rather than proving the definition good for all `T`. So an instantiation-time disjointness failure isn't a new kind of error — it's the same shape as a failed trait bound, and it lands in the same place: the caller's line, with the type argument named. The C++-template failure mode is an error reported *inside* someone else's body with no path back to the call. This one is reported on the call.

The honest cost: `func cached<T>(…) -> T or CacheError` is not total over `T`, and its signature doesn't say so in a single glance. That's the price of type-based branch selection. It's paid by a small set of generics — those that mix a type parameter with a concrete error in one `or` — and the diagnostic points at the fix.

**ER3b (`none` is the one variant that layers).** Disjointness exists so that *branch selection stays decidable* — on the producing side (which branch does `return x` pick?) and on the consuming side (which branch does `match … { E as e }` name?). Substituting `T = E` breaks the consuming side irreparably, because `E` carries a payload the caller wants and now names two branches.

`none` is different on both counts. It carries no payload, and its layers are reached in order: the outer operators (`?`, `??`, `!`, `== none`) act on the outer layer, and the inner layer is only visible after narrowing through it. One rule — a bare `none` literal means the outer absent — closes the only remaining ambiguity. So `none` layers and payload variants don't. See [optionals.md](optionals.md).

**ER4 (ErrorMessage bound).** A minimum bound on E solves three problems at once: (1) `r!` can always produce a useful panic message; (2) primitives can't accidentally be error types, so `i32 or i32` style ambiguities don't arise; (3) richer capabilities (context, codes, stack traces) layer opt-in on top without forcing complexity on simple errors.

**ER9 (auto-wrap return-only).** Auto-wrap at assignment/field/argument positions makes the branch choice invisible at the use site. Restricting it to `return` keeps the error branch visible — you can only produce a `T or E` by returning from a function declared to return one.

**ER14 (no `??` widening).** `??` that widens into `T or E` when the RHS doesn't match T would be a second type rule for one operator. Keeping `??` as strict-extract means one mental model ("fallback to an inner value"). Option→Result lifting uses the explicit `.to_result(err)` method.

**ER48 (`?? return` had to go).** `??` used to do double duty. `x ?? 50` supplies a value; `x ?? return err` bails out of the function. Both typechecked — `return err` has type `Never`, which coerces to anything — but the operator's meaning changed with its right side. Reading a line took a look at what came *after* the operator to know whether control flow was involved. Kotlin's `?: return` earns the same complaint.

So `??` is value-only and the bail-out gets its own form. The one I picked already existed:

<!-- test: skip -->
```rask
const sock = state is Connected else { return }     // pattern guard — already in the language
const ms = raw.parse() else { return BadRequest }   // presence guard — same shape, optional scrutinee
```

Extending the guard family to optionals and results costs no new keyword, no new grammar (an initialiser followed by `else {` was previously invalid), and no new rule to teach — CF13's "the `else` block must diverge" carries over unchanged. It's Rust's `let-else` and Swift's `guard let`, which is the part of optional handling both ecosystems got right.

The narrow carve-out is deliberate: only `??`'s right side rejects `Never`. Coercion stays untouched elsewhere, because elsewhere it isn't ambiguous — `Never` in a match arm or an `if` branch doesn't change what the surrounding construct means.

**Rejected: `expr? else return X`.** Postfix `?` is already a `bool` expression (ER12), so `expr?` would be a bool on its own and an unwrapped `T` with an `else`. One token, two result types — a worse double-reading than the one being fixed.

**ER44 (a fold operator, not a fold method).** The error-model redesign removed `.unwrap_or_else` and pointed its migration at `try { … } else |e| f(e)`. That was wrong: ER18 early-returns the transformed error, so it propagates and cannot produce a value. Nothing covered the *terminal* fold — collapsing `T or E` into `T` at a boundary with nothing above it. Every program has some: routers, `main`, task bodies.

<!-- test: skip -->
```rask
return dispatch(req) else |e| error_response(e)
```

The alternative was a method (`.recover(|e| …)`), and a method is how the zoo starts. The operator family already had the pieces — `else` for "the other branch is handled here", the `|e|` payload for "using the error value" — so the fold is a composition of what's there, not a ninth form invented for the occasion. The whole family, with `try` as the single bit that says whether the error can leave the function:

| Form | Meaning |
|---|---|
| `x ?? fallback` | fold with a constant |
| `x else \|e\| f(e)` | fold using the error |
| `x else { diverge }` | bind or bail |
| `try x` | propagate |
| `try x else \|e\| f(e)` | transform, then propagate |
| `try x else err_expr` | replace, then propagate |

The try/no-try distinction is safe rather than merely conventional: in the bare form `f(e)` must be a `T`, in the try form an `E`, and `T ≠ E` by disjointness (ER3). Writing the wrong one is a type error on that line.

**ER47 (dropping `|e|`).** In real code the replace-the-error case discards the binding constantly — the validation example wrote `else |e| ApiError.BadRequest("invalid JSON")` six times in one file, never touching `e`. The binding is required by grammar and ignored by the author, which is ceremony with no reader benefit. `try expr else err_expr` says the same thing. `try`'s presence plus the expression-vs-block shape keeps all three `else` forms apart without the binding having to disambiguate them.

**ER31/ER32 (libraries vs applications).** Libraries should expose precise union errors so callers can match and recover. Application code calling 5 libraries shouldn't re-declare every error on every function — `any Error` is the escape hatch, type-erased, with `is` downcast for recovery. Same split as Rust's thiserror + anyhow, built into the language.

**ER31a (the third shape).** Between the union and `any Error` sits the boundary enum: one variant per wrapped error, so the caller still gets a typed `match`. Before this rule that shape paid for itself at every call — the validation example carried 23 hand-written `else |e| e.to_api()` maps, the single most-repeated thing in a 1600-line program. The maps carried no information: the enum declaration already says `Store` is where a `StoreError` goes. The one-variant restriction is what makes the inference safe — where the enum is ambiguous the compiler says so instead of guessing.

**ER33/ER34 (origin opt-in).** Forcing 16 bytes of origin metadata on every error value violates transparency of cost — an error as small as `enum DivError { ByZero }` (1 byte) would become 17 bytes with always-on tracking. The overhead is paid on the error path, but also shows up in the size of any `T or E` union, cache lines, and return ABI. Making `@traced` opt-in means library authors decide per-type. `any Error` is already heap-boxed, so tracking origin there is marginal and the ergonomic payoff (application-level diagnostics) is highest.

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
    const text = try fs.read_text(path) else |e| context("reading {path}", e)
    return try Config.parse(text) else |e| context("parsing {path}", e)
}
```

**Typed domain errors.** For library-level errors, wrap in domain-specific types. A variant that adds context beyond the error needs the explicit map:

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_text(path) else |e| ConfigError.Io { path, source: e }
    return try Config.parse(text) else |e| ConfigError.Parse { path, source: e }
}
```

A variant that carries nothing but the error doesn't — ER31a fills it in:

<!-- test: skip -->
```rask
enum ConfigError {
    Io(IoError),
    Parse(ParseError),
}

func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_text(path)   // → ConfigError.Io(e)
    return try Config.parse(text)         // → ConfigError.Parse(e)
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
- `.origin` hover shows the capture site (on `@traced` types and `any Error`).
- Ghost text shows `[traced: N bytes]` on `@traced` error types to make the overhead visible.

### See Also

- [Optionals](optionals.md) — `T?`, operator family, narrowing (`type.optionals`)
- [Union Types](union-types.md) — `A | B` error composition (`type.unions`)
- [Type Aliases](type-aliases.md) — nominal vs transparent (`type.aliases`)
- [Gradual Constraints](gradual-constraints.md) — inferred signatures (`type.gradual`)
- [Ensure](../control/ensure.md) — `ensure … else |e|` pattern (`ctrl.ensure`)
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — decision record for the no-wrappers surface
