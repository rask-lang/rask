// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Built-in functions for the Rask prelude

use std::fmt;

/// Identifies a built-in function
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinKind {
    /// print(...) - prints without newline
    Print,
    /// println(...) - prints with newline
    Println,
    /// panic(msg) - terminates with error message
    Panic,
    /// todo() - panics with "not yet implemented"
    Todo,
    /// unreachable() - panics with "entered unreachable code"
    Unreachable,
}

impl fmt::Display for BuiltinKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuiltinKind::Print => write!(f, "print"),
            BuiltinKind::Println => write!(f, "println"),
            BuiltinKind::Panic => write!(f, "panic"),
            BuiltinKind::Todo => write!(f, "todo"),
            BuiltinKind::Unreachable => write!(f, "unreachable"),
        }
    }
}

/// A resolved built-in reference
#[derive(Debug, Clone)]
pub struct Builtin {
    pub kind: BuiltinKind,
}

impl Builtin {
    pub fn new(kind: BuiltinKind) -> Self {
        Self { kind }
    }
}
