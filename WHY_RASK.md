# Why Rask?

Rask asks one question: can you get memory safety without lifetime annotations or garbage collection?

The bet is yes — if you make one structural change: **references can't be stored**. They exist for the duration of an expression or block, then they're gone. No lifetimes to annotate, no GC to pause, no reference counting to cycle-collect.

This is a research language. It might not work out. Here's why I think it's worth trying.

---

## The Gap

There's a space in programming that nobody occupies well.

**Rust** is safe, but it demands you prove safety to the compiler. Lifetime annotations leak into every signature. The borrow checker rejects valid programs. Graphs require `Rc<RefCell<T>>` — reference counting plus interior mutability, which is the complexity you were trying to avoid. For application code (web services, CLIs, data pipelines), this proof burden often isn't worth the cost.

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
    const result = Vec.new()
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
- **Union error types** — `T or (IoError | ParseError)` with automatic widening. `try...else` chains error context inline: `try read(path) else |e| context("reading {path}", e)`. Private functions can use `or _` to let the compiler infer the error union from the body
- **Must-use task handles** — `spawn(|| { work() })` returns a handle that must be joined or detached. Forgetting is a compile error
- **No function coloring** — I/O pauses green tasks transparently. One concurrency model, no async/sync split

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

**Application developers** building web services, CLIs, data pipelines, game logic — programs where you want safety and predictable performance but don't need to control every byte.

**Rust developers** who found that lifetime annotations aren't worth the cost for application code. If you've ever wrapped something in `Rc<RefCell<T>>` just to make the borrow checker happy, Rask is exploring whether that detour was necessary.

**Go developers** who want the gaps closed. Compile-time nil safety, compile-time data race prevention, deterministic resource cleanup, no GC pauses — without giving up Go's readability.

**Researchers and language enthusiasts** interested in whether mutable value semantics can work for real programs. There aren't many data points yet. Rask is trying to be one.

## Who This Isn't For

**OS kernel and driver developers.** You need raw pointer manipulation, memory-mapped I/O, and control over every allocation. Rask has `unsafe` blocks but isn't optimized for code that's 50% unsafe.

**Nanosecond-budget hot paths.** Handle validation costs ~1-2ns per access (generation check + bounds check). For audio engines, HFT, or inner loops processing billions of items, that overhead matters. Copy data out and batch-process — or use a language with zero-cost references.

**Anyone who needs production-ready today.** Rask is in design phase. The interpreter runs programs. There's no optimizing compiler. Don't build your startup on this.

**People who like Rust's borrow checker.** It's genuinely more powerful than what Rask offers. If lifetimes and `'a` annotations feel like useful documentation to you rather than overhead, Rust is the better choice. Rask trades expressiveness for simplicity — that's not universally better.

---

## What You Give Up

### Clone calls

You can't return a reference to something inside a collection. When you need a non-Copy value outside its scope, you clone it.

Strings are Copy — they just work. The remaining clones are for collections and large structs at API boundaries:

```rask
// Strings copy freely — no clone needed
const names = Vec.new()
for entry in entries {
    if entry.active: names.push(entry.name)
}

// Structs >16 bytes need explicit clone
const user_copy = user.clone()
db.insert(id, user_copy)
```

In practice, clone calls concentrate at collection API boundaries — roughly 1-2% of lines, not spread through the code. I think that's better than lifetime annotations, and with Copy strings it's rarely visible in everyday code.

### Handle indirection

Parent pointers, back-references, and cross-references become handles into pools instead of direct pointers.

```rask
struct TreeNode {
    parent: Handle<TreeNode>?    // not a reference — a validated index
    children: Vec<Handle<TreeNode>>
    value: string
}
```

Each handle access validates a generation counter (~1-2ns). In most application code this is invisible. In tight inner loops, it adds up.

### Architectural restructuring

Some patterns that work naturally with references need rethinking. String slices stored in structs become indices or owned copies. Observer patterns need explicit handle registration. Iterators that yield references become iterators that yield copies or handles.

This isn't always more code — sometimes the handle-based version is clearer. But it's different, and the restructuring has a learning cost.

---

## What You Get

### Clean function signatures

No lifetime parameters, no where clauses, no borrow annotations. Function signatures describe the interface, not the memory model.

```rask
func search(entries: Vec<Entry>, query: string) -> Vec<Entry>
```
```rust
fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry>
```

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
func process(path: string) -> () or IoError {
    const file = try fs.open(path)
    ensure file.close()           // runs on every exit path
    // ...work with file...
}
```

### No function coloring

I/O operations pause green tasks transparently. Write one function, call it from anywhere.

```rask
func fetch_data(url: string) -> string or HttpError {
    const resp = try http.get(url)   // pauses the task, not the thread
    return resp.body()
}
```

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
func broken(path: string) -> () or IoError {
    const file = try fs.open(path)
    // compile error: `file` must be consumed (closed, passed, or ensured)
}
```

### Linear compilation

All analysis is function-local. No whole-program inference. Changing a function body doesn't recompile callers if the signature hasn't changed.

---

## Side by Side

The core loop of a grep clone. Same program, different languages.

**Rask:**
```rask
func grep_file(path: string, opts: GrepOptions) -> i64 or GrepError {
    const content = try fs.read_file(path)
        else |e| GrepError.FileError(e)

    const lines = content.lines()
    let match_count: i64 = 0
    let line_num = 0

    for line in lines {
        line_num = line_num + 1
        const matches = line_matches(line, opts.pattern, opts.ignore_case)
        const show = if opts.invert_match: !matches else: matches

        if show {
            match_count = match_count + 1
            if !opts.count_only {
                if opts.line_numbers {
                    println("{line_num}:{line}")
                } else {
                    println("{line}")
                }
            }
        }
    }

    if opts.count_only: println("{match_count}")
    return Ok(match_count)
}
```

**Rust equivalent:**
```rust
fn grep_file(path: &str, opts: &GrepOptions) -> Result<i64, GrepError> {
    let content = fs::read_to_string(path)
        .map_err(|e| GrepError::FileError(e.to_string()))?;

    let mut match_count: i64 = 0;

    for (line_num, line) in content.lines().enumerate() {
        let matches = line_matches(line, &opts.pattern, opts.ignore_case);
        let show = if opts.invert_match { !matches } else { matches };

        if show {
            match_count += 1;
            if !opts.count_only {
                if opts.line_numbers {
                    println!("{}:{}", line_num + 1, line);
                } else {
                    println!("{}", line);
                }
            }
        }
    }

    if opts.count_only { println!("{}", match_count); }
    Ok(match_count)
}
```

They're roughly the same length. The Rust version has `&str`, `&GrepOptions`, `Result<>`, `?`, `::`, `&`, semicolons. The Rask version has `try`, `else |e|`, `const`/`let`, string interpolation. Neither is dramatically shorter — the difference is that the Rask version has no borrowing annotations at all, and the safety guarantees are comparable. Note there's also no `.clone()` anywhere — strings are Copy, and all the values here flow naturally.

The point isn't "Rask is shorter." It's that Rask reads like Go but has Rust-level safety guarantees.

---

## Status

Design phase. Working interpreter. Three of five validation programs run (grep clone, game loop with entities, text editor with undo). No optimizing compiler yet.

The question being answered right now is whether the approach is viable — not whether the implementation is ready.

**Dig deeper:**
- [CORE_DESIGN.md](specs/CORE_DESIGN.md) — full design rationale
- [specs/](specs/) — formal specifications
- [examples/](examples/) — working programs
- [Language Guide](LANGUAGE_GUIDE.md) — complete feature walkthrough
