# Testing Framework

Built-in test framework with `test` blocks, `assert`/`check` assertions, `ensure` cleanup, parallel execution, seeded random, and comptime tests.

## Test Declaration

Tests are declared with `test` blocks. No attributes, no special function signatures.

```rask
test "addition works" {
    assert 1 + 1 == 2
}
```

| Property | Value |
|----------|-------|
| Syntax | `test "description" { body }` |
| Location | Anywhere a statement is valid |
| Visibility | Not exported, not callable |
| Compilation | Stripped in release builds |

## Test Location

Tests MAY appear:
1. **Inline** — In any `.rk` file, near the code they test
2. **Separate files** — In `*_test.rk` files in the same directory

| Location | Private access | Use case |
|----------|---------------|----------|
| Inline | Yes | Unit tests near implementation |
| `*_test.rk` (same package) | Yes | Larger test suites |
| `*_test.rk` (external) | No (`public` only) | Integration tests |

## Assertions

| Assertion | On Failure | Use Case |
|-----------|------------|----------|
| `assert expr` | Test stops immediately | Critical invariant |
| `check expr` | Test continues, marked failed | Gather all failures |

```rask
test "multiple checks" {
    check 1 + 1 == 2      // if fails, continue
    check 2 + 2 == 4      // runs even if above failed
    assert initialized()  // if fails, stop here
}
```

**Messages:**
```rask
assert a == b, "expected equal"
check x > 0, "x must be positive, got {x}"
```

**Output on failure:**
- File and line number
- Expression that failed
- Values of each side (if comparison)
- Optional message

**Rich comparison:**
```rask
assert_eq(got, expected)  // Pretty-prints diff on failure
```

## Table-Driven Tests

Native support via tuple iteration:

```rask
test "add cases" {
    for (a, b, expected) in [(1,2,3), (0,0,0), (-1,1,0)] {
        check add(a, b) == expected
    }
}
```

Named cases for clearer output:
```rask
test "parse" {
    for (name, input, expected) in [
        ("empty", "", none),
        ("single", "5", some(5)),
    ] {
        check parse(input) == expected, "case: {name}"
    }
}
```

## Test Cleanup

Tests use `ensure` for cleanup (same semantics as regular code):

```rask
test "file processing" {
    const file = try open("test.txt")
    ensure file.close()
    assert file.read() == "expected"
}
```

## Test Isolation

| Property | Behavior |
|----------|----------|
| Execution | Each test runs in isolation |
| Shared state | None between tests |
| Parallelism | Default on, opt-out with `--sequential` |
| Determinism | Random uses per-test seed |

**Seeded random:**
```rask
test "deterministic" {
    const rng = Random.from_seed(test.seed())
}
```

Re-run with same seed: `rask test --seed 0xDEADBEEF`

## Comptime Tests

Tests that verify compile-time functions:

```rask
comptime test "factorial" {
    assert factorial(5) == 120
}
```

| Property | Comptime Test | Runtime Test |
|----------|--------------|--------------|
| Execution | During compilation | After compilation |
| Available ops | Comptime subset | Full language |
| Failure | Compile error | Test failure |

## Subtests

Nested `test` blocks for grouping:

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

Output: `PASS: parser > numbers`

## Skipping and Expected Failures

```rask
test "platform specific" {
    if !platform.is_linux() { skip("linux only") }
    // ...
}

test "known issue #123" {
    expect_fail()
    assert buggy_function() == correct
}
```

## Doc Tests

Code blocks in doc comments are extracted and run as tests:

```rask
/// Adds two numbers.
///
/// ```
/// assert add(2, 3) == 5
/// ```
public func add(a: i32, b: i32) -> i32 { a + b }
```

| Block | Behavior |
|-------|----------|
| ` ``` ` or ` ```rask ` | Compiled and run |
| ` ```no_run ` | Compiled, not run |
| ` ```ignore ` | Not compiled |

## Benchmarking

### Benchmark Blocks

Benchmarks use `benchmark` blocks, mirroring `test` blocks:

```rask
benchmark "vec push" {
    const vec = Vec.new()
    for _ in 0..1000 {
        vec.push(42)
    }
}
```

| Property | Value |
|----------|-------|
| Syntax | `benchmark "description" { body }` |
| Location | Same rules as `test` blocks |
| Compilation | Stripped unless `rask benchmark` |
| Optimization | Release optimizations when run |

### Measurement

The runner handles iteration and statistics:
- Warmup (discarded)
- Auto-calibrated iterations
- Reports: min, median, mean, max, ops/sec

Entire block is timed. For setup, use helper functions.

### Benchmark CLI

```
rask benchmark              # Run all benchmarks
rask benchmark -f "vec"     # Filter by pattern
rask benchmark --json       # Machine-readable output
```

## Mocking

Trait-based injection (no magic frameworks):

```rask
trait Clock { func now() -> Timestamp }

func schedule<C: Clock>(clock: C, delay: Duration) -> Timestamp {
    clock.now() + delay
}

test "schedule" {
    const fake = FakeClock { current: Timestamp(1000) }
    assert schedule(fake, Duration.seconds(5)) == Timestamp(1005)
}
```

## Test Runner CLI

```
rask test              # Run all tests
rask test math         # Run tests in module
rask test -f "parser"  # Filter by pattern
rask test --sequential # Force sequential
rask test --seed X     # Reproducible run
rask test --verbose    # Show all names
```

---

## Remaining Issues

### Low Priority
1. **Fuzzing** — Property-based / fuzz testing built-in?
2. **Coverage** — Code coverage reporting approach?
3. **Fixtures** — Setup/teardown beyond `ensure`?
