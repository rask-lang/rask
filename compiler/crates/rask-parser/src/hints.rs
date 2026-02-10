// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Error hints - suggestions for fixing common mistakes.
//!
//! Kept separate from the main parser to avoid clutter.

use rask_ast::token::TokenKind;

/// Get a hint for an "expected X" error based on context.
pub fn for_expected(expected: &str, found: &TokenKind) -> Option<&'static str> {
    match (expected, found) {
        // Colon hints
        ("':'", TokenKind::Eq) => Some("use ':' for types, '=' for values"),
        ("':'", _) => Some("syntax: name: Type"),

        // Block hints
        ("'{'" , _) => Some("blocks start with '{'"),
        ("'{' or newline", _) => Some("function body starts with '{'"),
        ("'}'" , _) => Some("every '{' needs a matching '}'"),

        // Parentheses hints
        ("'('", _) => Some("function calls need parentheses"),
        ("')'", TokenKind::Eof) => Some("add ')' to close the parenthesis"),
        ("')'", _) => None,

        // Bracket hints
        ("'['", _) => None,
        ("']'", TokenKind::Eof) => Some("add ']' to close the bracket"),
        ("']'", _) => None,

        // Generic angle bracket
        ("'>'", _) => Some("close the generic parameter list with '>'"),

        // Operator hints
        ("operator like '+' or '-'", _) => Some("expected a binary operator"),

        // Expression hints
        ("expression", TokenKind::Eq) => Some("put the value after '='"),
        ("expression", TokenKind::Semi) => Some("statement is incomplete"),
        ("expression", TokenKind::Newline) => Some("statement is incomplete"),
        ("expression", _) => Some("try a value, variable, or function call"),

        // Name/identifier hints
        ("a name", TokenKind::Int(_, _)) => Some("names can't start with a number"),
        ("a name", _) => Some("names start with a letter or '_'"),
        ("identifier", _) => Some("names start with a letter or '_'"),

        // String hints
        ("a string", _) => Some("expected a quoted string like \"example\""),

        // Type hints
        ("type", _) => Some("try a type like 'i32', 'string', or a struct name"),

        // Pattern hints
        ("pattern", _) => Some("try a name, literal, or constructor like Some(x)"),

        // Declaration hints (match the full string from parser.rs)
        (s, _) if s.starts_with("declaration (") => {
            Some("start with 'func', 'struct', 'enum', 'const', etc.")
        }

        // Array/pointer type hints
        ("array size (number or name)", _) => Some("array size must be a number or const name"),

        // Statement terminator
        ("newline or ';'", _) => Some("end statements with a newline or ';'"),

        _ => None,
    }
}
