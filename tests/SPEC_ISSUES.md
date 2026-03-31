# Spec Issues Found During Test Audit

Issues discovered comparing existing and new tests against specs. Organized by severity.

## Resolved

### 1. ~~`.unwrap()` doesn't exist in spec~~ (fixed)

Tests updated to use `x!` per spec (OPT7). `.unwrap()` removed from t07, t08, t09.

### 2. ~~`return` inside closures~~ (resolved)

**Decision:** `return` in a closure exits the **closure**, not the enclosing function. Closures are anonymous functions — same return semantics. Block-bodied closures require explicit `return`, same as functions. Expression-bodied closures (`|x| x * 2`) implicitly return.

CF26 and closures.md updated to reflect this. The existing t06_closures.rk tests are correct.

### 3. ~~`context_missing.rk` contradicts CC7~~ (fixed)

Added `public` keyword to the function. Private functions get contexts inferred (CC7); only public functions require explicit `using` clauses.

### 4. ~~`borrow_stored.rk` doesn't test borrow escape~~ (fixed)

Rewrote to actually test S3/B3: storing a string slice (`string[..]`) in a struct, returning a slice from a function. `string` is owned/Copy and fine to store; slices are temporary borrows and cannot escape.

### 5. ~~Turbofish in `context_ambiguous.rk`~~ (fixed)

Replaced `Pool::<Player>.new()` with `Pool<Player>.new()` throughout.

### 6. ~~`assert` — parens or not?~~ (resolved)

**Decision:** `assert expr` without parens, consistent with `return`, `break`, `try`. Optional message: `assert expr, "message"`. Updated SYNTAX.md.

### 7. ~~Trait body — `func` keyword or not?~~ (resolved)

**Decision:** `func` required in trait bodies, same as everywhere else. SYNTAX.md was right. Updated traits.md, generics.md, and memory-layout.md to include `func` in all trait method signatures.

### 8. ~~Qualified vs unqualified variant names in patterns~~ (resolved)

**Decision:** Both forms valid. Unqualified is idiomatic (compiler infers enum type from match subject). Qualified (`Shape.Circle`) always works. Updated enums.md to document both forms.

### 9. ~~`map.contains()` vs `map.contains_key()`~~ (fixed)

Test updated to use `contains_key()` per spec.

### 10. ~~`vec.pop()` not in spec~~ (fixed)

Added `pop()` to collections spec as V6: `vec.pop()` returns `Option<T>`, `none` on empty. Test updated to check return value.

### 11. ~~`mutate` on Copy types~~ (resolved)

The edge case table in `mem.parameters` is explicit: "Copy type + mutate: Value is copied in; mutations affect the copy." Caller never sees changes. Not a contradiction — `mutate` on Copy is intentionally a no-op for the caller. The function gets its own copy.

### 12. ~~`with` blocks — always mutable~~ (resolved)

W5 is consistent: `with` is specifically for multi-statement *mutable* access. Read-only access uses inline expressions (`v[i]` copies out for Copy types per E1-E4, `.get()` returns `Option`). The compiler warning on never-mutated `with` bindings is correct — it guides users toward inline access when mutation isn't needed. The existing `t15_borrowing.rk` tests that only read inside `with` would correctly trigger this warning.

### 13. ~~`x!` precedence with message~~ (resolved)

**Decision:** `x! "msg"` accepts a string literal or string interpolation only — not arbitrary expressions. `x! "failed for {id}"` works. No precedence ambiguity since string literals are unambiguous tokens. Updated optionals.md and error-types.md.

### 14. ~~Comptime implicit returns~~ (fixed)

Added explicit `return` to all comptime functions in comptime_loop.rk.

### 15. ~~`error_mismatch.rk` mixes concerns~~ (fixed)

Moved Rust syntax rejection tests (`pub`, `fn`, `::`, `let mut`) to a new file `compile_errors/rust_syntax_rejected.rk`. error_mismatch.rk now only tests error type mismatch.

## Test Results (interpreter)

14 pass, 5 partial, 4 parse fail. Every failure is a compiler/interpreter gap — no test bugs.

### Parse failures (parser doesn't handle valid syntax)

| Test | What's not parsed |
|------|-------------------|
| t20 | `loop` as expression, tuple destructuring |
| t23 | `with` one-liner colon syntax |
| t25 | `for mutate` |

### Missing interpreter builtins

| Test | Score | Missing |
|------|-------|---------|
| t13 | 5/6 | `contains_key` — interp only has `contains` |
| t19 | 15/17 | `to_option` on Result |
| t24 | 15/16 | `1 << 31` typed as i64 instead of i32 |

### Unimplemented language features

| Test | Score | Missing |
|------|-------|---------|
| t15 | 10/11 | `mutate` params don't propagate field changes |
| t18 | 13/19 | `?.`, auto-wrapping, `filter` on Option |
| t22 | 8/10 | `mutate` reassignment, disjoint field mutation |

### Native-only issues (interpreter passes)

| Test | Issue |
|------|-------|
| t10 | `max` shadows builtin |
| t11 | vtable missing |

## Spec Gaps (features with zero test coverage)

The most critical gaps are now covered by new test files:
- `x!`, `??`, `?.`, `none` — `t18_option_operators.rk`
- `if`/`match` as expressions, `loop`+`break value`, `is` patterns — `t20_control_expressions.rk`
- `let` rebinding — `t21_let_bindings.rk`
- `mutate`/`take` parameter modes — `t22_parameter_modes.rk`
- `with` mutation — `t23_with_blocks.rk`
- Bitwise operators — `t24_bitwise_ops.rk`
- Iterator adapters — `t25_iterator_adapters.rk`

### Still uncovered
- Float NaN/Infinity/f32 (F1-F4)
- Type conversions (`truncate to`, `saturate to`, `try convert to`) (CV1-CV10)
- Integer overflow panics (OV1-OV4) — need compile_errors/ or panic-catching tests
- `@unique` and `@resource` structs (U1-U4, R1-R2)
- `private` field access rejection
- `discard` (D1-D3)
- Error union matching syntax
- Generic trait bounds (`where T: Comparable`)
- Or-patterns in match (`A | B =>`)
- Mutable capture in closures (`|mutate count|`)
- `for mutate` on Map
- Pool iteration
