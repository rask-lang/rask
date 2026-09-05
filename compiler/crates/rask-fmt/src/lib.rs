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
    if config.allow_keyword_fn_names {
        parser = parser.allow_keyword_fn_names();
    }
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

    /// #1043: a comment after the last declaration went out through a path
    /// that emitted no blank lines at all, so an explanatory block at the end
    /// of a file came back jammed against the closing brace. `stdlib/` is held
    /// to `fmt --check`, so that failed the gate for something no human would
    /// call a formatting mistake.
    #[test]
    fn keeps_the_blank_line_before_a_trailing_comment() {
        let input = "func a() -> i32 {\n    return 1\n}\n\n// A trailing note.\n";
        assert_eq!(format_source(input), input);
    }

    /// The other half of the same rule: separation the source didn't have
    /// isn't invented either.
    #[test]
    fn does_not_add_a_blank_line_before_a_trailing_comment() {
        let input = "func a() -> i32 {\n    return 1\n}\n// A trailing note.\n";
        assert_eq!(format_source(input), input);
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

        // The other direction: `..` binds looser than every binary operator, so
        // the parser already reads `i + 1..n` as `(i + 1)..n` and parenthesising
        // the endpoint says nothing. Adding them made 11 stdlib and example files
        // fail `fmt --check` against source that was fine.
        let ranged = format_source("func main() {\n    let x = i + 1..n\n}\n");
        assert!(
            ranged.contains("i + 1..n"),
            "a range endpoint needs no parentheses: {}",
            ranged
        );
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

    // `format_source` still falls back for callers with nowhere to put a
    // diagnostic — the LSP formats mid-keystroke, when the buffer often doesn't
    // parse and returning the text unchanged is the only sane answer.

    // An `extern "C" { … }` block flattens into one declaration per member, and
    // the printer used to reprint each as its own `extern "C" func` — so the
    // braces went away and any comment written inside them had nowhere to land
    // (#805). Members carry the offset of their own `extern` keyword now, which
    // is what puts the block back together.
    #[test]
    fn an_extern_block_keeps_its_braces() {
        let input = "extern \"C\" {\n    // The readers.\n    func read(fd: i64) -> i64\n    func write(fd: i64) -> i64\n}\n";
        let output = format_source(input);
        assert_eq!(output, input, "an extern block should round-trip: {}", output);
    }

    #[test]
    fn a_single_extern_stays_single() {
        let input = "extern \"C\" func alloc(n: i64) -> i64\n";
        let output = format_source(input);
        assert_eq!(output, input, "the single form has no braces to add: {}", output);
    }

    #[test]
    fn two_extern_blocks_do_not_merge() {
        let input = "extern \"C\" {\n    func a(n: i64) -> i64\n}\n\nextern \"C\" {\n    func b(n: i64) -> i64\n}\n";
        let output = format_source(input);
        assert_eq!(output, input, "each block keeps its own braces: {}", output);
    }

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
    fn a_comment_beside_a_field_or_variant_stays_beside_it() {
        // Neither is a statement, so nothing was flushing the comments written
        // among them: they stayed pending and came out below the closing brace,
        // where a comment about one member reads as a comment about the next
        // declaration (#805).
        let input = "\
enum E {
    A  // about A
    // about B
    B
}

struct S {
    public a: i64  // about a
    // about b
    public b: i64
}
";
        let output = format_source(input);
        assert!(output.contains("A  // about A"), "trailing on a variant:\n{}", output);
        assert!(
            output.contains("public a: i64  // about a"),
            "trailing on a field:\n{}", output,
        );
        // Standalone ones keep their own line, inside the braces.
        let body = output.split("enum E {").nth(1).unwrap();
        assert!(
            body.split('}').next().unwrap().contains("// about B"),
            "the standalone one stays inside the enum:\n{}", output,
        );
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

    #[test]
    fn a_trailing_comment_attaches_to_the_member_it_follows() {
        // Source order alone isn't enough: bounded only by the declaration's end,
        // the first pending comment attached to the *first* field whatever line it
        // was on. `Node { value, next // about next }` moved it up onto `value`.
        let input = "struct Node {\n    value: i32\n    next: i32  // about next\n}\n";
        let output = format_source(input);
        assert!(
            output.contains("next: i32  // about next"),
            "stays on `next`:\n{}", output,
        );
        assert!(
            !output.contains("value: i32  //"),
            "and not on `value`:\n{}", output,
        );
    }

    #[test]
    fn a_comment_stays_in_the_block_it_was_written_in() {
        // The pending-comment cursor is global and the block-end drain accepted any
        // comment indented at least as deep as the block — which is every comment
        // in every later block too. One `if` body swallowed the next one's, and one
        // function's body swallowed the comments out of the next function (#805).
        let input = "\
func f(n: i64) -> i64 {
    if n == 1 {
        return 1  // one
    }
    if n == 2 {
        return 2  // two
    }
    return 0
}

func g() {
    // about g
    println(\"g\")
}
";
        let output = format_source(input);
        assert!(output.contains("return 1  // one"), "first if keeps its own:\n{}", output);
        assert!(output.contains("return 2  // two"), "second one too:\n{}", output);
        let g = output.split("func g() {").nth(1).unwrap();
        assert!(g.contains("// about g"), "g keeps its comment:\n{}", output);
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

    #[test]
    fn a_comment_beside_a_match_arm_stays_beside_it() {
        let input = "\
func f(n: i64) -> i64 {
    match n {
        1 => return 10  // ten
        _ => return 0   // zero
    }
}
";
        let output = format_source(input);
        assert!(output.contains("1 => return 10  // ten"), "first arm:\n{}", output);
        assert!(output.contains("_ => return 0   // zero"), "and the last:\n{}", output);
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

    #[test]
    fn a_body_that_is_only_a_comment_keeps_its_braces_open() {
        // `{}` would drop the comment out of the braces entirely — it escaped to
        // column 0 below the enclosing `extend`.
        let input = "func stub() {\n    // nothing yet\n}\n";
        let output = format_source(input);
        assert!(output.contains("func stub() {"), "expanded:\n{}", output);
        assert!(output.contains("    // nothing yet"), "comment inside:\n{}", output);
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

    #[test]
    fn a_chain_with_a_comment_in_it_is_left_as_written() {
        // Reflowing the chain deletes the line the comment annotated, and the
        // comment then lands wherever the cursor flushes — above the whole chain.
        let input = "\
func main() {
    let xs = source()
        .filter(|n| n > 0)  // keep positives
        .map(|n| n * 2)     // double them
    println(\"{xs}\")
}
";
        let output = format_source(input);
        assert!(output.contains(".filter(|n| n > 0)  // keep positives"), "{}", output);
        assert!(output.contains(".map(|n| n * 2)     // double them"), "{}", output);
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

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

    // ── #805: the formatter's output has to still be the same program ──
    //
    // Each of these came from a file that stopped compiling after being
    // formatted. Nothing compared the two, so all of them shipped.

    /// Format `input` and assert the result contains `expected`.
    fn keeps(input: &str, expected: &str, what: &str) {
        let output = format_source(input);
        assert!(output.contains(expected), "{}: expected `{}` in:\n{}", what, expected, output);
        assert_eq!(output, format_source(&output), "{} isn't idempotent:\n{}", what, output);
    }

    #[test]
    fn keeps_parens_a_postfix_receiver_needs() {
        // `(a - b).f()` is a different call from `a - b.f()`.
        keeps(
            "func main() {\n    let n = (a - b).count()\n}\n",
            "(a - b).count()",
            "a binary receiver",
        );
        keeps(
            "func main() {\n    let n = (a ?? b).count()\n}\n",
            "(a ?? b).count()",
            "a coalescing receiver",
        );
    }

    #[test]
    fn keeps_parens_precedence_needs() {
        // The operators are left-associative, so the right operand needs
        // parentheses at equal precedence: `a - (b - c)` is not `a - b - c`.
        keeps("func main() {\n    let n = a - (b - c)\n}\n", "a - (b - c)", "right operand");
        keeps("func main() {\n    let n = (a + b) * c\n}\n", "(a + b) * c", "left operand");
        // A prefix operator binds tighter than every binary one.
        keeps("func main() {\n    let n = !(a < b)\n}\n", "!(a < b)", "prefix operand");
        keeps("func main() {\n    let n = -(a + b)\n}\n", "-(a + b)", "negated sum");
        // Comparison is non-associative — `a < b == c < d` is rejected.
        keeps(
            "func main() {\n    let n = (a < b) == (c < d)\n}\n",
            "(a < b) == (c < d)",
            "nested comparison",
        );
        // `-(3)` used to print as `-(3` — the folded literal's span stopped
        // before the closing paren and the printer echoes that span.
        keeps("func main() {\n    let n = -(-7)\n}\n", "-(-7)", "a negated folded literal");
    }

    #[test]
    fn keeps_an_as_binding() {
        keeps(
            "func main() {\n    if score? as s {\n        println(\"{s}\")\n    }\n}\n",
            "if score? as s {",
            "the presence binding",
        );
        keeps(
            "func main() {\n    if x is Shape.Circle {\n        println(\"c\")\n    } else as e {\n        println(\"{e}\")\n    }\n}\n",
            "} else as e {",
            "the else binding",
        );
    }

    #[test]
    fn keeps_a_using_clause() {
        keeps(
            "func heal(amount: i32) using players: Pool<Player> {\n    return\n}\n",
            "using players: Pool<Player>",
            "the context clause",
        );
    }

    #[test]
    fn keeps_attributes_and_modifiers() {
        keeps(
            "@message\nenum E {\n    @message(\"boom\")\n    Bad(i64)\n}\n",
            "@message(\"boom\")",
            "a variant attribute",
        );
        keeps(
            "@allow(idiom/duck-trait)\nduck trait Frobber {\n    func frob(self) -> i64\n}\n",
            "duck trait Frobber",
            "the duck modifier",
        );
    }

    #[test]
    fn keeps_the_surface_spelling_of_a_type() {
        // The parser normalizes `void` to `()`, which nobody can write.
        keeps("func f() -> void {\n    return\n}\n", "-> void", "the unit type");
        // `||` is the or-operator token, so a no-parameter closure type has to
        // keep the `func()` spelling.
        keeps(
            "func f(g: func() -> i64) -> i64 {\n    return 1\n}\n",
            "func() -> i64",
            "an empty closure type",
        );
        // The value/error split has to find the *top-level* comma.
        keeps(
            "func f() -> (string, string) or Error {\n    return (\"a\", \"b\")\n}\n",
            "(string, string) or Error",
            "a tuple value type",
        );
    }

    #[test]
    fn keeps_a_doc_comment_above_the_method_it_documents() {
        // Nothing consumed comments between `extend` members, so a `///` stayed
        // unclaimed until the body's first statement picked it up — and the doc
        // comment ended up inside the method.
        let input = "struct Foo { n: i64 }\n\nextend Foo {\n    /// Doc for a.\n    public func a(self) -> i64 {\n        return 1\n    }\n}\n";
        let output = format_source(input);
        assert!(
            output.contains("    /// Doc for a.\n    public func a"),
            "the doc comment stays above its method:\n{}", output,
        );
        assert_eq!(output, format_source(&output), "and idempotently:\n{}", output);
    }

    #[test]
    fn keeps_a_frozen_context_clause() {
        // `is_frozen` printed as `const`, which isn't the keyword.
        keeps(
            "func read_ok(h: Handle<Player>) -> i32 using frozen Pool<Player> {\n    return 1\n}\n",
            "using frozen Pool<Player>",
            "the frozen marker",
        );
    }

    #[test]
    fn converts_the_unit_type_inside_a_generic_argument() {
        // The top-level check caught `-> void` and missed `Receiver<void>`, which
        // came back out as `Receiver<()>` and stopped parsing.
        keeps(
            "func f() -> Receiver<void> {\n    todo()\n}\n",
            "Receiver<void>",
            "a nested unit type",
        );
    }

    #[test]
    fn compound_assignment_keeps_its_operator() {
        // `i += 1` is stored as `i = i + 1`, so writing the value out expanded
        // every compound assignment in the tree.
        for form in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="] {
            let input = format!("func main() {{\n    mut i = 0\n    i {} 1\n}}\n", form);
            let output = format_source(&input);
            assert!(
                output.contains(&format!("i {} 1", form)),
                "`{}` should survive:\n{}", form, output,
            );
        }
        // A plain assignment that happens to be `i = i + 1` stays that way.
        let output = format_source("func main() {\n    mut i = 0\n    i = i + 1\n}\n");
        assert!(output.contains("i = i + 1"), "not rewritten to `+=`:\n{}", output);
    }

    #[test]
    fn unsafe_keeps_the_form_it_was_written_in() {
        // `unsafe expr` and `unsafe { expr }` parse to the same node. Printing
        // the braced form for both turned `if unsafe f() { … }` into
        // `if unsafe { f() } { … }` — two braces for one `if`.
        keeps(
            "func main() {\n    let p = unsafe raw()\n}\n",
            "unsafe raw()",
            "the braceless form",
        );
        let output = format_source("func main() {\n    unsafe {\n        let p = raw()\n    }\n}\n");
        assert!(output.contains("unsafe {"), "the braced form stays braced:\n{}", output);
    }

    #[test]
    fn a_one_statement_block_stays_on_one_line() {
        keeps(
            "func f(n: i64) -> i64 {\n    if n <= 1 { return 1 }\n    return n\n}\n",
            "if n <= 1 { return 1 }",
            "an inline if body",
        );
        keeps(
            "func f(a: bool, b: i64) -> i64 {\n    let n = if a { b } else { 0 }\n    return n\n}\n",
            "if a { b } else { 0 }",
            "both inline branches",
        );
        // A comment in the block forces the expansion — a trailing comment has
        // nowhere to go on a line that continues with `}`.
        let output = format_source("func f(n: i64) -> i64 {\n    if n <= 1 { return 1 // base\n    }\n    return n\n}\n");
        assert!(
            !output.contains("{ return 1 // base }"),
            "a comment expands the block:\n{}", output,
        );
    }

    #[test]
    fn empty_declaration_bodies_use_one_spelling() {
        // A fieldless *type* body went through the inline-field branch and came
        // out `{  }` — two spaces, from an empty list between `{ ` and ` }`. The
        // tree writes `{ }` there, 47 times against 4.
        let output = format_source("struct Cell<T> { }\n");
        assert!(output.contains("struct Cell<T> { }"), "one space:\n{}", output);
        // An empty *function* body goes the other way: `{}` is what hand-written
        // Rask uses (137 sites, 103 of them `func main() {}`), where `{ }` shows
        // up 378 times and only ever in `stdlib/`'s signature stubs.
        let output = format_source("func main() { }\n");
        assert!(output.contains("func main() {}"), "tight for a function:\n{}", output);
    }

    #[test]
    fn a_trailing_comment_survives_a_comment_after_it() {
        // A statement's span runs past standalone comments that follow it, so
        // requiring bare whitespace after the trailing comment moved it on the
        // second pass whenever another comment came next.
        let input = "func main() {\n    let a = 4  // about a\n    // about the next thing\n}\n";
        let once = format_source(input);
        assert!(once.contains("let a = 4  // about a"), "stays put:\n{}", once);
        assert_eq!(once, format_source(&once), "and stays put on a second pass:\n{}", once);
    }

    #[test]
    fn keeps_parens_around_a_condition_ending_in_a_struct_literal() {
        // Without them the `{` of the body attaches to the literal.
        keeps(
            "func main() {\n    if (c == Shape.Circle { r: 4 }) {\n        println(\"y\")\n    }\n}\n",
            "if (c == Shape.Circle { r: 4 }) {",
            "the condition",
        );
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
    fn test_name_keeps_its_escapes() {
        // The name is a `String` on the declaration, already unescaped by the
        // lexer, so printing it raw wrote the characters themselves — and a
        // name containing `\u` came back out as a lone `\u`, which isn't a
        // valid escape. The formatted file then didn't parse (#850).
        let input = "test \"a \\\\u and \\\" and \\\\ and \\t\" {\n    assert true\n}\n";
        let output = format_source(input);
        assert!(
            output.contains(r#"test "a \\u and \" and \\ and \t""#),
            "escapes should survive: {}",
            output
        );
        let twice = format_source(&output);
        assert_eq!(output, twice, "should be idempotent");
    }

    #[test]
    fn benchmark_name_keeps_its_escapes() {
        let input = "benchmark \"q \\\" b\" {\n    let x = 1\n}\n";
        let output = format_source(input);
        assert!(output.contains(r#"benchmark "q \" b""#), "escapes should survive: {}", output);
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
