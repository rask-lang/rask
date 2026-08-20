// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR type system - all types are concrete, no generics.

/// Result layout offsets — single source of truth in `rask_mono::abi`.
pub use rask_mono::abi::{
    RESULT_ORIGIN_FILE_OFFSET, RESULT_ORIGIN_LINE_OFFSET, RESULT_PAYLOAD_OFFSET, RESULT_TAG_OFFSET,
    UNION_MEMBER_OFFSET, UNION_PAYLOAD_OFFSET,
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
    /// 128-bit signed. Two machine words; Cranelift lowers add/sub/mul, and
    /// div/rem go through runtime helpers (#762).
    I128,
    U8,
    U16,
    U32,
    U64,
    /// 128-bit unsigned. See `I128`.
    U128,
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
    /// P2: what `usize` is on this target — pointer-sized. The width comes
    /// from `rask_ast::primitives::pointer_bits`, the one place that decides it.
    pub fn usize_ty() -> MirType {
        if rask_ast::primitives::pointer_bits() == 32 { MirType::U32 } else { MirType::U64 }
    }

    /// P2: what `isize` is on this target.
    pub fn isize_ty() -> MirType {
        if rask_ast::primitives::pointer_bits() == 32 { MirType::I32 } else { MirType::I64 }
    }

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
                // A trait object is a 16-byte fat pointer in a stack slot, with
                // the local holding the slot's address — the same convention as
                // a struct. Leaving it out here while `is_aggregate_dst` in
                // codegen counted it as an aggregate is what broke reading one
                // back out of a `T?`: the payload copy sized itself for a
                // scalar and dropped the vtable half, so the call through it
                // segfaulted (#552).
                | MirType::TraitObject { .. }
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
            MirType::I128 | MirType::U128 => 16,
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
            // [member:8][member bytes] — the members are nominally distinct
            // types with nothing in their bytes to tell them apart, so the index
            // is stored (#776). `UNION_MEMBER_OFFSET` / `UNION_PAYLOAD_OFFSET`
            // name the two halves.
            MirType::Union(variants) => {
                UNION_PAYLOAD_OFFSET + variants.iter().map(|v| v.size()).max().unwrap_or(0)
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
            MirType::I128 | MirType::U128 => 16,
            MirType::Tuple(fields) => fields.iter().map(|f| f.align()).max().unwrap_or(1),
            MirType::Struct(sid) => sid.align,
            MirType::Enum(eid) => eid.align,
            MirType::Slice(_) | MirType::Option(_) | MirType::Result { .. } | MirType::Union(_) => 8,
            _ => 8,
        }
    }

    /// Does an array element of this type live *in* its slot?
    ///
    /// The slots of `[T; N]` are `i * T::size()` apart, so a value that occupies
    /// its slot is copied in whole and read back by address. One store rule and
    /// one read rule, both from here, because they only work as a pair: a store
    /// that writes a word where the reader expects a value in place hands back the
    /// address of whatever the first eight bytes were.
    ///
    /// Struct, enum and tuple only, deliberately — not every type that is
    /// `passed_by_address`. A `string` is a pointer to its 16 bytes and the whole
    /// read path expects that (#414). A wrapper element (`[i32?; 3]`) would want
    /// to be inline but isn't supported end to end: its size isn't a multiple of
    /// 8, and the `??` and tag reads expect a loaded value rather than an address,
    /// so indexing one hands back the slot address as if it were the payload
    /// (#783). Widening this without those is how that turns from "doesn't
    /// compile" into "compiles and prints an address".
    pub fn stored_inline_in_array(&self) -> bool {
        match self {
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) => true,
            // A `T?` element occupies its slot the way a struct does — tag beside
            // payload, 16 bytes for a scalar `T`. Left out, the store wrote one
            // word where the tag belongs and the read loaded one word back, so
            // `[i32?; 3]` couldn't hold a `none` at all (#783).
            //
            // The one exception is the niche: a `Handle<T>?` is a single word
            // where the handle *is* the value and `none` is the all-ones
            // sentinel, so it keeps the word store. `string` is a pointer to its
            // 16 bytes and keeps it too, which is why this isn't just
            // "everything passed by address".
            MirType::Option(inner) => **inner != MirType::Handle,
            _ => false,
        }
    }

    /// True for F32 and F64.
    pub fn is_float(&self) -> bool {
        matches!(self, MirType::F32 | MirType::F64)
    }

    /// True for unsigned integer types.
    pub fn is_unsigned(&self) -> bool {
        matches!(self, MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 | MirType::U128)
    }

    /// True for an integer primitive of either signedness — the set where a
    /// mixed-signedness comparison is well-defined (type.operators/CMP-mixed).
    pub fn is_int_like(&self) -> bool {
        matches!(
            self,
            MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64 | MirType::I128
                | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 | MirType::U128
        )
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
