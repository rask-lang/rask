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
    /// Handle<T> — pool handle, packed as i64 (index:32 | gen:32) in current codegen.
    Handle,
    /// Tuple type — struct-like layout with positional fields.
    /// Stored as (field types, total byte size).
    Tuple(Vec<MirType>),
    /// Slice — pointer + length (fat pointer).
    Slice(Box<MirType>),
    /// Option<T> — tagged union: u8 tag (0=None, 1=Some) + payload.
    /// Size = 8 (tag aligned) + payload size, rounded to 8-byte alignment.
    Option(Box<MirType>),
    /// Result<T, E> — tagged union: u8 tag (0=Ok, 1=Err) + max(T, E) payload.
    Result {
        ok: Box<MirType>,
        err: Box<MirType>,
    },
    /// Union of error types — tracks variant sizes for layout.
    Union(Vec<MirType>),
    /// SIMD vector: elem × lanes (e.g., F32 × 8 = f32x8).
    /// Passed as pointer in codegen (like structs/arrays).
    SimdVector {
        elem: Box<MirType>,
        lanes: u32,
    },
    /// Trait object: fat pointer (data_ptr + vtable_ptr). 16 bytes.
    TraitObject {
        trait_name: String,
    },
}

impl MirType {
    /// Byte size of this type. Structs/enums use pointer size as fallback.
    pub fn size(&self) -> u32 {
        match self {
            MirType::Void => 0,
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_)
            | MirType::Handle => 8,
            MirType::String => 16,
            MirType::Struct(_) | MirType::Enum(_) => 8,
            MirType::Array { elem, len } => elem.size() * len,
            MirType::Tuple(fields) => {
                let mut offset = 0u32;
                for f in fields {
                    let align = f.align();
                    offset = (offset + align - 1) & !(align - 1);
                    offset += f.size();
                }
                // Round up to max alignment
                let max_align = fields.iter().map(|f| f.align()).max().unwrap_or(1);
                (offset + max_align - 1) & !(max_align - 1)
            }
            MirType::Slice(_) => 16,         // ptr (8) + len (8)
            MirType::TraitObject { .. } => 16, // data_ptr (8) + vtable_ptr (8)
            MirType::Option(inner) => {
                // tag (8 bytes, aligned) + payload
                8 + inner.size()
            }
            MirType::Result { ok, err } => {
                // tag (8 bytes, aligned) + max(ok, err) payload
                8 + ok.size().max(err.size())
            }
            MirType::Union(variants) => {
                variants.iter().map(|v| v.size()).max().unwrap_or(0)
            }
            MirType::SimdVector { elem, lanes } => elem.size() * lanes,
        }
    }

    /// Alignment of this type in bytes.
    pub fn align(&self) -> u32 {
        match self {
            MirType::Bool | MirType::I8 | MirType::U8 | MirType::Void => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::Tuple(fields) => fields.iter().map(|f| f.align()).max().unwrap_or(1),
            MirType::Slice(_) | MirType::Option(_) | MirType::Result { .. } | MirType::Union(_) => 8,
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
