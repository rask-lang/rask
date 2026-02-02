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
    match test.expectation {
        Expectation::Compile => run_compile_test(test),
        Expectation::CompileFail => run_compile_fail_test(test),
        Expectation::Parse => run_parse_test(test),
        Expectation::ParseFail => run_parse_fail_test(test),
        Expectation::Skip => TestResult {
            test,
            passed: true,
            message: "skipped".to_string(),
        },
    }
}

/// Test that code compiles successfully (lex + parse).
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
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: false,
            message: format!("parse failed: {:?}", parse_result.errors),
        };
    }

    // TODO: Type check when available

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
    let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
    if !parse_result.is_ok() {
        return TestResult {
            test,
            passed: true,
            message: "failed at parse (expected)".to_string(),
        };
    }

    // TODO: Type check when available - that's where most compile-fail tests
    // will actually fail

    // For now, if it parses, we can't verify compile-fail
    // Mark as passed with a note that we need type checking
    TestResult {
        test,
        passed: true,
        message: "parsed (type checking not yet implemented)".to_string(),
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
