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
    /// Result type — also represents `T?` when err = Type::None.
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
    /// Trait object: `any TraitName` — heap-boxed, vtable-dispatched.
    TraitObject {
        trait_name: std::string::String,
    },
    /// Never type (for return, panic, etc.)
    Never,
    /// Absent sentinel: zero-field type with one inhabitant.
    /// The absent variant of `T?` (sugar for `T or none`).
    None,
    /// Error placeholder for recovery
    Error,
}

impl Type {
    /// Does this type still contain an inference variable anywhere inside it?
    ///
    /// An answer that does is present and useless: it converts to a plausible
    /// 8-byte scalar and nothing downstream can tell it from a real type. Three
    /// passes ask this — the checker reporting a binding it couldn't infer, its
    /// open-node census, and MIR deciding whether a recorded type can become a
    /// layout — and they each had their own copy of the match. They agreed, but
    /// only until someone added a `Type` variant and updated one of them.
    pub fn has_unsolved_var(&self) -> bool {
        match self {
            Type::Var(_) => true,
            Type::Result { ok, err } => ok.has_unsolved_var() || err.has_unsolved_var(),
            Type::RawPtr(inner) | Type::Slice(inner) => inner.has_unsolved_var(),
            Type::Array { elem, .. } => elem.has_unsolved_var(),
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().any(Type::has_unsolved_var)
            }
            Type::Fn { params, ret } => {
                params.iter().any(Type::has_unsolved_var) || ret.has_unsolved_var()
            }
            Type::SimdVector { elem, .. } => elem.has_unsolved_var(),
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args
                .iter()
                .any(|a| matches!(a, GenericArg::Type(t) if t.has_unsolved_var())),
            _ => false,
        }
    }

    /// P2: what `usize` is on this target — pointer-sized, not always 64-bit.
    /// The width comes from `rask_ast::primitives::pointer_bits`, which is the
    /// single place that decides it.
    pub fn usize_ty() -> Type {
        if rask_ast::primitives::pointer_bits() == 32 { Type::U32 } else { Type::U64 }
    }

    /// How many bytes a scalar of this type occupies, or `None` if it isn't a
    /// scalar.
    ///
    /// The one place the widths are written down. `rask_mono`'s layout pass and
    /// the atomic-payload check both read them here, so a struct's size and the
    /// rule about what fits a machine word can't drift apart (#1083).
    ///
    /// `char` is a full word: the runtime carries a code point in one.
    pub fn scalar_bytes(&self) -> Option<u32> {
        Some(match self {
            Type::Bool | Type::I8 | Type::U8 => 1,
            Type::I16 | Type::U16 => 2,
            Type::I32 | Type::U32 | Type::F32 => 4,
            Type::I64 | Type::U64 | Type::F64 | Type::Char => 8,
            Type::I128 | Type::U128 => 16,
            _ => return None,
        })
    }

    /// P2: what `isize` is on this target.
    pub fn isize_ty() -> Type {
        if rask_ast::primitives::pointer_bits() == 32 { Type::I32 } else { Type::I64 }
    }

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

    /// Construct `T?` = `T or none`.
    pub fn option(inner: Type) -> Type {
        Type::Result { ok: Box::new(inner), err: Box::new(Type::None) }
    }

    /// True if this is the optional shape (`T or none`).
    pub fn is_option(&self) -> bool {
        matches!(self, Type::Result { err, .. } if **err == Type::None)
    }

    /// Unwrap the inner type if this is an optional (`T or none`).
    pub fn as_option(&self) -> Option<&Type> {
        if let Type::Result { ok, err } = self {
            if **err == Type::None { return Some(ok); }
        }
        None
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
            Type::Unit => write!(f, "void"),
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
            Type::Result { ok, err } if **err == Type::None => write!(f, "{}?", ok),
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
            Type::TraitObject { trait_name } => write!(f, "any {}", trait_name),
            Type::Var(_) => write!(f, "_"),
            Type::Never => write!(f, "!"),
            Type::None => write!(f, "none"),
            Type::Error => write!(f, "<error>"),
        }
    }
}
