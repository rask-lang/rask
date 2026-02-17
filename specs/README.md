# Rask Language Specifications

Organized by what each category does.

## Reading Order

**New to Rask?** Start here. If you hit unfamiliar terms, check the [Glossary](GLOSSARY.md).

1. [memory/ownership.md](memory/ownership.md) — Single ownership, move semantics
2. [memory/value-semantics.md](memory/value-semantics.md) — Copy vs move, 16-byte threshold
3. [memory/borrowing.md](memory/borrowing.md) — One rule: views last as long as source is stable
4. [types/primitives.md](types/primitives.md) — Basic types
5. [control/control-flow.md](control/control-flow.md) — if, loops, expressions

**Building graphs/trees?**
- [memory/pools.md](memory/pools.md) — Handle-based indirection for cycles and entity systems

---

## Concept Index

Quick navigation by task or concept:

### Memory Management
| "How do I..." | See |
|---------------|-----|
| Store dynamic collections | [stdlib/collections.md](stdlib/collections.md) (Vec, Map) |
| Build graphs/trees with cycles | [memory/pools.md](memory/pools.md) (handles) |
| Pass data to functions | [memory/parameters.md](memory/parameters.md) (borrow vs take) |
| Ensure resources are cleaned up | [control/ensure.md](control/ensure.md) (deferred cleanup) |
| Work with files/connections | [memory/resource-types.md](memory/resource-types.md) (must-consume) |

### Error Handling
| "How do I..." | See |
|---------------|-----|
| Return errors from functions | [types/error-types.md](types/error-types.md) |
| Handle optional values | [types/optionals.md](types/optionals.md) (T?, ??) |
| Propagate errors automatically | [types/error-types.md](types/error-types.md) (try operator) |

### Concurrency
| "How do I..." | See |
|---------------|-----|
| Run tasks in parallel | [concurrency/async.md](concurrency/async.md) (spawn, join) |
| Share data between tasks | [concurrency/sync.md](concurrency/sync.md) (Mutex, Shared) |
| Wait on multiple channels | [concurrency/select.md](concurrency/select.md) |
| Use lock-free primitives | [memory/atomics.md](memory/atomics.md) |

### Type System
| "How do I..." | See |
|---------------|-----|
| Define custom types | [types/structs.md](types/structs.md), [types/enums.md](types/enums.md) |
| Write generic functions | [types/generics.md](types/generics.md) |
| Omit types in private functions | [types/gradual-constraints.md](types/gradual-constraints.md) |
| Define interfaces/contracts | [types/traits.md](types/traits.md) |
| Work with iterators | [types/iterator-protocol.md](types/iterator-protocol.md) |

### Low-Level
| "How do I..." | See |
|---------------|-----|
| Call C code | [structure/c-interop.md](structure/c-interop.md) |
| Call Rust code (via C ABI) | [structure/build.md](structure/build.md) (compile_rust) |
| Use raw pointers | [memory/unsafe.md](memory/unsafe.md) |
| Run code at compile time | [control/comptime.md](control/comptime.md) |
| Work with binary data | [types/binary.md](types/binary.md), [stdlib/bits.md](stdlib/bits.md) |

---

## Key Terms

| Term | Definition Location |
|------|---------------------|
| Handle | [memory/pools.md](memory/pools.md) — Opaque identifier into Pool |
| Borrow | [memory/borrowing.md](memory/borrowing.md) — Temporary read/write access |
| Take | [memory/parameters.md](memory/parameters.md) — Ownership transfer |
| Resource type | [memory/resource-types.md](memory/resource-types.md) — Must be consumed exactly once (linear resource) |
| Instant view | [memory/borrowing.md](memory/borrowing.md) — View released at semicolon (growable sources) |
| Persistent view | [memory/borrowing.md](memory/borrowing.md) — View held until block ends (fixed sources) |
| ensure | [control/ensure.md](control/ensure.md) — Deferred cleanup at scope exit |
| comptime | [control/comptime.md](control/comptime.md) — Compile-time execution |
| Gradual constraints | [types/gradual-constraints.md](types/gradual-constraints.md) — Omitting types/bounds in non-public functions |

---

## Types — What values can be

| Spec | Description |
|------|-------------|
| [primitives.md](types/primitives.md) | Integers, floats, bool, char, unit |
| [simd.md](types/simd.md) | SIMD vectors, masking, reductions, shuffles |
| [structs.md](types/structs.md) | Struct definition, methods, visibility |
| [enums.md](types/enums.md) | Sum types, pattern matching |
| [optionals.md](types/optionals.md) | Option<T>, T? syntax |
| [error-types.md](types/error-types.md) | Error trait, Result, union composition |
| [generics.md](types/generics.md) | Parametric polymorphism, constraints |
| [gradual-constraints.md](types/gradual-constraints.md) | Type/bound inference for private functions |
| [traits.md](types/traits.md) | Trait objects, dynamic dispatch |
| [iterator-protocol.md](types/iterator-protocol.md) | Iterator trait, adapters |
| [integer-overflow.md](types/integer-overflow.md) | Overflow semantics |
| [binary.md](types/binary.md) | Binary structs, bit-level layouts |

## Memory — How values are owned

| Spec | Description |
|------|-------------|
| [ownership.md](memory/ownership.md) | Core ownership rules, cross-task transfer |
| [value-semantics.md](memory/value-semantics.md) | Copy vs move, 16-byte threshold, move-only types |
| [borrowing.md](memory/borrowing.md) | Views last as long as source is stable |
| [parameters.md](memory/parameters.md) | Parameter modes: borrow (default) vs `take` |
| [resource-types.md](memory/resource-types.md) | Must-consume resources (linear resources), `ensure` integration |
| [closures.md](memory/closures.md) | Capture rules, scope constraints, Pool+Handle pattern |
| [pools.md](memory/pools.md) | Handle-based indirection, weak handles, cursors, freezing |
| [unsafe.md](memory/unsafe.md) | Unsafe blocks, raw pointers, FFI |
| [atomics.md](memory/atomics.md) | Atomic types, memory orderings, lock-free primitives |

## Control — How execution flows

| Spec | Description |
|------|-------------|
| [control-flow.md](control/control-flow.md) | if/else, match, break/continue |
| [loops.md](control/loops.md) | for-in syntax, desugaring |
| [ranges.md](control/ranges.md) | Range types, reverse iteration |
| [ensure.md](control/ensure.md) | Deferred cleanup |
| [comptime.md](control/comptime.md) | Compile-time execution |

## Concurrency — How tasks run in parallel

See [concurrency/README.md](concurrency/README.md) for the layered design.

| Spec | Description |
|------|-------------|
| [sync.md](concurrency/sync.md) | OS threads, channels, synchronization |
| [async.md](concurrency/async.md) | Green tasks, Multitasking |
| [select.md](concurrency/select.md) | Select statement, multiplexing |

## Structure — How code is organized

| Spec | Description |
|------|-------------|
| [modules.md](structure/modules.md) | Modules, imports, visibility |
| [packages.md](structure/packages.md) | Dependencies, versioning |
| [targets.md](structure/targets.md) | Library vs binary |

## Stdlib — Standard library

| Spec | Description |
|------|-------------|
| [README.md](stdlib/README.md) | **Overview**: 24 modules, batteries-included philosophy |
| [collections.md](stdlib/collections.md) | Vec, Map (indexed and keyed collections) |
| [strings.md](stdlib/strings.md) | String types, encoding |
| [iteration.md](stdlib/iteration.md) | Collection iteration patterns |
| [bits.md](stdlib/bits.md) | Bit manipulation, binary parsing |
| [testing.md](stdlib/testing.md) | Test conventions |

## Tooling — Developer tools

| Spec | Description |
|------|-------------|
| [canonical-patterns.md](canonical-patterns.md) | One obvious way: canonical patterns for every common operation |
| [rejected-features.md](rejected-features.md) | Why I didn't add: async/await, algebraic effects, lifetimes, supervision |
| [tooling/describe-schema.md](tooling/describe-schema.md) | `rask describe` JSON schema for module API summaries |
| [tooling/lint.md](tooling/lint.md) | `rask lint` naming convention and pattern enforcement |
| [tooling/debugging.md](tooling/debugging.md) | Debugging strategy: DWARF, time-travel, pool inspectors |

## Compiler — Compiler internals

| Spec | Description |
|------|-------------|
| [generation-coalescing.md](compiler/generation-coalescing.md) | Redundant generation check elimination |
| [semantic-hash-caching.md](compiler/semantic-hash-caching.md) | Incremental compilation, semantic hashing for generics |
| [hidden-params.md](compiler/hidden-params.md) | Hidden parameter compiler pass for `using` clauses |

---

## Status

Most specs are in **Draft** status. See [TODO.md](../TODO.md) for open questions.
