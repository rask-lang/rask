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
        /// U1–U4: marked @unique — no implicit copy even if small enough
        is_unique: bool,
        /// B1–G4: @binary struct for wire-format parsing/building
        is_binary: bool,
        /// V5: fields marked `private` — accessible only inside extend blocks
        private_fields: Vec<String>,
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
    /// Nominal type alias: same layout as underlying, distinct identity.
    NominalAlias {
        name: String,
        underlying: Type,
        with_traits: Vec<String>,
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

/// Endianness for multi-byte binary fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

/// A single field's binary layout specifier.
#[derive(Debug, Clone)]
pub struct BinaryFieldSpec {
    pub name: String,
    pub bits: u32,
    pub endian: Option<Endian>,
    pub runtime_type: Type,
    /// Byte offset within the struct where this field's bits start
    pub bit_offset: u32,
    /// Whether this is a fixed byte array ([N]u8)
    pub is_byte_array: bool,
    pub byte_array_len: usize,
}

/// Metadata for a @binary struct.
#[derive(Debug, Clone)]
pub struct BinaryStructInfo {
    pub name: String,
    pub fields: Vec<BinaryFieldSpec>,
    pub total_bits: u32,
    /// SIZE in bytes (rounded up)
    pub size_bytes: u32,
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
    /// TR5: implicit trait coercion sites. NodeId of expression → trait name.
    pub trait_coercions: HashMap<NodeId, String>,
    /// Unsafe operations recorded during type checking (span + category).
    pub unsafe_ops: Vec<(rask_ast::Span, super::UnsafeCategory)>,
    /// Types for binding names and parameters, keyed by (span.start, span.end).
    /// Used by the LSP for hover on identifiers that aren't expression nodes.
    pub span_types: HashMap<(usize, usize), Type>,
}
