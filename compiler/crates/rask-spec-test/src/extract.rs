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
    /// Must fail to compile at some stage
    CompileFail,
    /// Must parse successfully (skip type checking)
    Parse,
    /// Must fail to parse
    ParseFail,
    /// Don't test this block
    Skip,
    /// Run and verify output matches expected
    Run(String),
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

        // Check for run with inline expected output: "run | expected"
        if test_spec.starts_with("run") {
            let rest = test_spec.strip_prefix("run").unwrap().trim();
            if let Some(expected) = rest.strip_prefix("|") {
                let expected = process_escapes(expected.trim());
                return Some((Expectation::Run(expected), 1));
            }
        }

        let expectation = match test_spec {
            "compile" => Expectation::Compile,
            "compile-fail" => Expectation::CompileFail,
            "parse" => Expectation::Parse,
            "parse-fail" => Expectation::ParseFail,
            "skip" => Expectation::Skip,
            _ => return None,
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
    if test_spec != "run" {
        return None;
    }

    // Collect expected output until -->
    let mut expected_lines = Vec::new();
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "-->" {
            let expected = expected_lines.join("\n");
            return Some((Expectation::Run(expected), i - start + 1));
        }
        if line.trim().ends_with("-->") {
            // Last line with content before -->
            let content = line.trim().strip_suffix("-->").unwrap_or("").trim_end();
            if !content.is_empty() {
                expected_lines.push(content);
            }
            let expected = expected_lines.join("\n");
            return Some((Expectation::Run(expected), i - start + 1));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_single(line: &str) -> Option<Expectation> {
        parse_annotation_multi(&[line], 0).map(|(e, _)| e)
    }

    #[test]
    fn test_parse_annotation_single_line() {
        assert_eq!(parse_single("<!-- test: compile -->"), Some(Expectation::Compile));
        assert_eq!(parse_single("<!-- test: compile-fail -->"), Some(Expectation::CompileFail));
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

<!-- test: compile-fail -->
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
        assert_eq!(tests[1].expectation, Expectation::CompileFail);
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
