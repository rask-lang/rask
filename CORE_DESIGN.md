# Rask Core Design

## Design Principles

### 1. Safety Without Annotation

Memory safety is structural, not annotated. The compiler enforces safety through the type system and scope rules without requiring programmer-visible lifetime markers, borrow annotations, or ownership syntax at call sites.

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

### 5. Local Analysis Only

All compiler analysis is function-local. No whole-program inference, no cross-function lifetime tracking, no escape analysis.

**What this means:**
- Function signatures fully describe their interface
- Changing a function's implementation cannot break callers
- Incremental compilation is straightforward
- Compilation speed scales linearly with code size

### 6. Linear Resources

I/O handles and system resources are linear types: they must be consumed exactly once. You cannot forget to close a file or leak a socket.

**What this means:**
- Linear values can be read (borrowed) for inspection
- Linear values must eventually be consumed (closed, transferred, etc.)
- Forgetting to consume a linear value is a compile error

See [Linear Types](specs/memory/linear-types.md) for full specification.

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

**Default arguments:** `func connect(host: String, port: i32 = 8080)` — Optional parameters with compile-time constant defaults.

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
- `@linear` attribute: must be consumed exactly once (files, connections)
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
- `pool[h].field` — expression-scoped borrow (released at semicolon), panics on invalid handle
- `pool.get(h)` — returns Option<T> (T: Copy), safe for untrusted handles

Expression-scoped allows mutation between accesses—borrow ends at semicolon, so `pool.remove(h)` works after `pool[h].field = x`.

**Handles** are configurable identifiers with runtime validation:

```rask
Pool<T, PoolId=u32, Index=u32, Gen=u32>  // Defaults
```

Handle size = `sizeof(PoolId) + sizeof(Index) + sizeof(Gen)`. Default is 12 bytes (4 bytes under copy threshold, leaving headroom for future extension).

Override any parameter for different tradeoffs:
- `Pool<T, Gen=u64>` — 16-byte handles for high-churn scenarios
- `Pool<T, PoolId=u16, Index=u16, Gen=u32>` — 8-byte compact handles

Runtime validation catches: wrong pool (pool_id mismatch), stale handle (generation mismatch), invalid index (out of bounds).

**Linear resources:** Cannot be stored in Vec<T> (drop cannot propagate errors). Use Pool<T> with explicit `remove()` and consumption for linear types.

**Specifications:**
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
- [Linear Types](specs/memory/linear-types.md) - Linear resource consumption requirements

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
- All generic functions (public AND private) must declare explicit bounds
- This preserves local analysis—no call-graph tracing required
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
- **Parallelism** (thread pool): True simultaneous execution. For CPU-bound work. Use `threading.spawn { }`.

**Three spawn constructs:**

| Construct | Purpose | Requires |
|-----------|---------|----------|
| `spawn { }` | Green task | `with multitasking` |
| `threading.spawn { }` | Thread from pool | `with threading` |
| `raw_thread { }` | Raw OS thread | Nothing |

**No function coloring:** There is no `async`/`await`. Functions are just functions—I/O operations pause the task automatically. No ecosystem split.

**Affine handles:** All spawn constructs return handles that must be consumed—either joined or explicitly detached. Compile error if forgotten.

```rask
func fetch_user(id: u64) -> User {
    let response = http_get(url)?  // Pauses task, not thread
    parse_user(response)
}

// Spawn and wait
let h = spawn { fetch_user(1) }
let user = h.join()?

// Fire-and-forget (explicit)
spawn { fetch_user(2) }.detach()

// Multiple tasks
let (a, b) = join_all(
    spawn { work1() },
    spawn { work2() }
)

// CPU-bound work on thread pool
func process_image(img: Image) -> Image {
    threading.spawn { apply_filter(img) }.join()?
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

**Package = directory.** All `.rask` files in a directory form one package. No manifest needed.

**Visibility:** Two levels only.
- Default: visible within package (no keyword)
- `public`: visible to external packages

**Imports:** Qualified by default, selective unqualified with `using`.
- `import http` → `http.Request`
- `import http using Request` → `Request`

**Built-in types:** `String`, `Vec`, `Result`, `Option`, etc. are always available without import. Fixed set—cannot be extended.

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

## Limitations

1. **Explicit cloning:** Large values require explicit cloning to share access
2. **Key-based indirection:** Graphs and self-referential structures use handles, not pointers
3. **No shared mutable state:** Cross-task data sharing requires channels or explicit synchronization primitives
4. **Unsafe for low-level code:** OS/kernel work requires unsafe blocks with raw pointers
