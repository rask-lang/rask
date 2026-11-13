// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Turning a container's element type into what one element owns and where.
//!
//! Lowering says *what* the elements are (`rask_mir::elem_strs`); only codegen
//! has the layouts to say *where* the owned things sit. Two places need the
//! answer — the pass that registers the lists as read-only data, and the call
//! adapter that points a constructor at one — so the walk lives here rather
//! than once in each.
//!
//! An entry starts with one `int32`: the byte offset in the low 28 bits, and
//! what lives there in the top 4. A guard and a `Heap` slot carry plain words
//! after that one. A `Vec<Order>` whose `Order` holds a `Vec<Item>`
//! is the reason the kind exists — freeing the outer vector has to free each
//! element's inner one, and the old list of bare offsets could only say
//! "string". A plain offset still reads as a string at that offset, so every
//! hand-written list in the runtime keeps its meaning.
//!
//! An enum needs a fourth kind, because where its string sits depends on which
//! variant it is. `RASK_OWNED_TAG_IF` is a guard rather than a thing to free:
//! it says the entries after it apply only when the tag at some offset holds a
//! particular value, and one enum contributes a guard per variant that owns
//! anything. The header describes the words; `owned_walk` in `vec.c` reads it.

use rask_mono::runtime_abi;
use rask_mir::{ContainerKind, MirType};
use rask_layout::{EnumLayout, FieldLayout, StructLayout};

use rask_types::Type as RaskType;

/// How deep to look for a string inside a type before giving up. A recursive
/// type reaches MIR through a pointer, which this walk doesn't follow, so the
/// bound is only there so a pathological nesting can't turn a compile into a
/// hang.
const MAX_DEPTH: u32 = 8;

// What kind of owned thing an entry points at: `rask_mono::runtime_abi`'s
// `RASK_OWNED_*`, which the runtime reads from the generated rask_abi.h.
const KIND_SHIFT: u32 = runtime_abi::RASK_OWNED_KIND_SHIFT as u32;
const KIND_STRING: i32 = runtime_abi::RASK_OWNED_STRING as i32;
const KIND_VEC: i32 = runtime_abi::RASK_OWNED_VEC as i32;
const KIND_MAP: i32 = runtime_abi::RASK_OWNED_MAP as i32;
const KIND_TAG_IF: i32 = runtime_abi::RASK_OWNED_TAG_IF as i32;
/// The element is a pointer to a closure block, which describes itself: its
/// size and its environment-drop glue are the header words before it (#1149).
const KIND_CLOSURE: i32 = runtime_abi::RASK_OWNED_CLOSURE as i32;
/// The element is a `[data, vtable]` fat pointer; the block `data` names is the
/// container's to free, and its size is the vtable's first word (#1149).
const KIND_TRAITBOX: i32 = runtime_abi::RASK_OWNED_TRAITBOX as i32;
/// A `Heap<T>` at this offset: a pointer to a block the aggregate owns, with
/// the entries after it describing what is inside.
const KIND_HEAP: i32 = runtime_abi::RASK_OWNED_HEAP as i32;
/// The value at this offset is described by the list being walked, from its
/// start — how a type that reaches itself is described at all (#1202).
const KIND_SELF: i32 = runtime_abi::RASK_OWNED_SELF as i32;
/// A link inside the element, for the rack rather than for the free.
const KIND_LINK: i32 = runtime_abi::RASK_OWNED_LINK as i32;
/// A handle that frees itself through its type's `@handle(release: …)`. The
/// offset bits carry the release's index above the byte offset.
const KIND_HANDLE: i32 = runtime_abi::RASK_OWNED_HANDLE as i32;

/// One interface-box entry at offset zero: what to hand `rask_owned_release` when
/// the slot *is* the fat pointer, which is the shape of an interface-object field.
pub const TRAITBOX_AT_ZERO: i32 = KIND_TRAITBOX << KIND_SHIFT;

/// A `Heap<T>` slot: the pointer at `offset`, then a word counting the entries
/// after it that describe the block.
fn heap_entry(offset: i32, count: usize) -> Option<[i32; 2]> {
    Some([entry(fit_offset(offset)?, KIND_HEAP), fit_word(count as u64)?])
}

fn entry(offset: i32, kind: i32) -> i32 {
    offset | (kind << KIND_SHIFT)
}

/// A guard: the next `count` entries apply only when the tag at `tag_offset`,
/// `tag_size` bytes wide, holds `tag_value`. Four words: the kind with the
/// offset, then the width, the value and the count as plain numbers.
///
/// `None` only for a tag of a width the walk can't read. The caller then emits
/// nothing for that element, which leaks rather than describing it wrongly.
fn tag_guard(tag_offset: i32, tag_value: u64, count: usize, tag_size: u32) -> Option<[i32; 4]> {
    if !matches!(tag_size, 1 | 2 | 4 | 8) {
        return None;
    }
    Some([
        entry(fit_offset(tag_offset)?, KIND_TAG_IF),
        tag_size as i32,
        fit_word(tag_value)?,
        fit_word(count as u64)?,
    ])
}

fn fit_offset(offset: i32) -> Option<i32> {
    (0..=runtime_abi::RASK_OWNED_OFFSET_MASK as i32).contains(&offset).then_some(offset)
}

/// A word after an entry's first. Kept below 2^31, so its kind bits never read
/// as a link and a flat scan for links can't mistake one.
fn fit_word(v: u64) -> Option<i32> {
    i32::try_from(v).ok()
}

/// The program's layouts as the walks below need them, with each layout's
/// fields as MIR types (`rask_mir::FieldTypes`).
#[derive(Clone, Copy)]
pub struct Shapes<'a> {
    pub layouts: &'a [StructLayout],
    pub enums: &'a [EnumLayout],
    pub fields: &'a rask_mir::FieldTypes,
}

/// A struct or enum layout, by its position in its list: what a `Heap<T>`
/// payload is laid out by, and what a self-reference is matched against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Struct(usize),
    Enum(usize),
}

/// What one element of type `ty` owns, and where, or `None` when it owns
/// nothing.
pub fn owned_offsets(ty: &MirType, shapes: Shapes<'_>) -> Option<Vec<i32>> {
    let mut out = Vec::new();
    describe(ty, 0, shapes, 0, None, &mut out)?;
    (!out.is_empty()).then_some(out)
}

/// What a value of type `ty` at offset `base` owns, appended to `out`.
///
/// `self_layout` is the payload a `Heap<T>` description is being built for,
/// so a field that holds another one restarts the list instead of recursing.
fn describe(
    ty: &MirType,
    base: i32,
    shapes: Shapes<'_>,
    depth: u32,
    self_layout: Option<Layout>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    match ty {
        MirType::Buffer => out.push(entry(base, KIND_STRING)),
        // A link inside an element is the rack's business: delete nulls it in
        // place (#1804). An element that *is* a link is listed nowhere — the
        // rack removes those entries instead, and frees walk nothing for them.
        MirType::Link(_) if depth > 0 => out.push(entry(base, KIND_LINK)),
        MirType::Option(inner) if inner.is_niche_payload() && depth > 0 => {
            out.push(entry(base, KIND_LINK))
        }
        // The element is the container. Its own list travels with it, so this
        // level says "a Vec lives here" and stops there. A rack is an arena
        // whose nodes outlive any one element (mem.racks), so it's nobody's
        // here.
        MirType::Container(ContainerKind::Vec) => out.push(entry(base, KIND_VEC)),
        MirType::Container(ContainerKind::Map) => out.push(entry(base, KIND_MAP)),
        MirType::Container(ContainerKind::Handle(release)) => {
            let index = runtime_abi::handle_release_index(release)? as i32;
            let mask = runtime_abi::RASK_OWNED_HANDLE_OFFSET_MASK as i32;
            let shift = runtime_abi::RASK_OWNED_HANDLE_INDEX_SHIFT as u32;
            if base & !mask != 0 || (index << shift) & !(runtime_abi::RASK_OWNED_OFFSET_MASK as i32) != 0 {
                return None;
            }
            out.push(entry(base | (index << shift), KIND_HANDLE));
        }
        // The element *is* the pointer, so there is nothing to flatten: one
        // entry saying what kind of block it names.
        MirType::FuncPtr(_) => out.push(entry(base, KIND_CLOSURE)),
        MirType::InterfaceObject { .. } => out.push(entry(base, KIND_TRAITBOX)),
        MirType::Struct(id) => {
            let layout = shapes.layouts.get(id.id as usize)?;
            let tys = shapes.fields.struct_fields(id.id);
            flatten(&layout.fields, tys, base, shapes, depth + 1, self_layout, out)?;
        }
        MirType::Enum(id) => enum_arms(id.id, base, shapes, depth + 1, self_layout, out)?,
        // A niche option is the payload's own word with `none` reserved: no
        // tag, and nothing it points at is the container's.
        MirType::Option(inner) if !inner.is_niche_payload() => {
            let none = MirType::Void;
            wrapper_arms(ty, inner, &none, base, shapes, depth, self_layout, out)?;
        }
        MirType::Result { ok, err } => {
            wrapper_arms(ty, ok, err, base, shapes, depth, self_layout, out)?;
        }
        // Parts at their natural offsets, the packing tuple lowering uses.
        MirType::Tuple(parts) => {
            for (part, at) in parts.iter().zip(rask_mir::tuple_offsets(parts)) {
                describe(part, base + at as i32, shapes, depth + 1, self_layout, out)?;
            }
        }
        MirType::Array { elem, len } => {
            let stride = elem.size() as i32;
            for i in 0..*len as i32 {
                describe(elem, base + i * stride, shapes, depth + 1, self_layout, out)?;
            }
        }
        _ => {}
    }
    Some(())
}

/// A `T or E` or tagged `T?`: one guard per side that owns anything, on the
/// wrapper's own tag, with the side described at the payload offset.
#[allow(clippy::too_many_arguments)]
fn wrapper_arms(
    wrapper: &MirType,
    ok: &MirType,
    err: &MirType,
    base: i32,
    shapes: Shapes<'_>,
    depth: u32,
    self_layout: Option<Layout>,
    out: &mut Vec<i32>,
) -> Option<()> {
    use rask_mono::runtime_abi::{RASK_TAG_ABSENT, RASK_TAG_PRESENT};
    let payload_offset = wrapper.payload_offset();
    for (tag_value, side) in [(RASK_TAG_PRESENT as u64, ok), (RASK_TAG_ABSENT as u64, err)] {
        let mut arm = Vec::new();
        describe(side, base + payload_offset as i32, shapes, depth + 1, self_layout, &mut arm)?;
        if arm.is_empty() {
            continue;
        }
        let tag_offset = base + rask_types::abi::TAG_OFFSET as i32;
        out.extend(tag_guard(tag_offset, tag_value, arm.len(), rask_types::abi::TAG_BYTES)?);
        out.extend(arm);
    }
    Some(())
}

/// One guard plus its entries per variant of enum layout `id` that owns
/// anything.
///
/// `None` rather than a partial list: an arm that can't be described has to
/// take the whole element with it, because a guard whose count is wrong makes
/// the walk read the following entries as belonging to the wrong variant.
fn enum_arms(
    id: u32,
    base: i32,
    shapes: Shapes<'_>,
    depth: u32,
    self_layout: Option<Layout>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let layout = shapes.enums.get(id as usize)?;
    let tag_size = layout.tag_bytes();
    let tag_offset = base + layout.tag_offset as i32;
    for (i, variant) in layout.variants.iter().enumerate() {
        let mut arm = Vec::new();
        let fields: Vec<FieldLayout> = variant
            .fields
            .iter()
            .map(|f| FieldLayout { offset: variant.payload_offset + f.offset, ..f.clone() })
            .collect();
        let tys = shapes.fields.variant_fields(id, i);
        flatten(&fields, tys, base, shapes, depth + 1, self_layout, &mut arm)?;
        if arm.is_empty() {
            continue;
        }
        out.extend(tag_guard(tag_offset, variant.tag, arm.len(), tag_size)?);
        out.extend(arm);
    }
    Some(())
}

/// The fields of a struct or a variant, each at `base` plus its offset, by
/// its MIR type. A `Heap<T>` field is the one read off its declared type: its
/// payload's layout is what a self-reference is matched by.
fn flatten(
    fields: &[FieldLayout],
    tys: &[MirType],
    base: i32,
    shapes: Shapes<'_>,
    depth: u32,
    self_layout: Option<Layout>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return Some(());
    }
    for (f, ty) in fields.iter().zip(tys) {
        let at = base + f.offset as i32;
        if let Some(payload) = heap_payload(&f.ty) {
            let payload = layout_of(payload, shapes);
            // Only where a self-reference has a layout to match. The
            // container-element path passes `None`, because a `retain` would
            // have to copy the block and a block carries no size to copy — so
            // an element's `Heap` stays nobody's, as a boxed value's contents
            // are.
            let Some(sn) = self_layout else { continue };
            let mut body = Vec::new();
            match payload {
                Some(p) if p == sn => body.push(entry(0, KIND_SELF)),
                Some(p) => describe_layout(p, 0, shapes, depth + 1, Some(p), &mut body)?,
                // A scalar payload: nothing inside the block to describe.
                None => {}
            }
            out.extend(heap_entry(at, body.len())?);
            out.extend(body);
            continue;
        }
        // The nested container carries its own element list, set when it was
        // built, so freeing it walks its elements without this one knowing
        // what they are. A nested struct flattens into the same list, and a
        // nested enum or wrapper contributes its own guards at this offset.
        describe(ty, at, shapes, depth, self_layout, out)?;
    }
    Some(())
}

/// What the `T` inside a `Heap<T>` field owns — the list to hand
/// `rask_heap_field_release` alongside the slot.
///
/// T's own description, with no entry for the slot, and that is deliberate: a
/// `SELF` inside it means "a T lives here", so the list it restarts from has to
/// *be* this one. A list that began with the slot's own `HEAP` entry made the
/// first `SELF` read T's first bytes as a pointer — a `List`'s tag is 1, and
/// 0x1 is not an address.
///
/// Empty for a scalar payload, which is right: there is nothing inside the
/// block, and freeing the block is the caller's job either way.
///
/// The recursive payload is what this is for. `Cons(i64, Heap<List>)` holds a
/// `List` inside the block, and flattening that inline would never finish — so
/// the nested `Heap`'s body is one `SELF` and the runtime starts this list over
/// against the block. The recursion ends on a `Nil`, which matches no guard,
/// rather than on a depth cap that would free the first eight nodes of a list
/// and leak the rest (#1202).
pub fn heap_field_descriptor(ty: &RaskType, shapes: Shapes<'_>) -> Option<Vec<i32>> {
    let mut out = Vec::new();
    // A payload with no layout (a scalar) owns nothing inside the block.
    if let Some(payload) = layout_of(heap_payload(ty)?, shapes) {
        describe_layout(payload, 0, shapes, 0, Some(payload), &mut out)?;
    }
    Some(out)
}

/// The layout `ty` is laid out by, found by the type's identity: an
/// instance's own when one was made, else the declaration's. `None` for a
/// type with no struct or enum layout.
fn layout_of(ty: &RaskType, shapes: Shapes<'_>) -> Option<Layout> {
    rask_layout::struct_index_for(ty, shapes.layouts)
        .map(Layout::Struct)
        .or_else(|| rask_layout::enum_index_for(ty, shapes.enums).map(Layout::Enum))
}

/// What a value laid out by `layout` owns, relative to `base`.
fn describe_layout(
    layout: Layout,
    base: i32,
    shapes: Shapes<'_>,
    depth: u32,
    self_layout: Option<Layout>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    match layout {
        Layout::Struct(id) => {
            let tys = shapes.fields.struct_fields(id as u32);
            flatten(&shapes.layouts.get(id)?.fields, tys, base, shapes, depth, self_layout, out)
        }
        Layout::Enum(id) => enum_arms(id as u32, base, shapes, depth, self_layout, out),
    }
}

/// Is this field a `Heap<T>`? The release walk asks before it looks at
/// anything else, the way it asks about an interface object.
pub fn is_heap_field(ty: &RaskType) -> bool {
    heap_payload(ty).is_some()
}

/// `Heap<Big>` → `Big`. A wrapper around the handle is a different thing, the
/// same way it is for a container.
fn heap_payload(ty: &RaskType) -> Option<&RaskType> {
    match ty {
        RaskType::UnresolvedGeneric { name, args } if name == "Heap" => match args.as_slice() {
            [rask_types::GenericArg::Type(inner)] => Some(inner),
            _ => None,
        },
        _ => None,
    }
}

