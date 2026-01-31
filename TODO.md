# Rask Design TODO

Gaps and incomplete areas identified in the design documents.

---

## High Priority (Core Design)

### Control Flow
- [x] ~~`if`/`else` semantics~~ (specs/control-flow.md)
- [x] ~~`while` loops~~ (specs/control-flow.md)
- [x] ~~`loop` (infinite loop with break)~~ (specs/control-flow.md)
- [x] ~~`break`/`continue` with labels~~ (specs/control-flow.md)
- [x] ~~`return` semantics~~ (specs/control-flow.md)
- [x] ~~Expression vs statement distinction~~ (specs/control-flow.md)

### Primitives and Numeric Types
- [x] ~~Full list of primitive types~~ (specs/primitives.md)
- [x] ~~Floating point semantics (NaN, infinity, comparison)~~ (specs/primitives.md)
- [x] ~~Casting rules between numeric types~~ (specs/primitives.md)
- [x] ~~Numeric traits beyond `Numeric`~~ (specs/primitives.md)
- [x] ~~Boolean operations~~ (specs/primitives.md)
- [ ] SIMD types — Built-in vector types?
- [ ] `char` necessity — Is `char` needed or just use `u32` + validation?

### Union Types (Type System)
- [ ] Union type syntax (`A | B`)
- [ ] Canonical ordering and deduplication
- [ ] Subtyping rules (`A ⊆ A | B`)
- [ ] Memory layout (max size + discriminant)
- [ ] Pattern matching across union members
- [ ] Interaction with generics

---

## Medium Priority (Ecosystem)

### Testing Framework
- [x] ~~Test file convention (`*_test.rask`)~~ (specs/testing.md)
- [x] ~~Test function syntax~~ (`test "name" {}` blocks, specs/testing.md)
- [x] ~~Assertion functions~~ (`assert`/`check`, specs/testing.md)
- [x] ~~Test runner behavior~~ (specs/testing.md)
- [x] ~~Mocking/dependency injection patterns~~ (trait-based, specs/testing.md)
- [ ] Benchmark support (`bench` blocks?)
- [ ] Fuzzing / property-based testing
- [ ] Code coverage tooling

### Build System (`rask.build`)
- [ ] Build script syntax and capabilities
- [ ] Relationship to comptime
- [ ] Dependency on external tools
- [ ] Asset bundling
- [ ] Code generation hooks

### Standard Library Outline
- [ ] Core module contents
- [ ] I/O module (`io`, `fs`)
- [ ] Networking (`net`)
- [ ] Time and duration
- [ ] What's built-in vs imported?

### Error Types
- [x] ~~Built-in `Error` type definition~~ (specs/error-types.md)
- [x] ~~Custom error definition patterns~~ (specs/error-types.md)
- [x] ~~Error trait requirements~~ (specs/error-types.md)
- [x] ~~Error conversion/wrapping~~ (union types, specs/error-types.md)
- [ ] Panic vs Error guidelines

### Operators
- [ ] Full operator precedence table
- [ ] Bitwise operators (`&`, `|`, `^`, `~`, `<<`, `>>`)
- [ ] Assignment operators (`+=`, `-=`, etc.)
- [ ] Logical operators (`&&`, `||`, `!`)
- [ ] Comparison operators

---

## Lower Priority (Details)

### Attributes/Annotations
- [ ] Attribute syntax (`#[...]`)
- [ ] Built-in attributes list
- [ ] Custom attribute support (if any)
- [ ] Conditional compilation attributes

### Macros
- [ ] `format!` macro specification
- [ ] Macro system design (if planned)
- [ ] Procedural vs declarative macros

### Validation: Test Programs
Walk through the 7 litmus test programs from CLAUDE.md:
- [ ] HTTP JSON API server
- [ ] grep clone
- [ ] Text editor (dynamic buffer, undo)
- [ ] Log aggregation (streaming)
- [ ] Sensor processor (fixed memory, real-time)
- [ ] Game loop (dynamic entities)
- [ ] Database (indexes, caching)

---

## Known Issues from Specs

### Concurrency (from sync-concurrency.md)
- [ ] Linear types + channels silent failure (RAII wrapper silences close errors)
- [ ] Nursery nesting rules unclear
- [ ] Thread pool and resource limits unspecified
- [ ] Channel drop with items ("best-effort" undefined)

### Async (from async-runtime.md)
- [ ] Sync nursery blocks async runtime

### Comptime (from compile-time-execution.md)
- [ ] Should comptime have limited heap allocation (Vec/Map)?
- [ ] Comptime memoization strategy
- [ ] Step-through debugger for comptime?
- [ ] Which stdlib functions are comptime-compatible?

### Unsafe (from unsafe.md)
- [ ] Atomics and memory ordering specification
- [ ] Inline assembly (`asm!`) syntax and semantics
- [ ] Pointer provenance rules (Stacked Borrows equivalent?)
- [x] ~~UB detection tooling~~ (debug-mode safety added)
- [x] ~~Formal unsafe contracts~~ (contract syntax and exit invariants added)

---

## Design Review Concerns

Identified during comprehensive design review (2026-01):

### Closure Model Complexity
- [ ] Expression-scoped vs storable closure distinction is subtle — needs crystal-clear compiler errors
- [ ] Rules for when closure "accesses outer scope" vs "captures" need better documentation
- [ ] Learning curve concern: users may struggle without excellent diagnostics

### Multi-Element Access Ergonomics
- [ ] Closure pattern for multi-statement collection access adds ceremony:
      `pool.modify(h, |entity| { ... })?` vs hypothetical `with pool[h] as entity { ... }`
- [ ] Consider whether syntax sugar could reduce friction (low priority)

### Shared State Primitives
- [ ] No shared mutable state primitives beyond channels
- [ ] Read-heavy patterns (shared config, metrics) are inconvenient with channels only
- [ ] Consider `Arc<ReadWrite<T>>` equivalents or read-optimized concurrent containers

### Type System Gaps
- [ ] `Box<T>` semantics only mentioned in passing (sum-types.md) — needs full spec
- [ ] Trait composition syntax inconsistent: `:` in definition vs `+` in bounds (`HashKey<T>: Hashable<T> + Clone<T>`)
- [ ] Extension method import syntax is unique (`import string_utils::String.ext.to_snake_case`) — ensure it's intuitive

### Parameter Modes
- [ ] `read`, `mutate`, `transfer` parameter modes used everywhere but never formally specified in one place
- [ ] Need consolidated parameter passing spec (currently scattered across memory-model, structs, etc.)

### Syntax Still TBD
- [ ] Nursery syntax marked "TBD pending full language syntax design" (sync-concurrency.md)
- [ ] `discard` keyword for wildcards on non-Copy types (sum-types.md) — needs documentation
- [ ] Exact attribute syntax (`#[...]` vs `@...`) not finalized

### Compilation Model Concerns
- [ ] Intra-package init order "UNSPECIFIED" (module-system.md) — could cause subtle bugs
- [ ] Semantic hash caching for generics may be complex to implement correctly

### Metrics Validation Needed
- [ ] User study: Closure scope rules (BC1-BC5) predictability for PI ≥ 0.85 validation
- [ ] Quantify UCC: Test 10 canonical programs against 80%+ coverage claim
- [ ] SN metric calibration: Current 0.3 target may need adjustment (Go error handling is ~2.4)
- [ ] Add metrics sections to specs without them (~17 specs missing explicit metrics references)

### Specs Missing Metrics References
These specs were written without explicit METRICS.md consideration:
- [ ] integer-overflow.md — should reference MC (Mechanical Correctness)
- [ ] control-flow.md — should reference ED, SN (expression-oriented reduces nesting)
- [ ] sum-types---enums.md — should reference ED, SN (pattern matching vs type assertions)
- [ ] async-and-concurrency.md — should quantify UCC 80%+ claim
- [ ] string-handling.md — should reference ED (layered complexity, simpler than Rust)
- [ ] structs.md — should reference TC (no hidden copies)

---

## Cross-References to Add

These specs mention features that should link to other specs:

- [x] ~~String spec → Iteration spec~~ (fixed)
- [x] ~~Memory model → Structs spec~~ (specs/structs.md created)
- [x] ~~Generics → Unsafe spec~~ (specs/unsafe.md created)
- [ ] Module system → Build system spec (when created)
