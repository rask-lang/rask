// SPDX-License-Identifier: (MIT OR Apache-2.0)

mod comment;
mod config;
mod printer;

pub use config::FormatConfig;

/// Format Rask source code with default configuration.
/// Returns formatted source, or the original if parsing fails.
pub fn format_source(source: &str) -> String {
    format_source_with_config(source, &FormatConfig::default())
}

/// Format Rask source code with custom configuration.
pub fn format_source_with_config(source: &str, config: &FormatConfig) -> String {
    let comments = comment::extract_comments(source);
    let comment_list = comment::CommentList::new(comments);

    let mut lexer = rask_lexer::Lexer::new(source);
    let lex_result = lexer.tokenize();
    if !lex_result.errors.is_empty() {
        return source.to_string();
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();
    if !parse_result.is_ok() {
        return source.to_string();
    }

    let mut p = printer::Printer::new(source, comment_list, config);
    p.format_file(&parse_result.decls);
    p.finish()
}
