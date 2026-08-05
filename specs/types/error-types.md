<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: T or E is a builtin sum type with type-based branch disambiguation. No Ok/Err wrappers. Disjointness rule (T ≠ E) via the nominal/alias split, checked at the call site once a generic's type argument is known. E must implement ErrorMessage. Auto-wrap fires only at return. `or` supplies a value and never transfers control; `try` is the only exit marker, and a bare one always means an error is going to the caller — on an optional its else clause is required. `?` is absence-only, so results narrow with `is`. Neither wrapper has methods. No ?? on results, no fold method, no presence guard. -->
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
| **ER16: Extract or leave** | `try x` | Extracts the success payload, or **control leaves here**. Bare on a result, it hands the error to the caller, widened into this function's error type (ER31/ER31a/ER32). On an optional it needs an `else` clause naming what leaves (ER47). Either way the `else` clause can redirect anywhere that diverges (ER45) |
| **ER16a: Chain placement** | `try a.b().c` | `try` attaches to the one step in the postfix chain that is fallible — `try read_file(p).len()` is `(try read_file(p)).len()`, `try store.get(id)` is `try (store.get(id))`. The wrappers have no methods at all, so exactly one placement type-checks and no parentheses are needed. `try` does not slide into call arguments |
| **ER17: Propagate block** | `try { … }` | Each `try` inside propagates; the first other-branch value short-circuits out. Shape follows ER16 |
| **ER18: Block with a clause** | `try { … } else \|e\| …` | The `else` clause covers the whole block: the first error from any inner `try` goes to it. Same rule as ER45 — the clause must diverge |

<!-- test: skip -->
```rask
// Single-call propagation
const data = try read_file(path)

// Extract, then use — `try` binds loosely, so parenthesise to keep it on one line
const size = try read_file(path).len()

// Force
const config = load_config()!

// A block with a clause — covers the whole block, and must diverge (ER18)
const content = try {
    try fs.read_text(path)
} else |e| return context("reading {path}", e)
```

ER13, ER19, ER20 and ER26 are retired — the `?.` chain on results, the `if r?` predicate and its `as v` bind, and the `!r?` parse error, all of which assumed `?` worked on results. Narrowing a result is `is` (ER23). ER21 and ER22 survive: the `else`-narrows and `else as e` rules were never about `?`, so they re-home onto the `is` test unchanged.

## The Two Halves

`or` is the keyword the type uses — `T or E` — and it means the same thing at the value level: **`or` supplies the failing branch.** Optionals get `??` for the absent branch (`type.optionals/OPT11`); the two are deliberately not the same token.

Neither transfers control. **`try` is the half that can**, and its `else` clause says where control goes.

Every optional form carries a `?`. No failure form does. That's the whole mnemonic — you never look up which operator applies, because the type's own spelling tells you.

**`or` produces a value; `try` transfers control.** No form does both, so "does control leave this line?" is answered by whether `try` is on it.

| Rule | Form | Leaves? | The other branch |
|------|------|---------|------------------|
| **ER14** | `r or v` | no | is this value |
| **ER44** | `r or \|e\| f(e)` | no | is this value, computed from the error |
| **ER16** | `try r` | **yes** | leaves to the caller, widened into this function's error type |
| **ER45** | `try r else return e_val` | **yes** | leaves with `e_val` |
| **ER45** | `try r else break` / `else continue` | **yes** | leaves the loop |
| **ER46** | `try r else \|e\| return f(e)` | **yes** | leaves with something built from the old error |

| Rule | Description |
|------|-------------|
| **ER14: Other branch** | `r or v` yields the success payload, or `v`. Results only — absence uses `??` (`type.optionals/OPT11`). `v` is a **value**: it may not `return`, `break`, `continue` or `panic(…)`. `or` exists to produce the thing being bound, and control flow doesn't produce anything (ER48) |
| **ER44: Using the error** | `r or \|e\| f(e)` binds `e: E` and yields `f(e): T`. Nothing leaves |
| **ER45: `try … else`** | The `else` clause says where control goes instead of to the caller. Its body **must diverge**, and any divergence will do — `return`, `break`, `continue`, `panic(…)` — exactly the pattern guard's rule (`ctrl.flow/CF13`). `try r else return E` needs no binding; `try r else \|e\| return f(e)` binds the old error first |
| **ER47: bare `try` means an error leaves** | Bare `try r` propagates the error to the caller, so the enclosing function needs an error branch that accepts it. **On an optional, `try` requires an `else` clause** — `try opt` alone is a compile error asking what should leave. Absence exits are always written: `try opt else return none`, `try opt else return MyError`, `try opt else break`. The clause also lifts the error-branch constraint, so `try r else return none` is fine too |
| **ER45a: the `else` clause needs parens in a comma list** | Inside an argument list, struct literal, or collection literal, `try x else <diverge>` must be parenthesised: `f((try g() else return E), other)`. Bare, it's a compile error asking for them. `or` and `??` need nothing — they don't diverge, so there's no exit to locate |
| **ER48: `or` and `??` never transfer control** | `return`, `break`, `continue` and `panic(…)` on the right of `or` or `??` are compile errors pointing at `try … else`, which is where control flow lives. The two operators supply the value being bound; `try` is the one that can leave. `Never` coercion is untouched everywhere else in the language |

<!-- test: skip -->
```rask
// A value on the other branch
const port = config.port ?? 8080                  // optional — `??`
const theme = load_theme() or Theme.default()     // result — `or`, error ignored

// A value computed from the error — the fold at a boundary
return dispatch(req) or |e| error_response(e)

// Leave, naming the error
const ms = try raw.parse() else return ApiError.BadRequest("ms must be non-negative")
const dto = try json.decode(req.body) else return ApiError.BadRequest("invalid JSON")

// Leave, transforming the error first
const text = try fs.read_text(path) else |e| return context("reading {path}", e)

// Propagate unchanged — the common case, and the reason `try` exists
const data = try read_file(path)

// Braces when the clause needs more than one expression — still has to diverge
const text = try fs.read_text(path) else |e| {
    log("failed to read {path}: {e.message()}")
    return context("reading {path}", e)
}
```

Both the `or` right side and the `else` clause are expressions, and a block is an expression, so braces are available whenever they help and never required for a single one.

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

**Reading a line.** Look for `try`. If it's there, control can leave this line, and the `else` clause — if any — says with what. If it isn't, nothing leaves at all: `or` and `??` produce values and only values. That's the whole of it, and it's a glance rather than an inference.

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

Bare `try` always means the same thing: an error is on its way to the caller. Absence never leaves silently — on an optional the `else` clause is required, and it names what goes out (ER47).

### Absence and Error Are Spelled Differently

`?` marks absence. `or` and `try` mark errors. Nothing crosses over, so a line tells you which kind of failure it's handling without your having to remember what the scrutinee was.

| | Optionals `T?` | Errors `T or E` |
|---|---|---|
| test / bind | `x?`, `x? as v`, `x is none` | `r is T as v`, `r is E as e` |
| project | `x?.field` | `try r.field` (ER16a places the `try`) |
| other branch | `x or v` | `r or v`, `r or \|e\| f(e)` |
| leave, propagating | `try x else return none` | `try r` |
| leave, transforming | `try x else return MyError` | `try r else \|e\| return f(e)` |
| assert / dispatch | `x!`, `match` | `r!`, `match` |
| convert to the other | `try x else return MyError` | `r or none` |

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

### Where Control Goes [ER45]

Bare `try` leaves to the caller. The `else` clause redirects it, and takes any divergence — the same latitude the pattern guard has (`ctrl.flow/CF13`, where `const task = event is Ready else { break }` is already legal):

<!-- test: skip -->
```rask
const data = try read_file(path)                                  // to the caller
const dto  = try json.decode(body) else return bad_request()      // out of the function
const item = try queue.pop() else break                           // out of the loop
const name = try entry.as_string() else continue                  // next iteration
const home = try env("HOME") else panic("HOME must be set")       // or just env("HOME")!
```

`or` and `??` are the other half and never appear in that list: they supply the value being bound, so control flow has no place on their right (ER48). One line answers both questions at a glance — is there a `try`, and if so where does the `else` send it.

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

None. Neither wrapper has any — the operator surface is the whole API for both shapes.

Dropping the error to get a `T?` is `r or none` — the right side is a two-branch value with the same success type, so ER14a keeps it wrapped:

<!-- test: skip -->
```rask
const maybe_v = compute() or none      // "I don't care why it failed"
```

ER43 applies as it does to any discarded error: if `E` carries a must-consume payload, `or none` is rejected and you handle it with `or |e| …` or `match`.

Everything is gone: `ok`, `map`, `map_err`, `and_then`, and previously `unwrap_or`, `unwrap_or_else`, `is_ok`, `is_err`, `to_option`, `to_error`, `on_err`. The operator family covers the work — `.unwrap_or` is `r or v`, `.unwrap_or_else` is `r or |e| f(e)`, `.map_err` is `try r else |e| return f(e)`, `.ok()` is `r or none`.

There is no fold *method* either. A fold ends the error's journey, and journey-endings belong to the operator family — a `.recover()` would be the first step back toward the zoo the redesign removed (`std.api/SD4`).

`.ok()` was the last survivor, kept on the argument that it's a shape conversion rather than a combinator. Measurement broke that: of 45 uses, 41 are in statement position and throw the optional away — `tx.send(x).ok()` means "I don't care if this failed", not "give me an optional". Rask already spells a deliberate discard `const _ = f()` (`tool.warnings/W3`), so `.ok()` was a second, error-only way to do it that named the success branch while the author meant *drop the error*. The 4 real conversions are `r or none` and `try r else return none`.

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
| `try o` bare, any function | ER47 | Compile error — absence has no destination of its own. Add `else return none`, or whatever should leave |
| `try o else return none` in a `U?`-returning function | ER45/ER47 | Legal — the ordinary way to propagate absence |
| `try o else return MyError` | ER45 | Legal anywhere — the clause says what leaves |
| `try r else return none` in a `T?`-returning function | ER45 | Legal — drops the error detail, and says so |
| `try r else v` where `v` doesn't diverge | ER45 | Compile error — the clause must diverge, so the exit is written out |
| `Config { host: try g() else return E, port: 8080 }` | ER45a | Compile error asking for parens around the `try … else` |
| `Config { host: (try g() else return E), port: 8080 }` | ER45a | Legal — the parens show where the exit ends |
| `Config { host: g() ?? "localhost", port: 8080 }` | ER45a | Legal, no parens — `??` can't leave the function |
| `try r else break` / `else continue` / `else panic(…)` | ER45 | Legal — any divergence, same as the pattern guard (CF13) |
| `r or return X` | ER48 | Compile error pointing at `try r else return X` |
| `x ?? return X` | ER48 | Compile error pointing at `try x else return X` |
| `r or break` / `or continue` | ER48 | Compile error — use `try r else break` |
| `r or panic(…)` | ER48 | Compile error pointing at `r! "…"` or `try r else panic(…)` |
| `r or v` where `v: E` | ER14 | Type error — `or` produces a `T`. To leave with it, `try r else return v` |
| `try a.b().c` with one fallible step | ER16a | Legal — `try` attaches to that step; no parens |
| `r or none` | ER14a | Legal — yields `T?`, dropping the error. This is the old `.ok()` |
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

Propagating absence out of a function is `try opt else return none` — the clause is required, so the line says what leaves (ER47). The mirror case is a result in a `T?`-returning function: `try r else return none`, which reads the same and says plainly that the error detail is being dropped. To drop it without leaving, `r or none`.

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
2. **One shape, not two.** Half of Rust's vocabulary is `Option`↔`Result` plumbing (`ok`, `ok_or`, `err`, …). `T?` being `T or none` on the same machinery deletes the category outright: converting between the shapes is `r or none` one way and `try x else return MyError` the other, both of them ordinary uses of operators that already exist for other reasons.
3. **Operators can't breed.** Methods grow by one-line PR — that's how the zoo happened. An operator is a language change gated by the Ceremony Test (`CORE_DESIGN`). The family is *frozen by construction*: a handful of forms around one mental template (test, extract, other-branch, force, propagate, chain), learned once as a unit. Swift's `?`/`!`/`??` and Zig's `orelse`/`catch` are the precedent that a small closed operator set for this territory is learnable in a day and then disappears into fluency.

**"But the methods were for chaining."** The standard defence of combinators, and it's the only real one — so it's worth separating the two things it can mean. *Fallback* chaining is "try this, else that, else that", and Rask has it as operators: `user?.display_name ?? user?.email ?? "anon"`, `load_from_disk() or load_from_env() or load_from_net()`. That's 258 `??` and 17 `?.` uses in the tree, no method involved. *Transform* chaining is `r.map(f).and_then(g)` — stay wrapped, hop, unwrap at the end — and that's what the methods offered. `.and_then(` has never appeared in a `.rk` file in this repository's history, and every method chain that does appear is on a sequence (`v.filter(…).map(…)`), not on a wrapper.

The reason is one number: the tree holds 374 `try` tokens and **zero lines containing two of them**. Every fallible step is extracted on its own line, so a wrapped value never sits in the middle of an expression for a method to hang off. The shape combinators exist to serve doesn't occur — which makes cutting them a description of how the code is already written, not a restriction imposed on it. The cost is real and small: `read_file(path).map_err(Io)?.parse::<Config>().map_err(Parse)?` becomes two statements, and the intermediate gets a name.

The corollary is a discipline: the family beats the method zoo exactly as long as it stays frozen. An ergonomic itch is answered by composing existing operators or writing a `match` — never by minting form nine. And no combinator methods through the back door: a `map_err` in the stdlib would be the zoo regrowing (`std.api/SD4`); the error-transform need is served by `try … else |e| return …` (ER46).

That discipline was stated here for two revisions while the Methods section below still listed `map`, `map_err`, `and_then` and `filter` — the argument was made and the surface kept anyway, so the operators were added *on top of* the zoo instead of in place of it. Measurement settled it. Across stdlib, examples, projects and tests: `and_then` zero uses, `map` and `filter` on either wrapper zero (every hit is a sequence), `map_err` two. `.ok()` looked like the exception at 45, until the sites were read — 41 are in statement position, discarding the optional they build, which is `const _ = f()` and not a conversion at all.

So both wrappers have **no methods**. That's the load-bearing version of this argument: not "the methods are few and well-chosen" but "there is nothing to look up." A `T or E` value supports operators and `match`, full stop, and the same for `T?`. It also removes a bug class — `tests/suite/t31_channels.rk` exists because native codegen lowered `.ok()`'s receiver twice and `tx.send(x).ok()` sent every value twice (#349). Methods on a builtin wrapper need lowering; operators the compiler already understands don't.

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

`none` is different on both counts. It carries no payload, and its layers are reached in order: the outer operators (`?`, `or`, `!`, `is none`) act on the outer layer, and the inner layer is only visible after narrowing through it. One rule — a bare `none` literal means the outer absent — closes the only remaining ambiguity. So `none` layers and payload variants don't. See [optionals.md](optionals.md).

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

**Why nothing diverges on the right of `or`.** The visibility argument above is real, but there's a simpler one underneath it: in `const x = expr or v`, the `or` clause exists to supply **the value that gets bound to `x`**. A `return` doesn't produce a value — it abandons the binding. It typechecks only because `Never` coerces, which is the type system permitting something the operator's own purpose contradicts.

That argument doesn't stop at `return`. `break` and `continue` don't produce a value either, so they're rejected on `or`/`??` for the same reason. They aren't lost, though — they move to where control flow already lives:

<!-- test: skip -->
```rask
const item = try queue.pop() else break
const name = try entry.as_string() else continue
```

**Why `try` may transfer control and `or` may not.** `try` *is* the control-flow operator — that is its whole job, announced by a keyword at the head of the line. `or`'s right operand is declared to be the alternative *value*. Neither does the other's job, so the split isn't a carve-out; it's the design.

This puts Rask in the Swift/Rust camp rather than the Kotlin/Zig one. Kotlin (`?: return`) and Zig (`orelse return`) allow control flow in the fallback and skip the extra construct. Swift and Rust forbid it and add a binding form whose `else` block may diverge — `guard let x = y else { return }`, `let Some(x) = y else { continue };`. Both camps work; the second keeps the fallback operator meaning one thing. Rask gets it without adding a construct, because `try … else` already had to exist for error transformation, and because the pattern guard (CF13) had already established "a binding whose `else` must diverge" as a shape in the language.

Note what Swift and Rust both allow in that block: *any* divergence, not just `return`. `try`'s clause matches — `else break`, `else continue`, `else panic(…)` are all fine. An earlier draft of this rule only allowed `return`, which cost 14 sites in the tree a second line for no reason.

`panic(…)` is also just `x! "message"`, which is shorter; both spellings are fine.

**Rejected alternatives for the bail-out form.** Four were tried before landing here:

1. **`else` for all of it** — `const v = x else { return }`, extending the pattern guard. Grammatically free, but `else` says nothing about *what* went wrong: reading `x else { … }` doesn't tell you whether `x` was absent or failed.
2. **A presence guard construct** — same objection, plus it needed CF13's diverging-block rule to be restated for two more scrutinee shapes.
3. **A diverging right side on `or`** — `r or return E`. Reads well and composes, but it means two ways to leave the function and neither one marked. Replaced by `try r else return E`.
4. **`expr? else return X`** — postfix `?` is a bool, so `expr?` would be a bool alone and an unwrapped `T` with an `else`. One token, two result types.
5. **Bare `try` on an optional** — see ER47 below. It was in a draft, on a miscount, and stress-testing killed it.

**ER47 (bare `try` means an error leaves).** `try` works on both shapes — that part is settled, because `or` and `??` never transfer control (ER48), so without it absence would have no way to leave a function at all. What took two tries is whether the *bare* form should work on an optional.

A draft allowed it, so `try opt` propagated `none` and `try r` propagated the error. Stress-testing a real function killed it. `sst_point_lookup` in the LSM example returns `KeyValue? or SstError` and calls both shapes two lines apart:

<!-- test: skip -->
```rask
const index = try read_sstable_index(meta.path)            // -> Vec<BlockIndex> or SstError
const target_block = try find_block_for_key(index, key)    // -> i32?
```

Identical syntax, opposite meanings. The first leaves through the **error** branch — the disk read failed. The second leaves through the **success** branch — the key isn't in this table, which is a normal answer. Nothing in either line says which, and a reader has to open another file and check two signatures to find out. That's the objection that retired standalone `else` ("is the `x` none, or does the `x` error?") reappearing on `try`, and the version this replaced was *clearer*: `find_block_for_key(index, key) ?? return none` says what leaves.

So bare `try` is results-only, and it costs nothing. Of the 72 absence-exits in the tree, 65 already name a specific target (`?? return Token.Eof`, `?? break`, `?? panic("missing")`), and the remaining 7 are written `?? return none` — by hand, explicitly. Zero sites want the implicit form. The earlier draft justified it as "covers 7 sites", which counted sites where it *could* apply rather than sites that wanted it; those same 7 keep writing the `return none` they already write.

The payoff is that `try` has exactly one reading everywhere. See a bare `try` and an error is on its way to the caller — no signature lookup, no dependence on the callee's shape. Absence leaving a function is always spelled out, which is the same principle that made the `else` clause carry its own `return` (ER45).

What *is* shape-specific is what the bare form propagates, and that's ER47's whole content: `try r` hands out an error so the function needs an error branch, `try opt` hands out `none` so it needs a `T?` return. The `else` clause overrides the propagated value, and with it the constraint — which is why `try opt else return MyError` and `try r else return none` both work. Reading a `try` still tells you where control goes; which shape it came from is in the callee's signature, where it belongs.

`.to_result(err)` stays retired: that was `try opt else return err` spelled as a method.

**ER16a (`try` finds its place in the chain).** `try` has to bind loosely, or `try store.get(id)` would read as `(try store).get(id)`. That used to mean projecting off a propagated value needed parentheses — `(try read_file(p)).len()`. It doesn't: a wrapped value has no payload methods, so in `try a.b().c` at most one placement of the `try` type-checks, and the compiler can find it.

The rule has no exception, and that fell out of cutting the methods rather than being designed in. Ambiguity needs a name that resolves on the wrapper *and* on the payload; with zero methods on the wrapper there's nothing to collide, so the compiler always finds the one placement. An earlier draft had the exception firing constantly — the wrapper carried `map`/`filter`/`and_then` and so did every sequence, making `try v.map(f)` need parentheses — and a later one had it surviving for `ok` alone. Both are gone. `try` also doesn't slide into call arguments: `f(try g())` is written where it means.

**ER45a (parens in a comma list).** Found by writing the config-loading shape, which every program has:

<!-- test: skip -->
```rask
return Config {
    host: try get(raw, "host") else return ConfigError.Missing("host"),    // needs parens
    port: 8080,
}
```

The comma does end the `return`'s expression, so this could just be allowed. The reason not to: ER45 makes the `return` mandatory so the exit is *visible*, and an exit sitting in the middle of a field list is visible in the letter and hidden in practice — you scan a struct literal for fields, not for places the function might leave from. Parens put a boundary around it, which is the same move ER16a makes when a `try`'s placement is genuinely unclear.

`or` and `??` are exempt because they can't leave the function (ER48), so `host: get(raw, "host") ?? "localhost"` needs nothing. The rule keys on divergence, which is exactly the thing that has to be findable.

Zero sites in the tree pay for this — the house style extracts first anyway, and the extracted version reads better:

<!-- test: skip -->
```rask
const host = try get(raw, "host") else return ConfigError.Missing("host")
return Config { host: host, port: 8080 }
```

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
