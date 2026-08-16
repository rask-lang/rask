# Design Rationale

Why I made certain design choices. Mostly about what I didn't add from other languages.

---

## Ok/Err/Some/None Constructors

**Looked at:** Rust, ML-family

**Why rejected:**

Rask originally had `Option<T>` and `Result<T, E>` as standard enums with `Some(v)`, `None`, `Ok(v)`, `Err(e)` constructors (Rust-style). In practice these wrappers add a tag that is always the same tag — auto-wrapping (now subsumed by union widening) already coerced bare values at function boundaries, so the constructor survived only at intermediate sites. Every rebind form (`is Some(u)`, `is Some as u`, `const Some(u) = x`, magic rebind) existed because the wrapper needed to be unwrapped. Remove the wrapper and the rebind cloud evaporates.

`T or E` and `T?` are now builtin tagged unions. The compiler picks the branch from the value's type at return (enforced by the disjointness rule T ≠ E and the `Error` bound on E), so construction is keyword-free on both paths: `return config` or `return MyError.Failed`, never `return Ok(config)` / `return Err(MyError.Failed)`. `none` stays as the absent sentinel (literal, not a variant).

The trade-off is a disjointness rule (T ≠ E in `T or E`). Newtype is the escape hatch. The rule also rules out primitive-error patterns like `i32 or i32`, which were always bad style. In generic code it's an obligation read off the signature and checked at the call site (`type.errors/ER3a`). `none` is exempt — optionals nest, and the layers stay distinct (`type.optionals/OPT28`).

See [types/error-model-redesign-proposal.md](types/error-model-redesign-proposal.md) for the full design record.

---

## Algebraic Effects

**Looked at:** OCaml, Koka, Unison

Algebraic effects are elegant. Functions raise "effects" that handlers up the call stack intercept and resume. Clean abstraction for I/O, state, errors—without changing signatures.

```
// Function can raise Async effect without declaring it
func process(data: Data) -> Result {
    return parse(file.read())  // Implicitly raises Async, handled elsewhere
}
```

I'm not adding them. Here's why.

### They hide costs

I want major costs visible in code. With effects, `process(file)` could do I/O, allocate memory, jump to handlers halfway up the stack—none visible at the call site.

```rask
// Current Rask
let file = try open(path)    // I/O here
ensure file.close()            // Cleanup here
try process(file)              // Error propagation here

// With effects
process(file)  // Hidden costs
```

That breaks transparency.

### They break local analysis

To understand if a function is safe, you need to track all effects it can raise, trace all possible handlers in scope, analyze handler compositions across the call stack. Change a handler deep in the stack? Type-checking cascades everywhere.

I want function-local compilation. Effects require whole-program analysis.

### They break resource safety

Rask's safety is structural—must-consume resources, must-use handles, block-scoped cleanup. `ensure` blocks run on all exits, LIFO order, guaranteed.

Effects introduce non-local jumps. Does the handler run before or after cleanup? What if an effect jumps past an `ensure` block? You need to reason about effect boundaries intersecting with resource scopes. Structural guarantees become ambiguous.

### They're function coloring in disguise

I don't want `async`/`await` because it splits the ecosystem. Effects do the same—functions that raise effects become constrained. Can't use them without handlers. Same problem, hidden in effect types instead of keywords.

### Errors become invisible

Right now `let value = try some_operation()` tells you it can fail. With effects, handlers catch errors before they reach you. Error flow becomes hidden in handler chains.

### What I chose instead

Result types for errors. `using Multitasking { }` for async I/O (tasks pause automatically). `ensure` blocks for cleanup. Function parameters or `with` clauses for context. `Shared<T>` for shared state.

More verbose in places, but every cost is visible and every path is local. Effects give you less ceremony—I chose transparency. More `try` keywords in error-heavy code, but errors should be visible.

If Rask had effects, it wouldn't be Rask.

### Effect tracking as metadata

Rask does track effects—just not in the type system. The compiler infers IO, Async, and Mutation effects transitively for every function (see `comp.effects`). This metadata powers:

- **IDE ghost annotations** showing `[io]`, `[pure]`, etc. on function definitions
- **Compiler warnings** for IO in thread pools (`comp.effects/CW1`) and tight loops (`comp.effects/CW2`)
- **`@pure` lint annotation** that flags violations as warnings, not errors (`tool.lint/P1-P3`)

The key distinction: effects are information, not constraints. A "pure" function can call an IO function—it just inherits the IO effect. No function coloring, no ecosystem split, no effect polymorphism. Same pragmatic middle ground as the async model: information available through tooling, not enforced through syntax.

---

## Automatic Supervision

**Looked at:** Erlang, Elixir

Erlang's supervision trees are great—processes automatically restart when they crash. "Let it crash" philosophy. But automatic restart is a hidden side effect. Restarts aren't cheap, and I want costs visible:

```rask
// Explicit restart loop
mut restart_count = 0
loop {
    let h = spawn(|| { worker_task() })
    match h.join() {
        void  => { break }
        Error as e => {
            restart_count += 1
            if restart_count > 5 { return RestartError.TooMany }
            println("Restarting after error: {e.message()}")
        }
    }
}
```

That's intentionally explicit. Supervision is still there—just as library code, not language magic.

### What Conflicts with Rask

1. **Hidden costs** - Automatic restart happens invisibly
2. **Global analysis** - Supervision trees require whole-program process graph tracking
3. **Implicit propagation** - Process linking and failure cascades are magical
4. **Non-local behavior** - Changing a supervisor affects distant child processes

### What Rask Chose Instead

Supervision works fine as a library:

```rask
let sup = Supervisor.new()
sup.spawn_child("worker", || worker_task())
sup.spawn_child("logger", || logger_task())
sup.run()  // Monitors and restarts
```

I considered making it a `using supervisor { }` block, but supervisors typically run for the lifetime of the application. Scoped blocks cleanup on exit. Wrong model.

Also, how would the supervisor know which spawns to monitor? All of them? That breaks explicit tracking. Same reason TaskGroup is a struct and not a `with` block—you need explicit control over which tasks join.

---

## Scope Functions

**Looked at:** Kotlin

Kotlin has `.let`, `.apply`, `.also` with implicit receivers—`it` or `this`. Terse and convenient.

Rask already has the pattern, just with explicit parameters:

```rask
let users = with db.read() as d { d.users.values().to_vec() }
with db.write() as d { d.users.insert(id, user) }
```

Compare `obj.let { it.field }` vs `with obj as d { d.field }`. The `with...as` syntax shows intent—you're entering a scoped access, not just "letting" something happen. The binding name is explicit. And `return`/`try`/`break` work naturally.

Could add Kotlin-style methods as library code if needed—no parser changes required. But `with` covers the core use case.

---

## Lifetimes

**Looked at:** Rust

Rust's lifetime annotations are precise and powerful. But they break local analysis.

To verify a function is safe, you need to understand all lifetime parameters, how they relate, how callers will instantiate them, what constraints propagate up. This cascades—add one `&'a` and half your codebase needs annotations.

I chose block-scoped borrowing only:

```rask
func process(data: Vec<u8>) {
    let view = data.slice(0, 10)  // Borrow
    use(view)
    // Borrow ends
}
```

References can't escape the block. No annotations needed. Compiler verifies safety by looking at the block—that's it.

Tradeoff: Rust lets you return references and build complex borrowing graphs. Rask says clone or restructure. More `.clone()` calls, but I think that's better than `<'a, 'b, 'c>` everywhere.

---

## Async/Await

**Looked at:** Rust, JavaScript, C#, Python

Async/await is the standard for concurrent I/O. Mark functions `async`, add `.await` at call sites. Widely understood model with ecosystem support.

I'm not using it. Here's why.

### Function coloring splits ecosystems

Async/await creates two worlds:

```rust
// Rust - two different functions
async fn fetch() -> Result<Data>  // Returns Future<Result<Data>>
fn fetch_sync() -> Result<Data>   // Returns Result<Data>

// Can't mix them
fn sync_code() {
    let data = fetch().await?;  // ERROR: can't await in non-async
}
```

Libraries duplicate their entire API (sync and async versions). Code that works in one world doesn't work in the other. You commit to async or sync upfront and it cascades through your codebase.

I want one function that works everywhere.

### Different return types force duplication

In async/await, `async fn` returns `Future<T>`, not `T`. The type system treats them as separate:

```rust
let data: Data = fetch_sync()?;      // Returns Data
let data: Data = fetch().await?;     // Returns Future<Data>, must await

// Can't unify - they're different types
```

You need two implementations because the types are incompatible.

Rask uses the same return type regardless of context:

```rask
func fetch() -> Data or Error {
    let response = try http_get(url)
    return parse(response)
}

// Works in sync context (blocks thread)
let data = try fetch()

// Works in async context (pauses task)
using Multitasking {
    let data = try fetch()
}
```

Same function. Same signature. Runtime decides execution strategy.

### Syntactic noise dominates code

Every I/O operation needs `.await`:

```rust
// Rust async
let user = fetch_user(id).await?;
let posts = fetch_posts(&user).await?;
let comments = fetch_comments(&posts).await?;
```

That's 100% ceremony overhead. In typical async code, `.await` appears on 30-50% of lines.

```rask
// Rask
let user = try fetch_user(id)
let posts = try fetch_posts(user)
let comments = try fetch_comments(posts)
```

No `.await` needed. Just call the function.

### What Rask Chose Instead

One function definition that adapts to context:

```rask
func fetch_user(id: u64) -> User or Error {
    let response = try http_get(format("/users/{id}"))
    return parse_user(response)
}

// Sync mode - blocks thread
func main() {
    let user = try fetch_user(42)
}

// Async mode - pauses task
func main() {
    using Multitasking {
        spawn(|| { fetch_user(42) }).detach()
    }
}
```

`http_get()` checks the runtime context internally. If we're in a `Multitasking` context, it issues non-blocking I/O and yields the task. Otherwise, it blocks. The function signature doesn't change—the execution strategy does.

### The Transparency Tradeoff

Async/await shows suspension points explicitly (`.await`). Rask makes them implicit.

Does this violate transparency? Yes and no.

**What's hidden:** Pause points aren't in the code (unless you use IDE annotations).

**What's visible:** The `using Multitasking { }` at the top tells you I/O will pause. You know the execution model upfront.

**Why I chose this:** Function coloring is worse than implicit pausing. Async/await's ecosystem split, library duplication, and ceremony tax outweigh the benefit of explicit `.await`. Transparency of cost doesn't mean every small cost needs ceremony—I want major architecture decisions visible (spawn, threading, `Multitasking`), not every I/O call annotated.

Plus, IDEs can show pause points as ghost annotations. The information is available without syntax.

**Metrics:**
- Syntactic Noise: 0.15 (Rask) vs 0.50 (async/await)
- Ergonomic Delta: 1.1 vs Go (async/await would be 1.5)
- Function coloring: None (Rask) vs Yes (async/await)
- Transparency: 0.85-0.90 (Rask, with IDE) vs 0.95 (async/await)

I chose ergonomics over explicit visibility. The 5-10% transparency gap is worth the 3x reduction in ceremony.

### I/O Visibility Through Tooling

To address the transparency gap, the compiler will track which functions do I/O (transitively) and use that for:

**IDE annotations:**
```rask
let data = try file.read()         // 🔄 I/O operation
let user = try fetch_user(id)      // 🔄 performs I/O
let result = parse(data)           // (no marker)
```

**Compiler warnings:**
```rask
func main() {
    for i in 0..10000 {
        let data = try http_get(url)
        // ⚠️ I/O in loop without Multitasking (will block thread 10k times)
    }
}
```

**Generated docs:**
```
fetch_user(id: u64) -> User or Error
🔄 Performs I/O (network request)
```

Information without enforcement. The compiler knows which functions do I/O, but doesn't force it into the syntax or type system. Same clean code, better tooling.

One function that works everywhere, no ecosystem split, no `.await` noise—that's worth relying on IDE support for pause point visibility. 

---

## Affine Task Handles

**vs:** Go's fire-and-forget

Go lets you spawn and forget: `go handleRequest(conn)` and the task disappears. Easy to write, also easy to leak goroutines or miss errors.

Rask requires handles to be joined or detached:

```rask
spawn(|| { work() }).detach()  // Explicit

let h = spawn(|| { compute() }
let result = try h.join()

spawn(|| { work() }  // Compile error: unused TaskHandle
```

Compiler catches forgotten tasks. Six extra characters (`.detach()`) to prevent real bugs.

---

## Result Types vs Exceptions

**vs:** Java, C++, Python, C#

Most languages use exceptions. Hidden control flow—you don't know what throws without reading docs or source.

```java
// Java - where does this throw?
User user = database.getUser(id);
processUser(user);
sendEmail(user);
```

Rask puts errors in the type system:

```rask
let user = try database.get_user(id)     // -> User or DbError
try process_user(user)
try send_email(user)
```

Signature tells you it can fail. `try` shows propagation. All paths visible.

More `try` keywords, but errors should be visible.

---

## `const` for Local Bindings

**vs:** Rust's `let`/`let mut`, Zig's `const`/`var`

Rask shipped with `const x = 1` immutable, `mut x = 1` mutable for a while. The argument was semantic: "const" means constant, and `let` reads like "let it vary".

That lost to a friction argument. The immutable binding is the default and the single most common statement in the language, and `const` is five characters against `mut`'s three — the keyword lengths punish exactly the binding people should reach for. Zig has the same inversion with `const`/`var` and gets the same critique. So: `let x = 1` immutable, `mut x = 1` mutable, both three letters, mutation stays the marked case. I looked at `set`, `def`, and `fix` too — `set` reads as mutation, `def` smells like a Python function, `fix` is obscure. `let` is the least bad, and it's what half the industry already types for an immutable binding.

`const` survives only at module level, for package-level constants. In a function body it's a parse error pointing to `let`. And `let mut x` gets its own error: drop the `let`.

---

## General Macro System

**Looked at:** Rust (declarative + procedural macros), Zig (comptime), Nim (templates + macros)

Rust macros solve real problems: variadic arguments (`format!`, `vec!`), conditional compilation (`cfg!`), code generation (`derive`), and domain-specific syntax. But they're a second language with their own syntax, error messages, and learning curve. `macro_rules!` is notoriously hard to read. Procedural macros require a separate crate. Both make code harder to analyze.

I'm not adding a macro system. Here's what covers those use cases instead.

### Variadic arguments

`format()`, `println()`, and `print()` are compiler-known functions (`struct.modules/BF2`). The compiler parses templates, type-checks arguments, and generates specialized code. No variadic mechanism needed—the compiler handles these directly.

For user-defined variadic functions: Rask doesn't have them. Pass a `Vec` or use generic overloads. Variadics complicate type checking and make call-site errors harder to diagnose.

### Code generation

`comptime for` + `std.reflect` handles serialization, encoding, and struct-walking patterns (`ctrl.comptime/CT48-CT54`). Build scripts handle external codegen (protobuf, schemas). `comptime if cfg.*` handles conditional compilation.

The serialization story in particular is where other languages reach for macros. Rask covers it with three composable primitives:

- `comptime for field in reflect.fields<T>()` — unroll struct layout at compile time
- `value.(field.name)` — comptime-resolved field access
- Field annotations (`@default`, `@no_serialize`, `@no_encode`) — per-field metadata

Combined, these auto-derive `Encode` / `Decode` for any struct. Schema evolution (add/remove fields) works because field names are embedded by the comptime iteration. This is the Rust `#[derive(Serialize, Deserialize)]` story — without a procedural macro crate, a second language, or anything the formatter, linter, and IDE can't already see. See [stdlib/encoding.md](stdlib/encoding.md).

### Domain-specific syntax

Not supported. DSLs in macros look concise but break tooling—IDEs, formatters, linters all need macro expansion to understand the code. Write functions instead.

### What I chose instead

Compiler-known functions for the handful of variadic built-ins. Comptime for compile-time computation. Build scripts for complex codegen. No user-extensible syntax transformation.

This means Rask can't express `vec![1, 2, 3]`—you write `Vec.from([1, 2, 3])`. I think that's fine.

---

## Default Trait

Removed (it existed briefly with auto-derived universal zeros: `0` for ints, `false` for bool, `""` for string). Universal zeros are Go zero-values by another name — a back door around all-fields-required construction, handing out values nobody chose. Declared field defaults replaced it (`type.structs/FD1–FD6`): defaulted fields are omittable at construction, `Config {}` constructs the default when every field declares one, and a defaultless field is a compile error naming the field. One mechanism feeds construction, decode-missing-fields, and fresh values. No API ever used `T: Default` as a generic bound; if a constructible-empty bound is needed someday, it can return from usage evidence.

## From/Into Conversion Traits

Rust's most hand-implemented trait, deliberately absent. Its three jobs dissolve at the language level: error conversion for `?` (Rask's `try` widens error *unions* structurally — the `impl From<LibError> for MyError` ceremony class never exists), flexible string parameters (one `string` type — no `String`/`&str`/`Cow` to abstract over), and general conversion (the residue, covered by opt-in `Convert<From, To>`). Rust immigrants will ask; this is the answer.

## Higher-Kinded Types

**Looked at:** Haskell, Scala

HKT lets a type parameter itself take a parameter — `F<_>`. That's what buys you one `map` that works for `Vec`, `T?`, and `T or E` at once, and lets you *name* Functor/Monad/Traversable as reusable abstractions instead of re-writing the same shape per container. It's genuinely powerful and it reads well at the definition site. I'm still not adding the open form.

The felt value splits in two, and only one half is expensive. Consistency — `map`, `and_then`, `fold` meaning the same thing with the same shape on every container — is the part people actually feel, and I already bought it by convention (the stdlib naming pass, #303, plus `canonical-patterns.md` and lints). Rust's per-container impls drift because nothing forces them; Rask's can't, because the lint won't let them. Same user-visible payoff, no kind system. The expensive half is *abstraction over the container* — writing `process<F>` once — and systems code hits that far less than functional library code does.

Where a real "write it once" need shows up, the Rask-shaped tools are:

- **Specialize the monads that earn it into first-order syntax.** `try` *is* the error/option monad's bind, baked into a keyword. `Sequence` *is* the list monad, specialized to a fusing function type. When a third instance pays its rent, it gets its own specialization — the general `Monad` never gets a name.
- **Associated types** (deferred, not rejected — see [types/generics.md](types/generics.md)) cover most of the "powerful function, fewer lines" cases at `*`-level, with no kind polymorphism and no inference blowup. That's the borrow worth promoting off the deferred list, not HKT. (This used to cite a result-polymorphic `collect` as the motivating case. It isn't one any more — sequence terminals name what they build, `to_vec` / `to_map` / `join`, and the trait went away with them: `type.sequence/SEQ31`.)

What stays out is user-declarable Functor/Monad. A `where F: Monad` bound in a diagnostic is exactly the abstract spec-speak Rask is built against, and Monad-as-effect is the function coloring I already deleted (see Algebraic Effects, above). So: borrow the data-container ergonomics, refuse the effect-abstraction machinery. If Rask grew the kind of functional tower that truly needs HKT, it stopped being Rask somewhere earlier.

## Summary

Common thread: I optimize for transparency and local reasoning, but not at the cost of ergonomics.

**Rejected:** Exceptions, async/await keywords, lifetime annotations, automatic supervision, algebraic effects, implicit receivers, general macro system, higher-kinded types.

**Chose:** Result types, green tasks without coloring, block-scoped borrows, library patterns, explicit parameters.

**Key tradeoff:** Async/await is 5-10% more transparent (explicit pause points) but 3x noisier and splits ecosystems. I chose clean syntax with IDE-based transparency. When there's a conflict between "visible in syntax" and "simple to use," I lean toward simplicity—as long as the information is available through tooling.

Not a judgment on other languages—Kotlin, Erlang, OCaml, Rust made different tradeoffs for different goals. Those features work well in their contexts.

I'm targeting systems programming where costs must be visible, analysis must be local, safety must be structural. But "visible" doesn't mean "ceremony"—major decisions like `using Multitasking { }` are in the code, while pause points can be shown by IDEs. The features I rejected would either add ceremony without value (async/await, lifetimes) or hide costs through magic (effects, exceptions, supervision). Rask is explicit where it matters, simple where it doesn't.
