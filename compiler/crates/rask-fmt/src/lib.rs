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

    /// #885: the printer dropped the `as v` binding on `x?`, rewriting working
    /// code into code that no longer resolves. Idempotence alone didn't catch it —
    /// the output was a stable, valid parse of a *different* program — so this
    /// checks the binding survives rather than only that a second pass agrees.
    #[test]
    fn preserves_optional_test_binding() {
        let input = "func main() {\n    if h.p? as old {\n        println(\"{old}\")\n    }\n}\n";
        let once = format_source(input);
        assert!(
            once.contains("if h.p? as old {"),
            "the `as` binding must survive formatting: {}",
            once
        );
        let twice = format_source(&once);
        assert_eq!(once, twice, "and it must be idempotent: {}", once);
    }

    /// The same form without a binding must not gain one.
    #[test]
    fn optional_test_without_binding_stays_bare() {
        let input = "func main() {\n    if h.p? {\n        println(\"yes\")\n    }\n}\n";
        let once = format_source(input);
        assert!(
            once.contains("if h.p? {") && !once.contains(" as "),
            "a bare `?` must stay bare: {}",
            once
        );
    }

    /// #896: parentheses carry precedence, and there is no `Paren` node — the
    /// printer reconstructs them. A position that forgot to pass its binding power
    /// silently reassociated the expression, so `(a + b).sqrt()` became
    /// `a + b.sqrt()`: compiles, runs, different answer.
    #[test]
    fn keeps_parentheses_that_carry_precedence() {
        let cases = [
            ("(dx * dx + dy * dy).sqrt()", "method receiver"),
            ("(i + 1)..n", "range operand"),
            ("(1..10).rev()", "range as receiver"),
            ("!(5 < 3)", "unary operand"),
            ("(a + b).field", "field access"),
        ];
        for (expr, what) in cases {
            let input = format!("func main() {{\n    let x = {}\n}}\n", expr);
            let once = format_source(&input);
            assert!(
                once.contains(expr),
                "{} lost its parentheses: {}",
                what,
                once
            );
            assert_eq!(format_source(&once), once, "and must be idempotent: {}", once);
        }
    }

    /// The parser normalises `void` to `()`, which isn't spellable in source.
    #[test]
    fn prints_void_not_unit() {
        let input = "func f() -> void or string {\n    return \"x\"\n}\n";
        let once = format_source(input);
        assert!(
            once.contains("-> void or string") && !once.contains("-> ()"),
            "`void` must not print as `()`: {}",
            once
        );
    }

    /// ER22: `else as e` binds the complement branch. `If` printed it; `IfLet`
    /// dropped it — the same shape as #885.
    #[test]
    fn preserves_else_binding() {
        let input = "func main() {\n    if r is Ok(v) {\n        println(v)\n    } else as e {\n        println(e)\n    }\n}\n";
        let once = format_source(input);
        assert!(
            once.contains("} else as e {"),
            "the else-binding must survive: {}",
            once
        );
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
