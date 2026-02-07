# Rask Core Design

## The Struggle

I spent a long time trying to get this right: ergonomics without sacrificing transparency. Go feels great to write but gives you no safety guarantees. Rust is safe but you spend half your time fighting the borrow checker and annotating lifetimes. I wanted something in between.

The breakthrough was realizing that most of Rust's complexity comes from trying to allow storable references. If you eliminate those—make references impossible to store—you can skip lifetime annotations entirely. The cost is explicit indirection (handles instead of pointers), but that's a cost I can see and reason about.

---

## Design Principles

### 1. Safety Without Annotation

I enforce memory safety through the type system and scope rules without requiring lifetime markers, borrow annotations, or ownership syntax at call sites.

**What this means:**
- No lifetime parameters in function signatures
- No borrow checker annotations
- No `&`, `&mut`, or equivalent markers
- Safety is a property of well-typed programs, not extra work

### 2. Value Semantics

All types are values—data is embedded, not pointed-to. There is no distinction between "value types" and "reference types."

**What this means:**
- Assigning or passing a value either copies it (for small types) or moves it (transfers ownership)
- No implicit sharing; aliasing is explicit and controlled
- Memory layout is predictable and cache-friendly

### 3. No Storable References

References cannot outlive their lexical scope. You can borrow a value temporarily (for the duration of a function call or expression), but you cannot store that borrow in a struct, return it, or let it escape.

**What this means:**
- No types that "hold a pointer to another value"
- Collections use handles (opaque identifiers) instead of references
- Graphs, trees with parent pointers, and self-referential structures use key-based indirection
- Eliminates use-after-free, dangling pointers, and iterator invalidation by construction

**The tradeoff:** Some patterns require explicit indirection. **The gain:** No lifetime annotations, no borrow checker fights, no runtime tracking.

**Why I chose this:** Rust's borrow checker is technically brilliant but ergonomically exhausting. After writing enough Rust, I realized that most of the complexity comes from allowing references to escape. Eliminate that, and the whole lifetime system becomes unnecessary. The indirection cost is explicit and measurable—much better than hidden complexity.

### 4. Transparent Costs

Major costs are visible in code. Small safety checks can be implicit.

**Visible (explicit in code):**
- Allocations and reallocations
- Large copies and clones
- Locks, I/O, system calls
- Cross-task communication

**Implicit (no ceremony required):**
- Bounds checks
- Handle validity checks
- Small copies of primitive types

**The balancing act:** I want allocations visible (they're expensive), but I don't want ceremony on every array access. The rule is: if it's O(1) and cheap (bounds check, generation check), it can be implicit. If it's potentially expensive (allocation, I/O, locking), it must be explicit in the code.

### 5. Local Analysis Only

All compiler analysis is function-local. No whole-program inference, no cross-function lifetime tracking, no escape analysis.

**What this means:**
- Public function signatures fully describe their interface (explicit types required)
- Private function signatures may omit types; the compiler infers them from the function body only — never from callers
- Changing a public function's implementation cannot break external callers
- Changing a private function's body may change its inferred signature, breaking internal callers (compiler reports this clearly with smart diagnostics showing what changed, which line caused it, and which callers break)
- Incremental compilation is straightforward
- Compilation speed scales linearly with code size

**Clarification:** Body-local inference for private functions IS local analysis. The compiler examines one function body at a time, solving constraints within that scope. It does not trace through call graphs or analyze callers. See [Gradual Constraints](specs/types/gradual-constraints.md).

**Why this matters:** Rust's borrow checker does global analysis. Change one function and the ripple effects are unpredictable. I want compilation to scale linearly—doubling your codebase should double compile time, not quadruple it. Local-only analysis makes this possible.

### 6. Resource Types

I/O handles and system resources are resource types (linear resources): they must be consumed exactly once. You cannot forget to close a file or leak a socket.

**What this means:**
- Resource values can be read (borrowed) for inspection
- Resource values must eventually be consumed (closed, transferred, etc.)
- Forgetting to consume a resource value is a compile error

See [Resource Types](specs/memory/resource-types.md) for full specification.

### 7. Compiler Knowledge is Visible

Information the compiler can infer should be displayed by tooling, not required in source code. Code stays minimal; the IDE shows the full picture.

**What this means:**
- Type annotations are optional when inferrable; IDE shows inferred types as ghost text
- Trait implementations are structural; IDE shows matching types as "ghost impls"
- Ownership transfers are tracked; IDE shows move/copy decisions at use sites
- Closure captures are implicit; IDE shows capture list as ghost annotation
- Parameter modes are in signatures; IDE shows them at call sites

**The principle:** Write intent, not mechanics. The compiler knows the mechanics—let tooling reveal them.

**Tooling contract:** IDEs SHOULD display compiler-inferred information as unobtrusive ghost annotations. This is not optional polish—it's how the language achieves clarity without ceremony.

---

## Why Not X?

### Why Not Garbage Collection?

GC languages (Go, Java, C#) are ergonomic but give you no control over when cleanup happens. You can't predict when the GC will run or how long it will pause. For games, real-time systems, or anything with latency requirements, this is a non-starter.

I want deterministic cleanup. When a value goes out of scope, it's freed immediately. No pauses, no tuning GC parameters, no wondering why your 99th percentile latency spikes.

### Why Not Reference Counting?

Ref counting (Swift, Python) solves the GC pause problem but introduces overhead on every assignment and has the cycle problem. You end up with weak references and manual cycle breaking, which brings back the same cognitive load you were trying to avoid.

I'd rather have explicit `.clone()` calls than hidden overhead on every pointer operation.

### Why Not Rust's Borrow Checker?

Rust's borrow checker is technically sound, but ergonomically expensive. Lifetime annotations leak into function signatures. The complexity is front-loaded—you pay for it whether you need it or not.

I took a different approach: instead of tracking reference lifetimes, make references non-storable. This eliminates the need for lifetime tracking entirely. You trade storable references for explicit handles. I think that's a better tradeoff.

### Why Not Manual Memory Management?

C and C++ give you full control but no safety. Use-after-free, double-free, and dangling pointers are all possible. I wanted safety without the annotation burden, which rules out manual management.

The goal was to find a sweet spot: safer than C, more ergonomic than Rust, more predictable than GC languages. YOu can have unsafe and assembly if you want, just like in rust (although not as good compile time safety).

---

## Core Mechanisms

### Bindings

- `let x = 0` — mutable binding
- `const x = 0` — immutable binding
- `x = 5` — reassignment (must already exist)
- `let x = y` — shadowing allowed (IDE shows ghost annotation)

### Type Categories

**Plain values:** Owned data—primitives, structs, arrays. Either copied (small types) or moved (larger types) on assignment.

**Handles:** Opaque identifiers into collections. Can be freely stored, passed, and compared. Access via handle performs validation.

**Linear resources:** Must be consumed exactly once. Used for I/O, system resources, and anything requiring cleanup.

### Parameter Passing

Two parameter modes:

**Borrow (default):** `func process(data: Data)` — Temporary access. Caller keeps ownership. Mutability inferred from usage (read vs write).

**Take:** `func consume(take data: Data)` — Ownership transfer. Caller's binding becomes invalid.

**Default arguments:** `func connect(host: string, port: i32 = 8080)` — Optional parameters with compile-time constant defaults.

**Projections:** `func heal(p: Player.{health})` — Borrow only specific fields, enabling disjoint field borrows across function calls.

The calling convention is declared in the signature. Call sites do not repeat this information—the compiler knows from the signature. (Per Principle 7, IDE shows the mode at each call site as ghost text, including inferred mutability.)

### Ownership and Uniqueness

The compiler tracks ownership statically:
- Values have exactly one owner at any time
- Assignment transfers ownership (for non-copy types)
- After a move, the source binding is invalid
- To keep access while also passing a value: explicitly clone (visible allocation)

(Per Principle 7, IDE shows move vs. copy at each use site as ghost annotation.)

### Implicit copying

- Types ≤16 bytes: implicit copy (if all fields are Copy)
- Types >16 bytes: explicit `.clone()` or move semantics
- Threshold is **NOT configurable** (semantic stability, portability)
- `@unique` attribute: prevents copying (each instance is unique)
- `@resource` attribute: must be consumed exactly once (files, connections)
- Platform ABI differences handled at compiler level (semantics are portable)

**Specifications:**
- [Why Implicit Copy?](specs/memory/value-semantics.md#why-implicit-copy) - Fundamental justification
- [Threshold Configurability](specs/memory/value-semantics.md#threshold-non-configurability) - Fixed threshold rationale
- [Move-Only Types](specs/memory/value-semantics.md#move-only-types-opt-out) - Opt-out via `move` keyword
- [Copy Trait and Generics](specs/memory/value-semantics.md#copy-trait-and-generics) - Generic bounds behavior
- [Platform ABI Considerations](specs/memory/value-semantics.md#platform-abi-considerations) - Cross-platform portability
- [Structs](specs/types/structs.md) - Struct definition, methods, visibility, layout

### Integer Overflow

**Default: Panic on overflow** — consistent in debug and release (safer than Rust's wrap-in-release).

- `Wrapping<T>` — type where `+` wraps (hashing, checksums)
- `Saturating<T>` — type where `+` saturates (audio, DSP)
- `.wrapping_add()` — one-off wrapping operation
- Compiler elides checks via range analysis when overflow is provably impossible

No custom operators — types are clearer and reduce mental tax.

See [Integer Overflow](specs/types/integer-overflow.md) for full specification.

### Scoped Borrowing

**Block-scoped** for plain values (strings, struct fields):
- Valid from creation until end of enclosing block
- Cannot be stored in structs, returned, or sent cross-task
- Source cannot be mutated while borrowed
- Borrowing a temporary extends its lifetime

**Expression-scoped** for collections—see Collections section.

See [Borrowing](specs/memory/borrowing.md) for full specification.

### Collections and Handles

Three collection types cover most use cases:

**Vec<T>:** Ordered, indexed access. Use for sequences, buffers, arrays.
**Pool<T>:** Handle-based sparse storage. Use for graphs, caches, entities with stable identity.
**Map<K,V>:** Key-value associative lookup.

**Capacity model:** All collections support optional capacity constraints:
- Unbounded (default): grows as needed
- Bounded: capacity set at creation, cannot exceed
- Fixed: bounded + pre-allocated

**Allocation is fallible:** All growth operations (`push`, `insert`, `extend`) return `Result`. Rejected values are returned in the error for retry or logging.

**Access patterns:**
- `vec[i]` — copy out (T: Copy), panics on out-of-bounds
- `h.field` — expression-scoped borrow via context (released at semicolon), panics on invalid handle
- `pool.get(h)` — returns Option<T> (T: Copy), safe for untrusted handles

Expression-scoped allows mutation between accesses—borrow ends at semicolon, so `pool.remove(h)` works after `h.field = x`.

**Handles** are configurable identifiers with runtime validation:

```rask
Pool<T, PoolId=u32, Index=u32, Gen=u32>  // Defaults
```

Handle size = `sizeof(PoolId) + sizeof(Index) + sizeof(Gen)`. Default is 12 bytes (4 bytes under copy threshold, leaving headroom for future extension).

Override any parameter for different tradeoffs:
- `Pool<T, Gen=u64>` — 16-byte handles for high-churn scenarios
- `Pool<T, PoolId=u16, Index=u16, Gen=u32>` — 8-byte compact handles

Runtime validation catches: wrong pool (pool_id mismatch), stale handle (generation mismatch), invalid index (out of bounds).

**Resources:** Cannot be stored in Vec<T> (drop cannot propagate errors). Use Pool<T> with explicit `remove()` and consumption for resource types.

### Context Clauses

Functions using handles declare pool requirements with `with` clauses:

```rask
// Unnamed context: field access only
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount
}

// Named context: field access + structural operations
func spawn_wave(count: i32) with enemies: Pool<Enemy> {
    for i in 0..count {
        try enemies.insert(Enemy.new(random_pos()))
    }
}
```

The compiler threads pools as hidden parameters — no runtime registry, no lookups. Context requirements are part of the function signature and checked at every call site.

**Resolution:** Compiler finds pools in scope (local variables, parameters, `self` fields) and passes them automatically.

**Inference:** Private functions can omit `with` clauses (inferred from body). Public functions must declare them explicitly.

**Specifications:**
- [Context Clauses](specs/memory/context-clauses.md) - Pool requirement declarations, resolution rules, inference
- [Collections (Vec, Map)](specs/stdlib/collections.md) - Indexed and keyed collections
- [Pools and Handles](specs/memory/pools.md) - Handle-based sparse storage, weak handles, cursors, freezing

### Optionals

`Option<T>` represents values that may be absent. Rask provides syntax sugar for ergonomic handling:

| Syntax | Meaning |
|--------|---------|
| `T?` | `Option<T>` (type shorthand) |
| `none` | Absence literal (type inferred) |
| `x?.field` | Access if present, else none |
| `x ?? default` | Value or default |
| `x!` | Force unwrap (panic if none) |
| `if x?` | Check + smart unwrap in block |

**Example:**
```rask
let name = get_user(id)?.profile?.name ?? "Anonymous"

if user? {
    process(user)    // user is T here, not T?
}
```

`Option<T>` is a standard enum underneath — pattern matching with `Some`/`None` still works.

See [Optionals](specs/types/optionals.md) for full specification.

### Pattern Matching

One keyword: `match`. The compiler infers binding modes from how bindings are used:

- Only reads → borrows (immutable), original valid after
- Any mutation → borrows (mutable), original valid after
- Any `take` → consumes original

Highest mode wins across all arms. IDE displays inferred mode as ghost annotation.

See [Sum Types](specs/types/enums.md) for full specification.

### Closures

Two kinds of closures:

**Storable closures:** Capture by value (copy or move), can be stored and called later.
- Small captures copy implicitly
- Large captures move; to keep access, clone explicitly
- Can accept parameters passed on each call
- For mutable state access: capture handles, receive pools as parameters

**Expression-scoped closures:** Access outer scope without capturing, must execute immediately.
- Used in iterator adapters, immediate callbacks
- Can mutate outer scope (aliasing rules enforced)
- Cannot be stored (compile error if closure escapes)

(Per Principle 7, IDE shows the capture list and capture mode as ghost annotation.)

See [Closure Capture](specs/memory/closures.md) for full specification.

### Iteration

Loops yield indices or handles (Copy values), not borrowed references. Collection access uses expression-scoped borrows. This enables mutation during iteration while preventing iterator invalidation.

**Basic patterns:**
- `for i in vec { vec[i] }` — Index iteration, mutation allowed
- `for h in pool { pool[h] }` — Handle iteration
- `for item in vec.consume() { }` — Ownership transfer (consuming)
- `for i in 0..n { }` — Range iteration

**Key properties:**
- Collection is NOT borrowed during loop (mutation allowed)
- Each `collection[i]` access is independent (expression-scoped)
- Explicit `.consume()` required for ownership transfer
- No lifetime parameters needed

See [Iterators and Loops](specs/stdlib/iteration.md) for full specification.

### Scope-Exit Cleanup (`ensure`)

Guarantees cleanup runs when a block exits, regardless of how (normal, early return, `try`).

```rask
let file = try open("data.txt")
ensure file.close()              // Runs at scope exit
let data = try file.read()       // Safe: ensure registered
```

- Block-scoped, LIFO order (last ensure runs first)
- Errors ignored by default; `catch |e| ...` for opt-in handling
- Explicit consumption cancels ensure (transaction pattern)
- Satisfies linear tracking: `try` allowed after ensure

**Specifications:**
- [Ensure Cleanup](specs/control/ensure.md) - Deferred cleanup mechanism
- [Resource Types](specs/memory/resource-types.md) - Resource consumption requirements

### Strings

One owned type: `string` (UTF-8 validated, move semantics).

**API convention:** Public APIs always use `string`—either owned or borrowed via `read string`. No separate borrowed string type at API boundaries.

**Block-scoped slicing:** `s[i..j]` creates a view valid until end of block. Zero-copy, cannot escape block (no struct storage, no returns).

**Stored references:** Two options for internal use:
- `string_view` — Plain indices (start, end). No validation, user ensures source validity.
- `StringPool` + `StringSlice` — Handle-based with validation, follows Pool<T> pattern.

**C interop:** `cstring` type for null-terminated strings. Conversion requires unsafe block.

See [String Handling](specs/stdlib/strings.md) for full specification.

### Traits

Traits define compile-time contracts (interfaces). A type satisfies a trait if it has all required methods with matching signatures.

**Structural matching (default):** No explicit `extend` declaration is required. If the shape matches, the type can be used where the trait is expected.

**Explicit extend (optional):** An `extend` block can be provided to:
- Document intent ("this type IS a Reader")
- Override default method implementations
- Satisfy `explicit trait` requirements

**Default methods:** Traits can provide default implementations. Adding a defaulted method never breaks existing satisfying types. Types can override defaults via explicit extend.

**Explicit traits:** Trait authors can require explicit implementation for stability:
- `trait Foo { ... }` — structural matching allowed
- `explicit trait Foo { ... }` — requires `extend Foo for Type`

This lets libraries protect their API contracts. Users can rely on structural matching for internal code.

**Tooling contract:** The IDE should show "ghost implementations" - types that structurally satisfy traits without explicit extend. This enables discovery without requiring boilerplate.

### Generics and Bounds

Generic functions use trait bounds to constrain type parameters:
- Public generic functions must declare explicit bounds
- Private generic functions may omit bounds (inferred from body; see [Gradual Constraints](specs/types/gradual-constraints.md))
- Bounds checked at monomorphization site

**Monomorphization (default):** The compiler generates separate code for each concrete type.

```rask
func print_all<T: Display>(items: []T)

print_all(integers)  // generates print_all_i32
print_all(strings)   // generates print_all_String
```

Benefits: Fast execution (no indirection), full optimization possible.
Limitation: All items in a collection must be the same type.

**Runtime polymorphism (`any Trait`):** Store different types together, dispatch at runtime.

```rask
// Without any: impossible to mix types
let widgets: []Button = [...]  // only Buttons allowed

// With any: different types in same collection
let widgets: []any Widget = [button, textbox, slider]
for w in widgets {
    w.draw()  // calls the right draw() for each type
}
```

Use `any Trait` when you need heterogeneous collections: HTTP handlers, UI widgets, plugin systems, event listeners.

Cost: Small indirection (vtable lookup—a table of function pointers). The compiler stores type information alongside the value to dispatch method calls at runtime.

See [Generics](specs/types/generics.md) and [Runtime Polymorphism](specs/types/traits.md) for full specification.

### Concurrency

**Explicit resources:** `with multitasking { }` and `with threading { }` create and scope concurrency resources. No hidden schedulers or thread pools.

```rask
@entry
func main() {
    with multitasking, threading {
        run_server()
    }
}

// With configuration (rare)
with multitasking(4), threading(8) { ... }
```

**Concurrency vs Parallelism:**
- **Concurrency** (green tasks): Many tasks interleaved on few threads. For I/O-bound work. Use `spawn { }`.
- **Parallelism** (thread pool): True simultaneous execution. For CPU-bound work. Use `spawn_thread { }`.

**Three spawn constructs:**

| Construct | Purpose | Requires |
|-----------|---------|----------|
| `spawn { }` | Green task | `with multitasking` |
| `spawn_thread { }` | Thread from pool | `with threading` |
| `raw_thread { }` | Raw OS thread | Nothing |

**No function coloring:** There is no `async`/`await`. Functions are just functions—I/O operations pause the task automatically. No ecosystem split.

**Affine handles:** All spawn constructs return handles that must be consumed—either joined or explicitly detached. Compile error if forgotten.

```rask
func fetch_user(id: u64) -> User {
    const response = try http_get(url)  // Pauses task, not thread
    parse_user(response)
}

// Spawn and wait
const h = spawn { fetch_user(1) }
const user = try h.join()

// Fire-and-forget (explicit)
spawn { fetch_user(2) }.detach()

// Multiple tasks
let (a, b) = join_all(
    spawn { work1() },
    spawn { work2() }
)

// CPU-bound work on thread pool
func process_image(img: Image) -> Image {
    const handle = spawn_thread { apply_filter(img) }
    try handle.join()
}
```

**Task isolation:** Each task owns its data. No shared mutable memory between tasks.

**Ownership transfer:** Sending a value on a channel transfers ownership. Sender loses the value; receiver gains it. No copies for large values, no locks, no races.

**Sync mode (default):** Without Multitasking, I/O blocks and `spawn { }` is unavailable. Thread pool still works. This is the default for CLI tools, batch processing, and embedded systems.

**Async mode (opt-in):** With `with multitasking { }`, I/O pauses and green tasks are available.

See [Concurrency](specs/concurrency/) for full specification.

### Compile-Time Execution

The `comptime` keyword marks code that executes during compilation. A restricted subset of Rask runs in the compiler's interpreter—pure computation without I/O, pools, or concurrency.

**Use cases:**
- Compile-time constants and lookup tables
- Generic specialization with comptime parameters
- Conditional compilation based on features
- Type-level computation

**Example:**
```rask
comptime func build_table() -> [u8; 256] {
    let table = [0u8; 256]
    for i in 0..256 {
        table[i] = (i * 2) as u8
    }
    table
}

const LOOKUP: [u8; 256] = comptime build_table()
```

**Separate from build scripts:** Comptime runs in-compiler (limited subset). Build scripts (`rask.build`) run as separate programs before compilation (full language, I/O allowed).

See [Compile-Time Execution](specs/control/comptime.md) for full specification.

### C Interop

- C calls happen in explicitly marked unsafe blocks
- Raw pointers exist only in unsafe code
- At boundaries: convert between safe Rask values and C pointers
- Unsafe is quarantined; most code never touches pointers

See [Unsafe Blocks](specs/memory/unsafe.md) for raw pointers, unsafe operations, and safety boundaries. See [Module System](specs/structure/modules.md) for C import/export syntax.

### Module System

**Package = directory.** All `.rk` files in a directory form one package. No manifest needed.

**Visibility:** Two levels only.
- Default: visible within package (no keyword)
- `public`: visible to external packages

**Imports:** Qualified by default, selective unqualified with `using`.
- `import http` → `http.Request`
- `import http using Request` → `Request`

**Built-in types:** `string`, `Vec`, `Result`, `Option`, etc. are always available without import. Fixed set—cannot be extended.

**Re-exports:** `export internal.Parser` exposes internal types through a clean public API.

**Libraries vs Executables:** Package role determined by presence of `public func main()`. Libraries export `public` API; executables have entry point. See [Libraries vs Executables](specs/structure/targets.md).

**Dependencies:** Semantic versioning with minimal version selection (MVS), optional `rask.toml` manifest, generated lock file. See [Package Versioning and Dependencies](specs/structure/packages.md).

See [Module System](specs/structure/modules.md) for full specification.

---

## Error Handling

Errors are values, not exceptions. Operations that can fail return a result type indicating success or failure.

### Principles

**No hidden control flow:** Errors do not throw or unwind. All error paths are visible in types.

**Low ceremony:** A propagation mechanism allows errors to bubble up without nested conditionals on every line.

**Compatible with linear resources:** When an operation on a linear resource fails, the resource is still accounted for—either returned in the error, or handled by the failure path.

### Result Types

Fallible operations return a result containing either a success value or an error value. The caller must handle both cases (pattern match, propagate, or provide a default).

### Error Propagation

A lightweight propagation mechanism (operator or keyword) extracts the success value or returns early with the error. This keeps the happy path clean while ensuring errors are handled.

### Linear Resources and Errors

When a linear resource operation fails:
- The resource is returned alongside the error (caller still responsible for cleanup)
- Or the operation consumes the resource even on failure (documented in signature)

This ensures linear values cannot be forgotten even in error paths.

**Using `ensure` for cleanup:** Register cleanup with `ensure` to enable `try` propagation without manual cleanup on every path. See [Ensure Cleanup](specs/control/ensure.md).

---

## Design Tradeoffs

I'm not pretending there aren't costs to these choices. Every design has tradeoffs—mine are intentional.

### Clone Ergonomics

**Decision:** No storable references. References are block-scoped only.

**Cost:** Code that passes strings/data through multiple layers needs explicit `.clone()` calls. In string-heavy code (CLI parsing, HTTP routing), expect ~5% of lines to have a clone. Computation-heavy code (game loops, data processing) typically has 0% clones.

**Benefit:** No lifetime annotations, no borrow checker complexity, no "fighting the borrow checker" experience. The mental model is simple: values are owned, borrows are temporary.

**Comparison:**
- **Go:** Copies strings freely (implicit, hidden cost)
- **Rust:** Requires lifetime annotations to avoid copies
- **Rask:** Explicit `.clone()` when copies are needed (visible cost, no annotations)

**When this hurts:** Error handlers that capture context (`path.clone()` in map_err), shared configuration passed to multiple subsystems.

**When this is fine:** Most code. The clone calls are localized to API boundaries, not scattered through core logic.

### Pool Handle Overhead

**Decision:** Graph structures use `Pool<T>` + `Handle<T>` instead of references.

**Cost:** Each handle access involves:
1. Pool ID check (is this the right pool?)
2. Generation check (is this handle stale?)
3. Index lookup

Estimated overhead: ~1-2ns per access. In tight loops with millions of accesses, this adds up.

**Benefit:** Use-after-free impossible. No dangling pointers. Iterator invalidation caught at runtime. Self-referential structures work without unsafe code.

**When to use pools:** Graph structures, ECS entities, caches with stable identity, anything with cycles or parent pointers.

**When to avoid pools:** Tight inner loops where every nanosecond matters. For these cases, copy data out, process in batch, write back.

### No Storable References

**Decision:** References cannot be stored in structs or returned from functions.

**Cost:** Some patterns require restructuring:
- Parent pointers → store `Handle<Parent>` instead
- String slices in structs → store indices or use `StringPool`
- Caches holding references → use `Pool<T>` with handles

**Benefit:** Eliminates entire categories of bugs:
- Use-after-free (impossible by construction)
- Dangling pointers (references can't escape scope)
- Iterator invalidation (iteration uses handles/indices)

No lifetime annotations needed. Function signatures are simple. Reasoning about ownership is local.

**The fundamental choice:** I trade "hold a reference to data owned elsewhere" for "hold a handle/key/index to data in a collection." The former requires tracking lifetimes; the latter requires explicit indirection. I think the explicitness is worth it.

### Comptime Limitations

**Decision:** Compile-time execution runs a restricted subset of Rask.

**Cost:** At comptime, you cannot:
- Do I/O (except `@embed_file`)
- Use pools or handles
- Use concurrency
- Call unsafe code
- Exceed iteration/memory limits

**Benefit:** Comptime is predictable—it always terminates, never has side effects, produces the same result on every compilation.

**When this hurts:** Complex code generation that would benefit from full language features. Use build scripts (`rask.build`) for those cases.

### When to Use Rask

**Good fit:**
- Web services and APIs
- CLI tools and utilities
- Game logic (not engine internals)
- Data processing pipelines
- Embedded systems with known memory patterns

**Consider alternatives:**
- OS kernels, drivers → Need unsafe pointer manipulation (Rust, C)
- Soft real-time with nanosecond budgets → Handle overhead may matter (C++, Rust)
- Scripting/prototyping → GC languages are faster to write (Python, Go)
- Maximum raw performance → Manual memory control needed (C++, Rust, Zig)

I'm targeting the "90% of code" that doesn't need pointer-level control but benefits from memory safety and low ceremony. If you're writing an OS kernel or a real-time audio engine with nanosecond budgets, Rask might not be the right tool. But for most applications—web services, CLI tools, games, data pipelines—I think the tradeoffs make sense.

---

## Limitations

I'm upfront about what Rask doesn't do well:

1. **Explicit cloning:** Large values require explicit cloning to share access
2. **Key-based indirection:** Graphs and self-referential structures use handles, not pointers
3. **No shared mutable state:** Cross-task data sharing requires channels or explicit synchronization primitives
4. **Unsafe for low-level code:** OS/kernel work requires unsafe blocks with raw pointers

These aren't accidents—they're deliberate tradeoffs to achieve safety without annotations.
