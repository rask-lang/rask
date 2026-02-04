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
}

impl fmt::Display for BuiltinKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuiltinKind::Print => write!(f, "print"),
            BuiltinKind::Println => write!(f, "println"),
            BuiltinKind::Panic => write!(f, "panic"),
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
