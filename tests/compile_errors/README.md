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
| [cast_rules.rk](cast_rules.rk) | `as` cast rules: narrowing (CV2), sign reinterpret (CV3), float→int (CV4), int→char (CH5), int↔bool (BL3); misused conversion forms (CV5–CV10) |
| [index_types.rk](index_types.rk) | Index expression types: integer for Vec/slice/string, `K` for Map, `Handle<T>` for Pool; range slicing only on sequences (#310, V1, PL4) |
| [type_mismatch_arg.rk](type_mismatch_arg.rk) | Wrong argument type |
| [type_mismatch_return.rk](type_mismatch_return.rk) | Wrong return type |
| [wrong_arg_count.rk](wrong_arg_count.rk) | Wrong number of arguments |
| [error_mismatch.rk](error_mismatch.rk) | Incompatible error types with `try` |
| [ambiguous_error_wrap.rk](ambiguous_error_wrap.rk) | Two variants of the error enum wrap the same error (ER31a, E0359) — `try` asks which instead of picking |
| [unknown_type_name.rk](unknown_type_name.rk) | Typo'd type name in signature (PC2) — errors instead of becoming a generic |
| [type_called_as_function.rk](type_called_as_function.rk) | A struct or enum name in call position (E0345) — `Name(value)` is the nominal-type constructor (T7), structs have no tuple form (S1) |
| [single_letter_type_name.rk](single_letter_type_name.rk) | Single-letter concrete type names are reserved for type parameters (PC3) |
| [not_displayable.rk](not_displayable.rk) | `{}` on a type that can't render itself: a struct that never opted in, an optional with no missing case (D3, D4) |
| [unimplemented_module_fn.rk](unimplemented_module_fn.rk) | A stdlib module function marked `@unimplemented` — caught at the call instead of segfaulting there (#506) |
| [nominal_trait_not_listed.rk](nominal_trait_not_listed.rk) | A nominal newtype inherits only the traits its `with (…)` clause lists (T10) |
| [missing_return.rk](missing_return.rk) | Function without return statement |
| [trait_bound_unsatisfied.rk](trait_bound_unsatisfied.rk) | Type argument doesn't implement the bound's trait (#314) |
| [trait_bound_missing_method.rk](trait_bound_missing_method.rk) | Method not provided by the type param's bounds (#314) |
| [generic_disjointness.rk](generic_disjointness.rk) | Generic instantiation collapses `T or E` into `E or E` (ER3a, #488) — free function, propagated through a generic caller, and a method on a generic receiver |

### Ownership & Borrowing

| File | What it tests |
|------|--------------|
| [ownership_errors.rk](ownership_errors.rk) | Use-after-move, conditional move, @unique, @resource leak/double-consume, Vec never Copy |
| [linear_containers.rk](linear_containers.rk) | Vec/Map can't hold linear elements (RC1/RC3): annotation, push, param, return, field, transitive, nested, optional, alias, Map value/key (E0820) |
| [branch_merge.rk](branch_merge.rk) | Branch-merge soundness (O3, L1): move/consume on one branch of if, if-without-else, and match arms; move inside a loop body |
| [borrow_errors.rk](borrow_errors.rk) | Mutating read-only param, moving from borrow, storing slices, borrow escape, structural mutation in `with`, non-Copy element binding |
| [borrow_stored.rk](borrow_stored.rk) | Storing a string slice in a struct |
| [ensure_cancellation.rk](ensure_cancellation.rk) | `ensure` cancellation must be statically definite (C3/C4): resource consumed on some merging paths but not all — if-without-else, single match arm, nested block (E0821) |

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
| [stdlib_renames.rk](stdlib_renames.rk) | task-2b rename sweep (#302): old stdlib names are hard errors, not aliases — `recv`/`try_recv`, `as_secs`/`as_secs_f64`, `os.getpid`/`os.vars`, `fs.read_file`/`write_file`/`append_file`, removed `File.lines()` (E0313) |
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
