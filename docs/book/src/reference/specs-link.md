# Formal Specifications

The formal language specifications are maintained in the repository's `specs/` directory. These are detailed technical documents for language implementers and those who want deep understanding.

**[View Specifications →](https://github.com/rask-lang/rask/tree/main/specs)**

## Organization

Specs are organized by topic:

- **[Types](https://github.com/rask-lang/rask/tree/main/specs/types)** - Type system, generics, traits
- **[Memory](https://github.com/rask-lang/rask/tree/main/specs/memory)** - Ownership, borrowing, resources
- **[Control](https://github.com/rask-lang/rask/tree/main/specs/control)** - Loops, match, comptime
- **[Concurrency](https://github.com/rask-lang/rask/tree/main/specs/concurrency)** - Tasks, threads, channels
- **[Structure](https://github.com/rask-lang/rask/tree/main/specs/structure)** - Modules, packages, builds
- **[Stdlib](https://github.com/rask-lang/rask/tree/main/specs/stdlib)** - Standard library APIs

## Quick Access

Key specifications:

| Topic | Link |
|-------|------|
| Ownership | [ownership.md](https://github.com/rask-lang/rask/blob/main/specs/memory/ownership.md) |
| Borrowing | [borrowing.md](https://github.com/rask-lang/rask/blob/main/specs/memory/borrowing.md) |
| Collections | [collections.md](https://github.com/rask-lang/rask/blob/main/specs/stdlib/collections.md) |
| Pools | [pools.md](https://github.com/rask-lang/rask/blob/main/specs/memory/pools.md) |
| Error Types | [error-types.md](https://github.com/rask-lang/rask/blob/main/specs/types/error-types.md) |
| Concurrency | [async.md](https://github.com/rask-lang/rask/blob/main/specs/concurrency/async.md) |

## For Users vs Implementers

- **This Book** - User-facing documentation ("How do I use Rask?")
- **Specs** - Formal specifications ("How does Rask work internally?")

Most Rask users won't need the specs. If you're:
- **Building applications** → This book is for you
- **Building compilers/tools** → Read the specs
- **Curious about internals** → Specs provide complete detail

## See Also

- [CORE_DESIGN.md](https://github.com/rask-lang/rask/blob/main/specs/CORE_DESIGN.md) - Design philosophy and rationale
- [METRICS.md](https://github.com/rask-lang/rask/blob/main/specs/METRICS.md) - How the design is validated
