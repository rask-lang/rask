// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Parser for the Rask language.
//!
//! Transforms a token stream into an abstract syntax tree.

mod hints;
mod parser;

pub use parser::{ParseError, ParseResult, Parser};

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::DeclKind;
    use rask_ast::expr::{BinOp, ExprKind, UnaryOp};
    use rask_ast::stmt::StmtKind;

    fn parse(src: &str) -> ParseResult {
        let lex_result = rask_lexer::Lexer::new(src).tokenize();
        assert!(lex_result.is_ok(), "Lex errors: {:?}", lex_result.errors);
        Parser::new(lex_result.tokens).parse()
    }

    /// Parse source and return the statements from the first function body.
    fn parse_body(src: &str) -> Vec<rask_ast::stmt::Stmt> {
        let wrapped = format!("func __test__() {{\n{}\n}}", src);
        let result = parse(&wrapped);
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            f.body.clone()
        } else {
            panic!("Expected function");
        }
    }

    /// Parse source wrapped in a function, expecting errors.
    fn parse_body_err(src: &str) -> ParseResult {
        let wrapped = format!("func __test__() {{\n{}\n}}", src);
        let lex_result = rask_lexer::Lexer::new(&wrapped).tokenize();
        assert!(lex_result.is_ok(), "Lex errors: {:?}", lex_result.errors);
        let result = Parser::new(lex_result.tokens).parse();
        assert!(!result.is_ok(), "Expected parse error but got success");
        result
    }

    #[test]
    fn parse_all_examples() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()
            .parent().unwrap()
            .parent().unwrap()
            .join("examples");

        for entry in std::fs::read_dir(&examples_dir).expect("examples directory not found") {
            let path = entry.unwrap().path();
            if path.extension().map(|e| e == "rask").unwrap_or(false) {
                let src = std::fs::read_to_string(&path)
                    .expect(&format!("Failed to read {}", path.display()));
                let lex_result = rask_lexer::Lexer::new(&src).tokenize();
                assert!(lex_result.is_ok(), "Lex errors in {}: {:?}", path.display(), lex_result.errors);
                let parse_result = Parser::new(lex_result.tokens).parse();
                assert!(parse_result.is_ok(), "Parse errors in {}: {:?}", path.display(), parse_result.errors);
            }
        }
    }

    #[test]
    fn parse_grouped_imports_simple() {
        let result = parse("import std.{io, fs}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 2);

        // Check first import: std.io
        if let DeclKind::Import(ref imp) = result.decls[0].kind {
            assert_eq!(imp.path, vec!["std", "io"]);
            assert!(imp.alias.is_none());
        } else {
            panic!("Expected import declaration");
        }

        // Check second import: std.fs
        if let DeclKind::Import(ref imp) = result.decls[1].kind {
            assert_eq!(imp.path, vec!["std", "fs"]);
            assert!(imp.alias.is_none());
        } else {
            panic!("Expected import declaration");
        }
    }

    #[test]
    fn parse_grouped_imports_with_alias() {
        let result = parse("import pkg.{A as X, B, C as Y}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 3);

        if let DeclKind::Import(ref imp) = result.decls[0].kind {
            assert_eq!(imp.path, vec!["pkg", "A"]);
            assert_eq!(imp.alias, Some("X".to_string()));
        } else {
            panic!("Expected import declaration");
        }

        if let DeclKind::Import(ref imp) = result.decls[1].kind {
            assert_eq!(imp.path, vec!["pkg", "B"]);
            assert!(imp.alias.is_none());
        } else {
            panic!("Expected import declaration");
        }

        if let DeclKind::Import(ref imp) = result.decls[2].kind {
            assert_eq!(imp.path, vec!["pkg", "C"]);
            assert_eq!(imp.alias, Some("Y".to_string()));
        } else {
            panic!("Expected import declaration");
        }
    }

    #[test]
    fn parse_grouped_imports_nested_path() {
        let result = parse("import std.collections.map.{HashMap, Entry}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 2);

        if let DeclKind::Import(ref imp) = result.decls[0].kind {
            assert_eq!(imp.path, vec!["std", "collections", "map", "HashMap"]);
        } else {
            panic!("Expected import declaration");
        }

        if let DeclKind::Import(ref imp) = result.decls[1].kind {
            assert_eq!(imp.path, vec!["std", "collections", "map", "Entry"]);
        } else {
            panic!("Expected import declaration");
        }
    }

    #[test]
    fn parse_grouped_imports_trailing_comma() {
        let result = parse("import pkg.{A, B,}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 2);
    }

    #[test]
    fn parse_grouped_imports_multiline() {
        let result = parse("import pkg.{\n    A,\n    B,\n    C,\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 3);
    }

    #[test]
    fn parse_grouped_imports_lazy() {
        let result = parse("import lazy pkg.{A, B}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        assert_eq!(result.decls.len(), 2);

        if let DeclKind::Import(ref imp) = result.decls[0].kind {
            assert!(imp.is_lazy);
        } else {
            panic!("Expected import declaration");
        }

        if let DeclKind::Import(ref imp) = result.decls[1].kind {
            assert!(imp.is_lazy);
        } else {
            panic!("Expected import declaration");
        }
    }

    #[test]
    fn parse_grouped_imports_empty_braces_error() {
        let result = parse("import pkg.{}");
        assert!(!result.is_ok(), "Expected error for empty braces");
    }

    // Tests for Rust syntax error messages
    #[test]
    fn rust_syntax_pub_keyword() {
        let result = parse("pub struct Point { x: i32 }");
        assert!(!result.is_ok());
        assert_eq!(result.errors[0].message, "unknown keyword 'pub'");
        assert_eq!(result.errors[0].hint.as_deref(), Some("use 'public' instead of 'pub'"));
    }

    #[test]
    fn rust_syntax_fn_keyword() {
        let result = parse("fn add(a: i32) -> i32 { return a }");
        assert!(!result.is_ok());
        assert_eq!(result.errors[0].message, "unknown keyword 'fn'");
        assert_eq!(result.errors[0].hint.as_deref(), Some("use 'func' instead of 'fn'"));
    }

    #[test]
    fn struct_optional_commas() {
        // Commas between fields
        let result = parse("struct User {\n    name: string,\n    age: i32\n}");
        assert!(result.is_ok(), "commas between struct fields should be allowed");
        // All commas
        let result = parse("struct Vec3 { x: f64, y: f64, z: f64 }");
        assert!(result.is_ok(), "single-line comma-separated struct should parse");
        // No commas (original style)
        let result = parse("struct User {\n    name: string\n    age: i32\n}");
        assert!(result.is_ok(), "newline-separated struct fields should still work");
        // Trailing comma
        let result = parse("struct Point { x: i32, y: i32, }");
        assert!(result.is_ok(), "trailing comma should be allowed");
    }

    #[test]
    fn rust_syntax_double_colon() {
        let result = parse("func main() { const x = Result::Ok }");
        assert!(!result.is_ok());
        assert_eq!(result.errors[0].message, "unexpected '::'");
        assert_eq!(result.errors[0].hint.as_deref(), Some("use '.' for paths (e.g., Result.Ok) instead of '::'"));
    }

    #[test]
    fn rust_syntax_let_mut() {
        let result = parse("func main() { let mut counter = 0 }");
        assert!(!result.is_ok());
        assert_eq!(result.errors[0].message, "unexpected 'mut' keyword");
        assert_eq!(result.errors[0].hint.as_deref(), Some("'let' is already mutable in Rask. Use 'const' for immutable bindings"));
    }

    #[test]
    fn doc_comments_on_structs() {
        let result = parse("/// A point.\npublic struct Point {\n    x: f64\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Struct(ref s) = result.decls[0].kind {
            assert_eq!(s.doc.as_deref(), Some("A point."));
        } else {
            panic!("Expected struct");
        }
    }

    #[test]
    fn doc_comments_on_functions() {
        let result = parse("/// Add two numbers.\npublic func add(a: i32, b: i32) -> i32 { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.doc.as_deref(), Some("Add two numbers."));
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn doc_comments_multiline() {
        let result = parse("/// First line.\n/// Second line.\nfunc foo() { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.doc.as_deref(), Some("First line.\nSecond line."));
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn doc_comments_on_methods_in_extend() {
        let result = parse("extend Foo {\n    /// Do something.\n    public func bar(self) { }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Impl(ref i) = result.decls[0].kind {
            assert_eq!(i.methods[0].doc.as_deref(), Some("Do something."));
        } else {
            panic!("Expected impl");
        }
    }

    #[test]
    fn parse_all_stdlib_stubs() {
        let stdlib_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()
            .parent().unwrap()
            .parent().unwrap()
            .join("stdlib");
        if !stdlib_dir.exists() { return; }
        for entry in std::fs::read_dir(&stdlib_dir).expect("stdlib directory not found") {
            let path = entry.unwrap().path();
            if path.extension().map(|e| e == "rk").unwrap_or(false) {
                let src = std::fs::read_to_string(&path)
                    .expect(&format!("Failed to read {}", path.display()));
                let lex_result = rask_lexer::Lexer::new(&src).tokenize();
                assert!(lex_result.is_ok(), "Lex errors in {}: {:?}", path.display(), lex_result.errors);
                let parse_result = Parser::new(lex_result.tokens).parse();
                assert!(parse_result.is_ok(), "Parse errors in {}: {:?}", path.display(), parse_result.errors);
            }
        }
    }

    #[test]
    fn no_doc_means_none() {
        let result = parse("func plain() { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert!(f.doc.is_none());
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn parse_option_enum_stub() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent().unwrap().parent().unwrap().parent().unwrap()
                .join("stdlib/option.rk")
        ).unwrap();
        let result = parse(&src);
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn parse_result_type_in_func() {
        // () or E return type
        let result = parse("extend Foo {\n    public func push(mutate self, v: T) -> () or PushError<T> { }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn parse_func_type_param() {
        // func(T) -> R as a parameter type
        let result = parse("extend Foo {\n    public func read(self, f: func(T) -> R) -> Option<R> { }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn parse_unsafe_method_in_extend() {
        let result = parse("extend Foo {\n    public unsafe func as_ptr(self) -> i64 { }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn parse_comptime_method_in_extend() {
        let result = parse("extend Foo {\n    public comptime func freeze(self) -> i64 { }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn multiline_function_params() {
        let result = parse("func add(\n    a: i32,\n    b: i32,\n) -> i32 {\n    return a + b\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.params[0].name, "a");
            assert_eq!(f.params[1].name, "b");
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn multiline_params_no_trailing_comma() {
        let result = parse("func add(\n    a: i32,\n    b: i32\n) -> i32 { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn nested_generics_in_param_list() {
        let result = parse(
            "struct Foo { x: i32 }\nfunc process(m: Map<string, Handle<Foo>>, items: Vec<Foo>) -> i32 {\n    return 0\n}"
        );
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[1].kind {
            assert_eq!(f.params.len(), 2);
            assert_eq!(f.params[0].name, "m");
            assert!(f.params[0].ty.contains("Handle<Foo>"));
            assert_eq!(f.params[1].name, "items");
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn triple_nested_generics() {
        let result = parse("func foo(x: A<B<C<i32>>>) { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.params[0].ty, "A<B<C<i32>>>");
        } else {
            panic!("Expected function");
        }
    }

    #[test]
    fn try_else_multiline() {
        let result = parse(
            "func foo() -> i32 or string {\n    const x = try bar()\n        else |e| \"fallback\"\n    return x\n}"
        );
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn try_else_same_line() {
        let result = parse(
            "func foo() -> i32 or string {\n    const x = try bar() else |e| \"fallback\"\n    return x\n}"
        );
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn dep_as_variable_name() {
        let result = parse("func main() {\n    const dep = 42\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn dep_in_for_loop() {
        let result = parse("func main() {\n    const deps = Vec.new()\n    for dep in deps {\n        dep\n    }\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
    }

    #[test]
    fn dep_still_works_in_package() {
        let result = parse("package \"myapp\" \"0.1.0\" {\n    dep \"serde\" \"1.0\"\n}");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Package(ref p) = result.decls[0].kind {
            assert_eq!(p.deps.len(), 1);
            assert_eq!(p.deps[0].name, "serde");
        } else {
            panic!("Expected package declaration");
        }
    }

    // ================================================================
    // A. Newline-as-terminator edge cases
    // ================================================================

    #[test]
    fn newline_unary_minus_on_next_line() {
        // `a` then `-b` on next line = two separate statements
        let stmts = parse_body("const x = a\n-b");
        assert_eq!(stmts.len(), 2, "should be two statements");
        assert!(matches!(stmts[0].kind, StmtKind::Const { .. }));
        if let StmtKind::Expr(ref e) = stmts[1].kind {
            assert!(matches!(e.kind, ExprKind::Unary { op: UnaryOp::Neg, .. }));
        } else {
            panic!("second statement should be unary negation expression");
        }
    }

    #[test]
    fn newline_binary_op_at_end_of_line() {
        // Operator at end of line continues expression
        let stmts = parse_body("const x = 1 +\n2");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Add, .. }));
        } else {
            panic!("expected const with binary add");
        }
    }

    #[test]
    fn newline_binary_op_at_start_of_next_line() {
        // Operator at start of next line does NOT continue — `+ 2` is a new statement
        // `+` is not a valid prefix operator, so this errors
        let stmts = parse_body("const x = 1\n2 + 3");
        // First statement: const x = 1, second: 2 + 3
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn newline_method_chain_across_lines() {
        // `.` is in postfix-across-newline check, so this chains
        let stmts = parse_body("foo\n.bar()\n.baz()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::MethodCall { .. }));
        } else {
            panic!("expected method call chain");
        }
    }

    #[test]
    fn newline_bracket_on_next_line_is_index() {
        // `[` IS in postfix check, so arr\n[0] = arr[0]
        let stmts = parse_body("arr\n[0]");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::Index { .. }));
        } else {
            panic!("expected index expression");
        }
    }

    #[test]
    fn newline_paren_on_next_line_is_new_statement() {
        // `(` is NOT in postfix check, so foo\n(bar) = two statements
        let stmts = parse_body("foo\n(bar)");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn newline_return_then_expr_is_void_return() {
        // return\nfoo() = void return, then foo() as new statement
        let stmts = parse_body("return\nfoo()");
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0].kind, StmtKind::Return(None)));
        if let StmtKind::Expr(ref e) = stmts[1].kind {
            assert!(matches!(e.kind, ExprKind::Call { .. }));
        } else {
            panic!("expected call expression after return");
        }
    }

    #[test]
    fn newline_optional_chain_across_lines() {
        // `?.` is in postfix check
        let stmts = parse_body("x\n?.field");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::OptionalField { .. }));
        } else {
            panic!("expected optional field access");
        }
    }

    #[test]
    fn newline_in_grouping_parens_does_not_continue() {
        // Grouping parens (expr) do NOT skip newlines — unlike call parens f(args)
        // (1\n+ 2) fails: after parsing `1`, newline terminates, then expects `)`
        parse_body_err("const x = (1\n+ 2)");
    }

    #[test]
    fn newline_in_call_parens_continues() {
        // Call parens DO skip newlines (parse_args calls skip_newlines)
        let stmts = parse_body("const x = foo(\n1,\n2\n)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Call { .. }));
        } else {
            panic!("expected call");
        }
    }

    #[test]
    fn newline_infix_at_start_of_line_continues() {
        // `&&` at start of next line continues the expression
        let stmts = parse_body("const ok = a > b\n&& b > c");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::And, .. }));
        } else {
            panic!("expected const with logical and");
        }
    }

    #[test]
    fn newline_logical_op_at_end_of_line_continues() {
        // Operator at end of line continues
        let stmts = parse_body("const ok = a > b &&\nb > c");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::And, .. }));
        } else {
            panic!("expected const with logical and");
        }
    }

    #[test]
    fn newline_ambiguous_prefix_ops_do_not_continue() {
        // `+` and `-` at start of next line are new statements (prefix ambiguity)
        let stmts = parse_body("const x = a\n-b");
        assert_eq!(stmts.len(), 2);

        // Plain identifiers on next line are new statements
        let stmts = parse_body("const ok = a > b\nb > c");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn newline_multiple_infix_continuations() {
        // Multiple lines of infix continuation
        let stmts = parse_body("const ok = a == 1\n&& b == 2\n&& c == 3");
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn newline_comparison_continuation() {
        // `==` at start of next line continues
        let stmts = parse_body("const ok = a\n== b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Eq, .. }));
        } else {
            panic!("expected const with equality");
        }
    }

    #[test]
    fn newline_pipe_continuation() {
        // `||` at start of next line continues
        let stmts = parse_body("const ok = a\n|| b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Or, .. }));
        } else {
            panic!("expected const with logical or");
        }
    }

    #[test]
    fn newline_deeply_chained_optional() {
        // All `?.` continue across newlines
        let stmts = parse_body("a\n?.b\n?.c\n?.d");
        assert_eq!(stmts.len(), 1);
    }

    // ================================================================
    // B. Generic vs comparison disambiguation
    // ================================================================

    #[test]
    fn generic_method_call() {
        let stmts = parse_body("obj.method<i32>(x)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::MethodCall { ref type_args, .. } = e.kind {
                assert!(type_args.is_some(), "should have type args");
                assert_eq!(type_args.as_ref().unwrap(), &vec!["i32".to_string()]);
            } else {
                panic!("expected method call");
            }
        } else {
            panic!("expected expression statement");
        }
    }

    #[test]
    fn plain_comparison_lt() {
        let stmts = parse_body("a < b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::Binary { op: BinOp::Lt, .. }));
        } else {
            panic!("expected comparison");
        }
    }

    #[test]
    fn comparison_not_generic() {
        // a < b && b > c — two comparisons joined by &&
        let stmts = parse_body("a < b && b > c");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::Binary { op: BinOp::And, .. }));
        } else {
            panic!("expected logical and");
        }
    }

    #[test]
    fn nested_generics_double_gt_in_type() {
        let result = parse("func f(x: Vec<Vec<i32>>) { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.params[0].ty, "Vec<Vec<i32>>");
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn triple_nested_generics_gt_splitting() {
        let result = parse("func f(x: A<B<C<i32>>>) { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.params[0].ty, "A<B<C<i32>>>");
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn generic_struct_literal() {
        let stmts = parse_body("const p = Point<f64> { x: 1.0, y: 2.0 }");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::StructLit { ref name, .. } = init.kind {
                assert_eq!(name, "Point<f64>");
            } else {
                panic!("expected struct literal, got {:?}", init.kind);
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn generic_static_method() {
        let stmts = parse_body("Vec<i32>.new()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::MethodCall { .. }));
        } else {
            panic!("expected method call");
        }
    }

    #[test]
    fn type_name_in_comparison_not_generic() {
        // Size < limit — no `.` or `{` after `>`, so it's comparison
        let stmts = parse_body("const ok = Size < limit");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Lt, .. }));
        } else {
            panic!("expected comparison");
        }
    }

    #[test]
    fn nested_generic_in_param() {
        let result = parse("func f(x: Map<string, Vec<i32>>) { }");
        assert!(result.is_ok(), "Parse errors: {:?}", result.errors);
        if let DeclKind::Fn(ref f) = result.decls[0].kind {
            assert_eq!(f.params[0].ty, "Map<string, Vec<i32>>");
        } else {
            panic!("expected function");
        }
    }

    #[test]
    fn right_shift_not_generic() {
        let stmts = parse_body("const x = a >> b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Shr, .. }));
        } else {
            panic!("expected right shift");
        }
    }

    // ================================================================
    // C. Generic function calls (spec-parser alignment)
    //
    // The spec says `sort<i32>(items)` should parse as a generic call.
    // These tests verify the parser handles this correctly.
    // ================================================================

    #[test]
    fn generic_call_lowercase_func() {
        // sort<i32>(items) — generic function call
        let stmts = parse_body("sort<i32>(items)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref func, .. } = e.kind {
                // The function ident should include generic args
                if let ExprKind::Ident(ref name) = func.kind {
                    assert_eq!(name, "sort<i32>");
                } else {
                    panic!("expected ident with generics, got {:?}", func.kind);
                }
            } else {
                panic!("expected call expression, got {:?}", e.kind);
            }
        } else {
            panic!("expected expression statement");
        }
    }

    #[test]
    fn generic_call_uppercase_with_paren() {
        // Vec<i32>(items) — uppercase generic call
        let stmts = parse_body("Vec<i32>(items)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref func, .. } = e.kind {
                if let ExprKind::Ident(ref name) = func.kind {
                    assert_eq!(name, "Vec<i32>");
                } else {
                    panic!("expected ident, got {:?}", func.kind);
                }
            } else {
                panic!("expected call, got {:?}", e.kind);
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn generic_call_multiple_type_args() {
        // convert<i32, f64>(x) — multiple type args
        let stmts = parse_body("convert<i32, f64>(x)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref func, .. } = e.kind {
                if let ExprKind::Ident(ref name) = func.kind {
                    assert_eq!(name, "convert<i32, f64>");
                } else {
                    panic!("expected ident with generics");
                }
            } else {
                panic!("expected call");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn generic_call_nested_type_arg() {
        // process<Vec<i32>>(items) — nested generic in type arg
        let stmts = parse_body("process<Vec<i32>>(items)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref func, .. } = e.kind {
                if let ExprKind::Ident(ref name) = func.kind {
                    assert_eq!(name, "process<Vec<i32>>");
                } else {
                    panic!("expected ident with nested generics");
                }
            } else {
                panic!("expected call");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn comparison_not_generic_call_no_paren() {
        // a < b > c — no `(` after `>`, so it's comparison
        let stmts = parse_body("a < b > c");
        assert_eq!(stmts.len(), 1);
        // Should parse as (a < b) > c (two comparisons chained by precedence)
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::Binary { op: BinOp::Gt, .. }));
        } else {
            panic!("expected comparison");
        }
    }

    // ================================================================
    // D. Expression vs statement context
    // ================================================================

    #[test]
    fn if_as_expression() {
        let stmts = parse_body("const x = if true { 1 } else { 2 }");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::If { .. }));
        } else {
            panic!("expected const with if expression");
        }
    }

    #[test]
    fn match_as_expression() {
        let stmts = parse_body("const x = match y {\n    1 => \"a\",\n    _ => \"b\"\n}");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Match { .. }));
        } else {
            panic!("expected const with match expression");
        }
    }

    #[test]
    fn nested_if_expression() {
        let stmts = parse_body("const x = if a { if b { 1 } else { 2 } } else { 3 }");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::If { ref then_branch, .. } = init.kind {
                assert!(matches!(then_branch.kind, ExprKind::Block(_)));
            } else {
                panic!("expected if expression");
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn block_as_expression() {
        let stmts = parse_body("const x = {\n    const y = 1\n    y + 1\n}");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Block(ref block_stmts) = init.kind {
                assert_eq!(block_stmts.len(), 2);
            } else {
                panic!("expected block expression");
            }
        } else {
            panic!("expected const");
        }
    }

    // ================================================================
    // E. Closure edge cases
    // ================================================================

    #[test]
    fn closure_in_function_arg() {
        let stmts = parse_body("foo(|x| x + 1)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref args, .. } = e.kind {
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].expr.kind, ExprKind::Closure { .. }));
            } else {
                panic!("expected call");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn multiple_closures_as_args() {
        let stmts = parse_body("foo(|x| x, |y| y)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref args, .. } = e.kind {
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0].expr.kind, ExprKind::Closure { .. }));
                assert!(matches!(args[1].expr.kind, ExprKind::Closure { .. }));
            } else {
                panic!("expected call with two closure args");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn empty_closure() {
        let stmts = parse_body("const f = || { 42 }");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Closure { ref params, .. } = init.kind {
                assert!(params.is_empty());
            } else {
                panic!("expected closure");
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn bitwise_or_not_confused_with_closure() {
        let stmts = parse_body("const x = a | b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::BitOr, .. }));
        } else {
            panic!("expected bitwise or");
        }
    }

    // ================================================================
    // F. Operator interaction edge cases
    // ================================================================

    #[test]
    fn range_in_array_index() {
        let stmts = parse_body("const x = arr[1..3]");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Index { ref index, .. } = init.kind {
                assert!(matches!(index.kind, ExprKind::Range { .. }));
            } else {
                panic!("expected index expression");
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn try_with_method_chain() {
        // try binds at PREFIX_BP (23), `.` at 25, so `.bar()` chains inside try
        let stmts = parse_body("const x = try foo().bar()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Try { ref expr, .. } = init.kind {
                // The inner expression should be the method chain foo().bar()
                assert!(matches!(expr.kind, ExprKind::MethodCall { .. }));
            } else {
                panic!("expected try expression, got {:?}", init.kind);
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn optional_chaining_multiple() {
        let stmts = parse_body("const x = a?.b?.c");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            // The outer should be ?.c on something
            if let ExprKind::OptionalField { ref object, field: ref f, .. } = init.kind {
                assert_eq!(f, "c");
                assert!(matches!(object.kind, ExprKind::OptionalField { .. }));
            } else {
                panic!("expected optional field chain");
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn null_coalescing_chain() {
        let stmts = parse_body("const x = a ?? b ?? c");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::NullCoalesce { .. }));
        } else {
            panic!("expected null coalescing");
        }
    }

    #[test]
    fn cast_binds_tighter_than_add() {
        // `as` has bp=21, `+` has (19,20), so (a as i32) + b
        let stmts = parse_body("const x = a as i32 + b");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Binary { op: BinOp::Add, ref left, .. } = init.kind {
                assert!(matches!(left.kind, ExprKind::Cast { .. }));
            } else {
                panic!("expected add with cast on left");
            }
        } else {
            panic!("expected const");
        }
    }

    #[test]
    fn negation_of_method_call() {
        // PREFIX_BP (23) < postfix (25), so `-` applies to result of foo.bar()
        let stmts = parse_body("const x = -foo.bar()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            if let ExprKind::Unary { op: UnaryOp::Neg, ref operand, .. } = init.kind {
                assert!(matches!(operand.kind, ExprKind::MethodCall { .. }));
            } else {
                panic!("expected negation of method call");
            }
        } else {
            panic!("expected const");
        }
    }

    // ================================================================
    // G. Multi-line construct edge cases
    // ================================================================

    #[test]
    fn multiline_function_call() {
        let stmts = parse_body("foo(\n    a,\n    b,\n    c\n)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref args, .. } = e.kind {
                assert_eq!(args.len(), 3);
            } else {
                panic!("expected call");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn if_else_across_newlines() {
        let stmts = parse_body("if x > 0 {\n    a\n}\nelse {\n    b\n}");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::If { ref else_branch, .. } = e.kind {
                assert!(else_branch.is_some(), "should have else branch");
            } else {
                panic!("expected if expression");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn match_with_multiline_block_arm() {
        let stmts = parse_body("match x {\n    1 => {\n        foo()\n        bar()\n    },\n    _ => baz()\n}");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Match { ref arms, .. } = e.kind {
                assert_eq!(arms.len(), 2);
            } else {
                panic!("expected match");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn chained_methods_with_generic_across_lines() {
        let stmts = parse_body("obj\n.method<T>()\n.other()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::MethodCall { ref method, .. } = e.kind {
                assert_eq!(method, "other");
            } else {
                panic!("expected method call chain");
            }
        } else {
            panic!("expected expression");
        }
    }

    // ================================================================
    // H. Additional stress tests for combined edge cases
    // ================================================================

    #[test]
    fn generic_call_inside_method_chain() {
        // sort<i32>(items).len() — generic call then method chain
        let stmts = parse_body("sort<i32>(items).len()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::MethodCall { ref method, ref object, .. } = e.kind {
                assert_eq!(method, "len");
                assert!(matches!(object.kind, ExprKind::Call { .. }));
            } else {
                panic!("expected method call on generic call result");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn generic_in_if_condition() {
        // Generic call in a condition context (no brace exprs allowed)
        let stmts = parse_body("if is_valid<i32>(x) {\n    ok()\n}");
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn comparison_with_parens_disambiguated() {
        // (a < b) > (c) — parenthesized comparison, not generic
        let stmts = parse_body("const x = (a < b) > (c)");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Const { ref init, .. } = stmts[0].kind {
            assert!(matches!(init.kind, ExprKind::Binary { op: BinOp::Gt, .. }));
        } else {
            panic!("expected comparison");
        }
    }

    #[test]
    fn nested_function_call_with_comparison_not_generic() {
        // f(g(x < y)) — the `)` from g() should prevent generic scan
        // x < y is a comparison passed to g(), result passed to f()
        let stmts = parse_body("f(g(x < y))");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref args, .. } = e.kind {
                assert_eq!(args.len(), 1);
                // The arg should be g(...), itself a call
                if let ExprKind::Call { args: ref inner_args, .. } = args[0].expr.kind {
                    assert_eq!(inner_args.len(), 1);
                    // Inner arg: x < y (comparison)
                    assert!(matches!(inner_args[0].expr.kind, ExprKind::Binary { op: BinOp::Lt, .. }));
                } else {
                    panic!("expected inner call");
                }
            } else {
                panic!("expected outer call");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn multiline_closure_in_arg() {
        let stmts = parse_body("foo(|x| {\n    const y = x + 1\n    y\n})");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Expr(ref e) = stmts[0].kind {
            if let ExprKind::Call { ref args, .. } = e.kind {
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].expr.kind, ExprKind::Closure { .. }));
            } else {
                panic!("expected call with closure");
            }
        } else {
            panic!("expected expression");
        }
    }

    #[test]
    fn question_mark_on_next_line_chains() {
        // `?` is in postfix-across-newline check
        let stmts = parse_body("const x = try foo()\nconst y = bar()");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn return_with_value_same_line() {
        let stmts = parse_body("return foo()");
        assert_eq!(stmts.len(), 1);
        if let StmtKind::Return(Some(ref e)) = stmts[0].kind {
            assert!(matches!(e.kind, ExprKind::Call { .. }));
        } else {
            panic!("expected return with value");
        }
    }
}
