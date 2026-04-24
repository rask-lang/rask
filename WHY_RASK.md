# Why Rask?

Rask asks one question: can you get memory safety without lifetime annotations or garbage collection?

The bet is yes — if you make one structural change: **references can't be stored**. They exist for the duration of an expression or block, then they're gone. No lifetimes to annotate, no GC to pause, no reference counting to cycle-collect.

This is a research language. It might not work out. Here's why I think it's worth trying.

---

## The Gap

There's a space in programming that nobody occupies well.

**Rust** is safe, but it demands you prove safety to the compiler. Lifetime annotations leak into signatures. The borrow checker rejects valid programs. Graphs require `Rc<RefCell<T>>` — reference counting plus interior mutability, which is the complexity you were trying to avoid. In practice, many Rust application developers sidestep this by using `String` everywhere, `Arc` liberally, and `anyhow` for errors — pragmatic Rust that avoids most lifetime annotations. Rask's bet is that "just use owned types" shouldn't be a workaround; it should be the model.

**Go** is memory-safe (GC prevents use-after-free) and productive. But the safety has gaps: data races are possible and only caught by a runtime detector, nil panics crash at runtime instead of compile time, goroutines leak silently, and there's no way to enforce resource cleanup — forgetting to close a file is invisible. GC pauses trade predictable latency for convenience.

**Zig** is transparent and gives you real control. But memory management is manual. Use-after-free is your problem. It's honest about the tradeoff — you get power, you accept risk.

**Swift** has value types and ARC. But ARC adds overhead on every assignment, and cycles require weak references (bringing back the complexity). It's also practically tied to Apple's ecosystem.

**C++** has everything. Nothing is safe by default. Enough said.

**Hylo** is Rask's closest relative — same theoretical foundation (mutable value semantics, from Dave Abrahams' research at Google). I address the overlap [below](#rask-vs-hylo).

Rask's position: safety that emerges from structure, not from annotations you write or a runtime that manages memory for you.

---

## The Core Bet

Most of Rust's complexity comes from one feature: storable references. Once a reference can live inside a struct or be returned from a function, the compiler needs lifetime annotations to prove it won't outlive its source. That proof obligation cascades — lifetime parameters appear in function signatures, trait bounds, where clauses, and generic constraints.

Remove storable references, and the entire lifetime system becomes unnecessary.

In Rask, references are temporary. You can borrow a value for an expression or a `with` block, but you can't store that borrow anywhere. For complex data structures — graphs, caches, entity systems — you use handles: validated indices into typed pools, with generation counters that catch stale access.

Here's what that looks like in practice. A Rust function that processes borrowed data:

```rust
fn process_entries<'a>(entries: &'a [Entry], filter: &'a str) -> Vec<&'a str> {
    entries.iter()
        .filter(|e| e.tag == filter)
        .map(|e| e.name.as_str())
        .collect()
}
```

The same thing in Rask:

```rask
func process_entries(entries: Vec<Entry>, filter: string) -> Vec<string> {
    mut result = Vec.new()
    for entry in entries {
        if entry.tag == filter: result.push(entry.name)
    }
    return result
}
```

No `'a`. No lifetime bounds. No `.clone()` either — `string` is an immutable, refcounted Copy type (16 bytes). It copies like an integer. The compiler elides the atomic refcount operations in ~70-80% of cases, so the runtime cost is near-zero.

This approach is called *mutable value semantics* (MVS) — ban aliasing instead of banning mutation. [Hylo](https://www.hylo-lang.org/) pioneered the formal model. Rask builds on it with practical extensions for problems that pure MVS hits at scale.

---

## What's Genuinely Novel

Most of Rask is assembled from existing ideas. I'm not claiming otherwise.

**Borrowed directly:**
- Ownership and move semantics — Rust
- Simplicity as a design goal — Go
- Compile-time execution (`comptime`) — Zig
- Value semantics as the foundation — Hylo
- Generational references as a concept — Vale
- Deferred cleanup (`ensure`) — Swift's `defer`, Go's `defer`
- Result types and error propagation — Rust, ML family

**What Rask combines differently:**
- **Block-scoped references as the entire model** — not a restriction layered on top, but the foundation everything else builds on
- **Two-tier borrowing** — fixed-layout sources (struct fields, arrays) keep references to block end; growable sources (Vec, Map) release at the semicolon. The rule is simple: "can it reallocate?"
- **`with` blocks** — scoped mutable access with full control flow. `break`, `return`, and `try` propagate naturally because `with` is a real block, not a closure. Single-expression access works inline: `shared.read().timeout` holds the lock for just the expression
- **Immutable refcounted strings** — `string` is Copy (16 bytes), immutable, atomically refcounted. Copies like a primitive. The compiler elides refcount ops in most cases (`comp.string-refcount-elision`). Go's string ergonomics without GC
- **Context clauses** — `func damage(h: Handle<Entity>) using Pool<Entity>` declares pool dependencies; the compiler threads them implicitly. Same mechanism for custom allocators: `using Allocator` threads an arena or fixed-buffer allocator without polluting every function signature
- **Custom allocators** — `Arena`, `FixedBuffer`, scoped blocks (`using Arena.scoped(1MB) { ... }`). Data can't escape the arena scope — compiler-enforced, no lifetime annotations. Global allocator is zero-sized and the default
- **Errors without wrappers** — `T or E` is a builtin sum type. You return bare values, the compiler picks the branch by type. No `Ok(x)` / `Err(e)`. Every `E` must implement `ErrorMessage`. `@message` generates the method from variant templates. `try ... else |e|` chains transformation with propagation. See below
- **Option isn't an enum** — `T?` is a builtin status type with operator-only grammar (`?`, `?.`, `??`, `!`, `== none`, `try`). Match on `T?` is a compile error. Flow narrowing on `const` bindings. Kotlin/TypeScript nullable typing, not Rust Option
- **Must-use task handles** — `spawn(|| { work() })` returns a handle that must be joined or detached. Forgetting is a compile error
- **No call-site coloring** — I/O pauses green tasks transparently. No `async`/`await` at call sites. But `using Multitasking` propagates through signatures (scope-level coloring) — you don't write `.await`, but you do declare the capability. This is a deliberate tradeoff: uncolored calls, colored signatures

### Rask vs. Rust

Rask shares a lot of surface with Rust: move semantics, `T or E`, `try`, structural traits, explicit ownership. The thing I hear most often is "this is just Rust with syntax sugar and no lifetimes — which makes it less powerful." The syntax similarity is real. The underlying mechanics are not.

**Errors don't use wrappers.** Rust forces `Ok(x)` / `Err(e)` because `Result` is an ordinary enum. Rask made `T or E` a builtin specifically to drop the wrappers — you return bare values, the compiler picks the branch by type.

```rask
func divide(a: f64, b: f64) -> f64 or DivError {
    if b == 0.0 { return DivError.ByZero }  // E branch, by type
    return a / b                             // T branch, by type
}
```

The disjointness rule (T ≠ E) is the price — enforced via Rask's nominal-vs-alias split. No wrapper keyword, no variant constructors, no `unwrap()`.

**Option isn't an enum.** Rust's `Option<T>` is literally `enum { Some(T), None }` — you `match` or `if let`. In Rask, `T?` is a builtin status type with an operator-only surface. `match` on `T?` is a compile error.

```rask
const name = user?.profile?.display_name ?? "Anonymous"
if user == none { return default() }
if user? as u { greet(u) }
```

This is closer to Kotlin's `T?` or TypeScript's `T | undefined` than Rust's `Option`.

**Narrowing is flow typing, not pattern matching.** `if x? { use(x) }` narrows `x` to `T` inside the block because `const` bindings can't be reassigned. No destructuring pattern. The compiler uses a fact it already knows (constness) to refine types in branches.

**Errors are bounded.** Every `E` in `T or E` must implement `ErrorMessage` — a structural trait requiring `func message(self) -> string`. Primitives can't be errors. `r!` always produces a useful panic message. Rust has no equivalent constraint; any type can be a `Result` error.

**`@message` is builtin.** Rask generates the `message()` method from per-variant templates:

```rask
@message
enum FetchError {
    @message("not found: {pkg}") NotFound(pkg: string),
    @message("checksum mismatch") Checksum,
}
```

No `thiserror` macro, no hand-written match.

**`try ... else |e|` block form.** Propagate and transform in one step:

```rask
const data = try fs.read(path) else |e| context("reading {path}", e)
```

Rust needs `fs::read(path).map_err(|e| ...)?`.

**Linear resources in errors.** If an error variant carries a linear resource (file handle, socket), both branches of `T or E` must consume it. Rust's `Drop` runs automatically but can't return errors during cleanup — Rask's explicit consumption lets you `try file.close()` in the error arm and propagate if it fails.

**Beyond errors:**

| Capability | Rust | Rask |
|---|---|---|
| Short borrowed strings | `&str` with `'a` | not needed — `string` is Copy |
| Long-lived strings | `String` (owned) | `string` (Copy, refcounted, 16 bytes) |
| Graphs / cycles | `Rc<RefCell<T>>` | `Pool<T>` + `Handle<T>` (generation-checked, builtin) |
| Cleanup | `Drop` (implicit, can't error) | `@resource` + `ensure` (explicit, composes with errors) |
| Scoped mutation | closure-based | `with` block (real control flow) |
| Implicit state | thread-locals or params | `using Pool<T>`, `using Allocator` (context clauses) |
| Custom allocators | type parameter (`Vec<T, A>`) | `using Allocator` context (zero-sized default) |
| Concurrency | `async`/`await` (call-site coloring) | green tasks, `using` signature coloring, must-use handles |
| Zero-copy returns | lifetime-generic | not supported — return owned values or work within `with` scope |
| Zero-cost access | lifetime-generic borrows | `read`/`mutate` params, `with` blocks, expression-scoped borrows |

Rust can return borrowed data through lifetime-generic APIs — zero-copy parsers, borrowed iterators, view types. Rask can't return borrows, but gets zero-cost access through scoping: `with` blocks, parameter modes, and expression-scoped borrows cover the common cases without lifetime annotations.

**Honest caveat:** Pragmatic Rust narrows this gap. If you use `String` everywhere, `Arc` for sharing, `anyhow` for errors, and avoid lifetime-heavy APIs — the day-to-day experience is less painful than the table above suggests. The remaining gap is real (wrappers, function coloring, `Rc<RefCell<T>>` for graphs, `Pin` for async state) but smaller than comparing textbook Rust to Rask.

It's a different point in the design space, not a subset of Rust.

### Rask vs. Hylo

Hylo and Rask share the same foundation: mutable value semantics, no storable references, single ownership. This is the most common question I get — why not just use Hylo?

Hylo is more formal. It comes from Google Research, has academic publications behind it, and aims to prove MVS correct as a memory model. That's valuable work.

Rask is more pragmatic. I hit problems that pure MVS doesn't solve and built extensions:

- **Pool+Handle for graphs and cycles.** Hylo doesn't have a built-in answer for self-referential structures. Entity-component systems, dependency graphs, caches with cross-references — these need some form of indirect access. Rask provides typed pools with generational handles.
- **`with` blocks with control flow.** Hylo uses subscripts (similar to computed properties) for scoped access. These are closures underneath, which means you can't `return` from the enclosing function, `break` from a loop, or `try` an error inside them. Rask's `with` is a real lexical block — all control flow works.
- **Context clauses.** When every handle function needs a pool parameter, you end up threading pools through 15 layers of calls. `using Pool<T>` makes this implicit where it's noise and explicit where it matters (public API boundaries).
- **Custom allocators.** Arena, FixedBuffer, and scoped allocation blocks — all using the same `using` context mechanism as pools. Compiler-enforced scope restriction replaces lifetime annotations. Hylo doesn't specify custom allocator support.
- **Concurrency model.** Green tasks, channels, must-use handles, thread pools. Hylo doesn't specify a concurrency story yet.

Hylo may well end up being the better language. It has stronger theoretical backing. Rask's bet is that the practical extensions matter more than formal elegance for building real software — and that you can only discover the right extensions by trying to build real programs.

---

## Who This Is For

**Rust users who write application code** — web services, CLIs, data pipelines — and found that lifetime annotations don't pay for themselves outside of library code. If you've wrapped something in `Rc<RefCell<T>>` just to make the borrow checker happy, or reached for `String` everywhere to avoid `&str` lifetimes, Rask is exploring whether the ownership model can be simpler without giving up safety.

**Go users who feel the gaps** — data races the detector misses, nil panics in production, leaked goroutines, resources that stay open because `defer` is optional. If you want those closed at compile time without switching to Rust's annotation model.

**Researchers and language enthusiasts** interested in whether mutable value semantics can scale to real programs. Hylo is the formal exploration. Rask is the pragmatic one — less theory, more "does this actually work for a web server?"

This is a narrow audience. Most developers don't need a new language. If Go or pragmatic Rust works for you, it's the better choice — mature ecosystem beats better theory every time. Rask is for people who've hit the specific walls described above and want to see if there's a way through.

## Who This Isn't For

**OS kernel and driver developers.** You need raw pointer manipulation, memory-mapped I/O, and control over every allocation. Rask has `unsafe` blocks but isn't optimized for code that's 50% unsafe.

**Nanosecond-budget hot paths.** Handle validation has overhead on every access (generation check + bounds check, estimated ~1-2ns but not yet benchmarked). For audio engines, HFT, or inner loops processing many entities per frame, that overhead matters. The workaround is batch processing — which works, but adds friction.

**Anyone who needs production-ready today.** Rask is in design phase. The interpreter runs programs. There's no optimizing compiler. Don't build your startup on this.

**People who like Rust's borrow checker.** It's genuinely more powerful than what Rask offers. If lifetimes and `'a` annotations feel like useful documentation to you rather than overhead, Rust is the better choice. Rask trades expressiveness for simplicity — that's not universally better.

---

## What You Give Up

### Clone calls

Types ≤16 bytes with all-Copy fields copy implicitly. Larger types move on assignment. When you need a copy of a moved type, you write `.clone()`.

```rask
// Strings (16 bytes, Copy) — implicit
const name = user.name

// User struct (>16 bytes) — explicit
const user_copy = user.clone()
db.insert(id, user_copy)
```

This is deliberate. For types above the copy threshold, `.clone()` marks a decision: you're choosing to duplicate data rather than transfer ownership. Even when the clone is cheap (a struct of Copy fields is just a memcpy + refcount bumps), the explicit call keeps the cost visible. The 16-byte threshold matches register-passable size on most ABIs — below that, copies are genuinely free.

Clone calls concentrate at collection API boundaries, roughly 1-2% of lines. With Copy strings, everyday code rarely needs them.

### Handle indirection

Parent pointers, back-references, and cross-references become handles into pools instead of direct pointers.

```rask
struct TreeNode {
    parent: Handle<TreeNode>?    // not a reference — a validated index
    children: Vec<Handle<TreeNode>>
    value: string
}
```

Each handle access validates a generation counter. The overhead is estimated at ~1-2ns per access but hasn't been benchmarked in the current runtime — the real number could be higher with cache misses on pool metadata. In application code (web services, CLIs, game logic outside hot loops) this is likely invisible. In tight inner loops processing thousands of entities per frame, it's a real concern. The workaround — copy data out, batch-process, write back — is exactly the kind of restructuring that adds friction.

### No zero-copy returns

You can't return a reference to data inside a container. Functions that extract data return owned values — copies for Copy types, clones for the rest.

In practice, this costs less than it sounds. Strings are Copy (16 bytes, refcount bump). Primitives are Copy. Most function returns in application code are already owned values. The cost concentrates on extracting large non-Copy structs from collections — and that's where `.clone()` makes the cost visible.

What IS zero-cost in Rask:
- `read`/`mutate` parameter modes — functions borrow without copying
- `with` blocks — scoped mutable access to container internals, no copy
- Arena scopes — allocate into a region, work zero-copy inside, copy results out at the boundary. Parsers, binary protocols, request handling
- Iterator chains in a single expression — compiler inlines, no intermediate allocations
- Monomorphization — generics compiled to specialized code, no vtable
- Comptime — abstraction eliminated at compile time

---

## What You Get

### Clean function signatures

No lifetime parameters, no where clauses, no borrow annotations. Function signatures describe the interface, not the memory model.

```rask
func search(entries: Vec<Entry>, query: string) -> Vec<Entry>
```
```rust
// Lifetime-heavy Rust (returning borrowed data):
fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry>

// Pragmatic Rust (returning owned data — avoids lifetimes):
fn search(entries: &[Entry], query: &str) -> Vec<Entry>
```

The Rask version is always the simple case. In Rust, you get the simple version too if you're willing to clone — but the lifetime-annotated version is what Rust enables and encourages for performance. Rask takes the position that the owned-data version is the right default.

### Copy strings

`string` is immutable, refcounted, and Copy (16 bytes). It copies like an integer — no `.clone()`, no GC, no COW hidden costs. The compiler elides atomic refcount operations in ~70-80% of cases.

```rask
const name = user.name        // just copies — 16 bytes, like copying two pointers
const greeting = "hello {name}"
```

Go gives you this with garbage collection. Rask gives you this with deterministic cleanup and near-zero overhead.

### Custom allocators

Arena-scoped memory, fixed-buffer allocation for embedded, request-scoped scratch space — all using the same `using` context mechanism as pools. No Zig-style parameter threading, no lifetime annotations.

```rask
func handle_request(req: Request) -> Response {
    using Arena.scoped(256.kilobytes()) {
        const params = parse_query(req.url)
        const body = try parse_json(req.body)
        return Response.json(process(params, body))
    }
    // arena freed — all scratch memory gone
}
```

### Deterministic cleanup

Values are freed when their owner goes out of scope. I/O resources use `ensure` for guaranteed cleanup. No GC pauses, no unpredictable latency.

```rask
func process(path: string) -> void or IoError {
    const file = try fs.open(path)
    ensure file.close()           // runs on every exit path
    // ...work with file...
}
```

### No call-site coloring

I/O operations pause green tasks transparently. No `.await` at call sites.

```rask
func fetch_data(url: string) -> string or HttpError {
    const resp = try http.get(url)   // pauses the task, not the thread
    return resp.body()
}
```

The catch: functions that spawn tasks need `using Multitasking` in their signature, and that propagates to callers. You don't color call sites, but you do color signatures with capability requirements. It's less invasive than `async`/`await` but it's not free.

### Error context without boilerplate

`try...else` chains error context inline. `@message` generates Display-like methods from annotations. Private functions can use `or _` to let the compiler infer the error union.

```rask
func load_config(path: string) -> Config or _ {
    const text = try fs.read(path) else |e| context("reading {path}", e)
    const config = try parse(text) else |e| context("parsing {path}", e)
    return config
}
```

### Compiler-enforced resource cleanup

Files, sockets, and connections are linear types — the compiler rejects code that forgets to consume them.

```rask
func broken(path: string) -> void or IoError {
    const file = try fs.open(path)
    // compile error: `file` must be consumed (closed, passed, or ensured)
}
```

### Linear compilation

All analysis is function-local. No whole-program inference. Changing a function body doesn't recompile callers if the signature hasn't changed.

---

## Side by Side

Importing users from a CSV file. Same task, same structure, different languages.

**Rask:**
```rask
func import_users(path: string, mutate db: Database) -> i64 or ImportError {
    const file = try fs.open(path)
        else |e| ImportError.FileError(path, e)
    ensure file.close()

    mut imported = 0
    for line in file.lines() {
        const text = try line else |e| ImportError.ReadError(e)
        const parts = text.split(",")
        if parts.len() != 2 {
            return ImportError.BadFormat("expected name,email on line {imported + 1}")
        }
        db.add_user(parts[0].trim(), parts[1].trim())
        imported += 1
    }
    return imported
}
```

**Rust:**
```rust
fn import_users(path: &str, db: &mut Database) -> Result<i64, ImportError> {
    let file = File::open(path)
        .map_err(|e| ImportError::FileError(path.to_string(), e))?;
    let reader = BufReader::new(file);
    // file closed by Drop — close errors silently dropped

    let mut imported: i64 = 0;
    for line in reader.lines() {
        let text = line.map_err(|e| ImportError::ReadError(e))?;
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() != 2 {
            return Err(ImportError::BadFormat(
                format!("expected name,email on line {}", imported + 1),
            ));
        }
        db.add_user(parts[0].trim().to_string(), parts[1].trim().to_string());
        imported += 1;
    }
    Ok(imported)
}
```

No single line is revolutionary. The differences are incremental:

- `try ... else |e|` vs `.map_err(|e| ...)?` — same idea, less nesting
- `return ImportError.BadFormat(...)` vs `return Err(ImportError::BadFormat(...))` — no wrapper
- `return imported` vs `Ok(imported)` — no wrapper
- `parts[0].trim()` vs `parts[0].trim().to_string()` — Copy strings
- `"line {imported + 1}"` vs `format!("line {}", imported + 1)` — string interpolation
- `ensure file.close()` vs implicit Drop — Rask propagates close errors; Rust silently drops them

These compound. The Rask version has less noise on every line, and `ensure` is actually safer — Rust's `Drop` can't return errors, so a failing close is invisible. Using `anyhow` in Rust would shorten the error handling but lose the typed error variants that Rask keeps for free.

---

## Status

Design phase. Working interpreter. All five validation programs exist as source; three run in the interpreter (grep clone, game loop, text editor). The HTTP server and sensor processor exercise concurrency and resource types — the areas with the most unresolved design questions. No optimizing compiler; handle overhead and string refcount elision are estimated, not measured.

The question being answered right now is whether the approach is viable — not whether the implementation is ready.

**Dig deeper:**
- [CORE_DESIGN.md](specs/CORE_DESIGN.md) — full design rationale
- [specs/](specs/) — formal specifications
- [examples/](examples/) — working programs
- [Language Guide](LANGUAGE_GUIDE.md) — complete feature walkthrough
