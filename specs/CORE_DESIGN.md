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

**Clarification:** Body-local inference for private functions IS local analysis. The compiler examines one function body at a time, solving constraints within that scope. It does not trace through call graphs or analyze callers. See [Gradual Constraints](types/gradual-constraints.md).

**Why this matters:** Rust's borrow checker does global analysis. Change one function and the ripple effects are unpredictable. I want compilation to scale linearly—doubling your codebase should double compile time, not quadruple it. Local-only analysis makes this possible.

### 6. Resource Types

I/O handles and system resources are resource types (linear resources): they must be consumed exactly once. You cannot forget to close a file or leak a socket.

**What this means:**
- Resource values can be read (borrowed) for inspection
- Resource values must eventually be consumed (closed, transferred, etc.)
- Forgetting to consume a resource value is a compile error

See [Resource Types](memory/resource-types.md) for full specification.

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

### 8. Machine-Readable Code

Code should be analyzable by tools — linters, refactoring engines, IDE plugins — without whole-program analysis. This falls naturally out of the other principles but I'm making it explicit because it should guide future decisions.

**What this means:**
- Function signatures are self-describing specifications (parameter modes, error types, context clauses)
- Keywords carry unambiguous meaning (`try`, `own`, `take`, `read`, `ensure`)
- One idiomatic pattern per operation (one error model, one cleanup model, one concurrency model)
- Naming conventions encode semantics (`is_*` returns bool, `into_*` takes ownership, `to_*` allocates)

**Why it matters:** Tools that analyze code benefit from the same properties that help developers read it. Local reasoning, explicit intent, consistent patterns — these make static analysis tractable and refactoring safe.

See [Canonical Patterns](canonical-patterns.md) for conventions, naming patterns, and tooling specs.

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

The goal was to find a sweet spot: safer than C, more ergonomic than Rust, more predictable than GC languages. You can have unsafe and assembly if you want, just like in Rust (although not as good compile time safety).

For rejected features from other languages (async/await, algebraic effects, lifetimes, supervision, scope functions), see [rejected-features.md](rejected-features.md).

---

## Core Mechanisms

Each mechanism has its own spec with full details. This section gives the shape of the language — enough to understand how the pieces fit together.

**Bindings and syntax.** `const x = 0` for immutable, `let x = 0` for mutable. Newlines terminate statements. Functions use `func`, methods live in `extend` blocks. See [SYNTAX.md](SYNTAX.md).

**Ownership.** Every value has exactly one owner. Assignment transfers ownership (move) for non-copy types. After a move, the source binding is invalid. To keep access while passing, explicitly `.clone()`. `discard` explicitly drops a value and invalidates its binding. See [ownership.md](memory/ownership.md).

**Value semantics.** Types ≤16 bytes with all-Copy fields copy implicitly. Larger types move. The threshold is fixed (not configurable) for semantic stability across platforms. `@unique` prevents copying; `@resource` requires exactly-once consumption. See [value-semantics.md](memory/value-semantics.md).

**Borrowing.** References are block-scoped for fixed-layout sources (struct fields, arrays — valid until end of enclosing block) and statement-scoped for heap-buffered sources (Vec, Pool, Map, string — released at semicolon). Cannot be stored in structs, returned, or sent cross-task. See [borrowing.md](memory/borrowing.md).

**Parameters.** Three modes declared in the signature: borrow (default, read-only), `mutate` (mutable access, caller keeps ownership), and `take` (ownership transfer). Projections like `mutate p: Player.{health}` enable disjoint field borrows. See [parameters.md](memory/parameters.md).

**Collections.** `Vec<T>` for sequences, `Map<K,V>` for key-value lookup, `Pool<T>` for handle-based sparse storage (graphs, entities, caches). All growth operations return `Result` — allocation is fallible. `with pool[h] as entity { ... }` for multi-statement element access (sugar for closure-based `modify`). See [collections.md](stdlib/collections.md), [pools.md](memory/pools.md).

**Context clauses.** Functions using handles declare pool requirements with `using Pool<T>` clauses. The compiler threads pools as hidden parameters — no runtime registry. Private functions can omit these (inferred from body). See [context-clauses.md](memory/context-clauses.md).

**Error handling.** Errors are values: `T or E` result type, `try` for propagation, `T?` for optionals with `??` fallback and `x!` force-unwrap. Error types compose with `A | B` union syntax. Functions returning `T or E` auto-wrap bare returns as `Ok(T)`. No exceptions, no hidden control flow. See [error-types.md](types/error-types.md), [optionals.md](types/optionals.md).

**Resource cleanup.** `ensure` guarantees cleanup runs when a block exits (normal, early return, `try`). LIFO order, explicit consumption cancels it (transaction pattern). Linear resources must be consumed exactly once — compiler enforces this. See [ensure.md](control/ensure.md), [resource-types.md](memory/resource-types.md).

**Pattern matching.** `match` for multiple branches, `if x is Pattern` for single checks. Compiler infers binding modes (borrow vs take) from usage. See [enums.md](types/enums.md), [control-flow.md](control/control-flow.md).

**Closures.** Three kinds, inferred from context. Stored closures capture by value (copy or move) and can go anywhere. Inline closures access the outer scope directly without capturing — must be consumed in the expression (iterator chains). Scoped closures capture block-scoped borrows and can't escape the block where the borrowed data lives. IDE shows capture list as ghost annotation. See [closures.md](memory/closures.md).

**Traits.** Structural matching by default — if a type has the right methods, it satisfies the trait. `explicit trait` requires an `extend` declaration. Runtime polymorphism via `any Trait` for heterogeneous collections. Structural matching and generic constraints are in [generics.md](types/generics.md); `any Trait` runtime dispatch is in [traits.md](types/traits.md).

**Concurrency.** `spawn(|| {})` for green tasks (requires `using Multitasking`), `ThreadPool.spawn(|| {})` for CPU-bound work (requires `using ThreadPool`), `Thread.spawn(|| {})` for raw OS threads. No async/await, no function coloring — I/O pauses the task automatically. Task handles must be joined or detached (compile error if forgotten). Channels transfer ownership: no copies, no locks. Cooperative cancellation via cancel flag checked by I/O operations. See [concurrency/](concurrency/).

**Compile-time execution.** `comptime` runs a restricted subset of Rask in the compiler's interpreter — pure computation without I/O, pools, or concurrency. Build scripts (`build.rk`) handle full-language code generation. See [comptime.md](control/comptime.md).

**Strings.** One owned type: `string` (UTF-8, move semantics). Slicing is statement-scoped — strings own heap buffers, same as Vec. `string_view` for lightweight stored indices, `StringPool` for validated handle-based access. See [strings.md](stdlib/strings.md).

**Modules.** Package = directory. Two visibility levels: default (package-internal) and `public`. Imports are qualified by default; `using` for selective unqualified access. See [modules.md](structure/modules.md), [packages.md](structure/packages.md).

**C interop.** C calls in `unsafe` blocks. Raw pointers exist only in unsafe code. At boundaries, convert between safe Rask values and C pointers. See [unsafe.md](memory/unsafe.md), [c-interop.md](structure/c-interop.md).

---

## Design Tradeoffs

I'm not pretending there aren't costs to these choices. Every design has tradeoffs—mine are intentional.

### Clone Ergonomics

**Decision:** No storable references. References are block-scoped (struct fields, arrays) or statement-scoped (anything with a heap buffer).

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

**When this hurts:** Complex code generation that would benefit from full language features. Use build scripts (`build.rk`) for those cases.

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
3. **No shared mutable references:** Cross-task data sharing uses channels (ownership transfer), `Shared<T>` / `Mutex<T>` (closure-based access), or explicit synchronization
4. **Unsafe for low-level code:** OS/kernel work requires unsafe blocks with raw pointers

These aren't accidents—they're deliberate tradeoffs to achieve safety without annotations.
