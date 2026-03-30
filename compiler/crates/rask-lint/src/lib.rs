// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask lint` — convention enforcement.

pub mod idiom;
pub mod naming;
pub mod rules;
pub mod style;
pub mod types;
mod util;

pub use types::{LintOpts, LintReport, Severity};

/// Parse source and run lint rules.
pub fn lint(source: &str, file: &str, opts: LintOpts) -> LintReport {
    let mut lexer = rask_lexer::Lexer::new(source);
    let lex_result = lexer.tokenize();
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    let diagnostics = rules::run_rules(&parse_result.decls, source, &opts);

    let error_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warning_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();

    LintReport {
        version: 1,
        file: file.to_string(),
        success: error_count == 0,
        diagnostics,
        error_count,
        warning_count,
    }
}

/// Serialize a lint report to JSON.
pub fn lint_json(report: &LintReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lint_default(source: &str) -> LintReport {
        lint(source, "test.rk", LintOpts::default())
    }

    fn has_rule(report: &LintReport, rule: &str) -> bool {
        report.diagnostics.iter().any(|d| d.rule == rule)
    }

    // ─── style/snake-case-func ──────────────────────────────

    #[test]
    fn snake_case_func_catches_camel_case() {
        let report = lint_default("func getData() -> i32 { return 1 }");
        assert!(has_rule(&report, "style/snake-case-func"),
            "should flag camelCase function name");
    }

    #[test]
    fn snake_case_func_allows_snake_case() {
        let report = lint_default("func get_data() -> i32 { return 1 }");
        assert!(!has_rule(&report, "style/snake-case-func"),
            "should not flag snake_case function name");
    }

    #[test]
    fn snake_case_method_catches_camel_case() {
        let report = lint_default(
            "struct Foo {}\nextend Foo {\n    func doThing(self) {}\n}"
        );
        assert!(has_rule(&report, "style/snake-case-func"),
            "should flag camelCase method name");
    }

    // ─── style/pascal-case-type ─────────────────────────────

    #[test]
    fn pascal_case_type_catches_snake_case_struct() {
        let report = lint_default("struct my_type { x: i32 }");
        assert!(has_rule(&report, "style/pascal-case-type"),
            "should flag snake_case struct name");
    }

    #[test]
    fn pascal_case_type_allows_pascal_case() {
        let report = lint_default("struct MyType { x: i32 }");
        assert!(!has_rule(&report, "style/pascal-case-type"),
            "should not flag PascalCase struct name");
    }

    #[test]
    fn pascal_case_type_catches_snake_case_enum() {
        let report = lint_default("enum my_color { Red, Blue }");
        assert!(has_rule(&report, "style/pascal-case-type"),
            "should flag snake_case enum name");
    }

    // ─── naming/is prefix ───────────────────────────────────

    #[test]
    fn is_prefix_flags_non_bool_return() {
        let report = lint_default(
            "struct Foo {}\nextend Foo {\n    func is_valid(self) -> i32 { return 1 }\n}"
        );
        assert!(has_rule(&report, "naming/is"),
            "is_* method returning non-bool should be flagged");
    }

    #[test]
    fn is_prefix_allows_bool_return() {
        let report = lint_default(
            "struct Foo {}\nextend Foo {\n    func is_valid(self) -> bool { return true }\n}"
        );
        assert!(!has_rule(&report, "naming/is"),
            "is_* method returning bool should not be flagged");
    }

    // ─── Clean code passes without warnings ─────────────────

    #[test]
    fn clean_code_no_warnings() {
        let source = r#"
struct Player {
    name: string
    score: i32
}

extend Player {
    func new(name: string) -> Player {
        return Player { name: name, score: 0 }
    }

    func get_score(self) -> i32 {
        return self.score
    }

    func is_active(self) -> bool {
        return self.score > 0
    }
}

func main() {}
"#;
        let report = lint_default(source);
        assert_eq!(report.warning_count, 0,
            "clean code should have no warnings, got: {:?}",
            report.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    // ─── Report structure ───────────────────────────────────

    #[test]
    fn report_includes_fix_suggestion() {
        let report = lint_default("func getData() -> i32 { return 1 }");
        let diag = report.diagnostics.iter()
            .find(|d| d.rule == "style/snake-case-func")
            .expect("should have snake-case warning");
        assert!(diag.fix.contains("get_data"),
            "fix should suggest snake_case name, got: {}", diag.fix);
    }

    #[test]
    fn lint_json_roundtrips() {
        let report = lint_default("func getData() -> i32 { return 1 }");
        let json = lint_json(&report);
        assert!(json.contains("style/snake-case-func"));
        assert!(json.contains("getData"));
    }
}
