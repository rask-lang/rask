# Rask Language Specifications

Organized by what each category governs.

## Reading Order

**New to Rask?** Start here:
1. [memory/ownership.md](memory/ownership.md) — Foundation: ownership, borrowing, handles
2. [types/primitives.md](types/primitives.md) — Basic types
3. [control/control-flow.md](control/control-flow.md) — if, loops, expressions

---

## Types — What values can be

| Spec | Description |
|------|-------------|
| [primitives.md](types/primitives.md) | Integers, floats, bool, char, unit |
| [structs.md](types/structs.md) | Struct definition, methods, visibility |
| [enums.md](types/enums.md) | Sum types, pattern matching |
| [optionals.md](types/optionals.md) | Option<T>, T? syntax |
| [error-types.md](types/error-types.md) | Error trait, Result, union composition |
| [generics.md](types/generics.md) | Parametric polymorphism, bounds |
| [traits.md](types/traits.md) | Trait objects, dynamic dispatch |
| [iterator-protocol.md](types/iterator-protocol.md) | Iterator trait, adapters |
| [integer-overflow.md](types/integer-overflow.md) | Overflow semantics |

## Memory — How values are owned

| Spec | Description |
|------|-------------|
| [ownership.md](memory/ownership.md) | Ownership, borrowing, handles, linear types |
| [unsafe.md](memory/unsafe.md) | Unsafe blocks, raw pointers, FFI |

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
| [sync.md](concurrency/sync.md) | OS threads, nurseries, channels |
| [parallel.md](concurrency/parallel.md) | parallel_map, thread pools |
| [async.md](concurrency/async.md) | Green tasks, async/await |
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
| [collections.md](stdlib/collections.md) | Vec, Map, Pool |
| [strings.md](stdlib/strings.md) | String types, encoding |
| [iteration.md](stdlib/iteration.md) | Collection iteration patterns |
| [testing.md](stdlib/testing.md) | Test conventions |

---

## Status

Most specs are in **Draft** status. See [TODO.md](../TODO.md) for open questions.
