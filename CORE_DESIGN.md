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

Functions declare how they use parameters:

**Read:** Temporarily borrow for inspection. Caller retains ownership. Cannot mutate.

**Transfer:** Ownership moves to callee. Caller's binding becomes invalid.

**Mutate:** Caller retains ownership but value may be modified in place.

The calling convention is declared in the signature. Call sites do not repeat this information—the compiler knows from the signature. (Per Principle 7, IDE shows the mode at each call site as ghost text.)

### Ownership and Uniqueness

The compiler tracks ownership statically:
- Values have exactly one owner at any time
- Assignment transfers ownership (for non-copy types)
- After a move, the source binding is invalid
- To keep access while also passing a value: explicitly clone (visible allocation)

(Per Principle 7, IDE shows move vs. copy at each use site as ghost annotation.)

### Scoped Borrowing

**Block-scoped** for plain values (strings, struct fields):
- Valid from creation until end of enclosing block
- Cannot be stored in structs, returned, or sent cross-task
- Source cannot be mutated while borrowed
- Borrowing a temporary extends its lifetime

**Expression-scoped** for collections—see Collections section.

See [Memory Model](specs/memory-model.md) for full specification.

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

**Handles** are compact identifiers (pool_id + index + generation). Runtime validation catches:
- Wrong pool (pool_id mismatch)
- Stale handle (generation mismatch)
- Invalid index (out of bounds)

**Linear resources:** Cannot be stored in Vec<T> (drop cannot propagate errors). Use Pool<T> with explicit `remove()` and consumption for linear types.

### Optionals

Optional types represent values that may be absent. Used for:
- Handle lookups that may fail
- Parsing that may not match
- Any operation with a "not found" case

Pattern matching or propagation extracts the value.

### Pattern Matching

One keyword: `match`. The compiler infers binding modes from how bindings are used:

- Only `read` parameters → borrows, original valid after
- Any `mutate` parameter → mutable borrow
- Any `transfer` parameter → consumes original

Highest mode wins across all arms. IDE displays inferred mode as ghost annotation.

See [Sum Types](specs/sum-types---enums.md) for full specification.

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

See [Memory Model - Closure Capture](specs/memory-model.md#closure-capture-and-mutation) for full specification.

### Scope-Exit Cleanup (`ensure`)

Guarantees cleanup runs when a block exits, regardless of how (normal, early return, `?`).

```
let file = open("data.txt")?
ensure file.close()          // Runs at scope exit
let data = file.read()?      // Safe: ensure registered
```

- Block-scoped, LIFO order (last ensure runs first)
- Errors ignored by default; `catch |e| ...` for opt-in handling
- Explicit consumption cancels ensure (transaction pattern)
- Satisfies linear tracking: `?` allowed after ensure

See [Ensure Cleanup](specs/ensure-cleanup.md) for full specification.

### Strings

One owned type: `string` (UTF-8 validated, move semantics).

**API convention:** Public APIs always use `string`—either owned or borrowed via `read string`. No separate borrowed string type at API boundaries.

**Block-scoped slicing:** `s[i..j]` creates a view valid until end of block. Zero-copy, cannot escape block (no struct storage, no returns).

**Stored references:** Two options for internal use:
- `string_view` — Plain indices (start, end). No validation, user ensures source validity.
- `StringPool` + `StringSlice` — Handle-based with validation, follows Pool<T> pattern.

**C interop:** `cstring` type for null-terminated strings. Conversion requires unsafe block.

See [String Handling](specs/string-handling.md) for full specification.

### Traits

Traits define compile-time contracts (interfaces). A type satisfies a trait if it has all required methods with matching signatures.

**Structural matching (default):** No explicit `impl` declaration is required. If the shape matches, the type can be used where the trait is expected.

**Explicit impl (optional):** An `impl` block can be provided to:
- Document intent ("this type IS a Reader")
- Override default method implementations
- Satisfy `explicit trait` requirements

**Default methods:** Traits can provide default implementations. Adding a defaulted method never breaks existing satisfying types. Types can override defaults via explicit impl.

**Explicit traits:** Trait authors can require explicit implementation for stability:
- `trait Foo { ... }` — structural matching allowed
- `explicit trait Foo { ... }` — requires `impl Foo for Type`

This lets libraries protect their API contracts. Users can rely on structural matching for internal code.

**Tooling contract:** The IDE should show "ghost implementations" - types that structurally satisfy traits without explicit impl. This enables discovery without requiring boilerplate.

### Generics and Bounds

Generic functions use trait bounds to constrain type parameters:
- All generic functions (public AND private) must declare explicit bounds
- This preserves local analysis—no call-graph tracing required
- Bounds checked at monomorphization site

**Monomorphization (default):** The compiler generates separate code for each concrete type.

```
fn print_all<T: Display>(items: []T)

print_all(integers)  // generates print_all_i32
print_all(strings)   // generates print_all_String
```

Benefits: Fast execution (no indirection), full optimization possible.
Limitation: All items in a collection must be the same type.

**Runtime polymorphism (`any Trait`):** Store different types together, dispatch at runtime.

```
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

### Concurrency

**Task isolation:** Each task owns its data. No shared mutable memory between tasks.

**Ownership transfer:** Sending a value on a channel transfers ownership. Sender loses the value; receiver gains it. No copies for large values, no locks, no races.

**Structured parallelism:** Parallel computation over owned data with compiler-verified constraints on what can be read vs. mutated.

### Compile-Time Execution

Functions can be evaluated at compile time when their inputs are known. Enables:
- Constant folding
- Compile-time configuration
- Generic specialization
- Static assertions

### C Interop

- C calls happen in explicitly marked unsafe blocks
- Raw pointers exist only in unsafe code
- At boundaries: convert between safe Rask values and C pointers
- Unsafe is quarantined; most code never touches pointers

### Module System

**Package = directory.** All `.rask` files in a directory form one package. No manifest needed.

**Visibility:** Two levels only.
- Default: visible within package (no keyword)
- `pub`: visible to external packages

**Imports:** Qualified by default, selective unqualified with `using`.
- `import http` → `http.Request`
- `import http using Request` → `Request`

**Built-in types:** `String`, `Vec`, `Result`, `Option`, etc. are always available without import. Fixed set—cannot be extended.

**Re-exports:** `export internal.Parser` exposes internal types through a clean public API.

See [Module System](specs/module-system.md) for full specification.

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

**Using `ensure` for cleanup:** Register cleanup with `ensure` to enable `?` propagation without manual cleanup on every path. See [Ensure Cleanup](specs/ensure-cleanup.md).

---

## Open Design Questions

These areas are not yet finalized. Design exploration continues.

### Lib vs Executable

How do libraries differ from executables?
- Entry points (`main` function?)
- Can a package be both?
- Build configuration

### Package Versioning & Dependencies

How are external dependencies managed?
- Semantic versioning?
- Dependency resolution algorithm?
- Lock files?
- Version constraints syntax?

This is intentionally out of scope for now—focus is on language semantics first.

### Iterators and Loops

Borrows cannot be stored, but iterators need to hold a reference between `.next()` calls.

| Option | Tradeoff |
|--------|----------|
| Copy on iteration | Expensive for large items |
| Yield handles | `Pool` yields `Handle<T>`, user dereferences |
| Yield indices | `Vec` yields `usize`, user accesses via `vec[i]` |
| Compiler-special `for` | Loop body borrows collection for its scope |

Critical for ED ≤ 1.2 validation.

### Async and Concurrency

**Unspecified:** async syntax, `ensure` + task cancellation, structured concurrency vs. free-spawn, channel types (bounded/unbounded/rendezvous), select/multiplex.

### Copy Size Threshold

What is the size threshold for implicit Copy?
- Types below threshold: implicit clone on assignment/pass
- Types above threshold: require explicit `.clone()`
- Suggested: 16-32 bytes (2-4 machine words)

---

## Limitations

1. **Explicit cloning:** Large values require explicit cloning to share access
2. **Key-based indirection:** Graphs and self-referential structures use handles, not pointers
3. **No shared mutable state:** Cross-task data sharing requires channels or explicit synchronization primitives
4. **Unsafe for low-level code:** OS/kernel work requires unsafe blocks with raw pointers
