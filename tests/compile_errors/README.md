# Compile Error Examples

This directory contains code that **should not compile**. Each file demonstrates specific safety guarantees enforced by the Rask compiler.

Each `// ERROR:` comment indicates the expected error. If the compiler accepts any of these, that's a bug in the compiler — the spec says it should be rejected.

## Files

### Syntax

| File | What it tests |
|------|--------------|
| [syntax_rejected.rk](syntax_rejected.rk) | Rust-isms (`pub`, `fn`, `::`, `let mut`, turbofish, `?`), `const` in a body, let reassignment, missing return, chained comparison |
| [rust_syntax_rejected.rk](rust_syntax_rejected.rk) | Additional Rust keyword rejections |

### Type System

| File | What it tests |
|------|--------------|
| [type_errors.rk](type_errors.rk) | Implicit bool conversion, narrowing `as`, float comparison, Option no-auto-unwrap, try type mismatch, branch type mismatch, break value types |
| [cast_rules.rk](cast_rules.rk) | `as` cast rules: narrowing (CV2), sign reinterpret (CV3), float→int (CV4), int→char (CH5), int↔bool (BL3); conversion methods used where their policy means nothing — `floor` on an integer, `wrap` to a float, `round` int→int, `clamp` on a float (CV11–CV16, E0818); `as` to a collection or a struct, which reinterprets bits rather than converting and needs `unsafe` to say so (E0838, #862) |
| [index_types.rk](index_types.rk) | Index expression types: integer for Vec/slice/string, `K` for Map, `Handle<T>` for Pool; range slicing only on sequences (#310, V1, PL4) |
| [not_iterable.rk](not_iterable.rk) | `for` over something with no elements — an integer, a string, a struct (E0827), and an index type check that only reaches a container arrived at through a field (#632) |
| [implicit_widening_limits.rk](implicit_widening_limits.rk) | The int→int pairs CV1a does *not* make implicit: `u64`→`i64`, `i64`→`u64`, `u32`→`i32`, `u8`→`i8`, and plain narrowing (CV1a, CV2) |
| [int_float_arithmetic.rk](int_float_arithmetic.rk) | `+ - * /` between an integer and a float variable (CV1a, E0371, #816) — native used to drop the float operand and answer with an integer. An unsuffixed literal still takes the float slot |
| [mixed_signedness_arithmetic.rk](mixed_signedness_arithmetic.rk) | `+ - * / %` and `& \| ^ << >>` between a signed and an unsigned integer (ORD4, E0371) — comparison is the exception and stays legal (#778) |
| [int_literal_range.rk](int_literal_range.rk) | An integer literal past the slot it lands in: needs 128 bits in an `i64`, one past `i128::MAX`, negative in a `u128` (E0825, #800) |
| [int_literal_unwritable.rk](int_literal_unwritable.rk) | The two ends no type holds — digits past `u128::MAX` (lexer) and a negative below `i128::MIN` (parser sign fold) (#800) |
| [untyped_bindings.rk](untyped_bindings.rk) | Bindings that carried no type at all, so a wrong annotation unified happily: a struct-variant pattern's fields, an `is` binding, a tuple `for` binding (E0308, #809) |
| [newline_continuation.rk](newline_continuation.rk) | A line starting with `+` — excluded from newline continuation (P3) and not a statement either (#304) |
| [bare_shared_with.rk](bare_shared_with.rk) | Bare `with shared as v` — the lock has to be named `.read()` or `.write()` (conc.sync/R4, E0839, #880). Nothing enforced it: the interpreter hit a self-contradictory runtime error and native read the wrong bytes |
| [map_key_hashable.rk](map_key_hashable.rk) | A Map key that isn't Hashable (E0834, HA1/HA4, #812) — a nominal newtype with no `with (…)` clause, a float, a struct with a float field; each gets the way out that fits it |
| [generic_arg_identity.rk](generic_arg_identity.rk) | A user type as a generic argument keeps its identity — a wrong Map key or value on `Map<K, V>.new()` (E0340/E0308, #812) |
| [type_mismatch_arg.rk](type_mismatch_arg.rk) | Wrong argument type |
| [type_mismatch_return.rk](type_mismatch_return.rk) | Wrong return type |
| [wrong_arg_count.rk](wrong_arg_count.rk) | Wrong number of arguments |
| [error_mismatch.rk](error_mismatch.rk) | Incompatible error types with `try` |
| [try_shape_rule.rk](try_shape_rule.rk) | Bare `try` whose other branch doesn't fit the return (ER47, E0360/E0361) — an absence in a `T or E` function, an error in a `T?` function (#598) |
| [ambiguous_error_wrap.rk](ambiguous_error_wrap.rk) | Two variants of the error enum wrap the same error (ER31a, E0359) — `try` asks which instead of picking |
| [optional_operators_need_optionals.rk](optional_operators_need_optionals.rk) | `??`, `!` and `take` on something that can never be absent (OPT3/OPT11/OPT13/OPT32, E0831/E0832/E0365) — including `m[k] ?? d`, which points at `.get(k)` |
| [trait_bound_messages.rk](trait_bound_messages.rk) | What a failed trait requirement says, per source: a numeric bound (E0333, members not methods), an ordinary generic bound, a conformance header, an `as any Trait` cast, and a bound naming a trait nobody declared (E0833, did-you-mean) |
| [no_auto_wrap_outside_return.rk](no_auto_wrap_outside_return.rk) | A bare `T` becomes a `T or E` at `return` only (ER11, E0828) — binding, argument (free *and* method), and field are rejected, and the optional shape is exempt |
| [error_type_named_in_diagnostics.rk](error_type_named_in_diagnostics.rk) | Three codes that mention a `T or E` all name its error type rather than leaking `<type#N>` (#646) |
| [unknown_type_name.rk](unknown_type_name.rk) | Typo'd type name in signature (PC2) — errors instead of becoming a generic |
| [type_called_as_function.rk](type_called_as_function.rk) | A struct or enum name in call position (E0345) — `Name(value)` is the nominal-type constructor (T7), structs have no tuple form (S1) |
| [single_letter_type_name.rk](single_letter_type_name.rk) | Single-letter concrete type names are reserved for type parameters (PC3) |
| [not_displayable.rk](not_displayable.rk) | Rendering a type that can't render itself: a struct that never opted in, an optional with no missing case (D3, D4) — through `{}` and through `print`/`println` as a call (#772) |
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
| [mutate_marker_required.rk](mutate_marker_required.rk) | An argument to a `mutate` parameter with no `mutate` marker (PM4/PM5, E0373) — a Copy argument and a field path are no exception; a method receiver is exempt; the marker on a non-`mutate` parameter is E0328 (#530) |
| [mutate_through_binding.rk](mutate_through_binding.rk) | Writing through a name a test or a pattern introduced (E0372, #788) — `if x? as v`, a `mutate` argument, a plain `for` element, a match-arm payload, `while x? as v`. `for mutate` and write-back through the original stay legal |
| [mutate_param_left_empty.rk](mutate_param_left_empty.rk) | A `mutate` parameter consumed and not replaced (PM2, E0836, #815) — outright and on one path only; consume-and-replace stays legal, and `take` is how a function says it keeps the value |
| [owned_not_consumed.rk](owned_not_consumed.rk) | An `own` value that nothing consumes, one consumed twice, one handed to a `take` parameter and then dropped, and one consumed on only one branch (mem.linear/L1, L3, E0837, E0800, #819) |
| [consume_borrowed_param.rk](consume_borrowed_param.rk) | Giving away a parameter the caller only lent (PM1/L1, E0835, #804) — a `take self` method, a `take` parameter, `own` at the call site, and storing it into a field, which used to be reported as a borrow conflict about a mutation that wasn't happening (#818); `take` on the declaration is the way to say it |
| [with_guard_escapes.rk](with_guard_escapes.rk) | A `with` guard's bare identifier returned as the block's own value (#559, E0829) — struct payload rejected, field read/method call/scalar payload still compile |
| [small_size_fence.rk](small_size_fence.rk) | `@small` types over the 16-byte copy threshold (SM2, E0374) — a three-`i64` struct and a two-`string` one; plus the generic half, where `Pair<i64>` fits and `Pair<string>` doesn't (SM3, E0375) (#587) |
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
| [field_annotation_forms.rk](field_annotation_forms.rk) | Serialization annotations the compiler can't act on (E19/E21, E0376) — the old `@skip` spelling, and `@rename` given a bare name or a number instead of a string literal; plus an excluded field with no default, which blocks auto-`Decode` (E13a, E0377) (#603) |
| [module_needs_import.rk](module_needs_import.rk) | A stdlib module used with no import for it (IM1, E0210) — `json` and `net` were exempt because `stdlib/http.rk` imports them into a scope shared with user code (#780) |
| [stdlib_renames.rk](stdlib_renames.rk) | task-2b rename sweep (#302): old stdlib names are hard errors, not aliases — `recv`/`try_recv`, `as_secs`/`as_secs_f64`, `os.getpid`/`os.vars`, `fs.read_file`/`write_file`/`append_file`, removed `File.lines()` (E0313) |
| [let_reassign.rk](let_reassign.rk) | Reassigning a let binding |
| [read_lock_mutate.rk](read_lock_mutate.rk) | Mutating through a `shared.read()` with-binding (E0360, conc.sync/R1) |
| [undefined_variable.rk](undefined_variable.rk) | Using undefined variable |
| [comptime_loop.rk](comptime_loop.rk) | Comptime iteration limits |
| [resource_leak.rk](resource_leak.rk) | Resource type not consumed |
| [optional_resource.rk](optional_resource.rk) | A `@resource` inside an optional is still linear — the binding, the `? as` payload, and a `none` that gets filled (E0805, mem.linear/L1, #827) |
| [resource_field_debts.rk](resource_field_debts.rk) | A holder owes each resource field separately — closing one leaves the others, reported by field path (E0805, mem.linear/L1, #828) |
| [context_missing.rk](context_missing.rk) | Missing pool context clause |
| [context_ambiguous.rk](context_ambiguous.rk) | Ambiguous pool context |
| [context_unavailable.rk](context_unavailable.rk) | Pool context not in scope |
| [context_unnamed_structural.rk](context_unnamed_structural.rk) | Unnamed context used as binding |
| [context_on_entry_point.rk](context_on_entry_point.rk) | A `using` clause on the entry point (CC11, E0831) — nothing can supply the hidden param, so it used to run on garbage (#732) |

## Running Tests

```bash
rask test-specs tests/compile_errors/
```

Each file includes `// ERROR:` comments indicating expected error patterns. If the compiler accepts any of these files, it's a compiler bug — the spec requires rejection.
