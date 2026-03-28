// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Run extracted spec tests through the compiler and (optionally) native codegen.
//!
//! When a `rask_binary` path is provided, `Run` tests execute through both the
//! tree-walk interpreter and native compilation, comparing outputs. This
//! differential testing surfaces codegen/MIR bugs: if the interpreter produces
//! correct output but the native binary doesn't, the bug is in the backend.

use crate::extract::{Expectation, SpecTest};
use std::path::PathBuf;

/// Result of running a single spec test.
#[derive(Debug)]
pub struct TestResult {
    /// The test that was run
    pub test: SpecTest,
    /// Whether the interpreter test passed
    pub passed: bool,
    /// Description of what happened (interpreter)
    pub message: String,
    /// Native compilation result (only for Run tests when binary is available)
    pub native_result: Option<NativeResult>,
}

/// Result of native (compiled) execution.
#[derive(Debug)]
pub struct NativeResult {
    /// Whether native output matched expected
    pub passed: bool,
    /// Description of what happened
    pub message: String,
    /// The actual stdout from native execution (for diffing)
    pub actual_output: Option<String>,
}

/// Configuration for the test runner.
#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    /// Path to the `rask` binary for native compilation tests.
    /// When None, native tests are skipped.
    pub rask_binary: Option<PathBuf>,
}

/// Run a single spec test and return the result.
pub fn run_test(test: SpecTest) -> TestResult {
    run_test_with_config(test, &RunConfig::default())
}

/// Run a single spec test with configuration.
pub fn run_test_with_config(test: SpecTest, config: &RunConfig) -> TestResult {
    match test.expectation.clone() {
        Expectation::Compile => run_compile_test(test),
        Expectation::CompileFail => run_compile_fail_test(test),
        Expectation::Parse => run_parse_test(test),
        Expectation::ParseFail => run_parse_fail_test(test),
        Expectation::Skip => TestResult {
            test,
            passed: true,
            message: "skipped".to_string(),
            native_result: None,
        },
        Expectation::Run(expected) => run_run_test(test, &expected, config),
        Expectation::RunInterpOnly(expected) => run_run_test_interp_only(test, &expected),
    }
}

/// Test that code compiles successfully (lex + parse + resolve + typecheck).
fn run_compile_test(test: SpecTest) -> TestResult {
    // Lex
    let lex_result = rask_lexer::Lexer::new(&test.code).tokenize();
    if !lex_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("lex failed: {:?}", lex_result.errors),
            native_result: None,
        };
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
            native_result: None,
        };
    }

    // Desugar
    rask_desugar::desugar(&mut parse_result.decls);

    // Resolve
    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(errors) => {
            return TestResult {
                test,
                passed: false,
                message: format!("resolve failed: {:?}", errors),
                native_result: None,
            };
        }
    };

    // Type check
    let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(t) => t,
        Err(errors) => {
            return TestResult {
                test,
                passed: false,
                message: format!("type check failed: {:?}", errors),
                native_result: None,
            };
        }
    };

    // Ownership check
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("ownership check failed: {:?}", ownership_result.errors),
            native_result: None,
        };
    }

    TestResult {
        test,
        passed: true,
        message: "compiled".to_string(),
        native_result: None,
    }
}

/// Test that code fails to compile at some stage.
fn run_compile_fail_test(test: SpecTest) -> TestResult {
    // Lex
    let lex_result = rask_lexer::Lexer::new(&test.code).tokenize();
    if !lex_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at lex (expected)".to_string(),
            native_result: None,
        };
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at parse (expected)".to_string(),
            native_result: None,
        };
    }

    // Desugar
    rask_desugar::desugar(&mut parse_result.decls);

    // Resolve
    let resolved = match rask_resolve::resolve(&parse_result.decls) {
        Ok(r) => r,
        Err(_) => {
            return TestResult {
                test,
                passed: true,
                message: "failed at resolve (expected)".to_string(),
                native_result: None,
            };
        }
    };

    // Type check
    let typed = match rask_types::typecheck(resolved, &parse_result.decls) {
        Ok(t) => t,
        Err(_) => {
            return TestResult {
                test,
                passed: true,
                message: "failed at typecheck (expected)".to_string(),
                native_result: None,
            };
        }
    };

    // Ownership check
    let ownership_result = rask_ownership::check_ownership(&typed, &parse_result.decls);
    if !ownership_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at ownership check (expected)".to_string(),
            native_result: None,
        };
    }

    // All stages passed — expected failure didn't happen
    TestResult {
        test,
        passed: false,
        message: "expected compile failure, but compiled successfully".to_string(),
        native_result: None,
    }
}

/// Test that code parses successfully (lex + parse only).
fn run_parse_test(test: SpecTest) -> TestResult {
    // Lex
    let lex_result = rask_lexer::Lexer::new(&test.code).tokenize();
    if !lex_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("lex failed: {:?}", lex_result.errors),
            native_result: None,
        };
    }

    // Parse
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
            native_result: None,
        };
    }

    TestResult {
        test,
        passed: true,
        message: "parsed".to_string(),
        native_result: None,
    }
}

/// Test that code fails to parse.
fn run_parse_fail_test(test: SpecTest) -> TestResult {
    // Lex
    let lex_result = rask_lexer::Lexer::new(&test.code).tokenize();
    if !lex_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at lex (expected)".to_string(),
            native_result: None,
        };
    }

    // Parse
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at parse (expected)".to_string(),
            native_result: None,
        };
    }

    TestResult {
        test,
        passed: false,
        message: "expected parse failure, but parsed successfully".to_string(),
        native_result: None,
    }
}

/// Wrap code in a main function, keeping declarations at top level.
///
/// Detects enum, struct, func, extend, trait declarations and keeps them
/// outside main. Remaining statements go inside main.
fn wrap_in_main(code: &str) -> String {
    // Already has main or @entry - use as-is
    if code.contains("func main") || code.contains("@entry") {
        return code.to_string();
    }

    let decl_keywords = [
        "enum ", "struct ", "func ", "extend ", "trait ", "type ",
        "import ", "export ", "public enum ", "public struct ",
        "public func ", "public trait ", "public type ",
    ];

    let mut decls = String::new();
    let mut stmts = String::new();
    let mut in_decl = false;
    let mut brace_depth: i32 = 0;

    for line in code.lines() {
        let trimmed = line.trim();

        // Skip empty lines - add to current section
        if trimmed.is_empty() {
            if in_decl {
                decls.push('\n');
            } else {
                stmts.push('\n');
            }
            continue;
        }

        // At top level (brace_depth == 0), check if this starts a declaration
        if brace_depth == 0 && !in_decl {
            let is_decl = decl_keywords.iter().any(|kw| trimmed.starts_with(kw));
            if is_decl {
                in_decl = true;
            }
        }

        // Track braces
        for c in trimmed.chars() {
            match c {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        if in_decl {
            decls.push_str(line);
            decls.push('\n');
            // Declaration ends when braces balance back to 0
            if brace_depth == 0 {
                in_decl = false;
            }
        } else {
            stmts.push_str(line);
            stmts.push('\n');
        }
    }

    // Combine: declarations at top level, statements in main
    let decls = decls.trim_end();
    let stmts = stmts.trim_end();

    if stmts.is_empty() && decls.is_empty() {
        "func main() {}".to_string()
    } else if stmts.is_empty() {
        format!("{}\n\nfunc main() {{}}", decls)
    } else if decls.is_empty() {
        format!("func main() {{\n{}\n}}", stmts)
    } else {
        format!("{}\n\nfunc main() {{\n{}\n}}", decls, stmts)
    }
}

/// Run a test through interpreter only (escape hatch for unimplemented codegen).
fn run_run_test_interp_only(test: SpecTest, expected: &str) -> TestResult {
    let (passed, message) = run_interpreter(&test.code, expected);
    TestResult {
        test,
        passed,
        message,
        native_result: None,
    }
}

/// Run a test through both interpreter and native compilation.
fn run_run_test(test: SpecTest, expected: &str, config: &RunConfig) -> TestResult {
    let (interp_passed, interp_message) = run_interpreter(&test.code, expected);

    let native_result = config.rask_binary.as_ref().map(|binary| {
        run_native(&test.code, expected, binary)
    });

    TestResult {
        test,
        passed: interp_passed,
        message: interp_message,
        native_result,
    }
}

/// Run code through the tree-walk interpreter and compare output.
fn run_interpreter(code: &str, expected: &str) -> (bool, String) {
    let code = wrap_in_main(code);

    // Lex
    let lex_result = rask_lexer::Lexer::new(&code).tokenize();
    if !lex_result.is_ok() {
        return (false, format!("lex failed: {:?}", lex_result.errors));
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return (false, format!("parse failed: {:?}", parse_result.errors));
    }

    // Desugar
    rask_desugar::desugar(&mut parse_result.decls);

    // Run with captured output
    let (mut interp, output_buffer) = rask_interp::Interpreter::with_captured_output();

    match interp.run(&parse_result.decls) {
        Ok(_) => {
            let actual = output_buffer.lock().unwrap();
            let actual_trimmed = actual.trim_end();
            let expected_trimmed = expected.trim_end();

            if actual_trimmed == expected_trimmed {
                (true, "output matched".to_string())
            } else {
                (false, format!(
                    "output mismatch:\n  expected: {:?}\n  actual:   {:?}",
                    expected_trimmed, actual_trimmed
                ))
            }
        }
        Err(e) => (false, format!("runtime error: {}", e)),
    }
}

/// Run code through native compilation and compare output.
///
/// Writes code to a temp file, invokes `rask run <file>` (which defaults to
/// native compilation), captures stdout.
fn run_native(code: &str, expected: &str, rask_binary: &std::path::Path) -> NativeResult {
    let code = wrap_in_main(code);

    // Write to temp file
    let tmp_dir = std::env::temp_dir();
    let id = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_file = tmp_dir.join(format!("rask_spec_{}_{}.rk", id, ts));

    if let Err(e) = std::fs::write(&tmp_file, &code) {
        return NativeResult {
            passed: false,
            message: format!("failed to write temp file: {}", e),
            actual_output: None,
        };
    }

    // Run native compilation (rask run defaults to native)
    let result = std::process::Command::new(rask_binary)
        .arg("run")
        .arg(&tmp_file)
        .output();

    // Clean up temp file
    let _ = std::fs::remove_file(&tmp_file);

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let actual_trimmed = stdout.trim_end();
            let expected_trimmed = expected.trim_end();

            if !output.status.success() {
                NativeResult {
                    passed: false,
                    message: format!(
                        "native exited with {}: {}",
                        output.status.code().unwrap_or(-1),
                        stderr.lines().take(3).collect::<Vec<_>>().join("; "),
                    ),
                    actual_output: Some(stdout),
                }
            } else if actual_trimmed == expected_trimmed {
                NativeResult {
                    passed: true,
                    message: "native matched".to_string(),
                    actual_output: Some(stdout),
                }
            } else {
                NativeResult {
                    passed: false,
                    message: format!(
                        "native output mismatch:\n  expected: {:?}\n  actual:   {:?}",
                        expected_trimmed, actual_trimmed,
                    ),
                    actual_output: Some(stdout),
                }
            }
        }
        Err(e) => NativeResult {
            passed: false,
            message: format!("failed to run rask binary: {}", e),
            actual_output: None,
        },
    }
}

/// Summary statistics for a test run.
#[derive(Debug, Default)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub files: usize,
    /// Native tests attempted
    pub native_total: usize,
    /// Native tests passed
    pub native_passed: usize,
    /// Native tests failed
    pub native_failed: usize,
}

impl TestSummary {
    pub fn add(&mut self, result: &TestResult) {
        self.total += 1;
        if result.passed {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        if let Some(native) = &result.native_result {
            self.native_total += 1;
            if native.passed {
                self.native_passed += 1;
            } else {
                self.native_failed += 1;
            }
        }
    }
}
