<!-- id: std.testing -->
<!-- status: decided -->
<!-- summary: Test blocks, assertions, table-driven tests, benchmarks, parallel execution -->

# Testing

Built-in test framework with `test` blocks, `@test` functions, assertions, parallel execution, and benchmarks.

## Test Declaration

| Rule | Description |
|------|-------------|
| **T1: Test blocks** | `test "name" { body }` — standalone, not exported, stripped in release builds |
| **T2: @test functions** | `@test` on a function makes it both a test and a callable function |
| **T3: Location** | Tests may appear inline in any `.rk` file or in separate `*_test.rk` files |
| **T4: Private access** | Inline and same-package `*_test.rk` tests can access private members; external test files see `public` only |

```rask
test "addition works" {
    assert 1 + 1 == 2
}
```

<!-- test: skip -->
```rask
@test
func config_defaults_are_valid() -> bool {
    const cfg = Config.defaults()
    assert cfg.port > 0
    return cfg.is_valid()
}
```

## Assertions

| Rule | Description |
|------|-------------|
| **A1: assert** | `assert expr` — test stops immediately on failure |
| **A2: check** | `check expr` — test continues, marked failed |
| **A3: Messages** | Both accept optional message: `assert expr, "message"` |
| **A4: Rich comparison** | `assert_eq(got, expected)` pretty-prints diff on failure |

```rask
test "multiple checks" {
    check 1 + 1 == 2
    check 2 + 2 == 4
    assert initialized()
}
```

## Table-Driven Tests

| Rule | Description |
|------|-------------|
| **T5: Tuple iteration** | Loop over tuple arrays for table-driven tests |

```rask
test "add cases" {
    for (a, b, expected) in [(1,2,3), (0,0,0), (-1,1,0)] {
        check add(a, b) == expected
    }
}
```

## Test Execution

| Rule | Description |
|------|-------------|
| **T6: Isolation** | Each test runs in isolation with no shared state |
| **T7: Parallel** | Tests run in parallel by default; opt-out with `--sequential` |
| **T8: Seeded random** | Random uses per-test seed; reproduce with `--seed X` |
| **T9: Cleanup** | Tests use `ensure` for cleanup (same semantics as regular code) |

<!-- test: skip -->
```rask
test "file processing" {
    const file = try open("test.txt")
    ensure file.close()
    assert file.read() == "expected"
}
```

## Subtests

| Rule | Description |
|------|-------------|
| **T10: Nested blocks** | `test` blocks nest for grouping. Output: `PASS: parent > child` |

```rask
test "parser" {
    test "numbers" {
        check parse("42") == some(42)
    }
    test "invalid" {
        check parse("abc") == none
    }
}
```

## Comptime Tests

| Rule | Description |
|------|-------------|
| **T11: Comptime** | `comptime test` runs during compilation; failure is a compile error |

```rask
comptime test "factorial" {
    assert factorial(5) == 120
}
```

## Skipping and Expected Failures

| Rule | Description |
|------|-------------|
| **T12: Skip** | `skip("reason")` skips the rest of the test |
| **T13: Expected failure** | `expect_fail()` inverts pass/fail — passing is a failure |

<!-- test: skip -->
```rask
test "platform specific" {
    if !platform.is_linux() { skip("linux only") }
}

test "known issue #123" {
    expect_fail()
    assert buggy_function() == correct
}
```

## Doc Tests

| Rule | Description |
|------|-------------|
| **T14: Doc extraction** | Code blocks in doc comments are extracted and run as tests |
| **T15: Block tags** | ` ``` ` or ` ```rask ` = compiled and run; ` ```no_run ` = compiled only; ` ```ignore ` = skipped |

<!-- test: skip -->
```rask
/// Adds two numbers.
///
/// ```
/// assert add(2, 3) == 5
/// ```
public func add(a: i32, b: i32) -> i32 { return a + b }
```

## Benchmarks

| Rule | Description |
|------|-------------|
| **B1: Benchmark blocks** | `benchmark "name" { body }` — stripped unless `rask benchmark` |
| **B2: Auto-calibrated** | Runner handles warmup, iteration count, and statistics (min, median, mean, max, ops/sec) |

```rask
benchmark "vec push" {
    const vec = Vec.new()
    for _ in 0..1000 {
        vec.push(42)
    }
}
```

## Mocking

| Rule | Description |
|------|-------------|
| **T16: Trait injection** | Mocking via trait-based dependency injection — no magic frameworks |

<!-- test: skip -->
```rask
trait Clock { func now() -> Timestamp }

test "schedule" {
    const fake = FakeClock { current: Timestamp(1000) }
    assert schedule(fake, Duration.seconds(5)) == Timestamp(1005)
}
```

## CLI

```
rask test              # all tests
rask test math         # module filter
rask test -f "parser"  # pattern filter
rask test --sequential # force sequential
rask test --seed X     # reproducible run
rask test --verbose    # show all names
rask benchmark         # all benchmarks
rask benchmark -f "vec"    # filter benchmarks
rask benchmark --json      # machine-readable output
```

## Error Messages

```
ERROR [std.testing/A1]: assertion failed
   |
5  |      assert count == 3
   |             ^^^^^^^^^^ left: 2, right: 3

WHY: assert stops the test immediately when the expression is false.
```

```
ERROR [std.testing/T11]: comptime test failed
   |
2  |      assert factorial(5) == 120
   |             ^^^^^^^^^^^^^^^^^^^^ left: 0, right: 120

WHY: Comptime tests run during compilation; failures are compile errors.
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `test` block in release build | Stripped entirely | T1 |
| `@test` function in release build | Function compiled; not invoked by runner | T2 |
| Nested `test` inside `@test` function | Allowed | T10 |
| `check` failure in table loop | All iterations run; test marked failed | A2 |
| `assert` failure in table loop | Test stops at failing iteration | A1 |
| `comptime test` uses I/O | Compile error (comptime subset only) | T11 |
| `benchmark` in debug build | Stripped | B1 |

---

## Appendix (non-normative)

### Rationale

**T1 vs T2:** `test` blocks are for standalone assertions. `@test` functions let you write validation helpers that double as tests — useful for self-checking config defaults or parser invariants.

**A1 vs A2:** `assert` for invariants that make the rest of the test meaningless. `check` for collecting multiple failures in one run (especially table-driven tests).

**T16 (trait injection):** No runtime mocking or monkey-patching. Dependency injection through traits keeps tests explicit and avoids hidden magic.

### Open Issues

- **Fuzzing** — Property-based / fuzz testing built-in?
- **Coverage** — Code coverage reporting approach?
- **Fixtures** — Setup/teardown beyond `ensure`?

### See Also

- `ctrl.comptime` — Compile-time execution model
- `mem.resource-types` — `ensure` cleanup semantics
