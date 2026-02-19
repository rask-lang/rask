// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type definitions used throughout the checker.

use std::collections::HashMap;

use rask_ast::NodeId;
use rask_resolve::SymbolId;

use super::type_table::TypeTable;

use crate::types::Type;

/// Information about a user-defined type.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        methods: Vec<MethodSig>,
        is_resource: bool,
    },
    Enum {
        name: String,
        type_params: Vec<String>,
        variants: Vec<(String, Vec<Type>)>,
        methods: Vec<MethodSig>,
    },
    Trait {
        name: String,
        super_traits: Vec<String>,
        methods: Vec<MethodSig>,
        is_unsafe: bool,
    },
    Union {
        name: String,
        fields: Vec<(String, Type)>,
    },
}

/// Method signature.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: String,
    pub self_param: SelfParam,
    pub params: Vec<(Type, ParamMode)>,
    pub ret: Type,
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,   // Static method
    Value,  // self (read-only, default)
    Mutate, // mutate self (mutable)
    Take,   // take self (consumed)
}

/// How a parameter is passed to a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Default, // Normal pass (read-only, default)
    Mutate,  // mutate param (mutable borrow)
    Take,    // take param (consumed)
}

/// Builtin module method signature.
#[derive(Debug, Clone)]
pub struct ModuleMethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// Result of type checking.
#[derive(Debug)]
pub struct TypedProgram {
    /// Resolved symbols from name resolution.
    pub symbols: rask_resolve::SymbolTable,
    /// Symbol resolutions from name resolution.
    pub resolutions: HashMap<NodeId, SymbolId>,
    /// Type table with all type definitions.
    pub types: TypeTable,
    /// Computed type for each expression node.
    pub node_types: HashMap<NodeId, Type>,
    /// Resolved type arguments for each generic call site.
    /// Key is the Call/MethodCall expression's NodeId.
    pub call_type_args: HashMap<NodeId, Vec<Type>>,
}
