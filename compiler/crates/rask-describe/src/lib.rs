// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask describe` — structured module API summaries.

pub mod extract;
pub mod text;
pub mod types;

pub use types::{DescribeOpts, ModuleDescription};

/// Parse source and produce a module description.
pub fn describe(source: &str, file: &str, opts: DescribeOpts) -> ModuleDescription {
    let mut lexer = rask_lexer::Lexer::new(source);
    let lex_result = lexer.tokenize();
    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();

    extract::extract(&parse_result.decls, file, &opts)
}

/// Serialize a description to JSON.
pub fn describe_json(desc: &ModuleDescription) -> String {
    serde_json::to_string_pretty(desc).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}

/// Format a description as human-readable text.
pub fn describe_text(desc: &ModuleDescription) -> String {
    text::format_text(desc)
}
