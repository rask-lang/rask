# Testing Framework

## Design Rationale

| Inspiration | Feature | Why |
|-------------|---------|-----|
| Zig/D | `test "name" {}` blocks | Lowest ceremony, tests live near code |
| Go | `check` continues on failure | Reveal all failures, not just first |
| D | Inline tests as documentation | Tests verify examples stay accurate |
| Odin | Seeded random, cleanup LIFO | Reproducible failures |
| V | Assertions removed in prod | Zero runtime cost |
| Zig | Comptime tests | Verify compile-time functions |

## Test Declaration

Tests are declared with `test` blocks. No attributes, no special function signatures.

```
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
1. **Inline** — In any `.rask` file, near the code they test
2. **Separate files** — In `*_test.rask` files in the same directory

| Location | Private access | Use case |
|----------|---------------|----------|
| Inline | Yes | Unit tests near implementation |
| `*_test.rask` (same package) | Yes | Larger test suites |
| `*_test.rask` (external) | No (`pub` only) | Integration tests |

## Assertions

| Assertion | On Failure | Use Case |
|-----------|------------|----------|
| `assert expr` | Test stops immediately | Critical invariant |
| `check expr` | Test continues, marked failed | Gather all failures |

```
test "multiple checks" {
    check 1 + 1 == 2      // if fails, continue
    check 2 + 2 == 4      // runs even if above failed
    assert initialized()  // if fails, stop here
}
```

**Messages:**
```
assert a == b, "expected equal"
check x > 0, "x must be positive, got {x}"
```

**Output on failure:**
- File and line number
- Expression that failed
- Values of each side (if comparison)
- Optional message

**Rich comparison:**
```
assert_eq(got, expected)  // Pretty-prints diff on failure
```

## Table-Driven Tests

Native support via tuple iteration:

```
test "add cases" {
    for (a, b, expected) in [(1,2,3), (0,0,0), (-1,1,0)] {
        check add(a, b) == expected
    }
}
```

Named cases for clearer output:
```
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

```
test "file processing" {
    let file = open("test.txt")?
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
```
test "deterministic" {
    let rng = Random.from_seed(test.seed())
}
```

Re-run with same seed: `rask test --seed 0xDEADBEEF`

## Comptime Tests

Tests that verify compile-time functions:

```
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

```
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

```
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

```
/// Adds two numbers.
///
/// ```
/// assert add(2, 3) == 5
/// ```
pub fn add(a: i32, b: i32) -> i32 { a + b }
```

| Block | Behavior |
|-------|----------|
| ` ``` ` or ` ```rask ` | Compiled and run |
| ` ```no_run ` | Compiled, not run |
| ` ```ignore ` | Not compiled |

## Benchmarking

### Bench Blocks

Benchmarks use `bench` blocks, mirroring `test` blocks:

```
bench "vec push" {
    let vec = Vec.new()
    for _ in 0..1000 {
        vec.push(42)
    }
}
```

| Property | Value |
|----------|-------|
| Syntax | `bench "description" { body }` |
| Location | Same rules as `test` blocks |
| Compilation | Stripped unless `rask bench` |
| Optimization | Release optimizations when run |

### Measurement

The runner handles iteration and statistics:
- Warmup (discarded)
- Auto-calibrated iterations
- Reports: min, median, mean, max, ops/sec

Entire block is timed. For setup, use helper functions.

### Bench CLI

```
rask bench              # Run all benchmarks
rask bench -f "vec"     # Filter by pattern
rask bench --json       # Machine-readable output
```

## Mocking

Trait-based injection (no magic frameworks):

```
trait Clock { fn now() -> Timestamp }

fn schedule(clock: read impl Clock, delay: Duration) -> Timestamp {
    clock.now() + delay
}

test "schedule" {
    let fake = FakeClock { current: Timestamp(1000) }
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
