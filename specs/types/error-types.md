<!-- id: type.errors -->
<!-- status: decided -->
<!-- summary: T or E is a builtin sum type with type-based branch disambiguation. No Ok/Err wrappers. Disjointness rule (T ≠ E) via the nominal/alias split, checked at the call site once a generic's type argument is known. E must implement Error. Auto-wrap fires only at return. Three words: `try` propagates the bad branch of either shape (shape must fit the enclosing return; a flat `T? or E` operand needs `try … ?? …`); `??` is the absence fallback (value or written-out exit); `catch e =>` / `catch _ =>` is the failure fallback — binder mandatory, no bare-value form, so a discarded error is always visible. `?` is absence-only, so results narrow with `is`. Neither wrapper has methods. No fold method, no presence guard. -->
<!-- depends: types/types.md, types/optionals.md, types/union-types.md, types/type-aliases.md -->

# Error Types

Errors are values. `T or E` is a builtin sum type — compiler-generated tagged union — with type-based branch disambiguation. No `Ok` or `Err` constructors; the compiler picks the branch from the value's type at the return site. Every `E` implements the structural `Error` trait.

Libraries use union errors (`T or (A | B | C)`), applications use `any Error` (type-erased boxing). Match dispatches on type; operators cover the two-branch case.

## The Type

| Rule | Description |
|------|-------------|
| **ER1: Builtin sum** | `T or E` is a compiler-generated tagged union, not a user-definable enum. Optionals (`T?`) are sugar for `T or none` and share the same machinery — see [optionals.md](optionals.md) |
| **ER2: No user wrapper** | There is no `Ok` or `Err` constructor, keyword, or pattern. Success values are bare; error values are the error type's own constructor (e.g. `DivError.ByZero`) |
| **ER3: Disjointness** | `T or E` requires T ≠ E using Rask's nominal-vs-alias distinction (see [type-aliases.md](type-aliases.md)). Checked where the type is written, and again after generic substitution (ER3a). Same rule as [union-types.md](union-types.md) U6. **Exception:** `none` — see ER3b |
| **ER3a: Disjointness is a use-site obligation** | A signature that writes `T or E` with a type parameter on either side *is* the disjointness bound — there's no separate syntax to declare it. The compiler reads the obligation off the signature and checks it at the call site, where the type argument is known. A generic caller that forwards its own parameter passes the obligation on to *its* call sites, same as a trait bound (GF3) |
| **ER3b: `none` layers instead of colliding** | `none` is exempt from disjointness and from the duplicate-variant rule. `T?` where `T` is itself optional is a legal two-layer optional, not a collision — see [optionals.md](optionals.md) |
| **ER4: Error bound** | Every `E` must implement `Error` — `func message(self) -> string`, auto-derived for enums (ER6). Enforced at type formation. Primitives (`i32`, `f64`, `string`) don't qualify; newtype them. **Exception:** `none` is exempt — it's the absent sentinel for optionals (`T or none`), not an error type |
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

### The `Error` Trait

| Rule | Description |
|------|-------------|
| **ER6: Auto-derived for enums** | `Error` is nominal, auto-derived for enums: `message()` is the humanized variant name plus payload interpolation (`UnexpectedEnd(ctx)` → `"unexpected end: {ctx}"`); a single-payload variant whose payload implements `Error` delegates to it. Override with `extend E with Error { ... }` for hand-written prose — `rask lint` nudges public error types toward it. Structs declare conformance (usually the header of the block defining `message()`) |
| **ER7: Auto-Displayable** | Error types auto-satisfy `Displayable`; `to_string()` delegates to `message()` |
| **ER8: Layered traits** | Richer capabilities (`LinedError`, `ContextualError`, `CodedError`) are opt-in traits on top of `Error`. The minimum bound is just `message() -> string` |

<!-- test: skip -->
```rask
enum DivError { ByZero, Overflow }
// Nothing else needed — auto-derived (ER6):
//   ByZero → "by zero", Overflow → "overflow"

// Override for hand-written prose:
extend DivError with Error {
    func message(self) -> string {
        match self {
            DivError.ByZero   => "division by zero",
            DivError.Overflow => "arithmetic overflow",
        }
    }
}

// Structs declare conformance in the block defining message():
struct NotFound { key: string }
extend NotFound with Error {
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
| **ER12: No `?` on a result** | `r?` / `r?.field` / `r ?? v` | Compile error, all forms. `?` marks absence; a result's other branch is an error. Test with `is` (ER23) or `match`; fall back with `catch`; to project, extract first |
| **ER14: Handle the error** | `r catch e => <expr>` / `r catch _ => <expr>` | Yields T when present, else binds the error and evaluates the body — lazily, only on failure. The binder is **mandatory**: `e =>` to use the error, `_ =>` to discard it *visibly*. The body is a value or any divergence (`catch e => return wrap(e)` is transform-and-leave). There is **no bare-value form** — `r catch v` is a compile error — so an error can never be swallowed silently. Results only; absence uses `??` (`type.optionals/OPT11`) |
| **ER15: Force** | `r!` | Extracts T, or panics using `E.message()`; `r! "msg"` overrides with a custom message |
| **ER16: Extract or propagate** | `try x` | Extracts the success payload, or the bad branch **leaves to the caller** — the error from a `T or E` (widened into this function's error type, ER31/ER31a/ER32), the `none` from a `T?`. No clause exists; what leaves must fit the enclosing return (ER47) |
| **ER16a: Chain placement** | `try a.b().c` | `try` attaches to the one step in the postfix chain that is fallible — `try read_file(p).len()` is `(try read_file(p)).len()`, `try store.get(id)` is `try (store.get(id))`. The wrappers have no methods at all, so exactly one placement type-checks and no parentheses are needed. `try` does not slide into call arguments |
| **ER16b: `try` binds tighter than `??`** | `try f() ?? v` | Parses as `(try f()) ?? v` — no parens. This is the composite for a flat `T? or E` operand: `try` peels the error, `??` handles the absence — and now the line *says* so. (Zig ruled this the other way and its stdlib pays parens on the common order — issue #5436 there; ~160 error-then-absence composites against ~4 reversed) |
| **ER17: Propagate block** | `try { … }` | Each `try` inside propagates; the first other-branch value short-circuits out. Shape follows ER16 |
| **ER18: Block with a handler** | `try { … } catch e => …` | The handler covers the whole block: the first error from any inner `try` goes to it. A block-scoped handler, distinct from the per-expression operator; the handler body follows ER14 (a value replaces the block's result; a divergence leaves) |

<!-- test: skip -->
```rask
// Single-call propagation
let data = try read_file(path)

// Extract, then use — ER16a places the `try`; no parens needed
let size = try read_file(path).len()

// Force
let config = load_config()!

// A block with a handler — covers the whole block (ER18)
let content = try {
    try fs.read_text(path)
} catch e => return context("reading {path}", e)
```

ER13, ER19, ER20 and ER26 are retired — the `?.` chain on results, the `if r?` predicate and its `as v` bind, and the `!r?` parse error, all of which assumed `?` worked on results. Narrowing a result is `is` (ER23). ER22 survives: `else as e` is a binding, not a narrow, so it re-homes onto the `is` test unchanged. (ER21, ER24 and ER25 — the scrutinee-narrowing rules — were deleted in a follow-up pass; see Conditions and Binding.) ER44 folds into ER14 (the binder *is* the form now, so there's nothing separate to say). ER45 and ER46 (the `try … else` clause and its transform form) are deleted — the fallbacks' diverging right sides do both jobs; ER48 (fallbacks never transfer control) is deleted with them.

**ID history.** Two IDs in this file were repurposed with *reversed* meanings during the redesign, before the never-repurpose convention existed (`CONVENTIONS.md`): **ER12** once meant "`r?` is a boolean ok-test" and now means "no `?` on a result"; **ER14** once meant the bare-value fallback and now means `catch`. Citations of these IDs in issues, commits, or diagnostics from before #596 refer to the old meanings. No other ID in this file is reused, and none will be again.

## Three Words, Three Jobs

**`try` means something leaves. A `?` means something is missing. `catch` means something failed.** That's the whole glance-test, and each word carries one more fact: `try` preserves the bad branch (it arrives at the caller intact), `??` substitutes for a miss that carried no information, and `catch` stands wherever an *error* — a payload, a cost — is being handled or deliberately dropped.

The split between the two fallbacks is the point, not an accident. A fallback is the only form in the family that can **destroy information**: `??` discards a `none`, which is nothing; `catch` discards or transforms an `E`, which is something. Transparency of Cost says the second must be visible — so `catch` has **no bare-value form**. Its binder is mandatory: `e =>` when the error is used, `_ =>` when it's dropped, and `catch _ =>` is the loud, greppable spelling of "an error dies here."

`try` composes with them by definition rather than by rule: **`try r` is `r catch e => return e`**, and **`try x` is `x ?? return none`**.

| Rule | Form | Leaves? | On failure/absence |
|------|------|---------|------------------|
| **OPT11** | `x ?? v` | no | the value (nothing was lost) |
| **OPT11** | `x ?? return v` / `?? break` / `?? continue` | **yes — and the line says so** | leaves; the exit is written out |
| **ER14** | `r catch e => f(e)` | no | this value, computed from the error |
| **ER14** | `r catch _ => v` | no | this value; the error is dropped, **acknowledged** |
| **ER14** | `r catch e => return f(e)` / `catch _ => return v` | **yes — and the line says so** | leaves with something else |
| **ER16** | `try r` / `try x` | **yes** | the bad branch leaves unchanged |

| Rule | Description |
|------|-------------|
| **ER14: Handle the error** | `r catch <binder> => <expr>` yields the success payload, or evaluates the body with the error bound (lazily — only on failure). The binder is `e` (any name) or `_`; it is **never optional**, and there is no `r catch v`. The body is a **value or any divergence** — `return`, `break`, `continue`, `panic(…)` — legal because visible: the exit is written where it happens. Results only |
| **ER47: The shape rule** | What bare `try` propagates must fit the enclosing return: `try r` needs an error branch that accepts `E`, `try x` needs a `T?` return. On a flat `T? or E` **operand** — where both branches could leave — bare `try` is a compile error naming both escapes; write the composite `try f() ?? …` (ER16b) or handle the shapes explicitly |
| **ER45a: A diverging right side needs parens in a comma list** | Inside an argument list, struct literal, or collection literal, a diverging `??` or `catch` must be parenthesised: `f((g() catch _ => return E), other)`. Bare, it's a compile error asking for them. A value right side needs nothing — no exit to locate |

<!-- test: skip -->
```rask
// Absence: terse — the miss carried nothing
let port = config.port ?? 8080
let item = queue.pop() ?? break

// Failure: the error is always named or visibly dropped
let theme = load_theme() catch _ => Theme.default()    // dropped, and it says so
return dispatch(req) catch e => error_response(e)        // the fold at a boundary

// Leave, transforming the error first
let text = fs.read_text(path) catch e => return context("reading {path}", e)

// Propagate unchanged — the most common line in the language
let data = try read_file(path)

// Braces when the body needs more than one expression
let text = fs.read_text(path) catch e => {
    log("failed to read {path}: {e.message()}")
    return context("reading {path}", e)
}
```

The `??` right side and the `catch` body are expressions, and a block is an expression, so braces are available whenever they help and never required for a single one.

**Reading a line.** Three questions, answered by the tokens alone: *does control leave?* — a `try` at the head, or a visible `return`/`break`/`continue`/`panic`. *Was it absence or failure?* — a `?` for absence, `catch` for failure; the shapes are never spelled alike. *Did an error die?* — only ever at a `catch _ =>`, which is one grep. Nothing is inferred from a signature; nothing leaves through an unmarked value.

### Chaining [ER14a]

Because the right side sets the result type, a chain of fallbacks reads flat and needs no parentheses. `??` is left-associative, and that's the correct grouping:

<!-- test: skip -->
```rask
let name = user?.display_name ?? user?.email ?? "anon"
//           \_____ T? ______/     \___ T? ___/    \ T /
//           \________ T? ______________/
//           \_______________ string _______________/
```

Each step whose right side is still a `T?` stays wrapped and keeps going; the first bare `T` collapses it. A further `??` after the collapse is a type error, because the left side is no longer two-branch.

"First source that works" on results goes through `catch`, and each discarded error is acknowledged in the text — two errors genuinely die here, and the noise is honest:

<!-- test: skip -->
```rask
let cfg = load_from_disk() catch _ => load_from_env() catch _ => load_from_net()
//          T or DiskError            T or EnvError             T or NetError
//          result: T or NetError — the earlier errors are dropped, visibly
```

The `catch` body extends rightward (greedy, like a match-arm body), so the chain nests the natural way without parens; left of `catch`, the operand chain groups left at the same precedence level as `??` — the full ruling, including mixed `??`/`catch` lines, is in [operators.md](operators.md). The same linearity rule applies (ER43): an error carrying a must-consume payload can't be dropped with `_ =>` — bind it and consume it, or `match`.

**In statement position** — a `void or E` call whose result you aren't binding — the same forms apply, and the fold is the one that reads best: the success type is `void`, so a void-returning handler needs no ceremony.

<!-- test: skip -->
```rask
try save(d)                                     // propagate — the usual one
save(d) catch _ => return IoError.Full          // replace and leave
save(d) catch e => log(e.message())             // handle here: `log` returns void ✓
save(d)                                         // W2: unused result of type `void or IoError`
```

Deliberately ignoring the error entirely is `let _ = save(d)`, which silences W2 the same way any other unused binding does (`tool.warnings/W3`) — or `save(d) catch _ => {}`, which says the same thing at the error's own granularity.

**`e =>` is the match-arm binder.** Same marker, same rules: it names a value and its body belongs to the enclosing function, so `return` there leaves the *function* (`ctrl.flow/CF26`). Braces are optional exactly as in a match arm.

The body is not restricted to a return. It's an ordinary expression of the enclosing function — a value, a call, a brace block with statements, even a `try` (which propagates from the enclosing function, as it would anywhere else). The only requirement is the usual typing one: the body's value is the success type (carry on), or the body diverges (leave). So all of these are one rule, not four:

<!-- test: skip -->
```rask
save(d) catch e => log(e.message())                     // run code, carry on (void)
let t = load() catch _ => Theme.default()             // value
let c = fetch() catch _ => try load_cached()          // fall back to another fallible source
let text = fs.read_text(path) catch e => {            // block: arbitrary statements
    metrics.incr("read_failures")
    log("falling back to defaults: {e.message()}")
    default_text()                                      // block's value carries on
}
```

The one place a `catch` body is restricted is `ensure expr catch e => …` — cleanup runs at scope exit, so there is nowhere to propagate and `try` is rejected inside that handler (`ctrl.ensure/ER3`).

<!-- test: skip -->
```rask
let text = fs.read_text(path) catch e => return context("reading {path}", e)
//                                         ^^ leaves `load_config`
```

Nothing has to be said about what `e =>` *isn't*, which is the point of using it. Closure syntax is `|x| expr`, and `return` inside a closure exits the closure — so spelling the error binding `|e|` would have made `catch |e| return f(e)` fold instead of leave, and the transform-and-leave form unwritable.

**One family, shapes spelled apart.** `try`, `!` and `match` are shared — those operations genuinely don't care which branch went bad. The fallbacks are split on purpose:

| | Optionals `T?` | Errors `T or E` |
|---|---|---|
| test / bind | `x?`, `x? as v`, `x is none` | `r is T as v`, `r is E as e` |
| project | `x?.field` | `try r.field` (ER16a places the `try`) |
| other branch | `x ?? v` | `r catch e => f(e)`, `r catch _ => v` |
| leave, propagating | `try x` | `try r` |
| leave, transforming | `x ?? return MyError` | `r catch e => return f(e)` |
| assert / dispatch | `x!`, `match` | `r!`, `match` |
| convert to the other | `x ?? return MyError` | `r catch _ => none` |

What the split buys: a fallback site answers "was that an error, and did it survive?" from its own tokens — the question every reader asks there, and the one a merged word made them pay a signature lookup for. What it costs: one more word in the family, on the side the flagship uses six times against the absence side's twenty-eight.

Two more operators are shared for the same reason — the operation doesn't care which branch went bad:

- **`!`** asserts the good branch and panics otherwise. It never claims *which* branch failed; only the panic message differs (`"none"` versus `e.message()`).
- **`match`** is multi-arm dispatch, shape-neutral by construction.

`?.` is **not** among them. Chaining a projection through a result — `r?.field` yielding `Field or E` — threads a wrapped value through a pipeline, which is the shape this design deliberately doesn't have (see the extract-early argument in the appendix; it's why there's no `map` or `and_then` either). `?.` on a result would be `map` with punctuation, and it would borrow Rust's reading of `?` as propagation on top of that. Extract first, then project — `try` finds its own place in the chain (ER16a):

<!-- test: skip -->
```rask
let size = try read_file(path).len()        // ER16a places the `try`; no parens needed

let text = try read_file(path)              // or bind it, when the line gets long
let size = text.len()
```

Optional chaining is a different animal and stays: `user?.profile?.name` asks one question — is the whole path there? — and lands in `??`, `?`, or `!` immediately, rather than carrying a wrapper onward.

### The Flat Shape [ER47, ER16b]

`T? or E` — a function that can fail *and* whose answer can be absent — is the one place the shapes co-occur flat, and it's rare: 2 of 316 return types in the tree. On such an operand, bare `try` would have two possible meanings, so it's rejected with both spelled out. The composite is the idiomatic form, and precedence makes it paren-free:

<!-- test: skip -->
```rask
// sst_point_lookup returns KeyValue? or SstError
let index = try read_sstable_index(meta.path)              // -> Vec<BlockIndex> or SstError
let target = find_block_for_key(index, key) ?? return none        // -> i32? ; absence named

// consuming a flat shape: try peels the error, ?? handles the absence
let kv = try sst_point_lookup(sst, key) ?? continue        // (try …) ?? … — ER16b
```

The composite reads as one idiom — **error up, absence here** — and with the fallbacks split, the line says it outright: `try` is the error leaving, the `?` is the absence being handled. Its meaning isn't a new rule either. `T? or E` is `(T?) or E`, and operators act on the outer layer (`type.optionals/OPT30`, the same principle that makes `T??` work): `try` binds first and peels the outer bad branch — the error — leaving a `T?` for `??`. The phrase is also self-identifying: on a plain result or plain optional it doesn't type-check (`try` already produced the payload), so `try … ??` on one operand always means the callee is flat.

In an **infallible** function the flat shape has nowhere to propagate and three live branches, so no single fallback consumes it — `match` with three arms is the right tool when the outcomes genuinely differ. (Asserting is still one word: `x!` acts on the outer layer per OPT30, so `t.lookup(key)! "corrupt"` panics on the error and yields the `T?` — tiered_store's trusted-table read does exactly that.) The three-way match:

<!-- test: skip -->
```rask
match store.get(key) {
    string as v      => println(v),
    none             => println("absent"),
    StoreError as e  => println("failed: {e.message()}"),
}
```

## Conditions and Binding

There is **no flow typing anywhere in the language**. An `is` test is a plain boolean, exactly like `x?` on an optional — it never changes the scrutinee's type, in the block, in the else, or after a diverging arm. Getting at a branch's payload is always a **binding**: `as name`, on the test or on the else.

| Rule | Description |
|------|-------------|
| **ER23: Type pattern test and bind** | `if r is ErrType as e { … }` tests and binds `e` when `r`'s error side is (or contains) `ErrType`. Works for widened unions: `if r is IoError as io { … }`. `if r is T as v` tests the success side the same way. Without `as`, it's a bare bool. `r` itself is unchanged everywhere |
| **ER22: Bind in else** | `if r is Config as c { … } else as e { … }` binds the complement in the `else` branch |
| **ER21, ER24, ER25 deleted** | Scrutinee narrowing is gone: the else-narrow (ER21) and the early-exit fall-through narrow (ER24) let non-canonical error handling type-check — machinery maintained solely for forms the canon says not to write ([canonical-patterns.md](../canonical-patterns.md)). With them cut, `if r is E as e { return e }; use(r)` simply fails to type-check (`r` is still `T or E`), and the fix the compiler suggests is the guard: `let v = r catch e => return e`. No lint needed — the shape routes itself. ER25 (compounds don't narrow) is vacuously true now and retired |

<!-- test: skip -->
```rask
let r = divide(a, b)

if r is f64 as v {
    use(v)                        // v: f64 — the binding is the access
}

if r is f64 as v { use(v) }
else as e { log(e.message()) }    // e: DivError            [ER22]
```

What this buys: the checker has **zero** flow typing — tests are bools, payloads come from bindings, and the rule set is the same for `?` and `is`. What it costs: the two-line statement guard is gone; its job belongs to `catch` (which binds the success by construction) and was already the canon.

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

Match earns its keep on multi-error unions. Two-branch cases read better as the fold (`r catch e => f(e)`).

## Methods

None. Neither wrapper has any — the operator surface is the whole API for both shapes.

Dropping the error to get a `T?` is `r catch _ => none` — the discard is acknowledged, and the body is a two-branch value with the same success type, so ER14a keeps it wrapped:

<!-- test: skip -->
```rask
let maybe_v = compute() catch _ => none      // "I don't care why it failed" — said out loud
```

ER43 applies as it does to any discarded error: if `E` carries a must-consume payload, `catch _ =>` is rejected and you bind it with `catch e => …` and consume it, or `match`.

Everything is gone: `ok`, `map`, `map_err`, `and_then`, and previously `unwrap_or`, `unwrap_or_else`, `is_ok`, `is_err`, `to_option`, `to_error`, `on_err`. The operator family covers the work — `.unwrap_or(v)` is `x ?? v` on an optional and `r catch _ => v` on a result, `.unwrap_or_else` is `r catch e => f(e)`, `.map_err` is `r catch e => return f(e)`, `.ok()` is `r catch _ => none`. Rust needs the eager/lazy pairs (`unwrap_or`/`unwrap_or_else`, `ok_or`/`ok_or_else`) because method arguments evaluate eagerly; the fallbacks' right sides only evaluate on the miss, so each pair collapses into one form.

There is no fold *method* either. A fold ends the error's journey, and journey-endings belong to the operator family — a `.recover()` would be the first step back toward the zoo the redesign removed (`std.api/SD4`).

`.ok()` was the last survivor, kept on the argument that it's a shape conversion rather than a combinator. Measurement broke that: of 45 uses, 41 are in statement position and throw the optional away — `tx.send(x).ok()` means "I don't care if this failed", not "give me an optional". Rask already spells a deliberate discard `let _ = f()` (`tool.warnings/W3`), so `.ok()` was a second, error-only way to do it that named the success branch while the author meant *drop the error*. The 4 real conversions are `r catch _ => none` and `r catch _ => return none`.

## Union Widening, Wrapping, and Boxing

| Rule | Description |
|------|-------------|
| **ER31: Auto-widen** | `try` succeeds when the expression's error type is a subset of the current function's error union |
| **ER31a: Auto-wrap into a boundary enum** | `try` succeeds when the current function's error type is an enum with **exactly one** variant whose only payload is the propagated error type. `try f()` then means `f() catch e => return Outer.Variant(e)`. Two candidate variants is a compile error naming both — the wrap has to be unambiguous |
| **ER32: Auto-box to `any Error`** | `try` auto-boxes when the current function's error type is `any Error` — any `E` satisfying `Error` widens by boxing |

<!-- test: skip -->
```rask
// Library: precise union
func load() -> Config or (IoError | ParseError) {
    let content = try read_file(path)   // IoError ⊆ union
    let config = try parse(content)     // ParseError ⊆ union
    return config
}

// Service boundary: one enum, one variant per wrapped error
enum ApiError {
    Store(StoreError),
    Validation(ValidationError),
    BadRequest(string),
}

func view(id: TaskId) -> TaskView or ApiError {
    let task = try store.view_task(id)  // → ApiError.Store(e)
    return task
}
```

Libraries use union errors (precise, matchable). Applications use `any Error` (ergonomic, sufficient for logging). Downcast with `if r is IoError as e` for recovery.

ER31a is the enum spelling of ER31's subset check. The union form composes error types structurally; the enum form gives the composition a name and a `match` at the boundary. Both should propagate without ceremony — writing `catch e => return ApiError.Store(e)` at every call restates what the enum already says. The wrap is one hop: `StoreError` reaching an `ApiError` return is automatic, `StoreError` reaching a `TopError` that wraps `ApiError` is not.

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
    let text = try fs.read_text(path)     // ConfigError.NotFound captures origin
    return try Config.parse(text)            // ConfigError.Parse captures origin
}

if load_config(path) is ConfigError as e {
    log("{e.origin}: {e.message()}")         // "config.rk:42: not found: app.conf"
}

// any Error — origin always available
func start_app() -> App or any Error {
    let config = try load_config(path)     // IoError auto-boxes, gets origin
    return App.new(config)
}
```

## @message Annotation

`@message` generates the `message()` method from per-variant templates — eliminates the match boilerplate for error enums.

| Rule | Description |
|------|-------------|
| **ER35: @message opt-in** | `@message` on an enum generates `func message(self) -> string`. Compile error if the enum already defines `message()` manually |
| **ER36: Variant template** | `@message("template")` on a variant provides the format string. `{name}` for named payloads, `{0}` / `{1}` for positional |
| **ER37: Auto-delegate** | A variant with a single payload that itself satisfies `Error`, and no `@message` annotation, delegates to `inner.message()` |
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
    let text = try read_file(path)       // IoError
    let config = try parse(text)          // ParseError
    return config
}
// Inferred: -> Config or (IoError | ParseError)

// 2. Partial: `or _` — success explicit, error inferred
func load_config(path: string) -> Config or _ {
    let text = try read_file(path)
    return try parse(text)
}

// 3. Public — must be explicit
public func load_config(path: string) -> Config or (IoError | ParseError) {
    let text = try read_file(path)
    return try parse(text)
}
```

Each `try expr` where `expr` returns `T or E` contributes `E`. Each bare error return in the body contributes that error's type — including a `return` inside a fallback: `x ?? return e_val` and `r catch e => return f(e)` contribute the type of what is returned. A fallback whose right side is a value contributes **nothing** — no error leaves the function through it. The inferred union is deduplicated and sorted alphabetically for deterministic output.

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
| `T or i32` (primitive E) | ER4 | Compile error — E lacks `Error` |
| `T or none` | ER4 | Legal — `none` is exempt from the `Error` bound |
| `try x` on a `T?` in a `U?`-returning function | ER16/ER47 | Legal — propagates `none`; the ordinary way |
| `try x` on a `T?` in a `T or E`-returning function | ER47 | Compile error — `none` doesn't fit an error branch. Use `x ?? return <error>` |
| `try r` on a `T or E` in a `T?`-returning function | ER47 | Compile error — the error has nowhere to go. Use `r catch _ => return none` (drops the detail, acknowledged) |
| `try f()` where `f` returns `T? or E` (flat) | ER47 | Compile error naming both escapes. Write `try f() ?? <…>` (ER16b) or handle explicitly |
| `try f() ?? v` on a flat `T? or E` | ER16b | Legal, no parens — `(try f()) ?? v`; error propagates, absence gets `v` |
| `x ?? return MyError` | OPT11 | Legal anywhere `return MyError` is — normal return rules apply |
| `r catch _ => return none` in a `T?`-returning function | ER14 | Legal — drops the error detail, acknowledged |
| `Config { host: g() catch _ => return E, port: 8080 }` | ER45a | Compile error asking for parens around the diverging fallback |
| `Config { host: (g() catch _ => return E), port: 8080 }` | ER45a | Legal — the parens show where the exit ends |
| `Config { host: g() ?? "localhost", port: 8080 }` | ER45a | Legal, no parens — a value right side has no exit to locate |
| `x ?? break` / `?? continue` / `?? panic(…)` | OPT11 | Legal — any divergence, written where it happens. (`?? panic(…)` lints toward `x! "…"`, which is shorter) |
| `r catch v` (no binder) | ER14 | Compile error — the binder is mandatory. `catch e => v` to use the error, `catch _ => v` to drop it visibly |
| `r ?? v` on a result | ER12 | Compile error — `?` marks absence. Use `catch _ => v`, which says an error is being dropped |
| `try a.b().c` with one fallible step | ER16a | Legal — `try` attaches to that step; no parens |
| `r catch _ => none` | ER14a | Legal — yields `T?`, dropping the error visibly. This is the old `.ok()` |
| `r catch e => f(e)` where `f(e): T` | ER14 | The expression is that `T`; nothing leaves |
| `x catch e => …` on an optional | ER14 | Compile error — absence has nothing to catch. Use `??` |
| Unused `e` in `r catch e => f(e)` | ER14 | Lint suggesting `catch _ =>` |
| `x ?? v` where `x` is neither `T?` nor flat | OPT3 | Type error — `??` needs an optional left side |
| `a ?? b` / `a catch _ => b` on two bools | OPT3/ER14 | Type error suggesting `\|\|` |
| `void_call() catch _ => E` | ER14 | Type error — the body produces the success type, and that's `void`. Use `catch _ => return E`, or handle with `catch e => log(e)` |
| `r catch _ => v` where `E` carries a linear payload | ER43 | Compile error — a linear payload may not be discarded, even acknowledged. Bind it with `catch e => …` and consume it, or `match` |
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
12  |  let r = cached(|| CacheError.Miss)
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

**Missing Error [ER4]:**
```
ERROR [type.errors/ER4]: i32 cannot be an error type
   |
2  |  func f() -> string or i32
   |                        ^^^ i32 does not implement Error

WHY: Every error type must provide `func message(self) -> string`.

FIX: Newtype it and implement message():
     type StatusCode = i32
     extend StatusCode {
         func message(self) -> string { "status {self.value}" }
     }
```

**Auto-wrap outside return [ER11]:**
```
error[E0828]: a `i32` doesn't become a `i32 or MyError` here — auto-wrap only fires at `return`
   |
3  |  let r: i32 or MyError = 5
   |         ^^^^^^^^^^^^^^^^^^ this is a `i32`, and nothing wraps it
    = fix: get the value from something that already returns `i32 or MyError` — a call,
           or a small `func` whose `return` does the wrapping
    = why: at a `return` the branch is obvious from the signature; at an assignment it
           isn't, so the choice between the success and error branch is written rather
           than inferred. Optionals are exempt — a `T?` widens anywhere, because `none`
           is the only other branch [type.errors/ER11]
```

The same message covers all three non-return positions — binding, argument, field.

**Bare value after `catch` [ER14]:**
```
ERROR [type.errors/ER14]: `catch` needs a binder — `e =>` or `_ =>`
   |
7  |  let ms = raw.parse() catch ApiError.BadRequest("bad duration")
   |                               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ no binder

WHY: `catch` handles an error, and the error is a value someone might need.
     Name it, or drop it out loud — silence isn't an option.

FIX: let ms = raw.parse() catch _ => return ApiError.BadRequest("bad duration")
     let ms = raw.parse() catch e => return wrap(e)
```

**`catch` on an optional [ER14]:**
```
ERROR [type.errors/ER14]: nothing to catch — `config.port` is an optional
   |
4  |  let port = config.port catch e => default_port(e)
   |                           ^^^^^ `i32?` — the other branch is `none`, not an error

WHY: `catch` handles failures. Absence has no payload and isn't a failure.

FIX: let port = config.port ?? default_port()
```

**`try` where what leaves doesn't fit [ER47]:**
```
ERROR [type.errors/ER47]: `try` propagates `none`, but this function returns `Config or IoError`
   |
4  |  let x = try maybe_value
   |            ^^^ maybe_value: T? — the bad branch is `none`, and there is no
   |                optional branch in the return type to carry it

WHY: bare `try` hands the bad branch to the caller unchanged. `none` fits a `T?`
     return; an error fits an error branch. Neither converts silently.

FIX: name what should leave instead:
     let x = maybe_value ?? return IoError.NotFound
```

**Bare `try` on a flat `T? or E` [ER47]:**
```
ERROR [type.errors/ER47]: `try` is ambiguous here — two branches could leave
   |
4  |  let kv = try sst_point_lookup(sst, key)
   |             ^^^ returns `KeyValue? or SstError` — both `none` and the error
   |                 are "the bad branch"

WHY: bare `try` propagates the bad branch, and this operand has two of them.
     The composite means: error up, absence here.

FIX: let kv = try sst_point_lookup(sst, key) ?? return none
     let kv = try sst_point_lookup(sst, key) ?? continue
     (`try` binds tighter — no parens. It peels the error; `??` says what
      absence does.)
```

The mirror case is a result in a `T?`-returning function: `r catch _ => return none`, which says plainly that the error detail is being dropped. To drop it without leaving, `r catch _ => none`.

**`?` used as a success test on a result [ER12]:**
```
ERROR [type.errors/ER12]: `?` tests for absence, not for errors
   |
5  |  if r? { use(r) }
   |      ^ `r` is `f64 or DivError` — its other branch is an error, not `none`

WHY: `?` is the absence marker throughout the language. Errors are handled by
     `catch`, `try`, and `is`, so a line says which kind of failure it deals with.

FIX: if r is f64 as v { use(v) }          (test the success side)
     if r is DivError as e { … }           (test the error side)
     let v = r catch _ => fallback       (supply a value, dropping the error)
```

**Match on Option:**
`match x { Some(…) => …, None => … }` is rejected as `Some`/`None` are not valid Rask syntax (see the `Some(v)`/`None` diagnostic in [optionals.md](optionals.md)). The accepted form `match x { none => …, u => … }` is legal but emits a style lint suggesting the operator form.

---

## Appendix (non-normative)

### Rationale

**Why operators instead of combinator methods.** Rust handles the same territory with a method vocabulary — `unwrap`, `expect`, `unwrap_or`, `unwrap_or_else`, `map`, `map_err`, `and_then`, `or_else`, `ok`, `ok_or`, `ok_or_else`, and their siblings, doubled across `Option` and `Result`. Rask replaces the vocabulary with the operator family (ER12–ER18, ER44, ER47), and the replacement isn't cosmetic — three structural facts make the zoo unnecessary rather than merely renamed:

1. **The operators enforce extract-early style.** Rust's combinators exist to thread a *wrapped* value through a pipeline — transform the inside, stay wrapped, unwrap at the end. Rask's operators do the opposite: `try` gets the value or exits now, `??`/`catch` get the value or a replacement now. Once nothing threads wrapped values, `map`/`and_then`/`or_else` have no job. This is a style decision (handle it here, like Go) wearing concise syntax.
2. **One shape, not two.** Half of Rust's vocabulary is `Option`↔`Result` plumbing (`ok`, `ok_or`, `err`, …). `T?` being `T or none` on the same machinery deletes the category outright: converting between the shapes is `r catch _ => none` one way and `x ?? return MyError` the other, both of them ordinary uses of operators that already exist for other reasons.
3. **Operators can't breed.** Methods grow by one-line PR — that's how the zoo happened. An operator is a language change gated by the Ceremony Test (`CORE_DESIGN`). The family is *frozen by construction*: a handful of forms around one mental template (test, extract, other-branch, force, propagate, chain), learned once as a unit. Swift's `?`/`!`/`??` and Zig's `orelse`/`catch`/`try` are the precedent that a small closed operator set for this territory is learnable in a day and then disappears into fluency.

**"But the methods were for chaining."** The standard defence of combinators, and it's the only real one — so it's worth separating the two things it can mean. *Fallback* chaining is "try this, else that, else that", and Rask has it as one operator: `user?.display_name ?? user?.email ?? "anon"`, `load_from_disk() catch _ => load_from_env() catch _ => load_from_net()`. That's 258 fallback chains and 17 `?.` uses in the tree, no method involved. *Transform* chaining is `r.map(f).and_then(g)` — stay wrapped, hop, unwrap at the end — and that's what the methods offered. `.and_then(` has never appeared in a `.rk` file in this repository's history, and every method chain that does appear is on a sequence (`v.filter(…).map(…)`), not on a wrapper.

The reason is one number: the tree holds 374 `try` tokens and **zero lines containing two of them**. Every fallible step is extracted on its own line, so a wrapped value never sits in the middle of an expression for a method to hang off. The shape combinators exist to serve doesn't occur — which makes cutting them a description of how the code is already written, not a restriction imposed on it. The cost is real and small: `read_file(path).map_err(Io)?.parse::<Config>().map_err(Parse)?` becomes two statements, and the intermediate gets a name.

The corollary is a discipline: the family beats the method zoo exactly as long as it stays frozen. An ergonomic itch is answered by composing existing operators or writing a `match` — never by minting form nine. And no combinator methods through the back door: a `map_err` in the stdlib would be the zoo regrowing (`std.api/SD4`); the error-transform need is served by `catch e => return …` (ER14).

That discipline was stated here for two revisions while the Methods section below still listed `map`, `map_err`, `and_then` and `filter` — the argument was made and the surface kept anyway, so the operators were added *on top of* the zoo instead of in place of it. Measurement settled it. Across stdlib, examples, projects and tests: `and_then` zero uses, `map` and `filter` on either wrapper zero (every hit is a sequence), `map_err` two. `.ok()` looked like the exception at 45, until the sites were read — 41 are in statement position, discarding the optional they build, which is `let _ = f()` and not a conversion at all.

Expert Rust agrees. A census over tokio and ripgrep (see `docs/rust-corpus-census.md`) puts `ok_or`/`and_then`/`map_err` at ≤2 uses per 10k lines in both — the combinator style barely exists in expert hands, while `?`, `unwrap`, `if let` and `match` run at 200–450/10k. The forms this family keeps are the forms experts already live in; the forms it cuts are the ones they already avoid.

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

`none` is different on both counts. It carries no payload, and its layers are reached in order: the outer operators (`?`, `??`, `!`, `is none`) act on the outer layer, and the inner layer is only visible after narrowing through it. One rule — a bare `none` literal means the outer absent — closes the only remaining ambiguity. So `none` layers and payload variants don't. See [optionals.md](optionals.md).

**ER4 (Error bound).** A minimum bound on E solves three problems at once: (1) `r!` can always produce a useful panic message; (2) primitives can't accidentally be error types, so `i32 or i32` style ambiguities don't arise; (3) richer capabilities (context, codes, stack traces) layer opt-in on top without forcing complexity on simple errors.

**ER9 (auto-wrap return-only).** Auto-wrap at assignment/field/argument positions makes the branch choice invisible at the use site. Restricting it to `return` keeps the error branch visible — you can only produce a `T or E` by returning from a function declared to return one.

**ER14 (the fallback settled in three rounds — and what each round taught).** This rule reversed three times, and the record matters because each reversal was argued from a different premise, and the last one from reading rather than counting.

Round one: `??` covered both shapes (`x ?? 50`, `r ?? 0.0`), which put the absence marker on error handling. Round two split per shape — `??` for absence, value-level `or` for failure. Round three merged both into one word (`orelse`), on the argument that fallback is the same operation on both shapes — "the payload, or this instead" doesn't change meaning with the shape.

The merge was semantically true and it failed the reading test. The designer's own review of the migrated flagship found the constant unpaid question at every fallback site: *was that an error just now, and did it survive?* The merged word answered it nowhere. The miss in round three's argument: the fallbacks are the same **operation** but not the same **event**. Discarding a `none` destroys nothing; discarding an `E` destroys information — a payload, a diagnosis, a cost — and Transparency of Cost already rules that visible. `try` never met this objection because propagation preserves the bad branch; only the fallbacks destroy, and only on the error side.

So the final shape (this round): `??` returns as the absence fallback — terse *because* nothing is lost — and `catch` carries the failure side with a **mandatory binder**: `e =>` to use the error, `_ =>` to drop it in plain text. The flagship prices the split: 28 absence fallbacks, 6 failure fallbacks — the marked side is the rare side.

**What the binder actually buys — stated honestly.** The keyword alone already says an error dies: `f() catch Theme.default()` could not mean anything else, so "visibility" is carried by `catch`, not by `_ =>`. The binder's residual value is threefold: `grep 'catch _'` finds *exactly* the discard sites (bare `catch` would mix discards with handlers); the grammar has one form instead of an optional binder; and the binderless sugar creates a typo adjacency — `f() catch e` meaning `catch e => …` would silently compile as *substitute the value `e`* whenever a suitable `e` is in scope (Zig dodges this with its bracketed `|e|`; an unbracketed binder can't). Whether those three outweigh five characters of ceremony at the family's rarest form is deliberately deferred to the stdlib migration (#602): dropping the binder for value bodies is a pure relaxation, so deciding on corpus evidence costs nothing, and this corner has reversed enough times that stability itself has weight.

On the word: `catch` was rejected in an earlier round when it had a binderless value form — `parse(s) catch 8080` read as interception of something thrown, and Rask has no `throw`. The mandatory binder is what un-rejects it: `catch e =>` literally catches the error into a name, `catch _ =>` visibly drops it, and the misleading form no longer exists. `or` was the runner-up — it mirrors the type (`T or E` handled by `or e =>`) — but costs cast-grammar carve-outs (`x as (A or B)`) and reopens one-token-two-grammar-levels. Zig is the precedent for the whole triple, including the part that looks riskiest: Zig has `try` *and* `catch` with no exceptions, taught in a paragraph, for a decade.

**The debt this closes.** Earlier revisions carried "swallowed errors are unmarked" as an accepted cost, softened by a someday-lint. The mandatory binder deletes the debt at the grammar level instead: the lint that had no design is now the rule `catch` can't be written without.

**What the mark costs, measured.** Catch-shaped sites (a result's error handled in place, rather than propagated or asserted) run rare in real code: 3/10k lines in tokio, 9/10k in ripgrep, 26/10k in the rask compiler. How often the error is *discarded* at those sites is domain-shaped — the compiler binds it 68% of the time (errors become diagnostics), tokio discards 82% (best-effort I/O), ripgrep splits evenly — so `_ =>` versus Zig-style silence costs five characters at somewhere between 2 and 20 sites per 10k lines. The rarity is also why the mark won't wear out: Go's `if err != nil` at ~100/10k went invisible from repetition; a form a reader meets a couple of times per file stays load-bearing. Precedent runs one direction: Rust code already writes `let _ =` as an acknowledged discard at similar rates (112 sites in tokio src) without complaint; Go's silent error-drop in statement position spawned `errcheck`, one of its most-installed linters; Zig's silent `catch v` draws recurring "make swallowing harder" threads. Ecosystems that shipped the silent form retrofit the check.

**The boundary of the guarantee.** What the binder rule buys, stated precisely: **no error dies inside an expression without a mark.** Errors can still die *structurally*, and the spec owns the list rather than implying it's empty. (1) A one-armed narrow — `if r is T as v { … }` with no else — ignores the error case; the drop is visible as a missing else but not greppable. Measured at 0.5–4.6/10k (Rust's `if let Ok` rate), and reading those sites shrinks the category twice over: most are *probes* (env lookup, "does this parse as an IP") that Rask's stdlib returns as `T?` — so they're `if x? as v`, where the one-armed test drops nothing — and of the genuine results, the guard-shaped ones belong to `catch` — one canonical guard; [canonical-patterns.md](../canonical-patterns.md) has the ladder. What's left is genuine opportunism — continue either way, error valueless in context — which is why this stays legal. Lint candidate for the I-series, with a sharper trigger than first thought: a one-armed `is T` narrow whose body is the function's tail is a guard wearing a costume; suggest the `catch` form. (2) `ensure f.close()` drops a cleanup failure by default (`ctrl.ensure/ER1`). Deliberate: scope exit has nowhere to send an error mid-unwind, and taxing every ensure with `catch _ =>` to mark the rare interesting case would invert the economy — the drop lives in ensure's documented semantics, not in an innocent-looking expression. (3) `let _ = save(d)` discards result and error together — but that's already the acknowledged form, same class as `catch _`.

**ER12 (`?` means absence, and only absence).** Two spec files used to disagree: `optionals.md` OPT3 restricted the `?`-family to `T or none`, while this file handed out `r?` as a success test and `if r?` narrowing on results. Under the split, `?` never touches a result's success test — `is` does it, and `is` is strictly more informative, since `if r is IoError as e` names what it's testing while `if r?` makes the reader recall what `r` was.

Deleting the `if r?` narrowing family costs less than it looks like. On results the high-frequency operations are `try` and the fold; the narrowing family was the rare one, and where it's wanted `is` covers it in the same number of tokens.

Two operators stay shared, each because the operation genuinely doesn't care which branch went bad: `!` asserts the good branch without claiming which one failed, and `match` is multi-arm dispatch.

`?.` was the hard case. It survived an earlier draft of this rule on the argument that the `?` binds to the dot rather than to the value — which was a fudge. The real objection isn't the marking, it's that `r?.field` threads a `T or E` into a `Field or E`: a wrapped-value pipeline, the shape ruled out by the extract-early argument above, and the reason there's no `map`/`and_then`. It was also the last place Rask read `?` as Rust reads it, meaning propagation. Extract first — `try r.field`, with the placement rule doing the work (ER16a).

The cost measured out at nothing: across stdlib, examples, projects and tests there was not one `try … ?.` — every `?.` in the tree is an optional chain. Optional chaining stays because it isn't a pipeline; `user?.profile?.name` asks whether the path is there and terminates in `??`, `?`, or `!`.

**ER44 (a fold operator, not a fold method).** The error-model redesign removed `.unwrap_or_else` and pointed its migration at `try { … } else e => f(e)`. That was wrong: the block form early-returns the transformed error, so it propagates and cannot produce a value. Nothing covered the *terminal* fold — collapsing `T or E` into `T` at a boundary with nothing above it. Every program has some: routers, `main`, task bodies.

<!-- test: skip -->
```rask
return dispatch(req) catch e => error_response(e)
```

The alternative was a method (`.recover(|e| …)`), and a method is how the zoo starts. The operator family already had the pieces, so the fold composes what's there instead of minting a form for the occasion.

**The exit rule, and its reversal.** Three designs preceded this one, and the record is worth keeping because the final rule reverses a rule this file defended at length.

`?? return` — the original — let the fallback operator diverge. #565 rejected it, at the time for reading reasons. The next design routed exits through value-level `or` (`r or return E`), which created two spellings for "an error leaves" — `try r` and `r or return E` — with the more common one unmarked. The fix chosen then was maximal: **`try` owns every exit**, fallbacks may never diverge (ER48), and error transformation got its own clause (`try r else e => return f(e)`, ER45/ER46).

That rule was sound on its own terms and it died of its consequences. It forced a second propagation form (`try … else`) whose clause restated `try`'s job with different latitude; it made absence-exits verbose (`try x else return none` where the operator alone would do); and the clause construct it required is the thing this revision deletes — 116 sites of `try X else …` collapse onto the fallbacks, one operator instead of two. The visibility property ER48 protected — "scan the left margin for exits" — turned out to be narrower than claimed: what a reader needs is that **no exit is invisible**, and a `return` written in full on the right of a fallback is exactly as visible as one behind a `try else`. The exit is spelled where it happens; nothing leaves through an unmarked value. The one silent form is bare `try`, and it means one thing.

Kotlin (`?: return`) and Zig (`orelse return`) are the precedent for a diverging fallback, and Swift/Rust's counter-camp (`guard let`/`let-else`) exists to serve the same pattern with an extra construct. Rask was briefly in the second camp; it's now in the first, with the construct deleted rather than added.

The value-argument against divergence — "the right side exists to supply the value being bound, and `return` abandons the binding" — was this file's own strongest case for ER48. What it missed is that the same words describe the pattern guard (`const t = event is Ready else { break }`, CF13), which the language already accepts: a binding form whose other branch refuses to bind and says so in plain control flow. `Never` coercion is how the type system spells "this branch doesn't produce the value", and using it here is no more a contradiction than in an `if`/`else` where one arm returns.

**Rejected alternatives for the bail-out form** (kept for the record; the list's outcome changed):

1. **`else` for all of it** — `const v = x else { return }`. Still dead: `else` is the if-expression word *and* the CF13 pattern-guard word, in the same `const x = … else …` slot, with contradictory divergence rules.
2. **A presence guard construct** — still dead; same objection plus restating CF13 for more scrutinee shapes.
3. **A diverging right side on the fallback** — rejected then as `r or return E`, **adopted now** as a diverging fallback body (`x ?? return E`, `r catch _ => return E`). What changed: `try` no longer has a clause, so there's exactly one propagation form and the exits live on the fallbacks — the "two spellings for leaving" objection was an artifact of `try … else` existing alongside it.
4. **`expr? else return X`** — still dead, and the stated reason was wrong: "one token, two result types" already describes `x?` (bool) beside `x?.f` (optional). The real reason is item 1: `else` is taken.
5. **Bare `try` on an optional** — rejected then, **adopted now**; see ER47 below for what the earlier analysis got right and what it overgeneralized.

**ER47 (the shape rule — what bare `try` propagates must fit).** This rule flipped twice; the final form keeps the earlier analysis's true core and drops its overreach.

The case that drove the errors-only restriction: `sst_point_lookup` in the LSM example returns `KeyValue? or SstError` and calls both shapes two lines apart:

<!-- test: skip -->
```rask
let index = try read_sstable_index(meta.path)            // -> Vec<BlockIndex> or SstError
let target_block = try find_block_for_key(index, key)    // -> i32?
```

Identical syntax, opposite control flow — the first leaves through the error branch, the second through the success branch — and nothing on either line says which. The analysis was right that this is unacceptable. It was wrong about where the ambiguity lives. Rust's `?` covers both shapes without this problem because `Result<Option<T>, E>` **nests** — inside any one function, `?` has exactly one reading, given by the enclosing return type. Rask's flat sugar (`T?` = `T or none`) is what creates the two-readings case, and it creates it only where the flat shape actually occurs: the *enclosing* `T? or E` return, measured at **2 of 316** return types in the tree. Everywhere else the enclosing signature admits exactly one bad branch, and bare `try` has exactly one possible meaning — same one-hop lookup as Rust's `?`.

So the rule is targeted where the problem is: bare `try` on a flat `T? or E` **operand** is a compile error naming both escapes, and the composite `try f() ?? …` spells the resolution — `try` peels the error, `??` names what absence does. That composite is common enough to deserve first-class treatment: Zig's stdlib holds ~160 error-then-absence composites against ~4 reversed, and Zig's own precedence forces parens on the common order (their issue #5436). ER16b rules it the other way here — `try` binds tighter — so the common composite is paren-free.

The errors-only draft also leaned on a count: "of the 72 absence-exits in the tree, zero want the implicit form." The count was circular — the corpus had no way to write the implicit form, so no site could want it. Rust corpora, where `?` on `Option` exists, use it pervasively. The 65 absence-exits that name a specific target (`?? return Token.Eof`, `?? break`) keep their named exits; the 7 that propagate `none` unchanged are bare `try x` now.

`.to_result(err)` stays retired: that was `x ?? return err` spelled as a method.

**ER16a (`try` finds its place in the chain).** `try` has to bind loosely, or `try store.get(id)` would read as `(try store).get(id)`. That used to mean projecting off a propagated value needed parentheses — `(try read_file(p)).len()`. It doesn't: a wrapped value has no payload methods, so in `try a.b().c` at most one placement of the `try` type-checks, and the compiler can find it.

The rule has no exception, and that fell out of cutting the methods rather than being designed in. Ambiguity needs a name that resolves on the wrapper *and* on the payload; with zero methods on the wrapper there's nothing to collide, so the compiler always finds the one placement. An earlier draft had the exception firing constantly — the wrapper carried `map`/`filter`/`and_then` and so did every sequence, making `try v.map(f)` need parentheses — and a later one had it surviving for `ok` alone. Both are gone. `try` also doesn't slide into call arguments: `f(try g())` is written where it means.

**ER45a (parens in a comma list).** Found by writing the config-loading shape, which every program has:

<!-- test: skip -->
```rask
return Config {
    host: get(raw, "host") ?? return ConfigError.Missing("host"),    // needs parens
    port: 8080,
}
```

The comma does end the `return`'s expression, so this could just be allowed. The reason not to: the exit is written out so it stays *visible*, and an exit sitting in the middle of a field list is visible in the letter and hidden in practice — you scan a struct literal for fields, not for places the function might leave from. Parens put a boundary around it, which is the same move ER16a makes when a `try`'s placement is genuinely unclear.

A value right side is exempt — it can't leave the function, so `host: get(raw, "host") ?? "localhost"` needs nothing. The rule keys on divergence, which is exactly the thing that has to be findable.

Zero sites in the tree pay for this — the house style extracts first anyway, and the extracted version reads better:

<!-- test: skip -->
```rask
let host = get(raw, "host") ?? return ConfigError.Missing("host")
return Config { host: host, port: 8080 }
```

**Why `e =>` and not `|e|`.** Two drafts spelled the error binding `|e|`, and both needed a rule saying it wasn't a closure — because it looks exactly like one, and `return` inside a real closure exits the closure (`ctrl.flow/CF26`). A rule whose only job is to disclaim the syntax it's attached to is the syntax admitting it's wrong. The collision was live, too: `try` inside `|e| { … }` leaves the enclosing function, while `try` inside a closure `|x| { … }` leaves the closure, and the two are indistinguishable on the page.

Making it a genuine closure was the other way out, and it doesn't work. `r catch |e| return f(e)` would fold rather than leave, so the transform-and-leave form becomes unwritable and the error goes back to being returned implicitly — which is the thing #574 and the written-out `return` exist to prevent.

`=>` is the match-arm binder, and match arms already have the exact semantics wanted: a name bound over a body that belongs to the enclosing function, so `return` leaves the function. Braces optional, same as an arm. Nothing needs to be said about what it isn't, which is the whole point. It also frees the rule that had been explaining the resemblance — one fewer thing to hold.

**Why not `:`?** `catch e: fallback` is one character shorter and echoes Python's `except E as e:`. It loses on three concrete grounds. First, `:` already carries two meanings in this grammar — type annotation and struct-literal field — and both put it in exactly the positions `catch` appears in: `Config { host: g() catch _: "localhost", port: 8080 }` has three colons meaning three things, and `catch e:` on its own reads as "e has type …". Second, Rask has no colon-introduced body anywhere (Python's colon opens a suite; ours would separate a binder from an expression, a construct the language doesn't otherwise have), while `=>` reuses the one body-introducer that already exists with the right `return` semantics. Third, `=>` keeps the left side reading as a *pattern slot*: today's `e` and `_` are the two degenerate patterns (bind-all, ignore-all), and if a filtered catch is ever wanted (`catch DiskError.Full(n) => …`) the syntax is already sitting there — with `:` it would read as an annotation. Nesting was checked and is a wash: a `catch` inside a match arm puts two `=>` on one line, but they mean the *same* thing (a bound body follows), which is exactly the property Scala's `case e =>` inside try/catch has shipped on for two decades. The colon version nested in a struct literal is strictly worse.

The cost was checked before committing: 83 binding sites in the tree, **zero** of them inside a match arm, so the one thing `=>` could plausibly have collided with never co-occurs. `|_e|` and `|_|` sites (authors underscoring a binding they didn't want) become `_ =>` or drop the binding entirely.

**ER14's binder — mandatory, reversing #573's letter to keep its spirit.** #573 asked to drop the binding when the error isn't used, and a round of this spec obliged: the binder was optional, and the flagship wrote six decode guards that discarded an error with no trace in the text. The reading review showed what that ceremony had been carrying: the binder is the only thing at a fallback site that says *an error existed*. So the rule flipped — the binder is mandatory, but `_` satisfies it: `catch _ =>` is two characters of acknowledgment, not the dead `|e|` ceremony #573 objected to (which named a value nobody used and looked like a closure doing it). You never write a name you don't want; you always write *that there was something*.

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
    let text = fs.read_text(path) catch e => return context("reading {path}", e)
    return Config.parse(text) catch e => return context("parsing {path}", e)
}
```

**Typed domain errors.** For library-level errors, wrap in domain-specific types. A variant that adds context beyond the error needs the explicit map:

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or ConfigError {
    let text = fs.read_text(path) catch e => return ConfigError.Io { path, source: e }
    return Config.parse(text) catch e => return ConfigError.Parse { path, source: e }
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
    let text = try fs.read_text(path)   // → ConfigError.Io(e)
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
- [Ensure](../control/ensure.md) — `ensure … catch e =>` pattern (`ctrl.ensure`)
- [Error Model Redesign Proposal](error-model-redesign-proposal.md) — decision record for the no-wrappers surface
