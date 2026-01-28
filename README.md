# Rask Documentation

## Design

Core language design documents.

- **[SPECIFICATION.md](design/SPECIFICATION.md)** - Complete language specification
- **[MODE-BRIDGE.md](design/MODE-BRIDGE.md)** - How ergonomic and performance modes interact
- **[SYNTAX-STUDY.md](design/SYNTAX-STUDY.md)** - Syntax design rationale
- **[C-INTEROP.md](design/C-INTEROP.md)** - C foreign function interface

## Analysis

Design validation and implementation planning.

- **[IMPLEMENTATION-NOTES.md](analysis/IMPLEMENTATION-NOTES.md)** - Compiler architecture and decisions
- **[DEVILS-ADVOCATE-REVIEW.md](analysis/DEVILS-ADVOCATE-REVIEW.md)** - Critical design review

---

## Quick Reference

### Two Modes

| Mode | Keyword | Borrow Checker | Use Case |
|------|---------|----------------|----------|
| Ergonomic | (default) | OFF | 90% of code |
| Performance | `#[perf]` | ON | Hot paths |

### Syntax Changes from Rust

| Rust | Rask |
|------|--------|
| `<T>` | `[T]` |
| `::` | `.` |
| `let x` | `x =` |
| `format!("{}", x)` | `"{x}"` |
| `value.await` | `await value` |

### Key Rules

1. **No lifetimes in structs** - Use owned data or handles
2. **Methods use `&self`** - Even in ergonomic mode
3. **Closures capture by value** - In ergonomic mode
4. **`no_std` = perf only** - No goroutine runtime
