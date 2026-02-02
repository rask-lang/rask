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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        // Look for test annotation comments
        if let Some(expectation) = parse_annotation(lines[i]) {
            // Skip the annotation line
            i += 1;

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

/// Parse a test annotation comment.
/// Returns None if not a test annotation.
fn parse_annotation(line: &str) -> Option<Expectation> {
    let trimmed = line.trim();

    // Must be an HTML comment
    if !trimmed.starts_with("<!--") || !trimmed.ends_with("-->") {
        return None;
    }

    // Extract content between <!-- and -->
    let content = trimmed
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();

    // Must start with "test:"
    let test_spec = content.strip_prefix("test:")?.trim();

    match test_spec {
        "compile" => Some(Expectation::Compile),
        "compile-fail" => Some(Expectation::CompileFail),
        "parse" => Some(Expectation::Parse),
        "parse-fail" => Some(Expectation::ParseFail),
        "skip" => Some(Expectation::Skip),
        _ => None,
    }
}

/// Check if a line is a rask code fence opening.
fn is_rask_code_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```rask") || trimmed.starts_with("``` rask")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_annotation() {
        assert_eq!(parse_annotation("<!-- test: compile -->"), Some(Expectation::Compile));
        assert_eq!(parse_annotation("<!-- test: compile-fail -->"), Some(Expectation::CompileFail));
        assert_eq!(parse_annotation("<!-- test: parse -->"), Some(Expectation::Parse));
        assert_eq!(parse_annotation("<!-- test: skip -->"), Some(Expectation::Skip));
        assert_eq!(parse_annotation("not a comment"), None);
        assert_eq!(parse_annotation("<!-- not test -->"), None);
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
}
