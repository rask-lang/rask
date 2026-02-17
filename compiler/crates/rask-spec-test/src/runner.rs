// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Run extracted spec tests through the compiler.

use crate::extract::{Expectation, SpecTest};

/// Result of running a single spec test.
#[derive(Debug)]
pub struct TestResult {
    /// The test that was run
    pub test: SpecTest,
    /// Whether the test passed
    pub passed: bool,
    /// Description of what happened
    pub message: String,
}

/// Run a single spec test and return the result.
pub fn run_test(test: SpecTest) -> TestResult {
    match test.expectation.clone() {
        Expectation::Compile => run_compile_test(test),
        Expectation::CompileFail => run_compile_fail_test(test),
        Expectation::Parse => run_parse_test(test),
        Expectation::ParseFail => run_parse_fail_test(test),
        Expectation::Skip => TestResult {
            test,
            passed: true,
            message: "skipped".to_string(),
        },
        Expectation::Run(expected) => run_run_test(test, &expected),
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
        };
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
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
            };
        }
    };

    // Type check
    if let Err(errors) = rask_types::typecheck(resolved, &parse_result.decls) {
        return TestResult {
            test,
            passed: false,
            message: format!("type check failed: {:?}", errors),
        };
    }

    TestResult {
        test,
        passed: true,
        message: "compiled".to_string(),
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
        };
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at parse (expected)".to_string(),
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
            };
        }
    };

    // Type check
    if rask_types::typecheck(resolved, &parse_result.decls).is_err() {
        return TestResult {
            test,
            passed: true,
            message: "failed at typecheck (expected)".to_string(),
        };
    }

    // All stages passed — expected failure didn't happen
    TestResult {
        test,
        passed: false,
        message: "expected compile failure, but compiled successfully".to_string(),
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
        };
    }

    // Parse
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
        };
    }

    TestResult {
        test,
        passed: true,
        message: "parsed".to_string(),
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
        };
    }

    // Parse
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at parse (expected)".to_string(),
        };
    }

    TestResult {
        test,
        passed: false,
        message: "expected parse failure, but parsed successfully".to_string(),
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
        "enum ", "struct ", "func ", "extend ", "trait ",
        "import ", "export ", "public enum ", "public struct ",
        "public func ", "public trait ",
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

/// Test that code runs and produces expected output.
fn run_run_test(test: SpecTest, expected: &str) -> TestResult {
    // Wrap in main if needed
    let code = wrap_in_main(&test.code);

    // Lex
    let lex_result = rask_lexer::Lexer::new(&code).tokenize();
    if !lex_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("lex failed: {:?}", lex_result.errors),
        };
    }

    // Parse
    let mut parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
        };
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
                TestResult {
                    test,
                    passed: true,
                    message: "output matched".to_string(),
                }
            } else {
                TestResult {
                    test,
                    passed: false,
                    message: format!(
                        "output mismatch:\n  expected: {:?}\n  actual:   {:?}",
                        expected_trimmed, actual_trimmed
                    ),
                }
            }
        }
        Err(e) => TestResult {
            test,
            passed: false,
            message: format!("runtime error: {}", e),
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
}

impl TestSummary {
    pub fn add(&mut self, result: &TestResult) {
        self.total += 1;
        if result.passed {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
    }
}
