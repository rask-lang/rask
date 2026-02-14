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

    fn parse(src: &str) -> ParseResult {
        let lex_result = rask_lexer::Lexer::new(src).tokenize();
        assert!(lex_result.is_ok(), "Lex errors: {:?}", lex_result.errors);
        Parser::new(lex_result.tokens).parse()
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
}
