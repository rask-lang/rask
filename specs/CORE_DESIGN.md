# Rask Core Design

## The Struggle

I spent a long time trying to get this right: ergonomics without sacrificing transparency. Go feels great to write but gives you no safety guarantees. Rust is safe but you spend half your time fighting the borrow checker and annotating lifetimes. I wanted something in between.

The breakthrough was realizing that most of Rust's complexity comes from trying to allow storable references. If you eliminate those—make references impossible to store—you can skip lifetime annotations entirely. The cost is explicit indirection (handles instead of pointers), but that's a cost I can see and reason about.

---

## Design Principles

The nine principles below are applications of one meta-principle: **safety through visibility.** Wherever other systems-languages trade safety against ceremony, Rask tries to make safety *visible in source* — as explicit calls, scoped blocks, and named keywords — rather than hide it in destructors, lifetime annotations, or effect types. Cleanup you can see (`ensure file.close()`), aliasing control you can scope (`with`, inline access), mutation you can mark (`mutate`), cost you can spot (`.clone()`, `own`, `spawn`). The compiler still guarantees the invariants; the source still shows the mechanism. This is the thread tying the specific choices together.

### 1. Safety Without Annotation

I enforce memory safety through the type system and scope rules without requiring lifetime markers, borrow annotations, or ownership syntax at call sites.

**What this means:**
- No lifetime parameters in function signatures
- No borrow checker annotations
- No `&`, `&mut`, or equivalent markers
- Safety is a property of well-typed programs, not extra work

### 2. Everything is a Value

There is no distinction between "value types" and "reference types." Every type in Rask is a value — it has a single owner, it copies or moves on assignment, and it's freed when the owner goes out of scope.

**What this means:**
- Assigning or passing a value either copies it (for small types) or moves it (transfers ownership)
- No implicit sharing; aliasing is explicit and controlled
- Memory layout is predictable and cache-friendly

**This applies uniformly:**
- Primitives (`i32`, `bool`, `f64`) — values
- Structs — values (embed their fields)
- Enums — values (inline payloads)
- Collections (`Vec<T>`, `Map<K,V>`) — values that own heap buffers
- Strings (`string`) — immutable refcounted values (Copy)
- `any Trait` — values that own heap-allocated concrete data
- `Cell<T>` — values that own a heap-allocated inner value
- `Shared<T>`, `Mutex<T>` — values that own thread-safe inner data
- Closures — values that own their captured data

There's no `Box<T>` because there's no need to distinguish "heap-allocated value" from "value." Some values happen to own heap memory internally — that's an implementation detail, not a type-system concept. The allocation is visible at creation (`Vec.new()`, `Cell.new()`), not in the type's behavior.

**Why this matters:** When everything is a value, the ownership rules apply everywhere identically. Move a `Vec` and the buffer moves. Move a `Cell` and the inner value moves. Move an `any Widget` and the heap data moves. One model for owned data — the uniform rule.

**Honest carve-out: a small fixed set of language primitives with shared semantics.** `string`, `Shared<T>`, `Mutex<T>`, and `Atomic*<T>` are values in the ownership sense (single owner, move on assignment), but their internal semantics are refcounted or shared. `string.clone()` is a refcount bump, not a deep copy; `Shared<T>.clone()` shares access with other holders; moving a `Shared<T>` moves one reference to data that may have other references. These are not types users can define — they're compiler-privileged. The [Box family](memory/boxes.md) collects them under named disciplines (`Pool`, `Cell`, `Shared`, `Mutex`, `Owned`, plus `Atomic*` as adjacent); consult it when you need cross-scope mutable access. The uniformity claim holds for user-defined types; the primitives are the exceptions you should know about.

**Design space:** This approach is called *mutable value semantics* (MVS). The core idea: ban aliasing instead of banning mutation, then provide controlled mutation through parameter modes (`mutate`) and scoped access (`with`). [Hylo](https://www.hylo-lang.org/) (formerly Val, from Google Research) pioneered this as a formal model. [Rue](https://github.com/steveklabnik/rue) (by Steve Klabnik, author of *The Rust Programming Language*) explores the same tradeoff with `inout` parameters. Swift's value types are a partial version. Where Rask differs: `with` blocks for multi-statement collection access, `Pool`+`Handle` for graphs, disjoint field borrowing for partial borrows, and context clauses for implicit state threading — solutions to problems that pure MVS hits once you go beyond simple value passing.

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
- Incremental compilation is *bounded propagation*, not whole-program: a private body change invalidates direct callers whose inferred signatures shift, and transitively only as long as each hop's signature keeps shifting. Most changes stop at the first caller; none walk the whole graph.
- Compilation speed scales linearly with code size

**Clarification:** Body-local inference for private functions IS local analysis. The compiler examines one function body at a time, solving constraints within that scope. It does not trace through call graphs or analyze callers. See [Gradual Constraints](types/gradual-constraints.md).

**Why this matters:** Rust's borrow checker does global analysis. Change one function and the ripple effects are unpredictable. I want compilation to scale linearly—doubling your codebase should double compile time, not quadruple it. Local checking plus bounded invalidation makes this possible. The bound matters: "one function = one check" is true, but "one function = one invalidation" is not — we don't claim it.

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

### 9. Information Without Enforcement

The compiler tracks information the language deliberately keeps out of the type system. That information surfaces through tooling — ghost text, lints, generated docs, warnings — but never becomes a constraint that splits the ecosystem or colors function signatures.

**What this means:**
- I/O, async, and mutation effects are tracked transitively (`comp.effects`) but don't appear in function signatures and don't color call syntax. A caller of a function that does I/O writes the call the same way as a caller of a pure one.
- `@pure` is a lint annotation, not a type qualifier. A pure function can call an impure one; the lint warns.
- IDE ghost annotations show parameter modes, closure captures, inferred types, and pause points — the compiler knows, the source doesn't say.

**Honest carve-out: pool contexts color signatures.** `using Pool<T>` (and its named/frozen variants) is declared in signatures and propagates up the call graph via `mem.context/CC5`, because a pool is a value callees dereference — it must be threaded through as a hidden parameter reference. This is scope-level coloring, deliberately traded for uncolored call syntax. See [context-clauses.md](memory/context-clauses.md).

**Multitasking and ThreadPool do NOT color signatures.** `using Multitasking { ... }` is a block that installs a process-global runtime; it never appears on a function signature. The compiler infers which functions transitively reach `spawn` (as internal metadata, invisible in source) and checks the caller's lexical scope; cases that static analysis can't prove fall through to a runtime panic. See [concurrency/runtime.md](concurrency/runtime.md) and [concurrency/async.md](concurrency/async.md).

**Why I chose this:** Effect systems and async/await color every call site. Rask keeps effects as metadata (no call-site color) and restricts capability coloring to the one place it actually buys something — pool references, which are real values. Runtime capabilities are a single process-level resource and don't need to be threaded through every signature.

This principle is what makes the async model (no `async`/`await` at call sites), the purity story (no effect types), and the IDE experience (ghost annotations everywhere) coherent rather than a list of compromises. The common rule: the compiler knows, tooling shows, call syntax stays clean. Capability requirements remain visible at the signature boundary where policy decisions belong.

---

## Why Not X?

### Why Not Garbage Collection?

GC languages (Go, Java, C#) are ergonomic but give you no control over when cleanup happens. You can't predict when the GC will run or how long it will pause. For games, real-time systems, or anything with latency requirements, this is a non-starter.

I want deterministic cleanup. When a value goes out of scope, it's freed immediately. No pauses, no tuning GC parameters, no wondering why your 99th percentile latency spikes.

### Why Not Reference Counting?

Ref counting (Swift, Python) solves the GC pause problem but introduces overhead on every assignment and has the cycle problem. You end up with weak references and manual cycle breaking, which brings back the same cognitive load you were trying to avoid.

I'd rather have explicit `.clone()` calls than hidden overhead on every pointer operation.

`string` is the deliberate exception — immutable data where refcounting is safe and the ergonomic payoff is highest. The compiler aggressively elides the atomic ops (`comp.string-refcount-elision`). For mutable types, the argument stands: move semantics over hidden refcount overhead.

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

**Borrowing.** References are block-scoped for fixed-layout sources (struct fields, arrays — valid until end of enclosing block). Growable sources (Vec, Pool, Map, string) use inline access: expression-scoped for one-liners, `with...as` blocks for multi-statement operations. Cannot be stored in structs, returned, or sent cross-task. See [borrowing.md](memory/borrowing.md).

**Parameters.** Three modes declared in the signature: borrow (default, read-only), `mutate` (mutable access, caller keeps ownership), and `take` (ownership transfer). Passing `value.field` to a `mutate` parameter borrows only that field — disjoint fields don't conflict. See [parameters.md](memory/parameters.md), [borrowing.md](memory/borrowing.md).

**Collections.** `Vec<T>` for sequences, `Map<K,V>` for key-value lookup, `Pool<T>` for handle-based sparse storage (graphs, entities, caches). All growth operations return `Result` — allocation is fallible. `with pool[h] as entity { ... }` for multi-statement element access. See [collections.md](stdlib/collections.md), [pools.md](memory/pools.md).

**Context clauses.** `using` is Rask's ambient-context mechanism, full stop. A function declares its ambient dependencies (`using Pool<T>`, `using Multitasking`, `using ThreadPool`) and the compiler threads them as hidden parameters — no runtime registry, no lookup cost, compile-time checked. Public functions declare their contexts (part of the API contract); private functions can have them inferred. Today `using` threads pool dependencies, the multitasking runtime, and thread pools; the mechanism is general and intentionally so. See [context-clauses.md](memory/context-clauses.md).

**Error handling.** Errors are values: `T or E` is a builtin sum type with type-based branch disambiguation (no `Ok`/`Err` wrappers), `try` for propagation, `T?` for optionals with `??` fallback and `x!` force-unwrap. Error types compose with `A | B` union syntax. `E` must implement `ErrorMessage` (structural `message() -> string`). Disjointness rule (T ≠ E) makes construction unambiguous; newtype is the escape hatch. No exceptions, no hidden control flow. See [error-types.md](types/error-types.md), [optionals.md](types/optionals.md).

**Resource cleanup.** `ensure` guarantees cleanup runs when a block exits (normal, early return, `try`). LIFO order, explicit consumption cancels it (transaction pattern). Linear resources must be consumed exactly once — compiler enforces this. See [ensure.md](control/ensure.md), [resource-types.md](memory/resource-types.md).

**Pattern matching.** `match` for multiple branches, `if x is Pattern` for single checks. Compiler infers binding modes (borrow vs take) from usage. See [enums.md](types/enums.md), [control-flow.md](control/control-flow.md).

**Closures.** Closures capture what they use. If captured data is owned, the closure can go anywhere. If captured data is borrowed, the closure is limited to that scope. Mutable captures use explicit `mutate` annotation. The compiler optimizes inline closures (iterator chains) to access the outer scope directly. IDE shows capture list as ghost annotation. `Cell<T>` provides single-value mutable containers for sharing across closures without Pool+Handle ceremony — accessed via `with cell as v { ... }` (mutable by default). See [closures.md](memory/closures.md), [cell.md](memory/cell.md).

**Traits.** Structural matching by default — if a type has the right methods, it satisfies the trait. `explicit trait` requires an `extend` declaration. Runtime polymorphism via `any Trait` for heterogeneous collections. Structural matching and generic constraints are in [generics.md](types/generics.md); `any Trait` runtime dispatch is in [traits.md](types/traits.md).

**Concurrency.** `spawn(|| {})` for green tasks (requires `using Multitasking`), `ThreadPool.spawn(|| {})` for CPU-bound work (requires `using ThreadPool`), `Thread.spawn(|| {})` for raw OS threads. Call sites are uncolored — no `async`/`await` — and I/O pauses the task automatically. The cost of invisible suspension: a function reading from a socket may pause mid-call, including with locks held. Lint rules and IDE `[io]` annotations partially recover that visibility; the type system does not. Capability requirements (`using Multitasking` et al.) are declared in signatures and propagate via `mem.context/CC5`. Task handles must be joined or detached (compile error if forgotten). Channels transfer ownership: no copies, no locks. Cooperative cancellation via cancel flag checked by I/O operations. See [concurrency/](concurrency/).

**Compile-time execution.** `comptime` runs a restricted subset of Rask in the compiler's interpreter — pure computation without I/O, pools, or concurrency. Build scripts (`build.rk`) handle full-language code generation. See [comptime.md](control/comptime.md).

**Strings.** One type: `string` (UTF-8, immutable, refcounted, Copy). `StringBuilder` for construction (UTF-8 by construction — zero-copy `build()`). Slicing is inline — `.to_string()` copies bytes into a new independent string (no shared backing). `StringPool` for validated handle-based access. See [strings.md](stdlib/strings.md).

**Modules.** Package = directory. Two visibility levels: default (package-internal) and `public`. Imports are qualified by default; `using` for selective unqualified access. See [modules.md](structure/modules.md), [packages.md](structure/packages.md).

**C interop.** C calls in `unsafe` blocks. Raw pointers exist only in unsafe code. At boundaries, convert between safe Rask values and C pointers. See [unsafe.md](memory/unsafe.md), [c-interop.md](structure/c-interop.md).

---

## Design Tradeoffs

I'm not pretending there aren't costs to these choices. Every design has tradeoffs—mine are intentional.

### Clone Ergonomics

**Decision:** No storable references. References are block-scoped (struct fields, arrays) or inline expression-scoped (anything with a heap buffer). Multi-statement access via `with`.

**Cost:** Collections (`Vec`, `Map`) and custom types that need sharing require explicit `.clone()` calls. Strings copy freely — `string` is immutable, refcounted, and Copy (16 bytes). The remaining clones concentrate at API boundaries for collections, not strings.

**Benefit:** No lifetime annotations, no borrow checker complexity, no "fighting the borrow checker" experience. The mental model is simple: values are owned, borrows are temporary.

**Comparison:**
- **Go:** Copies strings freely (implicit, GC handles memory)
- **Rust:** Requires lifetime annotations or `.clone()` to avoid moves
- **Rask:** Strings copy freely like Go. Collections use explicit `.clone()` (visible cost, no annotations)

**When this hurts:** Collection cloning in error handlers that capture context, shared configuration passed to multiple subsystems.

**When this is fine:** Most code. String-heavy code (CLI parsing, HTTP routing) has near-zero ceremony now that strings are Copy. The remaining clone calls are localized to API boundaries for collections.

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

**Concrete benefit — relocatable state:** Because user-visible types contain only owned values and integer handles (never pointers), pool state can be serialized, memory-mapped, and sent across processes without pointer fixup. Handles survive round-trips because they're integers, not addresses. See `mem.relocatable` for the full specification.

**Concrete benefit — no Pin in async:** State machines from spawn closures only hold owned values (closures can't capture borrows cross-task — mem.closures/SL2). Self-referential futures are impossible by construction, so `Pin` is unnecessary. Tasks are plain movable values. See conc.runtime/T1.

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

1. **Explicit cloning:** Collections and large values require explicit cloning to share access (strings are Copy — no cloning needed)
2. **Key-based indirection:** Graphs and self-referential structures use handles, not pointers
3. **No shared mutable references:** Cross-task data sharing uses channels (ownership transfer), `Shared<T>` / `Mutex<T>` (`with`-based scoped access), or explicit synchronization
4. **Unsafe for low-level code:** OS/kernel work requires unsafe blocks with raw pointers

These aren't accidents—they're deliberate tradeoffs to achieve safety without annotations.
