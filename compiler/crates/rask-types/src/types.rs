//! Type definitions for the type system.

use std::hash::Hash;

/// Unique identifier for user-defined types (structs, enums, traits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Unique identifier for type variables during inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVarId(pub u32);

/// A type in Rask.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    /// Unit type
    Unit,
    /// Boolean
    Bool,
    /// Signed integers
    I8,
    I16,
    I32,
    I64,
    /// Unsigned integers
    U8,
    U16,
    U32,
    U64,
    /// Floating point
    F32,
    F64,
    /// Character
    Char,
    /// String
    String,
    /// Named user-defined type (struct, enum, etc.)
    Named(TypeId),
    /// Unresolved named type (before type registration)
    UnresolvedNamed(std::string::String),
    /// Generic type with parameters
    Generic {
        base: TypeId,
        args: Vec<Type>,
    },
    /// Unresolved generic (before type registration)
    UnresolvedGeneric {
        name: std::string::String,
        args: Vec<Type>,
    },
    /// Function type
    Fn {
        params: Vec<Type>,
        ret: Box<Type>,
    },
    /// Tuple type
    Tuple(Vec<Type>),
    /// Array type with fixed size
    Array {
        elem: Box<Type>,
        len: usize,
    },
    /// Slice type (view into array/vec)
    Slice(Box<Type>),
    /// Option type (T?)
    Option(Box<Type>),
    /// Result type
    Result {
        ok: Box<Type>,
        err: Box<Type>,
    },
    /// Type variable (for inference)
    Var(TypeVarId),
    /// Never type (for return, panic, etc.)
    Never,
    /// Error placeholder for recovery
    Error,
}
