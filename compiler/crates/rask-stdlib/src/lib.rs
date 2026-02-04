//! Rask Standard Library
//!
//! This crate provides built-in functions, types, and methods for the Rask language.
//! It is embedded in the compiler and provides the prelude (always-available symbols).

mod builtins;
pub mod types;

pub use builtins::{Builtin, BuiltinKind};
pub use types::{MethodDef, has_method, lookup_method};

/// Information about a built-in function for the resolver
#[derive(Debug, Clone)]
pub struct BuiltinInfo {
    pub name: &'static str,
    pub kind: BuiltinKind,
    pub arity: Arity,
}

/// Function arity
#[derive(Debug, Clone, Copy)]
pub enum Arity {
    /// Fixed number of arguments
    Fixed(usize),
    /// Variable arguments (minimum count)
    Variadic(usize),
}

/// Returns all built-in functions that should be injected into the global scope
pub fn builtins() -> Vec<BuiltinInfo> {
    vec![
        BuiltinInfo {
            name: "print",
            kind: BuiltinKind::Print,
            arity: Arity::Variadic(0),
        },
        BuiltinInfo {
            name: "println",
            kind: BuiltinKind::Println,
            arity: Arity::Variadic(0),
        },
        BuiltinInfo {
            name: "panic",
            kind: BuiltinKind::Panic,
            arity: Arity::Fixed(1),
        },
    ]
}
