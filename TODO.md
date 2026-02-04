# Rask Design TODO

Gaps and incomplete areas identified in the design documents.

---

## High Priority (Core Design)

### Control Flow
- [x] ~~`if`/`else` semantics~~ (specs/control/control-flow.md)
- [x] ~~`while` loops~~ (specs/control/control-flow.md)
- [x] ~~`loop` (infinite loop with break)~~ (specs/control/control-flow.md)
- [x] ~~`break`/`continue` with labels~~ (specs/control/control-flow.md)
- [x] ~~`return` semantics~~ (specs/control/control-flow.md)
- [x] ~~Expression vs statement distinction~~ (specs/control/control-flow.md)

### Primitives and Numeric Types
- [x] ~~Full list of primitive types~~ (specs/types/primitives.md)
- [x] ~~Floating point semantics (NaN, infinity, comparison)~~ (specs/types/primitives.md)
- [x] ~~Casting rules between numeric types~~ (specs/types/primitives.md)
- [x] ~~Numeric traits beyond `Numeric`~~ (specs/types/primitives.md)
- [x] ~~Boolean operations~~ (specs/types/primitives.md)
- [x] ~~SIMD types~~ (specs/types/simd.md — Vec[T, N] with native width, masking, reductions, shuffles)
- [ ] `char` necessity — Is `char` needed or just use `u32` + validation?

### Union Types (Type System)
- [x] ~~Union type syntax (`A | B`)~~ (specs/types/union-types.md)
- [x] ~~Canonical ordering and deduplication~~ (specs/types/union-types.md)
- [x] ~~Subtyping rules (`A ⊆ A | B`)~~ (specs/types/union-types.md)
- [x] ~~Memory layout (max size + discriminant)~~ (specs/types/union-types.md)
- [x] ~~Pattern matching across union members~~ (specs/types/union-types.md)
- [x] ~~Interaction with generics~~ (specs/types/union-types.md)

---

## Medium Priority (Ecosystem)

### Testing Framework
- [x] ~~Test file convention (`*_test.rask`)~~ (specs/testing.md)
- [x] ~~Test function syntax~~ (`test "name" {}` blocks, specs/testing.md)
- [x] ~~Assertion functions~~ (`assert`/`check`, specs/testing.md)
- [x] ~~Test runner behavior~~ (specs/testing.md)
- [x] ~~Mocking/dependency injection patterns~~ (trait-based, specs/testing.md)
- [x] ~~Benchmark support~~ (`benchmark` blocks, specs/stdlib/testing.md)
- [ ] Fuzzing / property-based testing
- [ ] Code coverage tooling

### Build System (`rask.build`)
- [ ] Build script syntax and capabilities
- [ ] Relationship to comptime
- [ ] Dependency on external tools
- [ ] Asset bundling
- [ ] Code generation hooks

### Standard Library Outline
- [x] ~~Core module contents~~ (specs/stdlib/README.md)
- [x] ~~What's built-in vs imported?~~ (specs/stdlib/README.md — Prelude section)
- [x] ~~Batteries-included scope~~ (specs/stdlib/README.md — 24 modules total)

**Core I/O (High Priority):**
- [ ] `io` — Reader/Writer traits
- [ ] `fs` — File operations
- [ ] `path` — Path manipulation

**Networking & Web (High Priority):**
- [ ] `net` — TCP/UDP sockets
- [ ] `http` — HTTP client and server (RFC 7230)
- [ ] `tls` — TLS/SSL connections
- [ ] `url` — URL parsing (RFC 3986)

**Data Formats:**
- [ ] `json` — JSON parsing (RFC 8259)
- [ ] `csv` — CSV parsing (RFC 4180)
- [ ] `encoding` — Base64, hex, URL encoding (RFC 4648)

**Utilities:**
- [ ] `cli` — Command-line argument parsing
- [ ] `time` — Duration, Instant, timestamps
- [ ] `hash` — SHA256, MD5, CRC32 (integrity)
- [x] ~~`bits`~~ — Bit manipulation, byte order (specs/stdlib/bits.md)
- [ ] `unicode` — Unicode utilities
- [ ] `terminal` — ANSI colors, terminal detection
- [ ] `math` — Mathematical functions
- [ ] `random` — Random number generation
- [ ] `os` — Platform-specific operations
- [ ] `fmt` — String formatting

### Error Types
- [x] ~~Built-in `Error` type definition~~ (specs/error-types.md)
- [x] ~~Custom error definition patterns~~ (specs/error-types.md)
- [x] ~~Error trait requirements~~ (specs/error-types.md)
- [x] ~~Error conversion/wrapping~~ (union types, specs/error-types.md)
- [ ] Panic vs Error guidelines

### Operators
- [x] ~~Full operator precedence table~~ (specs/types/operators.md)
- [x] ~~Bitwise operators~~ (`&`, `|`, `^`, `~`, `<<`, `>>`, specs/types/operators.md)
- [x] ~~Assignment operators~~ (`+=`, `-=`, etc., specs/types/operators.md)
- [x] ~~Logical operators~~ (`&&`, `||`, `!`, specs/types/operators.md)
- [x] ~~Comparison operators~~ (specs/types/operators.md)

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

### Concurrency (specs/concurrency/)
- [x] ~~Nursery model replaced with affine handles~~ (spawn/join/detach)
- [x] ~~Runtime vs Workers confusion~~ (now `multitasking` + `threading`)
- [x] ~~Function coloring~~ (no async/await, I/O pauses implicitly)
- [x] ~~Linear resource types + channels: close error handling on drop~~ (channels are non-linear, explicit close() for errors, implicit drop ignores errors)
- [x] ~~Channel drop with buffered items~~ (sender drop: items remain for receivers; receiver drop: items lost)
- [ ] Task-local storage syntax and semantics
- [x] ~~Select arm evaluation order~~ (`select` = random, `select_priority` = first-listed, specs/concurrency/select.md)

### Comptime (from compile-time-execution.md)
- [x] ~~Should comptime have limited heap allocation (Vec/Map)?~~ (No — workarounds exist, see specs/control/comptime.md)
- [ ] Comptime memoization strategy
- [ ] Step-through debugger for comptime?
- [ ] Which stdlib functions are comptime-compatible?

### Unsafe (from unsafe.md)
- [x] ~~Atomics and memory ordering specification~~ (specs/memory/atomics.md)
- [ ] Inline assembly (`asm!`) syntax and semantics
- [ ] Pointer provenance rules (Stacked Borrows equivalent?)
- [x] ~~UB detection tooling~~ (debug-mode safety added)
- [x] ~~Formal unsafe contracts~~ (contract syntax and exit invariants added)

---

## Design Review Concerns

Identified during comprehensive design review (2026-01):

### Closure Model Complexity
- [x] ~~Expression-scoped vs storable closure distinction is subtle~~ (added "Closures Are Suitcases" mental model + error message specs in closures.md)
- [x] ~~Rules for when closure "accesses outer scope" vs "captures" need better documentation~~ (added decision flowchart in closures.md)
- [x] ~~Learning curve concern: users may struggle without excellent diagnostics~~ (added detailed error message templates + IDE tooling section in closures.md)

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
- [x] ~~Nursery syntax~~ (replaced with affine handles: `spawn { }.join()` / `.detach()`)
- [ ] `discard` keyword for wildcards on non-Copy types (sum-types.md) — needs documentation
- [x] ~~Attribute syntax~~ (`@` prefix for both attributes and comptime intrinsics)

### Compilation Model Concerns
- [x] ~~Intra-package init order~~ (parallel topological sort, sync required for mutable state, specs/structure/modules.md)
- [ ] Semantic hash caching for generics may be complex to implement correctly

### Metrics Validation Needed
- [ ] User study: Closure scope rules (BC1-BC5) predictability for PI >= 0.85 validation (mental model and error messages added; validation still needed during prototyping)
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
