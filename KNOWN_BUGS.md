# Known Bugs

Tracked issues discovered through test suite expansion. Each entry references the test that exposes it.

## Native Codegen (Cranelift)

### Vec indexing returns garbage values
`v[0]` produces wrong values (e.g., `85899345930` instead of `10`). Affects all Vec element access in compiled code. Vec.len() and Vec.push() work correctly.
- **Test:** t16_iterators.rk "range-based vec iteration", t15_borrowing.rk "with block reads vec element"
- **Workaround:** Use interpreter (`rask run --interp`)
- **Root cause:** Likely wrong element size or offset calculation in codegen for Vec index operations

### for-in on Vec produces wrong values
Same root cause as Vec indexing — the iterator reads garbage from Vec memory.
- **Test:** t16_iterators.rk "for-in vec sums elements"
- **Workaround:** Use range-based loops with interpreter

### f64 equality assertion fails Cranelift verifier
`assert a == 3.14` generates code that passes f64 values where Cranelift expects i64, triggering a VerifierError.
- **Test:** t14_ownership.rk "f64 copies on assignment"
- **Root cause:** Assert comparison codegen doesn't handle f64 type properly

### Struct string field access returns empty string
`struct.name` returns `""` instead of the actual value when the struct has string fields in native codegen.
- **Test:** t15_borrowing.rk "struct string field read"
- **Root cause:** Likely wrong offset or missing string copy in struct field access codegen

### Result match segfaults
Matching on `Result<i32, string>` segfaults in native codegen.
- **Root cause:** Likely wrong layout for Result enum payload extraction in codegen

### Trait objects: vtable not found
`any Trait` dispatch fails with "vtable not found" in native codegen.
- **Test:** t11_traits.rk (codegen error in tests using `any Describable`)
- **Root cause:** Vtable generation not triggered for concrete type + trait pairs

### Functions returning u64 or Result break test discovery
Having a function with `-> u64` or `-> T or E` return type in a test file causes `rask test` to discover 0 tests. This makes it impossible to test error handling via `rask test`.
- **Test:** t17b_result.rk (0 tests found), reproducible with any helper returning u64
- **Root cause:** Unknown — likely a bug in test extraction or monomorphization

### Option match arm assignment broken
Matching `Some(v) => value = v` doesn't execute in native codegen — the variable stays at its initial value.
- **Test:** t17_option.rk "option some match"
- **Root cause:** Likely enum payload extraction in match codegen doesn't write to local variable

## Type Checker

### Const reassignment not enforced
`const x = 42; x = 43` passes type check. Should be a compile error.
- **Test:** tests/compile_errors/const_reassign.rk (integration test `#[ignore]`)
- **Spec:** const bindings are immutable

### Non-exhaustive match not enforced
`match c { Red => ... }` on a three-variant enum passes type check. Should require all variants.
- **Test:** tests/compile_errors/nonexhaustive_match.rk (integration test `#[ignore]`)
- **Spec:** Match must cover all variants or have wildcard

### Missing return not enforced
`func get_value() -> i32 { }` passes type check. Should require a return statement.
- **Test:** tests/compile_errors/missing_return.rk (integration test `#[ignore]`)

### Use-after-move not enforced
`const v2 = v; v.push(1)` passes type check for Vec (move-only type). Should be a compile error per O3.
- **Test:** t14_ownership.rk documents this in comments
- **Spec:** mem.ownership/O3

## Test Infrastructure

### `rask test` always compiles natively
No way to run test blocks through the interpreter. Tests that pass in interpreter may fail natively due to codegen bugs. The `--interp` flag only works with `rask run`, not `rask test`.
