// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type definitions for the type system.

use std::fmt;
use std::hash::Hash;

/// Unique identifier for user-defined types (structs, enums, traits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

/// Unique identifier for type variables during inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeVarId(pub u32);

/// A generic argument (for const generics support).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericArg {
    /// A type argument (regular generic)
    Type(Box<Type>),
    /// A const usize argument (const generic)
    ConstUsize(usize),
}

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
    I128,
    /// Unsigned integers
    U8,
    U16,
    U32,
    U64,
    U128,
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
        args: Vec<GenericArg>,
    },
    /// Unresolved generic (before type registration)
    UnresolvedGeneric {
        name: std::string::String,
        args: Vec<GenericArg>,
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
    /// Union type (error position only): `IoError | ParseError`
    /// Canonical form: sorted alphabetically, deduplicated.
    Union(Vec<Type>),
    /// Type variable (for inference)
    Var(TypeVarId),
    /// Raw pointer type (*T)
    RawPtr(Box<Type>),
    /// SIMD vector type: Vec[T, N] with shorthand aliases (f32x8, i32x4, etc.)
    SimdVector {
        elem: Box<Type>,
        lanes: usize,
    },
    /// Never type (for return, panic, etc.)
    Never,
    /// Error placeholder for recovery
    Error,
}

impl Type {
    /// Set the display name for Named types (used for readable error messages).
    /// Returns a new type with the name resolved if applicable.
    pub fn with_name(self, name: std::string::String) -> Type {
        match self {
            Type::Named(_) => Type::UnresolvedNamed(name),
            other => other,
        }
    }

    /// Build a canonical union type: sorted by Display name, deduplicated.
    /// Single-element unions collapse to the inner type.
    /// Nested unions are flattened.
    pub fn union(types: Vec<Type>) -> Type {
        let mut flat = Vec::new();
        for ty in types {
            match ty {
                Type::Union(inner) => flat.extend(inner),
                other => flat.push(other),
            }
        }
        // Sort by display name for canonical ordering
        flat.sort_by(|a, b| format!("{}", a).cmp(&format!("{}", b)));
        flat.dedup();
        match flat.len() {
            0 => Type::Unit,
            1 => flat.into_iter().next().unwrap(),
            _ => Type::Union(flat),
        }
    }

    /// Check if this type is a subset of another union type.
    pub fn is_subset_of(&self, other: &Type) -> bool {
        let self_types = match self {
            Type::Union(types) => types.as_slice(),
            other => std::slice::from_ref(other),
        };
        let other_types = match other {
            Type::Union(types) => types.as_slice(),
            other => std::slice::from_ref(other),
        };
        self_types.iter().all(|t| other_types.contains(t))
    }
}

impl fmt::Display for GenericArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericArg::Type(ty) => write!(f, "{}", ty),
            GenericArg::ConstUsize(n) => write!(f, "{}", n),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Unit => write!(f, "()"),
            Type::Bool => write!(f, "bool"),
            Type::I8 => write!(f, "i8"),
            Type::I16 => write!(f, "i16"),
            Type::I32 => write!(f, "i32"),
            Type::I64 => write!(f, "i64"),
            Type::I128 => write!(f, "i128"),
            Type::U8 => write!(f, "u8"),
            Type::U16 => write!(f, "u16"),
            Type::U32 => write!(f, "u32"),
            Type::U64 => write!(f, "u64"),
            Type::U128 => write!(f, "u128"),
            Type::F32 => write!(f, "f32"),
            Type::F64 => write!(f, "f64"),
            Type::Char => write!(f, "char"),
            Type::String => write!(f, "string"),
            Type::Named(id) => write!(f, "<type#{}>", id.0),
            Type::UnresolvedNamed(name) => write!(f, "{}", name),
            Type::Generic { base, args } => {
                write!(f, "<type#{}>", base.0)?;
                write!(f, "<")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ">")
            }
            Type::UnresolvedGeneric { name, args } => {
                write!(f, "{}<", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ">")
            }
            Type::Fn { params, ret } => {
                write!(f, "func(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Type::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", e)?;
                }
                write!(f, ")")
            }
            Type::Array { elem, len } => write!(f, "[{}; {}]", elem, len),
            Type::Slice(elem) => write!(f, "[{}]", elem),
            Type::Option(inner) => write!(f, "{}?", inner),
            Type::Result { ok, err } => write!(f, "{} or {}", ok, err),
            Type::Union(types) => {
                for (i, ty) in types.iter().enumerate() {
                    if i > 0 { write!(f, " | ")?; }
                    write!(f, "{}", ty)?;
                }
                Ok(())
            }
            Type::RawPtr(inner) => write!(f, "*{}", inner),
            Type::SimdVector { elem, lanes } => write!(f, "{}x{}", elem, lanes),
            Type::Var(_) => write!(f, "_"),
            Type::Never => write!(f, "!"),
            Type::Error => write!(f, "<error>"),
        }
    }
}
