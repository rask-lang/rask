// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Extract test cases from markdown spec files.
//!
//! Scans for HTML comment annotations followed by rask code blocks:
//! ```markdown
//! <!-- test: compile -->
//! ```rask
//! func add(a: i32, b: i32) -> i32 { a + b }
//! ```
//! ```

use std::path::PathBuf;

/// What behavior we expect from a code block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// Must compile without errors (lex + parse + future type check)
    Compile,
    /// Must fail to compile, at the named stage.
    ///
    /// The stage is required. Without it the check was "failed somewhere",
    /// which a fragment satisfies by naming symbols that don't exist — 8 of the
    /// 12 `compile-fail` blocks in `specs/` passed at *resolve*, i.e. because
    /// `pool` and `player` were undefined, not because the rule under test
    /// fired. Each reported a green tick and verified nothing. Naming the stage
    /// is the same idea as the registry claim-check in `differential.sh`: a red
    /// result is only honest while it is red for the stated reason.
    CompileFail(FailStage),
    /// Must parse successfully (skip type checking)
    Parse,
    /// Must fail to parse
    ParseFail,
    /// Don't test this block
    Skip,
    /// Canon syntax the compiler hasn't caught up to yet. Expected to fail;
    /// passing is a failure that says "implemented — promote the marker."
    Pending,
    /// Run and verify output matches expected (interpreter + native)
    Run(String),
    /// Run through interpreter only — escape hatch for unimplemented codegen
    RunInterpOnly(String),
    /// A `<!-- test: … -->` comment naming something that isn't an annotation.
    ///
    /// Unknown spellings used to return `None`, which is how the extractor says
    /// "not a test annotation" — so a typo'd marker was indistinguishable from
    /// ordinary prose and the block went untested in silence. Four blocks in
    /// `specs/compiler/advanced-analyses.md` sat on `<!-- test: pass -->` that
    /// way. Carrying the bad spelling through as a test that fails is what makes
    /// it say so.
    Invalid(String),
}

/// Where a `compile-fail` block is expected to be rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailStage {
    Lex,
    Parse,
    Resolve,
    Typecheck,
    Ownership,
    /// The rule is specified and not implemented: the block compiles today and
    /// shouldn't. Expected to "fail" by being accepted, and flips loudly when
    /// the check lands — the `pending_features.txt` idea, for a spec block.
    Unbuilt,
}

impl FailStage {
    pub fn parse(s: &str) -> Option<FailStage> {
        Some(match s {
            "lex" => FailStage::Lex,
            "parse" => FailStage::Parse,
            "resolve" => FailStage::Resolve,
            "typecheck" => FailStage::Typecheck,
            "ownership" => FailStage::Ownership,
            "unbuilt" => FailStage::Unbuilt,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            FailStage::Lex => "lex",
            FailStage::Parse => "parse",
            FailStage::Resolve => "resolve",
            FailStage::Typecheck => "typecheck",
            FailStage::Ownership => "ownership",
            FailStage::Unbuilt => "unbuilt",
        }
    }
}

/// A single test case extracted from a spec file.
#[derive(Debug, Clone)]
pub struct SpecTest {
    /// Path to the source markdown file
    pub path: PathBuf,
    /// Line number where the code block starts (1-indexed)
    pub line: usize,
    /// The extracted rask code
    pub code: String,
    /// What we expect when running this code
    pub expectation: Expectation,
}

/// Extract all annotated test cases from markdown content.
pub fn extract_tests(path: &PathBuf, markdown: &str) -> Vec<SpecTest> {
    let mut tests = Vec::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Look for test annotation comments (single-line or start of multi-line)
        if let Some((expectation, lines_consumed)) = parse_annotation_multi(&lines, i) {
            // Skip the annotation line(s)
            i += lines_consumed;

            // Skip blank lines between annotation and code block
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }

            // Look for opening code fence with rask language
            if i < lines.len() && is_rask_code_fence(lines[i]) {
                let code_start_line = i + 1; // 1-indexed line number
                i += 1; // Move past the opening fence

                // Collect code until closing fence
                let mut code_lines = Vec::new();
                while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                    code_lines.push(lines[i]);
                    i += 1;
                }

                if expectation != Expectation::Skip {
                    tests.push(SpecTest {
                        path: path.clone(),
                        line: code_start_line + 1, // Convert to 1-indexed
                        code: code_lines.join("\n"),
                        expectation,
                    });
                }
            }
        }
        i += 1;
    }

    tests
}

/// Parse a test annotation comment, potentially spanning multiple lines.
/// Returns the expectation and number of lines consumed.
fn parse_annotation_multi(lines: &[&str], start: usize) -> Option<(Expectation, usize)> {
    let trimmed = lines[start].trim();

    // Must start with <!--
    if !trimmed.starts_with("<!--") {
        return None;
    }

    // Single-line annotation (ends with -->)
    if trimmed.ends_with("-->") {
        let content = trimmed
            .strip_prefix("<!--")?
            .strip_suffix("-->")?
            .trim();

        // Must start with "test:"
        let test_spec = content.strip_prefix("test:")?.trim();

        // Check for run variants with inline expected output: "run | expected"
        if test_spec.starts_with("run-interp") {
            let rest = test_spec.strip_prefix("run-interp").unwrap().trim();
            if let Some(expected) = rest.strip_prefix("|") {
                let expected = process_escapes(expected.trim());
                return Some((Expectation::RunInterpOnly(expected), 1));
            }
        } else if test_spec.starts_with("run") {
            let rest = test_spec.strip_prefix("run").unwrap().trim();
            if let Some(expected) = rest.strip_prefix("|") {
                let expected = process_escapes(expected.trim());
                return Some((Expectation::Run(expected), 1));
            }
        }

        if let Some(rest) = test_spec.strip_prefix("compile-fail") {
            let rest = rest.trim();
            let staged = rest.strip_prefix(':').map(str::trim).unwrap_or("");
            return Some(match FailStage::parse(staged) {
                Some(stage) => (Expectation::CompileFail(stage), 1),
                // Bare `compile-fail`, or a stage nobody recognises. Both are
                // the same mistake: the block claims a rejection without saying
                // which pass does the rejecting.
                None => (Expectation::Invalid(format!("compile-fail: {}", staged)), 1),
            });
        }

        let expectation = match test_spec {
            "compile" => Expectation::Compile,
            "parse" => Expectation::Parse,
            "parse-fail" => Expectation::ParseFail,
            "skip" => Expectation::Skip,
            "pending" => Expectation::Pending,
            // Reached only once the comment has already said `test:`, so this is
            // a marker someone meant, not prose that happens to look like one.
            other => Expectation::Invalid(other.to_string()),
        };
        return Some((expectation, 1));
    }

    // Multi-line annotation (for test: run)
    // Format: <!-- test: run\nexpected\noutput\n-->
    let first_line_content = trimmed.strip_prefix("<!--")?.trim();
    if !first_line_content.starts_with("test:") {
        return None;
    }

    let test_spec = first_line_content.strip_prefix("test:")?.trim();
    let interp_only = match test_spec {
        "run" => false,
        "run-interp" => true,
        other => return Some((Expectation::Invalid(other.to_string()), 1)),
    };

    // Collect expected output until -->
    let mut expected_lines = Vec::new();
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "-->" {
            let expected = expected_lines.join("\n");
            let exp = if interp_only { Expectation::RunInterpOnly(expected) } else { Expectation::Run(expected) };
            return Some((exp, i - start + 1));
        }
        if line.trim().ends_with("-->") {
            // Last line with content before -->
            let content = line.trim().strip_suffix("-->").unwrap_or("").trim_end();
            if !content.is_empty() {
                expected_lines.push(content);
            }
            let expected = expected_lines.join("\n");
            let exp = if interp_only { Expectation::RunInterpOnly(expected) } else { Expectation::Run(expected) };
            return Some((exp, i - start + 1));
        }
        expected_lines.push(line);
        i += 1;
    }

    None // Unclosed comment
}

/// Process escape sequences in expected output (e.g., \n → newline).
fn process_escapes(s: &str) -> String {
    s.replace("\\n", "\n")
}

/// Check if a line is a rask code fence opening.
fn is_rask_code_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```rask") || trimmed.starts_with("``` rask")
}

/// Check if a `.rk` file contains `test` blocks (Rask's built-in test system).
///
/// Returns true if the file has `test "..."` blocks, making it eligible for
/// differential testing via `rask test <file>`.
pub fn has_rk_tests(source: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("test \"") || trimmed.starts_with("test '")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_single(line: &str) -> Option<Expectation> {
        parse_annotation_multi(&[line], 0).map(|(e, _)| e)
    }

    #[test]
    fn test_parse_annotation_single_line() {
        assert_eq!(parse_single("<!-- test: compile -->"), Some(Expectation::Compile));
        // Bare `compile-fail` is the mistake the stage exists to catch: it
        // reads as "rejected somewhere", which a fragment satisfies by naming
        // undefined symbols.
        assert_eq!(
            parse_single("<!-- test: compile-fail -->"),
            Some(Expectation::Invalid("compile-fail: ".to_string())),
        );
        assert_eq!(
            parse_single("<!-- test: compile-fail: typecheck -->"),
            Some(Expectation::CompileFail(FailStage::Typecheck)),
        );
        assert_eq!(
            parse_single("<!-- test: compile-fail: unbuilt -->"),
            Some(Expectation::CompileFail(FailStage::Unbuilt)),
        );
        assert_eq!(
            parse_single("<!-- test: compile-fail: nonsense -->"),
            Some(Expectation::Invalid("compile-fail: nonsense".to_string())),
        );
        assert_eq!(parse_single("<!-- test: parse -->"), Some(Expectation::Parse));
        assert_eq!(parse_single("<!-- test: skip -->"), Some(Expectation::Skip));
        assert_eq!(parse_single("not a comment"), None);
        assert_eq!(parse_single("<!-- not test -->"), None);
    }

    #[test]
    fn test_parse_annotation_run_multiline() {
        let lines = vec!["<!-- test: run", "Hello", "World", "-->"];
        let result = parse_annotation_multi(&lines, 0);
        assert_eq!(result, Some((Expectation::Run("Hello\nWorld".to_string()), 4)));
    }

    #[test]
    fn test_parse_annotation_run_compact() {
        // Compact single-line format with | separator
        assert_eq!(
            parse_single("<!-- test: run | Hello -->"),
            Some(Expectation::Run("Hello".to_string()))
        );
        // With escape sequences
        assert_eq!(
            parse_single("<!-- test: run | Hello\\nWorld -->"),
            Some(Expectation::Run("Hello\nWorld".to_string()))
        );
    }

    #[test]
    fn test_extract_tests() {
        let markdown = r#"
# Example Spec

Some text here.

<!-- test: compile -->
```rask
func add(a: i32, b: i32) -> i32 { a + b }
```

More text.

<!-- test: compile-fail: typecheck -->
```rask
let x: i32 = "bad"
```

<!-- test: skip -->
```rask
// This won't be tested
```
"#;
        let path = PathBuf::from("test.md");
        let tests = extract_tests(&path, markdown);

        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].expectation, Expectation::Compile);
        assert!(tests[0].code.contains("func add"));
        assert_eq!(tests[1].expectation, Expectation::CompileFail(FailStage::Typecheck));
    }

    #[test]
    fn test_extract_run_test() {
        let markdown = r#"
<!-- test: run
Hello
-->
```rask
println("Hello")
```
"#;
        let path = PathBuf::from("test.md");
        let tests = extract_tests(&path, markdown);

        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].expectation, Expectation::Run("Hello".to_string()));
        assert!(tests[0].code.contains("println"));
    }
}
