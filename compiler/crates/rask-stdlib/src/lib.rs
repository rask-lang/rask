// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask Standard Library
//!
//! This crate provides built-in functions, types, and methods for the Rask language.
//! It is embedded in the compiler and provides the prelude (always-available symbols).

mod builtins;
pub mod types;
pub mod registry;
pub mod stubs;

pub use builtins::{Builtin, BuiltinKind};
pub use types::{MethodStub, has_method, lookup_method, methods_for};
pub use stubs::StubRegistry;
pub use registry::{
    type_method_names, module_method_names, has_type_method, has_module_method,
    StdlibLayer, type_layer, module_layer,
};

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
