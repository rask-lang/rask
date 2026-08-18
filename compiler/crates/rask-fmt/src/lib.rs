// SPDX-License-Identifier: (MIT OR Apache-2.0)

mod comment;
mod config;
mod printer;

pub use config::FormatConfig;

/// Why a file couldn't be formatted. There's nothing to print from a source the
/// parser didn't understand, and echoing it back is how `fmt --check` came to
/// pass every file with a syntax error in it (#801).
#[derive(Debug)]
pub enum FormatError {
    Lex(Vec<rask_lexer::LexError>),
    Parse(Vec<rask_parser::ParseError>),
}

/// Format Rask source code with default configuration.
///
/// Falls back to the original text when the source doesn't parse. Only for
/// callers with nowhere to put a diagnostic — an editor mid-keystroke. Anything
/// that can report should use `try_format_source`.
pub fn format_source(source: &str) -> String {
    try_format_source(source).unwrap_or_else(|_| source.to_string())
}

/// Format Rask source code, reporting why not.
pub fn try_format_source(source: &str) -> Result<String, FormatError> {
    try_format_source_with_config(source, &FormatConfig::default())
}

/// Format Rask source code with custom configuration.
///
/// Falls back to the original text when the source doesn't parse; see
/// `format_source`.
pub fn format_source_with_config(source: &str, config: &FormatConfig) -> String {
    try_format_source_with_config(source, config).unwrap_or_else(|_| source.to_string())
}

/// Format Rask source code with custom configuration, reporting why not.
pub fn try_format_source_with_config(
    source: &str,
    config: &FormatConfig,
) -> Result<String, FormatError> {
    let comments = comment::extract_comments(source);
    let comment_list = comment::CommentList::new(comments);

    let mut lexer = rask_lexer::Lexer::new(source);
    let lex_result = lexer.tokenize();
    if !lex_result.errors.is_empty() {
        return Err(FormatError::Lex(lex_result.errors));
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();
    if !parse_result.is_ok() {
        return Err(FormatError::Parse(parse_result.errors));
    }

    let mut p = printer::Printer::new(source, comment_list, config);
    p.format_file(&parse_result.decls);
    Ok(p.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_messy_spacing() {
        let input = "func    main(   ) {\nlet x=42\n}";
        let output = format_source(input);
        assert!(output.contains("func main()"), "should normalize spacing: {}", output);
        assert!(output.contains("let x = 42"), "should add spaces around =: {}", output);
    }

    #[test]
    fn idempotent_on_clean_code() {
        let clean = "func main() {\n    let x = 42\n    println(x.to_string())\n}\n";
        let once = format_source(clean);
        let twice = format_source(&once);
        assert_eq!(once, twice, "formatting should be idempotent");
    }

    #[test]
    fn preserves_comments() {
        let input = "// This is a comment\nfunc main() {}\n";
        let output = format_source(input);
        assert!(output.contains("// This is a comment"), "should preserve comments");
    }

    #[test]
    fn formats_struct_declaration() {
        let input = "struct Point{x:i32\ny:i32}";
        let output = format_source(input);
        assert!(output.contains("struct Point"), "should have struct name");
        assert!(output.contains("x: i32"), "should format fields with spacing: {}", output);
    }

    #[test]
    fn formats_enum_declaration() {
        let input = "enum Color{Red\nGreen\nBlue}";
        let output = format_source(input);
        assert!(output.contains("enum Color"), "should have enum name");
        assert!(output.contains("Red"), "should preserve variants");
    }

    // `format_source` still falls back for callers with nowhere to put a
    // diagnostic — the LSP formats mid-keystroke, when the buffer often doesn't
    // parse and returning the text unchanged is the only sane answer.
    #[test]
    fn returns_original_on_parse_error() {
        let broken = "func {{{ invalid syntax";
        let output = format_source(broken);
        assert_eq!(output, broken, "should return original on parse error");
    }

    // #801: the fallback is why `fmt --check` used to pass every file with a
    // syntax error in it — the echoed copy is byte-identical, so the file looked
    // formatted. Anything that can report an error asks instead of assuming.
    #[test]
    fn try_format_reports_a_parse_error() {
        let broken = "func main( {\n  let x =\n}\n";
        match try_format_source(broken) {
            Err(FormatError::Parse(errors)) => {
                assert!(!errors.is_empty(), "a parse failure carries its errors");
            }
            Err(other) => panic!("expected a parse error, got {:?}", other),
            Ok(out) => panic!("a file that doesn't parse must not format: {}", out),
        }
    }

    #[test]
    fn try_format_reports_a_lex_error() {
        // Digits past `u128::MAX` — the lexer's own refusal, before the parser.
        let broken = "func main() {\n    let a = 340282366920938463463374607431768211456\n}\n";
        match try_format_source(broken) {
            Err(FormatError::Lex(errors)) => {
                assert!(!errors.is_empty(), "a lex failure carries its errors");
            }
            Err(other) => panic!("expected a lex error, got {:?}", other),
            Ok(out) => panic!("a file that doesn't lex must not format: {}", out),
        }
    }

    // #801: a statement's span runs to the newline that terminates it, which is
    // past its trailing comment — so "is the comment on this line" answered no
    // every time and every trailing comment in the tree moved onto its own line.
    // That changes what the comment annotates.
    #[test]
    fn trailing_comments_stay_on_their_line() {
        let input = "func main() {\n    let a = 4  // one\n    let b = 5  // another\n}\n";
        let output = format_source(input);
        assert!(
            output.contains("let a = 4  // one"),
            "a trailing comment stays put: {}", output,
        );
        assert!(
            output.contains("let b = 5  // another"),
            "including the next one: {}", output,
        );
        assert_eq!(output, format_source(&output), "and idempotently: {}", output);
    }

    // The other half of the same rule: a comment alone on its line is a
    // standalone comment and doesn't get pulled up onto the code above it.
    #[test]
    fn standalone_comments_keep_their_own_line() {
        let input = "func main() {\n    let a = 4\n    // about b\n    let b = 5\n}\n";
        let output = format_source(input);
        assert!(
            output.contains("let a = 4\n    // about b"),
            "a standalone comment stays standalone: {}", output,
        );
    }

    #[test]
    fn formats_function_with_params() {
        let input = "func add(a:i32,b:i32)->i32{return a+b}";
        let output = format_source(input);
        assert!(output.contains("a: i32"), "should space params: {}", output);
        assert!(output.contains("-> i32"), "should space return type: {}", output);
    }

    #[test]
    fn formats_extend_block() {
        let input = "struct Foo{}\nextend Foo{\nfunc bar(self)->i32{return 1}\n}";
        let output = format_source(input);
        assert!(output.contains("extend Foo"), "should have extend block: {}", output);
        assert!(output.contains("func bar(self)"), "should format method: {}", output);
    }

    #[test]
    fn handles_empty_input() {
        let output = format_source("");
        assert!(output.is_empty() || output.trim().is_empty(), "empty input should give empty output");
    }

    #[test]
    fn handles_multiline_function() {
        let input = r#"
func process(items: Vec<i32>) -> i32 {
    let sum = 0
    for i in 0..items.len() {
        sum = sum + items[i]
    }
    return sum
}
"#;
        let output = format_source(input);
        let twice = format_source(&output);
        assert_eq!(output, twice, "multiline function should be idempotent");
    }
}
