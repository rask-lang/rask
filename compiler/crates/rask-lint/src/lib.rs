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
