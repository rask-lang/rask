// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR type system - all types are concrete, no generics.

/// Result layout offsets — single source of truth in `rask_mono::abi`.
pub use rask_mono::abi::{
    RESULT_ORIGIN_FILE_OFFSET, RESULT_ORIGIN_LINE_OFFSET, RESULT_PAYLOAD_OFFSET, RESULT_TAG_OFFSET,
};

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
    /// Does a value of this type live in its own storage, so it's handed around
    /// as an address rather than as a register-sized value?
    ///
    /// Every place that needs to know this used to spell out its own list, and
    /// the lists didn't match — `Struct | Enum` in one, plus `Tuple | String` in
    /// another, plus `Array` in a third. A tuple guard behind a `Mutex` was
    /// classified as word-sized on the strength of one of the short lists and
    /// got loaded as a single i64.
    pub fn passed_by_address(&self) -> bool {
        matches!(
            self,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Array { .. }
                | MirType::String
                | MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Union(_)
                | MirType::SimdVector { .. }
        )
    }

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
            MirType::Struct(sid) => sid.byte_size,
            MirType::Enum(eid) => eid.byte_size,
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
                // [tag:8][origin_file:8][origin_line:8][payload] — offsets in rask_mono::abi (ER15).
                RESULT_PAYLOAD_OFFSET + ok.size().max(err.size())
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
            MirType::Struct(sid) => sid.align,
            MirType::Enum(eid) => eid.align,
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

#[derive(Debug, Clone, Copy)]
pub struct StructLayoutId {
    pub id: u32,
    pub byte_size: u32,
    pub align: u32,
}

impl StructLayoutId {
    pub fn new(id: u32, byte_size: u32, align: u32) -> Self {
        Self { id, byte_size, align }
    }
}

impl PartialEq for StructLayoutId {
    fn eq(&self, other: &Self) -> bool { self.id == other.id }
}
impl Eq for StructLayoutId {}
impl std::hash::Hash for StructLayoutId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.id.hash(state); }
}

#[derive(Debug, Clone, Copy)]
pub struct EnumLayoutId {
    pub id: u32,
    pub byte_size: u32,
    pub align: u32,
}

impl EnumLayoutId {
    pub fn new(id: u32, byte_size: u32, align: u32) -> Self {
        Self { id, byte_size, align }
    }
}

impl PartialEq for EnumLayoutId {
    fn eq(&self, other: &Self) -> bool { self.id == other.id }
}
impl Eq for EnumLayoutId {}
impl std::hash::Hash for EnumLayoutId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.id.hash(state); }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignatureId(pub u32);
