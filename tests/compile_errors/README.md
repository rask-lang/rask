# Compile Error Examples

This directory contains code that **should not compile**. Each file demonstrates specific safety guarantees enforced by the Rask compiler.

Each `// ERROR:` comment indicates the expected error. If the compiler accepts any of these, that's a bug in the compiler — the spec says it should be rejected.

## Files

### Syntax

| File | What it tests |
|------|--------------|
| [syntax_rejected.rk](syntax_rejected.rk) | Rust-isms (`pub`, `fn`, `::`, `let mut`, turbofish, `?`), const reassignment, missing return, chained comparison |
| [rust_syntax_rejected.rk](rust_syntax_rejected.rk) | Additional Rust keyword rejections |

### Type System

| File | What it tests |
|------|--------------|
| [type_errors.rk](type_errors.rk) | Implicit bool conversion, narrowing `as`, float comparison, Option no-auto-unwrap, try type mismatch, branch type mismatch, break value types |
| [type_mismatch_arg.rk](type_mismatch_arg.rk) | Wrong argument type |
| [type_mismatch_return.rk](type_mismatch_return.rk) | Wrong return type |
| [wrong_arg_count.rk](wrong_arg_count.rk) | Wrong number of arguments |
| [error_mismatch.rk](error_mismatch.rk) | Incompatible error types with `try` |
| [missing_return.rk](missing_return.rk) | Function without return statement |

### Ownership & Borrowing

| File | What it tests |
|------|--------------|
| [ownership_errors.rk](ownership_errors.rk) | Use-after-move, conditional move, @unique, @resource leak/double-consume, Vec never Copy |
| [borrow_errors.rk](borrow_errors.rk) | Mutating read-only param, moving from borrow, storing slices, borrow escape, structural mutation in `with`, non-Copy element binding |
| [borrow_stored.rk](borrow_stored.rk) | Storing a string slice in a struct |

### Pattern Matching

| File | What it tests |
|------|--------------|
| [match_errors.rk](match_errors.rk) | Non-exhaustive match, wildcard on linear resource, guard without diverge, or-pattern binding mismatch |
| [nonexhaustive_match.rk](nonexhaustive_match.rk) | Non-exhaustive enum match |

### Closures

| File | What it tests |
|------|--------------|
| [closure_errors.rk](closure_errors.rk) | Double mutable capture, scope-limited escape, mutate params on closures |

### Other

| File | What it tests |
|------|--------------|
| [const_reassign.rk](const_reassign.rk) | Reassigning a const binding |
| [undefined_variable.rk](undefined_variable.rk) | Using undefined variable |
| [comptime_loop.rk](comptime_loop.rk) | Comptime iteration limits |
| [resource_leak.rk](resource_leak.rk) | Resource type not consumed |
| [context_missing.rk](context_missing.rk) | Missing pool context clause |
| [context_ambiguous.rk](context_ambiguous.rk) | Ambiguous pool context |
| [context_unavailable.rk](context_unavailable.rk) | Pool context not in scope |
| [context_unnamed_structural.rk](context_unnamed_structural.rk) | Unnamed context used as binding |

## Running Tests

```bash
rask test-specs tests/compile_errors/
```

Each file includes `// ERROR:` comments indicating expected error patterns. If the compiler accepts any of these files, it's a compiler bug — the spec requires rejection.
