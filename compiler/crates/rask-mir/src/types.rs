// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR type system - all types are concrete, no generics.

/// Result layout offsets — single source of truth in `rask_types::abi`.
pub use rask_types::abi::{
    RESULT_ORIGIN_FILE_BYTES, RESULT_ORIGIN_FILE_OFFSET, RESULT_ORIGIN_LINE_BYTES,
    RESULT_ORIGIN_LINE_OFFSET, TAG_BYTES, TAG_OFFSET,
};

/// Which container a handle points at. What frees it is codegen's to name;
/// this is the vocabulary both ends share.
///
/// `Handle` is any other runtime handle that frees itself: its type declares
/// `@handle(release: "…")` (`Random`, `Atomic<T>`, `Staged<T>`, `cstring`),
/// and the declared release is all there is to know about freeing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    Vec,
    Map,
    Rack,
    Handle(&'static str),
}

impl ContainerKind {
    /// A self-freeing handle by its declared release, if `release` is one.
    pub fn handle(release: &str) -> Option<ContainerKind> {
        rask_stdlib::handles::declared_releases()
            .into_iter()
            .find(|r| *r == release)
            .map(ContainerKind::Handle)
    }
}

impl MirType {
    /// How many `T?` / `T or E` layers wrap the value: `i64` is 0, `Vec<i64>?`
    /// is 1, `i64? or E` is 2. A value with fewer layers than the slot it goes
    /// into still needs wrapping; one with as many already is the wrapper.
    ///
    /// The count is the question both lowering and codegen ask before
    /// wrapping. Codegen used to ask it as type equality instead, and equality
    /// also compares what the payload is: `Vec<i64>?` against `ptr?` said
    /// "not the same, so wrap", and `return none` came back wrapped twice.
    pub fn wrapper_depth(&self) -> usize {
        match self {
            MirType::Option(inner) => 1 + inner.wrapper_depth(),
            MirType::Result { ok, .. } => 1 + ok.wrapper_depth(),
            _ => 0,
        }
    }

    /// This type in a slot that holds something another owner releases. A
    /// container handle there is a plain word: `Container(kind)` says the
    /// slot's owner frees it, and a borrowed element isn't the slot's.
    ///
    /// A string needs no such spelling: a borrowed one copied into a slot
    /// takes a reference of its own, so the slot does own one.
    pub fn borrowed(&self) -> MirType {
        match self {
            MirType::Container(_) => MirType::Ptr,
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

/// MIR type - all sizes known, no generic type parameters. Its size is
/// `rask_layout::size_of`'s to say, through the `Shaped` impl below.
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
    /// A raw pointer `*T`: an address, released by nothing. Never null, so
    /// `*T?` is this one word with null for `none` (mem.unsafe/PT6). `Ptr` is
    /// every other address, and an option over one has a tag.
    RawPtr,
    /// A container handle: one word, freed by its kind's free.
    ///
    /// Every `Vec`, `Map` and `Rack` is this, wherever it sits — a local, a
    /// field, a tuple part, an optional's payload. A bare `Ptr` here said
    /// nothing about what freed it, so `Vec<Point> or JsonError` lowered to
    /// `ptr or enum#7`, releasing it walked straight past the vector, and
    /// `json.decode<Vec<Point>>(t) catch e => …` leaked everything it decoded.
    ///
    /// It used to be stripped back to `Ptr` in every stored type, with the
    /// kind kept on the side, because codegen told an already-wrapped return
    /// from a bare payload by type equality (see `wrapper_depth`).
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
    /// The checker erases `Heap<T>` to `T` (HP5), so this is the only place
    /// the fact survives.
    Heap(Box<MirType>),
    /// A refcounted, immutable byte buffer — 16 bytes, inline when short.
    /// `string` and `bytes` are both this. What the bytes mean (text or not)
    /// is the checker's to say, and lowering asks it there; nothing after
    /// lowering needs to know.
    Buffer,
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

impl rask_layout::Shaped for MirType {
    fn shape(&self) -> rask_layout::Shape<'_, MirType> {
        use rask_layout::Shape;
        use rask_types::Type;
        match self {
            MirType::Void => Shape::Void,
            MirType::Bool => Shape::scalar(Type::Bool),
            MirType::I8 => Shape::scalar(Type::I8),
            MirType::I16 => Shape::scalar(Type::I16),
            MirType::I32 => Shape::scalar(Type::I32),
            MirType::I64 => Shape::scalar(Type::I64),
            MirType::I128 => Shape::scalar(Type::I128),
            MirType::U8 => Shape::scalar(Type::U8),
            MirType::U16 => Shape::scalar(Type::U16),
            MirType::U32 => Shape::scalar(Type::U32),
            MirType::U64 => Shape::scalar(Type::U64),
            MirType::U128 => Shape::scalar(Type::U128),
            MirType::F32 => Shape::scalar(Type::F32),
            MirType::F64 => Shape::scalar(Type::F64),
            MirType::Char => Shape::scalar(Type::Char),
            MirType::Ptr | MirType::RawPtr | MirType::FuncPtr(_) | MirType::Link(_) | MirType::Container(_)
            | MirType::Heap(_) => Shape::Word,
            MirType::Buffer => Shape::Buffer,
            MirType::Struct(sid) => Shape::Fixed { size: sid.byte_size, align: sid.align },
            MirType::Enum(eid) => Shape::Fixed { size: eid.byte_size, align: eid.align },
            MirType::Array { elem, len } => Shape::Array(elem, *len),
            MirType::Tuple(fields) => Shape::Tuple(fields),
            // A niche option is its payload's word, with no tag.
            MirType::Option(inner) if inner.is_niche_payload() => Shape::Word,
            MirType::Option(inner) => Shape::Option(inner),
            MirType::Result { ok, err } => Shape::Result(ok, err),
            MirType::Union(members) => Shape::Union(members),
            MirType::SimdVector { elem, lanes } => Shape::Simd(elem, *lanes),
            MirType::InterfaceObject { .. } => Shape::FatPointer,
        }
    }
}

impl MirType {
    /// P2: what `usize` is on this target — pointer-sized.
    pub fn usize_ty(target: rask_ast::Target) -> MirType {
        if target.pointer_bits() == 32 { MirType::U32 } else { MirType::U64 }
    }

    /// P2: what `isize` is on this target.
    pub fn isize_ty(target: rask_ast::Target) -> MirType {
        if target.pointer_bits() == 32 { MirType::I32 } else { MirType::I64 }
    }

    /// Does a value of this type live in its own storage, so it's handed around
    /// as an address rather than as a register-sized value?
    ///
    /// Every place that needs to know this used to spell out its own list, and
    /// the lists didn't match — `Struct | Enum` in one, plus `Tuple | String` in
    /// another, plus `Array` in a third. A tuple guard behind a `Mutex` was
    /// classified as word-sized on the strength of one of the short lists and
    /// got loaded as a single i64.
    ///
    /// A niche option — `Link<T>?` — is the exception that's spelled like an
    /// aggregate: one word, the link itself or null for `none`, so it travels
    /// in a register like the link does. Answered "by address", a
    /// `Vec<Handle<T>?>` read every slot as present: an address is never the
    /// sentinel (#959).
    pub fn passed_by_address(&self) -> bool {
        if self.niche_none().is_some() {
            return false;
        }
        matches!(
            self,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Array { .. }
                | MirType::Buffer
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

    /// Does a value of this type live in a stack slot of its own, with its
    /// local holding the slot's address? The slot is `size_of` bytes. A SIMD
    /// vector is addressed too, but its buffer lives elsewhere and its local is
    /// the pointer.
    pub fn needs_slot(&self) -> bool {
        self.passed_by_address() && !matches!(self, MirType::SimdVector { .. })
    }

    /// True for the one-word niche payloads: a link and a raw pointer.
    ///
    /// `Link<T>?` and `*T?` are the same word as their payload — no tag, no
    /// separate payload slot — with the null address reserved to mean `none`.
    /// Everywhere that asks "is this option a niche?" is asking this.
    pub fn is_niche_payload(&self) -> bool {
        matches!(self, MirType::Link(_) | MirType::RawPtr)
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
            MirType::Link(_) => Some(rask_types::abi::LINK_NONE_SENTINEL),
            // Only an option directly over a link. `Link<T>??` is a tagged
            // option whose payload happens to be a niche one, and its `none`
            // is a tag, not the null address.
            MirType::Option(inner) if inner.is_niche_payload() => match inner.as_ref() {
                MirType::RawPtr => Some(rask_types::abi::RAW_PTR_NONE_SENTINEL),
                _ => inner.niche_none(),
            },
            _ => None,
        }
    }

    /// Does an array element of this type live *in* its slot?
    ///
    /// The slots of `[T; N]` are `i * size_of(T)` apart, so a value that occupies
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
            MirType::Buffer => true,
            // The one exception is the niche: a `Link<T>?` is a single word
            // where the link *is* the value and `none` is the null address, so
            // it keeps the word store. That's why this isn't just "everything
            // passed by address".
            MirType::Option(inner) => !inner.is_niche_payload(),
            // `T or E` and a union lay their tag and payload out in the slot
            // the same way. The literal always copied them in whole, so
            // reading one back as a word handed out the first eight bytes as
            // the element's address: `[f(3), f(1)]` segfaulted on `v[0]`
            // (#1846).
            MirType::Result { .. } | MirType::Union(_) => true,
            _ => false,
        }
    }

    /// A number, bool or char: the bytes are the whole value, so a copy of
    /// one neither holds nor points at any storage.
    pub fn is_plain_scalar(&self) -> bool {
        self.is_int_like() || self.is_float() || matches!(self, MirType::Bool | MirType::Char | MirType::Void)
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
pub fn spawn_payload_is_boxed(ty: &MirType, target: rask_ast::Target) -> bool {
    match ty {
        MirType::Void => false,
        MirType::F32 | MirType::F64 => true,
        other => rask_layout::size_of(other, target) > 8 || rask_layout::wrapper_of(other, target).is_some(),
    }
}

/// Each struct field's and each enum variant field's MIR type, indexed the way
/// the layouts are: `structs[layout id][field]` and
/// `variants[layout id][variant][field]`.
///
/// A layout carries its fields' *checker* types, and codegen has no way to turn
/// those into MIR types. Its ownership walks used to keep a second, checker-typed
/// copy of every arm for the fields of a struct or a variant, and the copy was
/// missing arms the MIR walk had: a tuple, a closure or an optional field was
/// never released. Lowering computes these once, with the same conversion every
/// other MIR type comes from.
#[derive(Debug, Clone, Default)]
pub struct FieldTypes {
    /// The machine these layouts are for: what a pointer-sized field measures.
    pub target: rask_ast::Target,
    pub structs: Vec<Vec<MirType>>,
    pub variants: Vec<Vec<Vec<MirType>>>,
    /// Where each struct layout carries edges: `RASK_RACK_FIELD_*` kind and byte
    /// offset, at any depth of inline nesting. Computed from the checker's
    /// types, because a `Vec<Link<T>>` and a `Vec<i64>` are the same
    /// `MirType::Container` and only the element type tells them apart.
    pub struct_links: Vec<Vec<LinkSlot>>,
}

/// One place a value holds an edge the rack must know about: a link, or a
/// `Vec`/`Map` whose values are links. `kind` is a `RASK_RACK_FIELD_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSlot {
    pub kind: i32,
    pub offset: u32,
}

/// The struct layouts that hold a `Rack` anywhere inside them, by value: in a
/// field, a nested struct, a tuple or array, or an optional's payload.
pub fn rack_holding_structs(fields: &FieldTypes) -> std::collections::HashSet<u32> {
    fn holds(ty: &MirType, holders: &std::collections::HashSet<u32>) -> bool {
        match ty {
            MirType::Container(ContainerKind::Rack) => true,
            MirType::Struct(id) => holders.contains(&id.id),
            MirType::Tuple(parts) => parts.iter().any(|p| holds(p, holders)),
            MirType::Array { elem, .. } | MirType::Option(elem) => holds(elem, holders),
            MirType::Result { ok, err } => holds(ok, holders) || holds(err, holders),
            _ => false,
        }
    }
    let mut holders = std::collections::HashSet::new();
    loop {
        let before = holders.len();
        for (id, tys) in fields.structs.iter().enumerate() {
            if !holders.contains(&(id as u32)) && tys.iter().any(|t| holds(t, &holders)) {
                holders.insert(id as u32);
            }
        }
        if holders.len() == before {
            return holders;
        }
    }
}

/// The struct and enum layouts that are Copy (mem.value/VS1): sixteen bytes
/// or fewer as laid out, and every field Copy. A string is Copy (the
/// compiler counts it); a container, a heap block, a closure or an interface
/// object isn't. The release pass reads this to tell a Copy value copied
/// into a keeper from a value moved into one.
#[derive(Debug, Clone, Default)]
pub struct CopyLayouts {
    target: rask_ast::Target,
    pub structs: std::collections::HashSet<u32>,
    pub enums: std::collections::HashSet<u32>,
}

impl CopyLayouts {
    pub fn from_fields(fields: &FieldTypes) -> CopyLayouts {
        let mut copy = CopyLayouts { target: fields.target, ..CopyLayouts::default() };
        loop {
            let before = copy.structs.len() + copy.enums.len();
            for (id, tys) in fields.structs.iter().enumerate() {
                let id = id as u32;
                if !copy.structs.contains(&id) && tys.iter().all(|t| copy.is_copy(t)) {
                    copy.structs.insert(id);
                }
            }
            for (id, variants) in fields.variants.iter().enumerate() {
                let id = id as u32;
                if !copy.enums.contains(&id) && variants.iter().flatten().all(|t| copy.is_copy(t)) {
                    copy.enums.insert(id);
                }
            }
            if copy.structs.len() + copy.enums.len() == before {
                return copy;
            }
        }
    }

    /// How many bytes a value of `ty` takes on the target these layouts were
    /// built for.
    pub fn size_of(&self, ty: &MirType) -> u32 {
        rask_layout::size_of(ty, self.target)
    }

    /// Is a value of `ty` Copy?
    pub fn is_copy(&self, ty: &MirType) -> bool {
        if rask_layout::size_of(ty, self.target) > 16 {
            return false;
        }
        match ty {
            MirType::Container(_) | MirType::Heap(_) | MirType::FuncPtr(_) | MirType::InterfaceObject { .. } => false,
            MirType::Struct(id) => self.structs.contains(&id.id),
            MirType::Enum(id) => self.enums.contains(&id.id),
            MirType::Tuple(parts) | MirType::Union(parts) => parts.iter().all(|p| self.is_copy(p)),
            MirType::Array { elem, .. } | MirType::Option(elem) => self.is_copy(elem),
            MirType::Result { ok, err } => self.is_copy(ok) && self.is_copy(err),
            _ => true,
        }
    }
}

/// Every place a value of type `ty` holds an edge, offsets from the value's
/// start.
///
/// The one answer to "which slots does the rack need to hear about": `insert`
/// records a node's from it, `delete` drops them, `snapshot` re-points them,
/// and a struct or tuple landing in ordinary storage registers them. A link
/// one level down (`node.cursor.at`) is as much an edge as one on the node
/// itself, and an answer that stopped at the top level left it unrecorded, so
/// `delete` never nulled it (#1795).
///
/// An enum payload isn't descended into: which variant is live decides
/// whether its slots hold links at all, and the descriptor has no way to say
/// so.
pub fn link_slots(ty: &MirType, fields: &FieldTypes) -> Vec<LinkSlot> {
    link_slots_by(ty, &|id| fields.struct_links(id).to_vec(), fields.target)
}

/// `link_slots`, with each struct layout's own slots coming from `struct_links`
/// — for lowering, which builds the table these are read from.
pub fn link_slots_by(
    ty: &MirType,
    struct_links: &dyn Fn(u32) -> Vec<LinkSlot>,
    target: rask_ast::Target,
) -> Vec<LinkSlot> {
    let mut out = Vec::new();
    collect_link_slots(ty, 0, struct_links, target, &mut out);
    out
}

fn collect_link_slots(
    ty: &MirType,
    base: u32,
    struct_links: &dyn Fn(u32) -> Vec<LinkSlot>,
    target: rask_ast::Target,
    out: &mut Vec<LinkSlot>,
) {
    match ty {
        MirType::Link(_) => out.push(LinkSlot {
            kind: rask_mono::runtime_abi::RASK_RACK_FIELD_LINK as i32,
            offset: base,
        }),
        MirType::Option(inner) if inner.is_niche_payload() => {
            collect_link_slots(inner, base, struct_links, target, out)
        }
        MirType::Struct(id) => out.extend(
            struct_links(id.id).into_iter().map(|s| LinkSlot { kind: s.kind, offset: base + s.offset }),
        ),
        MirType::Tuple(parts) => {
            for (part, at) in parts.iter().zip(rask_layout::tuple_offsets(parts, target)) {
                collect_link_slots(part, base + at, struct_links, target, out);
            }
        }
        MirType::Array { elem, len } => {
            for i in 0..*len {
                collect_link_slots(elem, base + i * rask_layout::size_of(elem.as_ref(), target), struct_links, target, out);
            }
        }
        _ => {}
    }
}

impl FieldTypes {
    /// The fields of struct layout `id`, in layout order. Empty when the table
    /// doesn't cover it.
    pub fn struct_fields(&self, id: u32) -> &[MirType] {
        self.structs.get(id as usize).map_or(&[], Vec::as_slice)
    }

    /// Where struct layout `id` holds edges (`link_slots`). Empty when the
    /// table doesn't cover it.
    pub fn struct_links(&self, id: u32) -> &[LinkSlot] {
        self.struct_links.get(id as usize).map_or(&[], Vec::as_slice)
    }

    /// The fields of variant `variant` of enum layout `id`, in layout order.
    pub fn variant_fields(&self, id: u32, variant: usize) -> &[MirType] {
        self.variants
            .get(id as usize)
            .and_then(|v| v.get(variant))
            .map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_layout::{align_of, payload_offset_of, size_of, tuple_offsets};

    fn arm() -> rask_ast::Target {
        rask_ast::Target::for_arch("arm")
    }

    /// A pointer-sized MIR type is the target's word, and so is everything
    /// built from one: a tuple's offsets and a `T?`'s payload move with it.
    #[test]
    fn sizes_follow_the_target_word() {
        let x86 = rask_ast::Target::LP64;
        assert_eq!((size_of(&MirType::Ptr, x86), size_of(&MirType::Ptr, arm())), (8, 4));
        let parts = [MirType::U8, MirType::Ptr, MirType::U8];
        assert_eq!(tuple_offsets(&parts, x86), vec![0, 8, 16]);
        assert_eq!(tuple_offsets(&parts, arm()), vec![0, 4, 8]);
        let tuple = MirType::Tuple(parts.to_vec());
        assert_eq!((size_of(&tuple, x86), size_of(&tuple, arm())), (24, 12));
        let opt = MirType::Option(Box::new(MirType::Ptr));
        assert_eq!((size_of(&opt, x86), payload_offset_of(&opt, x86)), (16, 8));
        assert_eq!((size_of(&opt, arm()), payload_offset_of(&opt, arm())), (8, 4));
        assert_eq!((align_of(&MirType::InterfaceObject { interface_name: "I".into() }, arm())), 4);
    }

    /// A `T?` over a 16-byte, 16-aligned payload is 32 bytes. MIR used to say
    /// 24 and monomorphization 32.
    #[test]
    fn a_wide_option_is_the_same_size_everywhere() {
        let x86 = rask_ast::Target::LP64;
        assert_eq!(size_of(&MirType::Option(Box::new(MirType::I128)), x86), 32);
    }

    /// A niche option is its payload's word, with no tag.
    #[test]
    fn a_niche_option_is_one_word() {
        let opt = MirType::Option(Box::new(MirType::RawPtr));
        assert_eq!(size_of(&opt, arm()), 4);
        assert_eq!(payload_offset_of(&opt, arm()), 0);
    }

    /// A closure environment lays each capture out at its own alignment, a
    /// by-reference one as the target's word.
    #[test]
    fn captures_follow_the_target_word() {
        use crate::LocalId;
        let items = || {
            vec![
                (LocalId(0), MirType::U8, false, true),
                (LocalId(1), MirType::I64, true, false),
                (LocalId(2), MirType::I32, false, true),
            ]
        };
        let offsets = |t| crate::lay_out_captures(items(), t).iter().map(|c| c.offset).collect::<Vec<_>>();
        assert_eq!(offsets(rask_ast::Target::LP64), vec![0, 8, 16]);
        assert_eq!(offsets(arm()), vec![0, 4, 8]);
    }

    /// `usize` and `isize` are the target's pointer width.
    #[test]
    fn usize_is_pointer_sized() {
        assert_eq!(MirType::usize_ty(arm()), MirType::U32);
        assert_eq!(MirType::isize_ty(rask_ast::Target::LP64), MirType::I64);
    }
}
