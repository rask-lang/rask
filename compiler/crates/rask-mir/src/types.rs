// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR type system - all types are concrete, no generics.

/// Result layout offsets — single source of truth in `rask_mono::abi`.
pub use rask_mono::abi::{
    RESULT_ORIGIN_FILE_OFFSET, RESULT_ORIGIN_LINE_BYTES,
    RESULT_ORIGIN_LINE_OFFSET, TAG_BYTES, TAG_OFFSET,
};

/// Which container a handle points at. What frees it is codegen's to name;
/// this is the vocabulary both ends share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    Vec,
    Map,
    Rack,
}

impl MirType {
    /// This type with every container kind stripped back to a bare pointer —
    /// the spelling MIR stores. See `MirType::Container`.
    pub fn without_container_kinds(&self) -> MirType {
        match self {
            MirType::Container(_) => MirType::Ptr,
            MirType::Option(inner) => {
                MirType::Option(Box::new(inner.without_container_kinds()))
            }
            MirType::Result { ok, err } => MirType::Result {
                ok: Box::new(ok.without_container_kinds()),
                err: Box::new(err.without_container_kinds()),
            },
            MirType::Tuple(elems) => {
                MirType::Tuple(elems.iter().map(|e| e.without_container_kinds()).collect())
            }
            MirType::Array { elem, len } => MirType::Array {
                elem: Box::new(elem.without_container_kinds()),
                len: *len,
            },
            MirType::Union(vs) => {
                MirType::Union(vs.iter().map(|v| v.without_container_kinds()).collect())
            }
            other => other.clone(),
        }
    }
}

impl ContainerKind {
    /// The kind a type's head name stands for, if it is a container.
    pub fn from_head(name: &str) -> Option<ContainerKind> {
        match name {
            "Vec" => Some(ContainerKind::Vec),
            "Map" => Some(ContainerKind::Map),
            "Rack" => Some(ContainerKind::Rack),
            _ => None,
        }
    }

    /// The container a checker type is, if it is one. An optional or a result
    /// holding one is a wrapper, not the handle, and answers `None`.
    pub fn of(ty: &rask_types::Type, names: &std::collections::HashMap<rask_types::TypeId, String>) -> Option<ContainerKind> {
        ty.head_name(names).and_then(Self::from_head)
    }
}

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
    /// A container handle, tagged with what frees it.
    ///
    /// Every container is a pointer, so `Vec<Point> or JsonError` lowers to
    /// `ptr or enum#7`: releasing that freed the error's string and walked
    /// straight past the vector, and `json.decode<Vec<Point>>(t) catch e => …`
    /// leaked everything it decoded. The checker's type knows which it is;
    /// this is how lowering says so.
    ///
    /// **This never reaches a stored type.** `BlockBuilder` strips it out of
    /// every local and return type it is handed and keeps the kind on the
    /// `MirLocal` instead, so MIR's own types are exactly what they were.
    /// They have to be: equality on `MirType` is how codegen tells an
    /// already-wrapped return from a bare payload, and a second spelling of
    /// `Vec<i64>?` made `return none` wrap the option a second time —
    /// `maybe(0)? as w` took the present branch and called `Vec_len` on the
    /// tag. Codegen puts the kind back on the one type it walks, in the one
    /// place that walks it.
    Container(ContainerKind),
    /// `Heap<T>` — the address of a block holding a `T`.
    ///
    /// `Heap(x)` always allocates, so this is a block pointer and nothing else.
    /// It used to skip the allocation when the payload fit the slot, which
    /// meant `Heap<i64>` was an `i64`, `Heap<Vec<i64>>` was the vector's own
    /// handle, and nothing in a type said which of the two a given word was.
    /// Lowering answered from a flag on the binding, and a flag can't cross a
    /// carrier — a struct field or a tuple holds a value, not a binding — so
    /// `*b` and `drop(b)` were each wrong for eight of the sixteen payload
    /// kinds (#1234, #1256).
    ///
    /// Unlike `Container`, this one *does* reach stored types. It has to: the
    /// checker erases `Heap<T>` to `T` (HP5), so this is the only place the
    /// fact survives.
    Heap(Box<MirType>),
    String,
    Struct(StructLayoutId),
    Enum(EnumLayoutId),
    Array {
        elem: Box<MirType>,
        len: u32,
    },
    FuncPtr(SignatureId),
    /// Link<T> — the address of a node whose payload has this layout
    /// (mem.racks/RK2). Carrying the layout is what makes `l.field` an ordinary
    /// base+offset projection: the local holds the address, exactly as an
    /// aggregate local does. `Link<T>?` is the same word with the null address
    /// for `none`, which is the whole niche.
    Link(StructLayoutId),
    /// Tuple type — struct-like layout with positional fields.
    /// Stored as (field types, total byte size).
    Tuple(Vec<MirType>),
    /// Option<T> — tagged union: u8 tag (0=Some, 1=None, the same side as
    /// a Result's Ok/Err — `T?` is `T or none`) + payload.
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
    /// Interface object: fat pointer (data_ptr + vtable_ptr). 16 bytes.
    InterfaceObject {
        interface_name: String,
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
                // An interface object is a 16-byte fat pointer in a stack slot, with
                // the local holding the slot's address — the same convention as
                // a struct. Leaving it out here while `is_aggregate_dst` in
                // codegen counted it as an aggregate is what broke reading one
                // back out of a `T?`: the payload copy sized itself for a
                // scalar and dropped the vtable half, so the call through it
                // segfaulted (#552).
                | MirType::InterfaceObject { .. }
        )
    }

    /// True for the one-word niche payload: a link.
    ///
    /// `Link<T>?` is the same word — no tag, no separate payload slot — with
    /// the null address reserved to mean `none`. Everywhere that asks "is this
    /// option a niche?" is asking this.
    pub fn is_niche_payload(&self) -> bool {
        matches!(self, MirType::Link(_))
    }

    /// The node layout a link names, whether the link is spelled bare or as a
    /// niche option.
    ///
    /// `Link<T>` and `Link<T>?` are the same word and the same edge — a store
    /// to either has to be registered with the rack, or `delete` has no slot to
    /// null. Matching only the bare form is what made `w.focus = n` a plain
    /// store and left a root edge live past its target (#959).
    pub fn as_link(&self) -> Option<StructLayoutId> {
        match self {
            MirType::Link(sid) => Some(*sid),
            MirType::Option(inner) => match inner.as_ref() {
                MirType::Link(sid) => Some(*sid),
                _ => None,
            },
            _ => None,
        }
    }

    /// True when a store to a slot of this type is an edge write.
    pub fn is_link_slot(&self) -> bool {
        self.as_link().is_some()
    }

    /// The word that means `none` for this type, if it is a niche.
    ///
    /// A niche picks a value its own domain can't produce: a link is an
    /// address, so null is it. Asking the type is what keeps this in one place
    /// — nothing downstream should reach for the sentinel constant directly.
    pub fn niche_none(&self) -> Option<i64> {
        match self {
            MirType::Link(_) => Some(rask_mono::abi::LINK_NONE_SENTINEL),
            MirType::Option(inner) => inner.niche_none(),
            _ => None,
        }
    }

    /// Byte size of this type. Structs/enums use pointer size as fallback.
    pub fn size(&self) -> u32 {
        match self {
            MirType::Void => 0,
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_)
            | MirType::Link(_) | MirType::Container(_)
            | MirType::Heap(_) => 8,
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
            MirType::InterfaceObject { .. } => 16, // data_ptr (8) + vtable_ptr (8)
            MirType::Option(_) | MirType::Result { .. } | MirType::Union(_) => {
                self.wrapper_layout().map_or(8, |l| l.size)
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
            MirType::Option(_) | MirType::Result { .. } | MirType::Union(_) => {
                self.wrapper_layout().map_or(8, |l| l.align)
            }
            MirType::Array { elem, .. } => elem.align(),
            MirType::SimdVector { .. } => self.size().min(32),
            _ => 8,
        }
    }

    /// Where a `T?`, `T or E` or error union keeps its payload, and its size.
    /// `rask_mono::abi` is the one definition; `None` for anything else, and
    /// for a niche option, which is its payload.
    pub fn wrapper_layout(&self) -> Option<rask_mono::abi::WrapperLayout> {
        match self {
            MirType::Option(inner) if inner.is_niche_payload() => None,
            MirType::Option(inner) => {
                Some(rask_mono::abi::option_layout(inner.size(), inner.align()))
            }
            MirType::Result { ok, err } => Some(rask_mono::abi::result_layout(
                ok.size().max(err.size()),
                ok.align().max(err.align()),
            )),
            MirType::Union(members) => Some(rask_mono::abi::union_layout(
                members.iter().map(|m| m.size()).max().unwrap_or(0),
                members.iter().map(|m| m.align()).max().unwrap_or(1),
            )),
            _ => None,
        }
    }

    /// The byte offset of a wrapper's payload. A niche option is its payload,
    /// at offset 0.
    pub fn payload_offset(&self) -> u32 {
        self.wrapper_layout().map_or(0, |l| l.payload_offset)
    }

    /// Does an array element of this type live *in* its slot?
    ///
    /// The slots of `[T; N]` are `i * T::size()` apart, so a value that occupies
    /// its slot is copied in whole and read back by address. One store rule and
    /// one read rule, both from here, because they only work as a pair: a store
    /// that writes a word where the reader expects a value in place hands back the
    /// address of whatever the first eight bytes were.
    ///
    /// Not every type that is `passed_by_address`: each kind below was added
    /// once its reads were checked end to end, because widening this without
    /// them turns "doesn't compile" into "compiles and prints an address".
    pub fn stored_inline_in_array(&self) -> bool {
        match self {
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) => true,
            // `[[i32; 2]; 3]`: the inner array's bytes are the element. Stored
            // as a word, the slot held the address of the literal's temporary
            // and `x[0][1]` read through the element's bytes as a pointer.
            MirType::Array { .. } => true,
            // A `T?` element occupies its slot the way a struct does — tag beside
            // payload, 16 bytes for a scalar `T`. Left out, the store wrote one
            // word where the tag belongs and the read loaded one word back, so
            // `[i32?; 3]` couldn't hold a `none` at all (#783).
            //
            // A string's 16 bytes live in the element slot too. Storing the
            // pointer instead aimed every element at whichever frame built the
            // literal: fine while that frame is alive, garbage the moment it
            // isn't — a module-level `const [string; N]` copied to the heap came
            // back pointing at the dead init thunk (#1000), and `join` got an
            // array of addresses where it wanted strings (#1021).
            MirType::String => true,
            // The one exception is the niche: a `Link<T>?` is a single word
            // where the link *is* the value and `none` is the null address, so
            // it keeps the word store. That's why this isn't just "everything
            // passed by address".
            MirType::Option(inner) => !matches!(**inner, MirType::Link(_)),
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

/// Does a spawned task box this result before handing it to the runtime?
///
/// A task's result travels as one machine word — `GreenTask::result` is an
/// `int64_t` — so anything that doesn't fit goes on the heap and the word is its
/// address. A float is boxed too even though it fits: it comes back in a *float*
/// register, and the runtime calls the closure through `int64_t (*)(void *)`, so
/// the value was never in the register it read (#963).
///
/// A `T?` or `T or E` is boxed whatever its size: a function returns one
/// through a destination pointer, never in a register (`returns_through_dst`).
///
/// Both sides ask this: MIR lowering when it decides to put a boxing thunk in
/// front of the closure, and codegen when the join decides whether to copy
/// through an address. One definition, so they can't drift apart.
pub fn spawn_payload_is_boxed(ty: &MirType) -> bool {
    match ty {
        MirType::Void => false,
        MirType::F32 | MirType::F64 => true,
        other => other.size() > 8 || other.wrapper_layout().is_some(),
    }
}
