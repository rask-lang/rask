# Spec Issues Found During Test Audit

Issues discovered comparing existing and new tests against specs. Organized by severity.

## Contradictions Between Spec and Tests

### 1. `.unwrap()` doesn't exist in spec
**Files:** `t07_option.rk`, `t08_result.rk`, `t09_vec.rk`
**Spec:** `type.optionals/OPT7`, `type.errors`

The spec defines `x!` for force unwrap. The Option method table lists `map`, `filter`, `to_result`, `is_some`, `is_none` — no `unwrap()`. The Result method table is similar. Tests use `.unwrap()` which is a Rust-ism not in the spec.

**Resolved:** `x!` is correct. Existing tests need updating to use `x!` instead of `.unwrap()`.

### 2. ~~`return` inside closures~~ (resolved)

**Decision:** `return` in a closure exits the **closure**, not the enclosing function. Closures are anonymous functions — same return semantics. Block-bodied closures require explicit `return`, same as functions. Expression-bodied closures (`|x| x * 2`) implicitly return.

CF26 and closures.md updated to reflect this. The existing t06_closures.rk tests are correct.

### 3. `context_missing.rk` contradicts CC7
**File:** `tests/compile_errors/context_missing.rk`
**Spec:** `mem.context/CC7`

The test expects a compile error for a *private* function accessing handle fields without `using`. But CC7 says private functions get unnamed contexts inferred automatically. The function should be `public` to trigger this error.

### 4. `borrow_stored.rk` doesn't test borrow escape
**File:** `tests/compile_errors/borrow_stored.rk`
**Spec:** `mem.borrowing/S3, B3`

The test claims to demonstrate "Cannot store a reference type in a struct" but stores `input: string`. In Rask, `string` is owned, immutable, refcounted, and Copy (16 bytes) — it has different semantics than other collections. Strings don't participate in `with` blocks (W2 note: "Strings are immutable — `with` doesn't apply"). Storing a `string` in a struct is completely fine. To actually test S3 (borrow escape), the test needs to attempt storing a string *slice* (`s[0..5]`) in a struct, which are temporary per B3. Line 56 also stores a slice result without `.to_string()` — that itself should be the error under test.

### 5. Turbofish in `context_ambiguous.rk`
**File:** `tests/compile_errors/context_ambiguous.rk`
**Spec:** `SYNTAX.md`

Uses `Pool::<Player>.new()` throughout. SYNTAX.md explicitly says "no turbofish." Should be `Pool<Player>.new()`.

## Spec Internal Inconsistencies

### 6. ~~`assert` — parens or not?~~ (resolved)

**Decision:** `assert expr` without parens, consistent with `return`, `break`, `try`. Optional message: `assert expr, "message"`. Updated SYNTAX.md.

### 7. ~~Trait body — `func` keyword or not?~~ (resolved)

**Decision:** `func` required in trait bodies, same as everywhere else. SYNTAX.md was right. Updated traits.md, generics.md, and memory-layout.md to include `func` in all trait method signatures.

### 8. ~~Qualified vs unqualified variant names in patterns~~ (resolved)

**Decision:** Both forms valid. Unqualified is idiomatic (compiler infers enum type from match subject). Qualified (`Shape.Circle`) always works. Updated enums.md to document both forms.

### 9. `map.contains()` vs `map.contains_key()`
**File:** `t13_map.rk`
**Spec:** `std.collections` Map Convenience Methods table

Spec says `contains_key(k)`. Test uses `contains()`. One is wrong.

### 10. `vec.pop()` not in spec
**File:** `t09_vec.rk`
**Spec:** `std.collections`

The collections spec lists `remove(i)` but not `pop()`. Either the spec is missing it or the test uses a non-existent API.

### 11. ~~`mutate` on Copy types~~ (resolved)

The edge case table in `mem.parameters` is explicit: "Copy type + mutate: Value is copied in; mutations affect the copy." Caller never sees changes. Not a contradiction — `mutate` on Copy is intentionally a no-op for the caller. The function gets its own copy.

### 12. ~~`with` blocks — always mutable~~ (resolved)

W5 is consistent: `with` is specifically for multi-statement *mutable* access. Read-only access uses inline expressions (`v[i]` copies out for Copy types per E1-E4, `.get()` returns `Option`). The compiler warning on never-mutated `with` bindings is correct — it guides users toward inline access when mutation isn't needed. The existing `t15_borrowing.rk` tests that only read inside `with` would correctly trigger this warning.

### 13. ~~`x!` precedence with message~~ (resolved)

**Decision:** `x! "msg"` accepts a string literal or string interpolation only — not arbitrary expressions. `x! "failed for {id}"` works. No precedence ambiguity since string literals are unambiguous tokens. Updated optionals.md and error-types.md.

### 14. Comptime implicit returns
**File:** `tests/compile_errors/comptime_loop.rk`
**Spec:** `ctrl.comptime/CT9`

CT9 requires explicit `return` in comptime functions. The `factorial` function in `comptime_loop.rk` uses implicit block expression returns (`if n <= 1 { 1 } else { n * factorial(n - 1) }`), which contradicts CT9.

### 15. `error_mismatch.rk` mixes concerns
**File:** `tests/compile_errors/error_mismatch.rk`

Lines 108-128 contain Rust syntax (`pub`, `fn`, `::`, `let mut`) mixed into a file whose purpose is testing error type mismatch. These should be separate test files in compile_errors/ if they're intentional syntax rejection tests.

## Spec Gaps (features with zero test coverage)

See the audit summary in the PR/commit for the full missing coverage table. The most critical gaps:
- `x!`, `??`, `?.`, `none` — now covered by `t18_option_operators.rk`
- `if`/`match` as expressions, `loop`+`break value`, `is` patterns — now covered by `t20_control_expressions.rk`
- `let` rebinding — now covered by `t21_let_bindings.rk`
- `mutate`/`take` parameter modes — now covered by `t22_parameter_modes.rk`
- `with` mutation — now covered by `t23_with_blocks.rk`
- Bitwise operators — now covered by `t24_bitwise_ops.rk`
- Iterator adapters — now covered by `t25_iterator_adapters.rk`

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
