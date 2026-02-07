// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Error hints - suggestions for fixing common mistakes.
//!
//! Kept separate from the main parser to avoid clutter.

use rask_ast::token::TokenKind;

/// Get a hint for an "expected X" error based on context.
pub fn for_expected(expected: &str, found: &TokenKind) -> Option<&'static str> {
    match (expected, found) {
        // Colon hints
        ("':'", TokenKind::Eq) => Some("Use ':' for types, '=' for values"),
        ("':'", _) => Some("Syntax: name: Type"),

        // Block hints
        ("'{'" , _) => Some("Blocks start with '{'"),
        ("'}'" , _) => Some("Every '{' needs a matching '}'"),

        // Parentheses hints
        ("'('", _) => Some("Function calls need parentheses"),
        ("')'", TokenKind::Eof) => Some("Add ')' to close the parenthesis"),
        ("')'", _) => None,

        // Bracket hints
        ("'['", _) => None,
        ("']'", TokenKind::Eof) => Some("Add ']' to close the bracket"),
        ("']'", _) => None,

        // Expression hints
        ("expression", TokenKind::Eq) => Some("Put the value after '='"),
        ("expression", TokenKind::Semi) => Some("Statement is incomplete"),
        ("expression", TokenKind::Newline) => Some("Statement is incomplete"),
        ("expression", _) => Some("Try a value, variable, or function call"),

        // Name/identifier hints
        ("a name", TokenKind::Int(_)) => Some("Names can't start with a number"),
        ("a name", _) => Some("Names start with a letter or '_'"),

        // Type hints
        ("type", _) => Some("Try a type like 'i32', 'string', or a struct name"),

        // Pattern hints
        ("pattern", _) => Some("Try a name, literal, or constructor like Some(x)"),

        // Declaration hints
        ("declaration (func, struct, enum, trait, extend, import, const)", _) => {
            Some("Start with 'func', 'struct', 'enum', 'const', etc.")
        }

        // Statement terminator
        ("newline or ';'", _) => Some("End statements with a newline or ';'"),

        _ => None,
    }
}
