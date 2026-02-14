<!-- id: compiler.testing -->
<!-- status: decided -->
<!-- summary: Test strategy for compiler validation across all phases -->

# Test Infrastructure

**Note:** The spec testing command is `rask verify-specs` (renamed from `rask test-specs` for clarity).

Systematic validation for parser, type checker, codegen, runtime. Multiple test levels from unit to end-to-end.

## Test Pyramid

| Level | What | Tools | Location | Speed |
|-------|------|-------|----------|-------|
| **L1: Unit** | Individual functions, passes | Rust `#[test]` | Inline in source | Fast (ms) |
| **L2: Component** | Full compiler phases | Rust `tests/` dir | Per-crate tests/ | Fast (ms) |
| **L3: Spec** | Literate tests from specs | `rask verify-specs` | `specs/**/*.md` | Medium (100ms) |
| **L4: Integration** | Parser → codegen → runtime | Test suite | `tests/integration/` | Medium (100ms) |
| **L5: End-to-end** | Full programs compiled + run | Test programs | `tests/e2e/` | Slow (seconds) |
| **L6: Validation** | Real-world programs | Example apps | `examples/` | Very slow (minutes) |

## L1: Unit Tests (Inline)

Test individual functions and helper methods directly in source files.

```rust
// In compiler/crates/rask-types/src/unify.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unify_i32_i32() {
        let mut ctx = UnifyContext::new();
        let result = unify(&mut ctx, Type::I32, Type::I32);
        assert!(result.is_ok());
    }

    #[test]
    fn unify_i32_string_fails() {
        let mut ctx = UnifyContext::new();
        let result = unify(&mut ctx, Type::I32, Type::String);
        assert!(result.is_err());
    }
}
```

| Rule | Description |
|------|-------------|
| **U1: Inline location** | Tests live in same file as code under test |
| **U2: Small scope** | Test one function or method |
| **U3: Fast execution** | Complete in <10ms per test |
| **U4: No I/O** | Pure logic, no file/network access |

Target: 80%+ coverage of utility functions, data structures, algorithms.

## L2: Component Tests (tests/ dir)

Test complete compiler phases in isolation using the public API of each crate.

```
compiler/crates/rask-parser/tests/
  parser_test.rs           -- Full parse tests
  error_recovery_test.rs   -- Error recovery behavior
  span_tracking_test.rs    -- Source location tracking
```

Example test:

```rust
// compiler/crates/rask-parser/tests/parser_test.rs
#[test]
fn parse_function_declaration() {
    let source = "func add(a: i32, b: i32) -> i32 { return a + b }";
    let result = parse_source(source);

    assert!(result.is_ok());
    let ast = result.unwrap();
    assert_eq!(ast.decls.len(), 1);

    match &ast.decls[0].kind {
        DeclKind::Fn(f) => {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
        }
        _ => panic!("Expected function declaration"),
    }
}
```

| Rule | Description |
|------|-------------|
| **C1: Public API only** | Test through crate's public interface |
| **C2: Isolated phase** | Parser tests don't run type checker |
| **C3: Comprehensive** | Cover all syntax forms, edge cases |
| **C4: Error cases** | Test error detection and messages |

Target: 90%+ coverage of each compiler phase.

## L3: Spec Tests (Literate)

Tests embedded in spec markdown files using annotations. Already implemented in `rask-spec-test`.

Annotations:
- `<!-- test: parse -->` — Must parse successfully
- `<!-- test: parse-fail -->` — Must fail to parse
- `<!-- test: compile -->` — Must type-check
- `<!-- test: compile-fail -->` — Must fail type-check
- `<!-- test: run -->` — Must execute (interpreter only for now)
- `<!-- test: skip -->` — Don't test

Example from spec:

````markdown
<!-- test: parse -->
```rask
func factorial(n: u32) -> u32 {
    if n <= 1 { return 1 }
    return n * factorial(n - 1)
}
```
````

| Rule | Description |
|------|-------------|
| **S1: Documentation first** | Tests verify spec examples work |
| **S2: Single responsibility** | Each block tests one concept |
| **S3: Run on spec edit** | Auto-run via hook when specs change |
| **S4: Staleness warnings** | Warn if spec dependencies changed |

Command: `rask verify-specs specs/` (already implemented)

## L4: Integration Tests

Full compiler pipeline from source to object file or bytecode, but don't execute. Verify codegen correctness.

```
tests/integration/
  codegen/
    simple_function.rk
    simple_function.expected.ll    -- Expected LLVM IR or similar

  mir_lowering/
    match_exhaustive.rk
    match_exhaustive.expected.mir  -- Expected MIR

  monomorphization/
    generic_vec_i32.rk
    generic_vec_i32.expected.symbols  -- Expected mangled symbols
```

Test structure:

```rust
// tests/integration/codegen_test.rs
#[test]
fn test_simple_function_codegen() {
    let source = read_file("tests/integration/codegen/simple_function.rk");
    let module = compile_to_mir(source)?;

    // Verify MIR structure
    assert_eq!(module.functions.len(), 1);
    assert_eq!(module.functions[0].name, "add");

    // Lower to machine code (when implemented)
    let object = compile_to_object(module)?;
    assert!(object.contains_symbol("_R4main_F3add"));
}
```

| Rule | Description |
|------|-------------|
| **I1: Full pipeline** | Test multiple phases together |
| **I2: No execution** | Verify codegen, not runtime behavior |
| **I3: Symbol checking** | Verify name mangling correctness |
| **I4: Format validation** | Check MIR/IR structure |

**Symbol file format** (`.expected.symbols`):

Plain text, one symbol per line. Lines starting with `#` are comments. Blank lines ignored.

Example `tests/integration/monomorphization/vec_push.expected.symbols`:
```
# Expected symbols for Vec<i32> push operation

_R4core_S3Vec_Gi32
_R4core_M3Vec4push_Gi32
_R4main_F4main
```

**Matching:** Test passes if compiled object file contains all listed symbols (subset match). Additional symbols are allowed.

## L5: End-to-End Tests

Compile and execute complete programs. Verify output, exit codes, file operations.

```
tests/e2e/
  hello_world/
    main.rk
    output.txt          (expected stdout)
    exit_code           (file containing "0")

  error_handling/
    panic.rk
    stderr.txt          (expected stderr)
    exit_code           (file containing "1")

  collections/
    vec_operations.rk
    output.txt          (expected stdout)
```

**File format:**
- `output.txt` — Expected stdout (exact match)
- `stderr.txt` — Expected stderr (exact match)
- `exit_code` — Plain text file with exit code number (e.g., "0", "1")
- If `exit_code` missing, assume 0

**Interpreter vs Compiled:**

Rask has a tree-walk interpreter (already implemented) for fast testing and a compiler for full validation:
- **Interpreter**: Fast iteration, no codegen, useful for parser/type-checker validation
- **Compiler**: Full pipeline including codegen, linking, optimization

E2E tests run in both modes to ensure consistency.

Test runner:

```rust
#[test]
fn test_hello_world() {
    let test_dir = "tests/e2e/hello_world";
    let program = compile_program(&format!("{}/main.rk", test_dir));
    let output = execute_compiled(program);

    // Check exit code
    let expected_exit = read_file(&format!("{}/exit_code", test_dir))
        .unwrap_or("0".to_string())
        .trim()
        .parse::<i32>()
        .unwrap();
    assert_eq!(output.exit_code, expected_exit);

    // Check stdout
    if let Ok(expected_out) = read_file(&format!("{}/output.txt", test_dir)) {
        assert_eq!(output.stdout, expected_out);
    }

    // Check stderr
    if let Ok(expected_err) = read_file(&format!("{}/stderr.txt", test_dir)) {
        assert_eq!(output.stderr, expected_err);
    }
}
```

| Rule | Description |
|------|-------------|
| **E1: Full execution** | Compile + link + run |
| **E2: Observable behavior** | Check stdout, stderr, exit code, files |
| **E3: Realistic programs** | Multi-file, with stdlib usage |
| **E4: Both modes** | Test compiled binary AND interpreter (tree-walk interpreter for fast iteration) |

Categories:
- Basic features (variables, functions, control flow)
- Collections (Vec, Map, Pool operations)
- Error handling (try, panic, ensure)
- Concurrency (spawn, channels, join)
- FFI (calling C, being called from C)

## L6: Validation Programs

Large realistic programs that stress test the entire system. Not run in CI (too slow), but manually tested.

```
examples/
  http_server/     -- JSON API server
  grep_clone/      -- Text search tool
  game_demo/       -- Entity-component game loop
  text_editor/     -- Editor with undo
```

| Program | Tests | Lines of Code |
|---------|-------|---------------|
| HTTP server | Concurrent requests, JSON parsing, routing | ~500 |
| grep clone | File I/O, regex, multi-threading | ~300 |
| Game demo | Pool handles, update loop, hot reload | ~600 |
| Text editor | Rope data structure, undo/redo, syntax highlight | ~800 |

Run: `./run_validation.sh` (manually, not in CI)

## Test Organization

```
rask/
  compiler/
    crates/
      rask-parser/
        src/
          parser.rs               # Code + unit tests inline
        tests/
          parser_test.rs          # Component tests
      rask-types/
        src/
          checker.rs              # Code + unit tests
        tests/
          type_inference_test.rs  # Component tests

  specs/
    memory/
      borrowing.md                # Contains <!-- test: parse --> blocks

  tests/
    integration/
      codegen/
        *.rk + *.expected.*       # MIR/codegen validation
    e2e/
      */
        main.rk + expected_*.txt  # Full compile + execute

  examples/
    http_server/
      main.rk                     # Validation program
```

## Running Tests

| Command | Level | Speed | CI |
|---------|-------|-------|-----|
| `cargo test` | L1 + L2 | Fast | Yes |
| `rask verify-specs specs/` | L3 | Medium | Yes |
| `cargo test --test integration` | L4 | Medium | Yes |
| `cargo test --test e2e` | L5 | Slow | Yes |
| `./run_validation.sh` | L6 | Very slow | No |
| `cargo test --all` | L1-L5 | Medium | Yes |

CI pipeline runs L1-L5 on every PR. L6 run manually before releases.

## Error Message Testing

Every error type must have a test verifying the message format.

```rust
#[test]
fn error_message_format() {
    let source = "func f() { const x: i32 = \"hello\" }";
    let result = type_check(source);

    assert!(result.is_err());
    let error = result.unwrap_err();

    // Verify error code
    assert_eq!(error.code, "type.checker/T3");

    // Verify message structure
    assert!(error.message.contains("type mismatch"));
    assert!(error.message.contains("expected: i32"));
    assert!(error.message.contains("found: string"));

    // Verify span accuracy
    assert_eq!(error.span.start, 22);  // Start of "hello"
    assert_eq!(error.span.end, 29);    // End of "hello"
}
```

| Rule | Description |
|------|-------------|
| **ER1: Message tests** | Every error type has message format test |
| **ER2: Span accuracy** | Verify error points to correct source location |
| **ER3: Fix suggestions** | Test that fix suggestions are present |
| **ER4: Error codes** | Verify error codes match spec IDs |

## Fuzzing

Property-based testing and fuzzing for robustness.

### Parser Fuzzing

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn parser_doesnt_crash(source in "\\PC{0,1000}") {
        // Parser should never panic, only return errors
        let _ = parse_source(&source);
    }
}
```

### Type Checker Fuzzing

Generate random valid ASTs, verify type checker produces result (success or error, but never panic).

```rust
#[test]
fn type_checker_never_panics() {
    let mut gen = AstGenerator::new();
    for _ in 0..10000 {
        let ast = gen.generate_random_ast();
        let _ = type_check_ast(ast);  // Should not panic
    }
}
```

| Rule | Description |
|------|-------------|
| **F1: No panics** | Compiler never panics on any input |
| **F2: Fast rejection** | Invalid input rejected quickly |
| **F3: Deterministic** | Same input = same output |

Run: `cargo test --release fuzz` (separate from main test suite)

## Performance Benchmarks

Track compilation speed, memory usage, generated code quality.

```rust
// benches/compilation_speed.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_parse_large_file(c: &mut Criterion) {
    let source = read_file("benches/fixtures/large_program.rk");

    c.bench_function("parse 10k lines", |b| {
        b.iter(|| parse_source(black_box(&source)))
    });
}

criterion_group!(benches, bench_parse_large_file);
criterion_main!(benches);
```

Run: `cargo bench`

Metrics:
- Parse time per line of code
- Type checking time per AST node
- Codegen time per function
- Memory usage during compilation
- Generated code size
- Generated code performance

## Test Data Fixtures

Shared test data for multiple test levels.

```
tests/fixtures/
  simple_program.rk           -- Minimal valid program
  all_syntax.rk               -- Exercises every syntax form
  large_enum.rk               -- 100+ variant enum
  deep_nesting.rk             -- Deeply nested expressions
  unicode.rk                  -- Unicode identifiers and strings
```

## Continuous Integration

GitHub Actions workflow:

```yaml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
      - run: cargo test --all
      - run: cargo build --release
      - run: ./target/release/rask verify-specs specs/
      - run: cargo test --test integration
      - run: cargo test --test e2e
```

## Test Metrics

Track test coverage and quality:

| Metric | Target | Measurement |
|--------|--------|-------------|
| Line coverage | 80%+ | `cargo tarpaulin` |
| Branch coverage | 75%+ | `cargo tarpaulin` |
| Error message tests | 100% | Manual audit |
| Spec test success | 100% | `rask verify-specs` |
| E2E test pass rate | 100% | CI results |

## Testing New Features

When adding a new feature:

1. **Spec first**: Write spec with `<!-- test: -->` annotations
2. **Unit tests**: Test individual functions as you write them
3. **Component tests**: Add `tests/` file for the phase (parser, types, etc.)
4. **Integration test**: Add `.rk` + `.expected.*` file
5. **E2E test**: Add full program in `tests/e2e/`
6. **Update validation**: Ensure validation programs still compile

## Debugging Failed Tests

Test failures should provide:
- Source code location
- Expected vs actual values
- Relevant context (AST, types, MIR)
- Reproduction steps

Example:

```
FAILED: tests/integration/codegen/simple_function.rk
Expected symbol: _R4main_F3add
Found symbols:   _R4main_F3add_Gi32i32

Hint: Name mangling includes parameter types.
Check: compiler/specs/name-mangling.md rule M1
```

## Error Messages for Test Failures

When spec tests fail:

```
ERROR: Spec test failed in specs/memory/borrowing.md:42

Source:
  const x = vec![1, 2, 3]
  const y = x
  x.push(4)  // ERROR expected

Expected: compile-fail
Got: compiled successfully

Hint: This test expects a borrow checker error.
      The code should fail because 'x' was moved to 'y'.
```

## See Also

- `compiler.mangling` — Symbol naming rules to validate
- `compiler.layout` — Memory layout to verify in codegen tests
- Existing `rask-spec-test` crate — Spec testing implementation
