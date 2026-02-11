# Design Rationale

Why I made certain design choices. Mostly about what I didn't add from other languages.

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
const file = try open(path)    // I/O here
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

Rask's safety is structural—linear resources, affine handles, block-scoped cleanup. `ensure` blocks run on all exits, LIFO order, guaranteed.

Effects introduce non-local jumps. Does the handler run before or after cleanup? What if an effect jumps past an `ensure` block? You need to reason about effect boundaries intersecting with resource scopes. Structural guarantees become ambiguous.

### They're function coloring in disguise

I don't want `async`/`await` because it splits the ecosystem. Effects do the same—functions that raise effects become constrained. Can't use them without handlers. Same problem, hidden in effect types instead of keywords.

### Errors become invisible

Right now `const value = try some_operation()` tells you it can fail. With effects, handlers catch errors before they reach you. Error flow becomes hidden in handler chains.

### What I chose instead

Result types for errors. `using Multitasking { }` for async I/O (tasks pause automatically). `ensure` blocks for cleanup. Function parameters or `with` clauses for context. `Shared<T>` for shared state.

More verbose in places, but every cost is visible and every path is local. Effects give you less ceremony—I chose transparency. More `try` keywords in error-heavy code, but errors should be visible.

If Rask had effects, it wouldn't be Rask.

---

## Automatic Supervision

**Looked at:** Erlang, Elixir

Erlang's supervision trees are great—processes automatically restart when they crash. "Let it crash" philosophy. But automatic restart is a hidden side effect. Restarts aren't cheap, and I want costs visible:

```rask
// Explicit restart loop
let restart_count = 0
loop {
    const h = spawn { worker_task() }
    match h.join() {
        Ok(()) => { break },
        Err(e) => {
            restart_count += 1
            if restart_count > 5: return Err("too many restarts")
            println("Restarting after error: {e}")
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
const sup = Supervisor.new()
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
const users = db.read(|d| d.users.values().collect())
db.write(|d| { d.users.insert(id, user) })
```

Compare `obj.let { it.field }` vs `obj.read(|d| d.field)`. The parameter name shows intent—you're entering a read scope, not just "letting" something happen. More characters, but clearer. Parameter names also show up in stack traces when debugging.

Could add Kotlin-style methods as library code if needed—no parser changes required. But explicit parameters work well enough.

---

## Lifetimes

**Looked at:** Rust

Rust's lifetime annotations are precise and powerful. But they break local analysis.

To verify a function is safe, you need to understand all lifetime parameters, how they relate, how callers will instantiate them, what constraints propagate up. This cascades—add one `&'a` and half your codebase needs annotations.

I chose block-scoped borrowing only:

```rask
func process(data: Vec<u8>) {
    const view = data.slice(0, 10)  // Borrow
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
    const response = try http_get(url)
    return parse(response)
}

// Works in sync context (blocks thread)
const data = try fetch()

// Works in async context (pauses task)
using Multitasking {
    const data = try fetch()
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
const user = try fetch_user(id)
const posts = try fetch_posts(user)
const comments = try fetch_comments(posts)
```

No `.await` needed. Just call the function.

### What Rask Chose Instead

One function definition that adapts to context:

```rask
func fetch_user(id: u64) -> User or Error {
    const response = try http_get(format("/users/{id}"))
    return parse_user(response)
}

// Sync mode - blocks thread
func main() {
    const user = try fetch_user(42)
}

// Async mode - pauses task
func main() {
    using Multitasking {
        spawn { fetch_user(42) }.detach()
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
const data = try file.read()         // 🔄 I/O operation
const user = try fetch_user(id)      // 🔄 performs I/O
const result = parse(data)           // (no marker)
```

**Compiler warnings:**
```rask
func main() {
    for i in 0..10000 {
        const data = try http_get(url)
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
spawn { work() }.detach()  // Explicit

const h = spawn { compute() }
const result = try h.join()

spawn { work() }  // Compile error: unused TaskHandle
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
const user = try database.get_user(id)     // -> User or DbError
try process_user(user)
try send_email(user)
```

Signature tells you it can fail. `try` shows propagation. All paths visible.

More `try` keywords, but errors should be visible.

---

## const/let Semantics

**vs:** Rust's `let`/`let mut`

Rust: `let x = 1` immutable, `let mut x = 1` mutable.

Rask: `const x = 1` immutable, `let x = 1` mutable.

Why flip it? "Const" means constant. "Let" means let it vary. Semantics match the names.

Rust programmers will find this backwards. I think it's clearer for everyone else.

---

## Summary

Common thread: I optimize for transparency and local reasoning, but not at the cost of ergonomics.

**Rejected:** Exceptions, async/await keywords, lifetime annotations, automatic supervision, algebraic effects, implicit receivers.

**Chose:** Result types, green tasks without coloring, block-scoped borrows, library patterns, explicit parameters.

**Key tradeoff:** Async/await is 5-10% more transparent (explicit pause points) but 3x noisier and splits ecosystems. I chose clean syntax with IDE-based transparency. When there's a conflict between "visible in syntax" and "simple to use," I lean toward simplicity—as long as the information is available through tooling.

Not a judgment on other languages—Kotlin, Erlang, OCaml, Rust made different tradeoffs for different goals. Those features work well in their contexts.

I'm targeting systems programming where costs must be visible, analysis must be local, safety must be structural. But "visible" doesn't mean "ceremony"—major decisions like `using Multitasking { }` are in the code, while pause points can be shown by IDEs. The features I rejected would either add ceremony without value (async/await, lifetimes) or hide costs through magic (effects, exceptions, supervision). Rask is explicit where it matters, simple where it doesn't.
