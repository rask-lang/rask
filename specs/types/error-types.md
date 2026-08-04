<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: T or E is a builtin sum type with type-based branch disambiguation. No Ok/Err wrappers. Disjointness rule (T ≠ E) via the nominal/alias split, checked at the call site once a generic's type argument is known. E must implement ErrorMessage. Auto-wrap fires only at return. `or` supplies the other branch (a T alone, an E under try); `try` marks that an error may leave the function; `?` is absence-only, so results narrow with `is`. No ??, no fold method, no presence guard. -->
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
| **ER12: No `?` on a result** | `r?` / `r?.field` | Compile error, both forms. `?` marks absence; a result's other branch is an error. Test with `is` (ER23) or `match`; to project, extract first |
| **ER14: Other branch** | `r or v` | Yields T when present, else `v`. Results only — the absent branch of a `T?` uses `??` (`type.optionals/OPT11`). See the `or` family below |
| **ER15: Force** | `r!` | Extracts T, or panics using `E.message()`; `r! "msg"` overrides with a custom message |
| **ER16: Propagate** | `try x` | Extracts the success payload, or returns early with the other branch: an error widened into this function's error type (ER31/ER31a/ER32), or `none` in a `T?`-returning function. An `else` clause overrides what leaves (ER45) |
| **ER16a: Chain placement** | `try a.b().c` | `try` attaches to the one step in the postfix chain that is fallible — `try read_file(p).len()` is `(try read_file(p)).len()`, `try store.get(id)` is `try (store.get(id))`. A wrapped value has no payload methods, so normally exactly one placement type-checks. If two do, that's a compile error asking for parentheses. `try` does not slide into call arguments |
| **ER17: Propagate block** | `try { … }` | Each `try` inside propagates; the first other-branch value short-circuits out. Shape follows ER16 |
| **ER18: Block with a handler** | `try { … } or \|e\| …` | The block is the left side of an ordinary `or`, so the same rule applies: a value folds, a `return` leaves. `try { … } or \|e\| return f(e)` is the block spelling of ER46 |

<!-- test: skip -->
```rask
// Single-call propagation
const data = try read_file(path)

// Extract, then use — `try` binds loosely, so parenthesise to keep it on one line
const size = try read_file(path).len()

// Force
const config = load_config()!

// A block as the left side of `or` — same rule, so `return` to leave
const content = try {
    try fs.read_text(path)
} else |e| return context("reading {path}", e)
```

ER13, ER19, ER20 and ER26 are retired — the `?.` chain on results, the `if r?` predicate and its `as v` bind, and the `!r?` parse error, all of which assumed `?` worked on results. Narrowing a result is `is` (ER23). ER21 and ER22 survive: the `else`-narrows and `else as e` rules were never about `?`, so they re-home onto the `is` test unchanged.

## The `or` Family

`or` is the keyword the type uses — `T or E` — and it means the same thing at the value level: **`or` supplies the failing branch.** Optionals get their own operator, `??`, for the absent branch (`type.optionals/OPT11`); the two are deliberately not the same token.

Every optional form carries a `?`. No failure form does. That's the whole mnemonic — you never look up which operator applies, because the type's own spelling tells you.

**`or` never leaves the function.** Every line that can exit wears `try`, so "does control leave here?" is answered by looking at the line rather than by reading the right-hand side.

| Rule | Form | Leaves? | The other branch |
|------|------|---------|------------------|
| **ER14** | `r or v` | no | is this value |
| **ER44** | `r or \|e\| f(e)` | no | is this value, computed from the error |
| **ER16** | `try r` | **yes** | propagates as-is, widened into this function's error type |
| **ER45** | `try r else return e_val` | **yes** | exits with `e_val` |
| **ER46** | `try r else \|e\| return f(e)` | **yes** | exits with something built from the old error |

| Rule | Description |
|------|-------------|
| **ER14: Other branch** | `r or v` yields the success payload, or `v`. Results only — absence uses `??` (`type.optionals/OPT11`). `v` may `break`, `continue` or `panic(…)`, which are local and visible; it may **not** `return`, because leaving the function is `try`'s job (ER48) |
| **ER44: Using the error** | `r or \|e\| f(e)` binds `e: E` and yields `f(e): T`. Nothing leaves |
| **ER45: `try … else`** | The `else` clause replaces what `try` would have propagated. Its body **must diverge** — the same rule as the pattern guard (`ctrl.flow/CF13`) — so the `return` is written out and the control path is visible at the line. `try r else return E` needs no binding; `try r else \|e\| return f(e)` binds the old error first |
| **ER47: `try` supplies or is supplied** | Bare `try x` propagates the other branch unchanged, so the enclosing function's other branch has to accept it: `try r` needs an error type, `try opt` needs a `T?` return. With an `else`, that constraint lifts — the clause says what leaves, so `try opt else return MyError` and `try r else return none` are both fine |
| **ER48: `or` and `??` don't `return`** | A `return` on the right of `or` or `??` is a compile error pointing at `try … else`. Everything that leaves the function carries `try`; nothing else does |

<!-- test: skip -->
```rask
// A value on the other branch
const port = config.port ?? 8080                  // optional — `??`
const theme = load_theme() or Theme.default()     // result — `or`, error ignored

// A value computed from the error — the fold at a boundary
return dispatch(req) or |e| error_response(e)

// Leave, naming the error — same shape of line, one operator each
const ms = try raw.parse() else return ApiError.BadRequest("ms must be non-negative")
const dto = try json.decode(req.body) else return ApiError.BadRequest("invalid JSON")

// Leave, transforming the error first
const text = try fs.read_text(path) else |e| return context("reading {path}", e)

// Leave the loop
const item = queue.pop() ?? break

// Propagate unchanged — the common case, and the reason `try` exists
const data = try read_file(path)

// Braces when the right side needs more than one expression
const text = fs.read_text(path) or |e| {
    log("failed to read {path}: {e.message()}")
    return context("reading {path}", e)
}
```

The right side of `or` is an expression, and a block is an expression, so braces are available whenever they help and never required for a single one.

### Chaining [ER14a]

Because the right side sets the result type, a chain of fallbacks reads flat and needs no parentheses. `or` is left-associative, and that's the correct grouping:

<!-- test: skip -->
```rask
const name = user?.display_name ?? user?.email ?? "anon"
//           \_____ T? ______/    \___ T? ___/    \ T /
//           \________ T? _____________________/
//           \_______________ string ______________/
```

Each step whose right side is still a `T?` stays wrapped and keeps going; the first bare `T` collapses it. A further `or` after the collapse is a type error, because the left side is no longer two-branch.

The same rule gives "first source that works" on results, keeping the last error:

<!-- test: skip -->
```rask
const cfg = try load_from_disk() or load_from_env() or load_from_net()
//                T or DiskError    T or EnvError     T or NetError
//          result: T or NetError — earlier errors are discarded
```

Discarding the earlier errors is the same information loss as `r or v`, and the same linearity rule applies (ER43): an error carrying a must-consume payload can't be dropped this way.

**Reading a line.** Look for `try`. If it's there, control can leave this line, and the `else` clause — if any — says with what. If it isn't, nothing leaves: `or` and `??` produce values. That's the whole of it, and it's a glance rather than an inference.

**In statement position** — a `void or E` call whose result you aren't binding — the same forms apply, and the fold is the one that reads best: the success type is `void`, so a void-returning handler needs no ceremony.

<!-- test: skip -->
```rask
try save(d)                                     // propagate — the usual one
try save(d) else return IoError.Full            // replace and leave
save(d) or |e| log(e.message())                 // handle here: `log` returns void ✓
save(d)                                         // W2: unused result of type `void or IoError`
```

`save(d) or IoError.Full` is a type error — `or` produces the success type, and that's `void`. Deliberately ignoring the error is `const _ = save(d)`, which silences W2 the same way any other unused binding does (`tool.warnings/W3`).

**The `|e|` is not a closure.** It looks like one — Rask closures are `|x| expr` — but the right side of `or` is an ordinary expression in the enclosing scope, with `e` bound in it. So `return` there exits the function, which is the whole point of ER45/ER46:

<!-- test: skip -->
```rask
const text = try fs.read_text(path) else |e| return context("reading {path}", e)
//                                     ^^^^^^ leaves `load_config`, not a closure

const text = fs.read_text(path) or |e| {         // braces are fine, still not a closure
    log("failed to read {path}: {e.message()}")
    return context("reading {path}", e)
}
```

`with pool[h] as entity { … }` already works this way — a binding plus a real block, where `return`/`try`/`break`/`continue` mean what they say. `or |e|` is the same shape for the same reason. Were it a closure, CF26 would capture the `return` and the transform-and-leave form would be unwritable.

**`try` and `or` don't overlap.** They answer different questions, and a line uses one or the other:

| | |
|---|---|
| `try r` | leaves, keeping the error (converted to this function's error type — ER31/ER31a/ER32) |
| `try r else return E` | leaves, replacing it |
| `try r else \|e\| return f(e)` | leaves, replacing it with something derived from it |
| `r or v` | **doesn't leave** — the error is handled here |

An earlier draft let `or` return as well, which meant two spellings for leaving and no way to tell from the line whether one had happened. `try` owning every exit removes both problems, and the mandatory `return` in the `else` clause means the exit is never implied by a bare value.

**Why the marker goes at the front.** `try` is the most common operation in the language (369 sites in the tree) and putting it at the head of the line means a reader scanning the left margin sees every place control can leave. That is the property the whole split is for. The error-conversion rules — widening into a union, wrapping into a boundary enum, boxing into `any Error` — are attached to `try` as well, so bare `try r` does work no other form does.

**`try` doesn't care which shape it's given.** It propagates the other branch, whatever that branch is — an error from a `T or E`, an absence from a `T?`:

<!-- test: skip -->
```rask
func lookup(id: UserId) -> Profile? {
    const user = try find_user(id)        // absent → this function returns none
    return try user.profile               // same again
}

func load(path: string) -> Config or IoError {
    const text = try fs.read_text(path)   // failed → this function returns the IoError
    return try Config.parse(text) else return IoError.Malformed(path)
}
```

Bare `try` needs the caller's other branch to accept what it propagates (ER47). When it doesn't, the `else` clause says what leaves instead.

### Absence and Error Are Spelled Differently

`?` marks absence. `or` and `try` mark errors. Nothing crosses over, so a line tells you which kind of failure it's handling without your having to remember what the scrutinee was.

| | Optionals `T?` | Errors `T or E` |
|---|---|---|
| test / bind | `x?`, `x? as v`, `x == none` | `r is T as v`, `r is E as e` |
| project | `x?.field` | `try r.field` (ER16a places the `try`) |
| other branch | `x or v` | `r or v`, `r or \|e\| f(e)` |
| propagate | `try x` (in a `T?` fn) | `try r` |
| leave otherwise | `try x else return e_val` | `try r else \|e\| return f(e)` |
| assert / dispatch | `x!`, `match` | `r!`, `match` |

One operator per shape, and the shape is visible in the line: the `?`-family for absence, `or` and `try` for failure. Nothing has to be looked up.

Two operators are shared outright, each because the operation genuinely doesn't care which branch went bad:

- **`!`** asserts the good branch and panics otherwise. It never claims *which* branch failed; only the panic message differs (`"none"` versus `e.message()`).
- **`match`** is multi-arm dispatch, shape-neutral by construction.

`?.` is **not** among them. Chaining a projection through a result — `r?.field` yielding `Field or E` — threads a wrapped value through a pipeline, which is the shape this design deliberately doesn't have (see the extract-early argument in the appendix; it's why there's no `map` or `and_then` either). `?.` on a result would be `map` with punctuation, and it would borrow Rust's reading of `?` as propagation on top of that. Extract first, then project — `try` finds its own place in the chain (ER16a):

<!-- test: skip -->
```rask
const size = try read_file(path).len()        // ER16a places the `try`; no parens needed

const text = try read_file(path)              // or bind it, when the line gets long
const size = text.len()
```

Optional chaining is a different animal and stays: `user?.profile?.name` asks one question — is the whole path there? — and lands in `or`, `?`, or `!` immediately, rather than carrying a wrapper onward.

### Loop Exits and Panics [ER14]

`break`, `continue` and `panic(…)` stay legal on the right of `or` and `??`. They don't leave the function, they're visible where they are, and the one-liner is worth keeping:

<!-- test: skip -->
```rask
const item = queue.pop() ?? break                   // out of the loop
const line = reader.next() ?? continue              // next iteration
const home = env("HOME") ?? panic("no HOME")        // same as `env("HOME")!`
```

`return` is the one that needs `try`, because a function exit is the thing a reader has to be able to find by scanning. When the check reads better as a condition than as a fallback, the early-exit narrow is still there (ER24, OPT21):

<!-- test: skip -->
```rask
const dto = json.decode(body)
if dto is JsonError {
    log("bad body from {req.peer}")
    return Response.bad_request("invalid JSON")
}
save(dto)                                 // dto: Dto from here
```

## Conditions and Narrowing

Narrowing rides on `const`. See [optionals.md](optionals.md) for the shared semantics; on `T or E` the predicate is a type pattern rather than `?`.

| Rule | Description |
|------|-------------|
| **ER23: Type pattern narrow** | `if r is ErrType as e { … }` narrows and binds when `r`'s error side is (or contains) `ErrType`. Works for widened unions: `if r is IoError as io { … }`. `if r is T as v` tests the success side the same way |
| **ER21: else branch narrows** | On a const scrutinee, the `else` of an `is` test narrows to the complement: `if r is Config { … } else { … }` gives the error side in the `else` |
| **ER22: Bind in else** | `if r is Config as c { … } else as e { … }` binds the complement in the `else` branch |
| **ER24: Early-exit narrow** | If a branch diverges, the fall-through is narrowed to the opposite variant |
| **ER25: Compound does not narrow** | `r is A && s is B` is a legal bool but does not narrow either side |

<!-- test: skip -->
```rask
const r = divide(a, b)

if r is f64 as v {
    use(v)                        // v: f64
}

if r is f64 { use(r) }
else as e { log(e.message()) }    // e: DivError            [ER22]

if r is DivError as e {
    log(e.message())              // e: DivError
    return
}
// r: f64 here (early-exit narrow)   [ER24]
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

Match earns its keep on multi-error unions. Two-branch cases usually read better as the fold (`r or |e| f(e)`) or a type-pattern narrow.

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

Methods removed from the old spec: `.unwrap_or`, `.unwrap_or_else`, `.is_ok`, `.is_err`, `.to_option`, `.to_error`, `.on_err`. Operators and the four surviving methods cover every case — `.unwrap_or` is `x or v` (ER14), `.unwrap_or_else` is `x or |e| f(e)` (ER44). See the [redesign proposal](error-model-redesign-proposal.md) for the full migration map.

There is deliberately no fold *method*. A fold ends the error's journey, and journey-endings belong to the operator family — a `.recover()` would be the first step back toward the method zoo the redesign removed (`std.api/SD4`).

## Union Widening, Wrapping, and Boxing

| Rule | Description |
|------|-------------|
| **ER31: Auto-widen** | `try` succeeds when the expression's error type is a subset of the current function's error union |
| **ER31a: Auto-wrap into a boundary enum** | `try` succeeds when the current function's error type is an enum with **exactly one** variant whose only payload is the propagated error type. `try f()` then means `try f() or \|e\| Outer.Variant(e)`. Two candidate variants is a compile error naming both — the wrap has to be unambiguous |
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

ER31a is the enum spelling of ER31's subset check. The union form composes error types structurally; the enum form gives the composition a name and a `match` at the boundary. Both should propagate without ceremony — writing `or |e| ApiError.Store(e)` at every call restates what the enum already says. The wrap is one hop: `StoreError` reaching an `ApiError` return is automatic, `StoreError` reaching a `TopError` that wraps `ApiError` is not.

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

Each `try expr` where `expr` returns `T or E` contributes `E`. Each bare error return in the body contributes that error's type. `try expr else return e_val` and `try expr else |e| return f(e)` contribute the type of what is returned. `or` and `??` contribute **nothing** — no error leaves the function through them. The inferred union is deduplicated and sorted alphabetically for deterministic output.

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
| `try o` in a `U?`-returning function | ER47 | Legal — absence propagates as absence |
| `try o` in a `T or E`-returning function, no `else` | ER47 | Compile error — nothing to propagate as an error. Add `else return MyError` |
| `try o else return MyError` | ER45 | Legal anywhere — the clause says what leaves |
| `try r else return none` in a `T?`-returning function | ER45 | Legal — drops the error detail, and says so |
| `try r else v` where `v` doesn't diverge | ER45 | Compile error — the clause must diverge, so the exit is written out |
| `r or return X` | ER48 | Compile error pointing at `try r else return X` |
| `x ?? return X` | ER48 | Compile error pointing at `try x else return X` |
| `r or break` / `or continue` / `or panic(…)` | ER14 | Legal — local and visible; only `return` needs `try` |
| `r or v` where `v: E` | ER14 | Type error — `or` produces a `T`. To leave with it, `try r else return v` |
| `try a.b().c` with one fallible step | ER16a | Legal — `try` attaches to that step; no parens |
| `try v.map(f)` where both `v` and `v.map` are fallible | ER16a | Compile error — two placements type-check; parenthesise |
| `r or \|e\| f(e)` where `f(e): T` | ER44 | The expression is that `T`; nothing leaves |
| `o or …` on an optional | ER14 | Compile error — `or` is for failure. Use `??` |
| Unused `e` in `r or \|e\| f(e)` | ER44 | Lint suggesting `r or v` |
| `x or v` where `x` is neither `T?` nor `T or E` | ER14 | Type error — `or` needs a two-branch left side |
| `a or b` on two bools | ER14 | Type error suggesting `\|\|` |
| `void_call() or E` | ER14 | Type error — the success type is `void`. Use `try void_call() else return E`, or handle it with `or \|e\| log(e)` |
| `r or v` where `E` carries a linear payload | ER43 | Compile error — the error is discarded, and a linear payload may not be. Use `r or \|e\| …` and consume it, or `match` |
| `r?` as a bool | ER12 | Parse error — `?` is absence. Use `if r is E`, or `match` |
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

**`return` on the right of `or` [ER48]:**
```
ERROR [type.errors/ER48]: `or` doesn't leave the function
   |
7  |  const dto = json.decode(body) or return ApiError.BadRequest("bad json")
   |                                   ^^^^^^ this returns

WHY: every line that can exit the function carries `try`, so a reader scanning
     the left margin sees all of them. `or` handles the error here instead.

FIX: const dto = try json.decode(body) else return ApiError.BadRequest("bad json")
```

**Error value where a `T` is expected [ER14]:**
```
ERROR [type.errors/ER14]: `or` needs a value of type `i32` here
   |
7  |  const ms = raw.parse() or ApiError.BadRequest("bad duration")
   |                            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ this is an ApiError

WHY: `or` handles the failure here and produces a value, so it has to be the
     same type as the good branch.

FIX: to leave with it instead:
     const ms = try raw.parse() else return ApiError.BadRequest("bad duration")
```

**`else` clause that doesn't diverge [ER45]:**
```
ERROR [type.errors/ER45]: the `else` clause of a `try` must diverge
   |
4  |  const dto = try json.decode(body) else ApiError.BadRequest("bad json")
   |                                         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ falls through

WHY: `try` means control can leave here. Writing the exit out — rather than
     letting a bare value imply it — is what keeps the path visible.

FIX: else return ApiError.BadRequest("bad json")

  or, if you meant to handle it here rather than leave:
     const dto = json.decode(body) or fallback_dto()
```

**Binding form on an optional [ER44]:**
```
ERROR [type.errors/ER44]: `or |e|` has nothing to bind on an optional
   |
4  |  const port = config.port or |e| default_port(e)
   |                              ^^^ `config.port` is `i32?` — `none` carries no value

WHY: The binding form folds an *error* into a value. Absence has no payload.

FIX: const port = config.port or default_port()
```

**`try` on an optional with nothing to propagate [ER47]:**
```
ERROR [type.errors/ER47]: `try` has nothing to propagate here
   |
4  |  const x = try maybe_value
   |            ^^^ maybe_value: T?, but this function returns `Config or IoError`

WHY: bare `try` hands the other branch to the caller unchanged, and `none` isn't
     an error. Either say what should leave, or return an optional.

FIX: const x = try maybe_value else return IoError.NotFound
```

In a function that returns an optional, `try opt` is fine — absence propagates as absence. The mirror case is a result in a `T?`-returning function: `try r else return none`, which says plainly that the error detail is being dropped. `.ok()` keeps the wrapper for chaining.

**`?` used as a success test on a result [ER12]:**
```
ERROR [type.errors/ER12]: `?` tests for absence, not for errors
   |
5  |  if r? { use(r) }
   |      ^ `r` is `f64 or DivError` — its other branch is an error, not `none`

WHY: `?` is the absence marker throughout the language. Errors are handled by
     `or`, `try`, and `is`, so a line says which kind of failure it deals with.

FIX: if r is f64 as v { use(v) }          (test the success side)
     if r is DivError as e { … }           (test the error side)
     const v = r or fallback               (just supply a value)
```

**Match on Option:**
`match x { Some(…) => …, None => … }` is rejected as `Some`/`None` are not valid Rask syntax (see the `Some(v)`/`None` diagnostic in [optionals.md](optionals.md)). The accepted form `match x { none => …, u => … }` is legal but emits a style lint suggesting the operator form.

---

## Appendix (non-normative)

### Rationale

**Why operators instead of combinator methods.** Rust handles the same territory with a method vocabulary — `unwrap`, `expect`, `unwrap_or`, `unwrap_or_else`, `map`, `map_err`, `and_then`, `or_else`, `ok`, `ok_or`, `ok_or_else`, and their siblings, doubled across `Option` and `Result`. Rask replaces the vocabulary with the operator family (ER12–ER18, ER44–ER48), and the replacement isn't cosmetic — three structural facts make the zoo unnecessary rather than merely renamed:

1. **The operators enforce extract-early style.** Rust's combinators exist to thread a *wrapped* value through a pipeline — transform the inside, stay wrapped, unwrap at the end. Rask's operators do the opposite: `try` gets the value or exits now, `or` gets the value or a replacement now. Once nothing threads wrapped values, `map`/`and_then`/`or_else` have no job. This is a style decision (handle it here, like Go) wearing concise syntax.
2. **One shape, not two.** Half of Rust's vocabulary is `Option`↔`Result` plumbing (`ok`, `ok_or`, `err`, …). `T?` being `T or none` on the same machinery deletes the category — `.ok()` is the one conversion that remains (cross-shape `try`, above).
3. **Operators can't breed.** Methods grow by one-line PR — that's how the zoo happened. An operator is a language change gated by the Ceremony Test (`CORE_DESIGN`). The family is *frozen by construction*: a handful of forms around one mental template (test, extract, other-branch, force, propagate, chain), learned once as a unit. Swift's `?`/`!`/`??` and Zig's `orelse`/`catch` are the precedent that a small closed operator set for this territory is learnable in a day and then disappears into fluency.

The corollary is a discipline: the family beats the method zoo exactly as long as it stays frozen. An ergonomic itch is answered by composing existing operators or writing a `match` — never by minting form nine. And no combinator methods through the back door: a `map_err` in the stdlib would be the zoo regrowing (`std.api/SD4`); the error-transform need is served by `try … else |e| return …` (ER46).

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

`none` is different on both counts. It carries no payload, and its layers are reached in order: the outer operators (`?`, `or`, `!`, `== none`) act on the outer layer, and the inner layer is only visible after narrowing through it. One rule — a bare `none` literal means the outer absent — closes the only remaining ambiguity. So `none` layers and payload variants don't. See [optionals.md](optionals.md).

**ER4 (ErrorMessage bound).** A minimum bound on E solves three problems at once: (1) `r!` can always produce a useful panic message; (2) primitives can't accidentally be error types, so `i32 or i32` style ambiguities don't arise; (3) richer capabilities (context, codes, stack traces) layer opt-in on top without forcing complexity on simple errors.

**ER9 (auto-wrap return-only).** Auto-wrap at assignment/field/argument positions makes the branch choice invisible at the use site. Restricting it to `return` keeps the error branch visible — you can only produce a `T or E` by returning from a function declared to return one.

**ER14 (one operator per shape).** `??` used to cover both shapes — `x ?? 50` on an optional, `r ?? 0.0` on a result — which put the absence marker on error handling and meant a line couldn't tell you which kind of failure it dealt with. An intermediate draft fixed that by unifying the other way: delete `??`, give `or` both shapes. That was worse, and measurably so — it traded one token for two standing inferences (which shape is this line about; does `try` apply here), and the rule count went *up* rather than down.

So: one operator each, matching the type's own spelling.

<!-- test: skip -->
```rask
const port = config.port ?? 8080          // T?     — the `?`-family
const theme = load_theme() or fallback    // T or E — `or`, like the type
```

The mnemonic isn't a rule, it's a glance: **a `?` in the line means something is missing; no `?` means something failed.** That's the cheapest kind of thing to remember, and it's why this is smaller than the merged version despite one more token — a keyword is recall, paid once; an inference is paid on every line you read.

Zig is the precedent: `orelse` for optionals, `catch` for error unions, one keyword each, and the smallest surface of any language that covers both shapes without falling back on verbosity. The languages with fewer forms either have one shape (Kotlin) or accept `if err != nil` at every site (Go, Gleam).

`a or b` on two bools stays a catchable mistake, with a diagnostic pointing at `||`.

**ER12 (`?` means absence, and only absence).** Two spec files used to disagree: `optionals.md` OPT3 restricted the `?`-family to `T or none`, while this file handed out `r?` as a success test and `if r?` narrowing on results. Under the split, `?` never touches a result's success test — `is` does it, and `is` is strictly more informative, since `if r is IoError as e` names what it's testing while `if r?` makes the reader recall what `r` was.

Deleting the `if r?` narrowing family costs less than it looks like. On results the high-frequency operations are `try` and the fold; the narrowing family was the rare one, and where it's wanted `is` covers it in the same number of tokens.

Two operators stay shared, each because the operation genuinely doesn't care which branch went bad: `!` asserts the good branch without claiming which one failed, and `match` is multi-arm dispatch.

`?.` was the hard case. It survived an earlier draft of this rule on the argument that the `?` binds to the dot rather than to the value — which was a fudge. The real objection isn't the marking, it's that `r?.field` threads a `T or E` into a `Field or E`: a wrapped-value pipeline, the shape ruled out by the extract-early argument above, and the reason there's no `map`/`and_then`. It was also the last place Rask read `?` as Rust reads it, meaning propagation. Extract first — `try r.field`, with the placement rule doing the work (ER16a).

The cost measured out at nothing: across stdlib, examples, projects and tests there was not one `try … ?.` — every `?.` in the tree is an optional chain. Optional chaining stays because it isn't a pipeline; `user?.profile?.name` asks whether the path is there and terminates in `or`, `?`, or `!`.

**ER44 (a fold operator, not a fold method).** The error-model redesign removed `.unwrap_or_else` and pointed its migration at `try { … } else |e| f(e)`. That was wrong: the block form early-returns the transformed error, so it propagates and cannot produce a value. Nothing covered the *terminal* fold — collapsing `T or E` into `T` at a boundary with nothing above it. Every program has some: routers, `main`, task bodies.

<!-- test: skip -->
```rask
return dispatch(req) or |e| error_response(e)
```

The alternative was a method (`.recover(|e| …)`), and a method is how the zoo starts. The operator family already had the pieces, so the fold composes what's there instead of minting a form for the occasion.

**ER45/ER48 (`try` owns every exit).** Two designs were tried before this one and both failed on visibility.

`?? return` — the original — let one operator both supply a value and transfer control, so reading a line meant looking past the operator to find out which. #565 rejected it.

Routing exits through `or` instead — `r or return E` — fixed the double-reading but created a worse problem: `try r` and `r or return E` both let an error leave, so *whether a line can exit the function* was no longer answerable by looking at it. Two spellings for one job, and the more common one wasn't marked.

The rule that fixes both: **every line that can exit the function carries `try`, and nothing else does.**

<!-- test: skip -->
```rask
const port = config.port ?? 8080                     // no `try` — nothing leaves
const theme = load_theme() or Theme.default()        // no `try` — handled here
const data = try read_file(path)                     // `try` — leaves
const dto  = try json.decode(b) else return BadRequest("bad json")   // `try` — leaves
```

Scan the left margin and you have every exit point. That is the property the whole design is for, and it's why the marker goes at the front rather than in the middle.

**Why the `else` clause must diverge.** An earlier draft allowed `try r else v`, with `v` implicitly becoming the propagated error. It's shorter, and it was wrong in the same way: `else v` reads as "use `v` instead" and performs a return. Requiring the clause to diverge — the same rule as the pattern guard, `ctrl.flow/CF13` — means the `return` is written where it happens. `try` says control may leave; `else return v` says with what.

This is what #573 asked for, with the missing half added. Dropping the `|e|` binding when the error isn't used was the right request; dropping the `return` along with it hid the control flow.

**`break` and `continue` are exempt.** They stay legal on `or` and `??`, because a loop exit is local — it's visible where it is, and it doesn't change what the function returns. The thing a reader needs to find by scanning is the function exits.

**Rejected alternatives for the bail-out form.** Four were tried before landing here:

1. **`else` for all of it** — `const v = x else { return }`, extending the pattern guard. Grammatically free, but `else` says nothing about *what* went wrong: reading `x else { … }` doesn't tell you whether `x` was absent or failed.
2. **A presence guard construct** — same objection, plus it needed CF13's diverging-block rule to be restated for two more scrutinee shapes.
3. **A diverging right side on `or`** — `r or return E`. Reads well and composes, but it means two ways to leave the function and neither one marked. Replaced by `try r else return E`.
4. **`expr? else return X`** — postfix `?` is a bool, so `expr?` would be a bool alone and an unwrapped `T` with an `else`. One token, two result types.
5. **`try` on optionals *instead of* an explicit clause** — bare `try opt` propagates absence, which is right in a `T?`-returning function and covers 7 sites in the tree. The other 47 turn absence into a named error or a plain value, and those need the clause: `try opt else return E`.

**ER47 (`try` is failure-only).** `try` means "hand this error to my caller". An optional has no error to hand over, so `try opt` would have to invent one — the objection the old cross-shape diagnostic always made. Propagating absence as absence is real but rare (7 sites in the tree against 47 that turn absence into something else), and `opt ?? return none` covers it in one line while saying what is returned.

An intermediate draft made `try` shape-agnostic, with a 2×2 of which combinations were legal. Under one-operator-per-shape that table stops existing: `try` belongs to the `or` family, so using it on an optional isn't a rule violation, it's the wrong keyword — the same class of mistake as `??` on a result. Structure doing the work a rule was doing.

`.to_result(err)` stays retired: that was `try opt else return err` spelled as a method.

**ER16a (`try` finds its place in the chain).** `try` has to bind loosely, or `try store.get(id)` would read as `(try store).get(id)`. That used to mean projecting off a propagated value needed parentheses — `(try read_file(p)).len()`. It doesn't: a wrapped value has no payload methods, so in `try a.b().c` at most one placement of the `try` type-checks, and the compiler can find it.

The exception is real but narrow. `map`, `and_then` and `filter` exist on the wrapper *and* on sequences, so `try v.map(f)` with `v: Vec<i32> or E` has two placements that both type-check. That's a compile error asking for parentheses, not a silent pick. `try` also doesn't slide into call arguments — `f(try g())` is written where it means.

**ER44/ER46 (the binding is never mandatory).** In real code the replace-the-error case discards the binding constantly — the validation example wrote `else |e| ApiError.BadRequest("invalid JSON")` six times in one file, never touching `e`. A binding required by grammar and ignored by the author is ceremony with no reader benefit, which is what #573 asked to remove. Under this rule it falls out rather than needing a rule: `|e|` is how you *get* the error, so you write it when you want it. `try x else return E` and `try x else |e| return f(e)` are the same form with and without a payload you happened to need — and the `return` stays in both, which is the half #573 didn't ask for but needed.

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
    const text = try fs.read_text(path) else |e| return context("reading {path}", e)
    return try Config.parse(text) else |e| return context("parsing {path}", e)
}
```

**Typed domain errors.** For library-level errors, wrap in domain-specific types. A variant that adds context beyond the error needs the explicit map:

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or ConfigError {
    const text = try fs.read_text(path) else |e| return ConfigError.Io { path, source: e }
    return try Config.parse(text) else |e| return ConfigError.Parse { path, source: e }
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
- [Ensure](../control/ensure.md) — `ensure … or |e|` pattern (`ctrl.ensure`)
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — decision record for the no-wrappers surface
