// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR type system - all types are concrete, no generics.

/// MIR type - all sizes known, no generic type parameters
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MirType {
    Void,
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Char,
    Ptr,
    String,
    Struct(StructLayoutId),
    Enum(EnumLayoutId),
    Array {
        elem: Box<MirType>,
        len: u32,
    },
    FuncPtr(SignatureId),
}

impl MirType {
    /// Byte size of this type. Structs/enums use pointer size as fallback.
    pub fn size(&self) -> u32 {
        match self {
            MirType::Void => 0,
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_) => 8,
            MirType::String => 16,
            MirType::Struct(_) | MirType::Enum(_) => 8,
            MirType::Array { elem, len } => elem.size() * len,
        }
    }

    /// Alignment of this type in bytes.
    pub fn align(&self) -> u32 {
        match self {
            MirType::Bool | MirType::I8 | MirType::U8 | MirType::Void => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            _ => 8,
        }
    }

    /// True for F32 and F64.
    pub fn is_float(&self) -> bool {
        matches!(self, MirType::F32 | MirType::F64)
    }

    /// True for unsigned integer types.
    pub fn is_unsigned(&self) -> bool {
        matches!(self, MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StructLayoutId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumLayoutId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignatureId(pub u32);
