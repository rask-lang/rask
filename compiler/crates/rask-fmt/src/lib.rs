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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_messy_spacing() {
        let input = "func    main(   ) {\nconst x=42\n}";
        let output = format_source(input);
        assert!(output.contains("func main()"), "should normalize spacing: {}", output);
        assert!(output.contains("const x = 42"), "should add spaces around =: {}", output);
    }

    #[test]
    fn idempotent_on_clean_code() {
        let clean = "func main() {\n    const x = 42\n    println(x.to_string())\n}\n";
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

    #[test]
    fn returns_original_on_parse_error() {
        let broken = "func {{{ invalid syntax";
        let output = format_source(broken);
        assert_eq!(output, broken, "should return original on parse error");
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
