// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Turning a container's element type into what one element owns and where.
//!
//! Lowering says *what* the elements are (`rask_mir::elem_strs`); only codegen
//! has the layouts to say *where* the owned things sit. Two places need the
//! answer — the pass that registers the lists as read-only data, and the call
//! adapter that points a constructor at one — so the walk lives here rather
//! than once in each.
//!
//! Each entry is one `int32`: the byte offset in the low 28 bits, and what
//! lives there in the top 4. A `Vec<Order>` whose `Order` holds a `Vec<Item>`
//! is the reason the kind exists — freeing the outer vector has to free each
//! element's inner one, and the old list of bare offsets could only say
//! "string". A plain offset still reads as a string at that offset, so every
//! hand-written list in the runtime keeps its meaning.
//!
//! An enum needs a fourth kind, because where its string sits depends on which
//! variant it is. `RASK_OWNED_TAG_IF` is a guard rather than a thing to free:
//! it says the entries after it apply only when the tag at some offset holds a
//! particular value, and one enum contributes a guard per variant that owns
//! anything. The header describes the packing; `owned_walk` in `vec.c` reads it.

use rask_mir::{ContainerKind, MirType};
use rask_mono::{EnumLayout, FieldLayout, StructLayout};
use std::collections::HashMap;

use rask_types::{Type as RaskType, TypeId};

/// How deep to look for a string inside a type before giving up. A recursive
/// type reaches MIR through a pointer, which this walk doesn't follow, so the
/// bound is only there so a pathological nesting can't turn a compile into a
/// hang.
const MAX_DEPTH: u32 = 8;

/// What kind of owned thing an entry points at. Mirrors `RASK_OWNED_*` in
/// `rask_runtime.h`, which is the only other place that reads these.
const KIND_SHIFT: u32 = 28;
const KIND_STRING: i32 = 0;
const KIND_VEC: i32 = 1;
const KIND_MAP: i32 = 2;
const KIND_TAG_IF: i32 = 3;
/// The element is a pointer to a closure block, which describes itself: its
/// size and its environment-drop glue are the header words before it (#1149).
const KIND_CLOSURE: i32 = 4;
/// The element is a `[data, vtable]` fat pointer; the block `data` names is the
/// container's to free, and its size is the vtable's first word (#1149).
const KIND_TRAITBOX: i32 = 5;
/// A `Heap<T>` at this offset: a pointer to a block the aggregate owns, with
/// the entries after it describing what is inside.
const KIND_HEAP: i32 = 6;
/// The value at this offset is described by the list being walked, from its
/// start — how a type that reaches itself is described at all (#1202).
const KIND_SELF: i32 = 7;

/// One interface-box entry at offset zero: what to hand `rask_owned_release` when
/// the slot *is* the fat pointer, which is the shape of an interface-object field.
pub const TRAITBOX_AT_ZERO: i32 = KIND_TRAITBOX << KIND_SHIFT;

/// A `Heap<T>` slot: the pointer at `offset`, and `count` entries after this one
/// describing the block.
///
/// `None` when either won't fit — an offset over 65535 or more than 4095 entries
/// in the block's description. The caller then describes nothing, which leaks
/// rather than freeing the wrong bytes.
fn heap_entry(offset: i32, count: usize) -> Option<i32> {
    if !(0..=0xFFFF).contains(&offset) || count > 0xFFF {
        return None;
    }
    Some(entry(offset | ((count as i32) << 16), KIND_HEAP))
}

fn entry(offset: i32, kind: i32) -> i32 {
    offset | (kind << KIND_SHIFT)
}

/// A guard: the next `count` entries apply only when the tag at `tag_offset`
/// holds `tag_value`.
///
/// `None` when any field won't fit its share of the 28 bits — an offset over
/// 4095, more than 255 variants, more than 63 entries in one arm, or a tag
/// wider than eight bytes. The caller then emits nothing for that element,
/// which leaks rather than describing it wrongly.
fn tag_guard(tag_offset: i32, tag_value: u64, count: usize, tag_size: u32) -> Option<i32> {
    let width = match tag_size {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        _ => return None,
    };
    if !(0..=0xFFF).contains(&tag_offset) || tag_value > 0xFF || count > 0x3F {
        return None;
    }
    let packed = tag_offset | ((tag_value as i32) << 12) | ((count as i32) << 20) | (width << 26);
    Some(entry(packed, KIND_TAG_IF))
}

/// What one element of type `ty` owns, and where, or `None` when it owns
/// nothing.
pub fn owned_offsets(
    ty: &MirType,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
) -> Option<Vec<i32>> {
    let mut out = Vec::new();
    describe(ty, 0, layouts, enums, names, 0, &mut out)?;
    (!out.is_empty()).then_some(out)
}

/// What a value of type `ty` at offset `base` owns, appended to `out`.
fn describe(
    ty: &MirType,
    base: i32,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
    depth: u32,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    match ty {
        MirType::String => out.push(entry(base, KIND_STRING)),
        // The element is the container. Its own list travels with it, so this
        // level says "a Vec lives here" and stops there. A rack is an arena
        // whose nodes outlive any one element (mem.racks), so it's nobody's
        // here.
        MirType::Container(ContainerKind::Vec) => out.push(entry(base, KIND_VEC)),
        MirType::Container(ContainerKind::Map) => out.push(entry(base, KIND_MAP)),
        // The element *is* the pointer, so there is nothing to flatten: one
        // entry saying what kind of block it names.
        MirType::FuncPtr(_) => out.push(entry(base, KIND_CLOSURE)),
        MirType::InterfaceObject { .. } => out.push(entry(base, KIND_TRAITBOX)),
        MirType::Struct(id) => {
            let fields = layouts.get(id.id as usize)?.fields.clone();
            flatten(&fields, base, layouts, enums, names, depth + 1, None, out)?;
        }
        MirType::Enum(id) => {
            let layout = enums.get(id.id as usize)?;
            enum_arms(layout, base, layouts, enums, names, depth + 1, None, out)?;
        }
        // A niche option is the payload's own word with `none` reserved: no
        // tag, and nothing it points at is the container's.
        MirType::Option(inner) if !inner.is_niche_payload() => {
            let none = MirType::Void;
            wrapper_arms(Wrapper::Option, inner, &none, base, layouts, enums, names, depth, out)?;
        }
        MirType::Result { ok, err } => {
            wrapper_arms(Wrapper::Result, ok, err, base, layouts, enums, names, depth, out)?;
        }
        // Parts at their natural offsets, the packing tuple lowering uses.
        MirType::Tuple(parts) => {
            let mut at = 0u32;
            for part in parts {
                let align = part.align().max(1);
                at = (at + align - 1) & !(align - 1);
                describe(part, base + at as i32, layouts, enums, names, depth + 1, out)?;
                at += part.size();
            }
        }
        MirType::Array { elem, len } => {
            let stride = elem.size() as i32;
            for i in 0..*len as i32 {
                describe(elem, base + i * stride, layouts, enums, names, depth + 1, out)?;
            }
        }
        _ => {}
    }
    Some(())
}

#[derive(Clone, Copy)]
enum Wrapper {
    Result,
    Option,
}

/// A `T or E` or tagged `T?`: one guard per side that owns anything, on the
/// wrapper's own tag, with the side described at the payload offset.
fn wrapper_arms(
    kind: Wrapper,
    ok: &MirType,
    err: &MirType,
    base: i32,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
    depth: u32,
    out: &mut Vec<i32>,
) -> Option<()> {
    let (tag_offset, payload_offset) = match kind {
        Wrapper::Result => (rask_mono::abi::RESULT_TAG_OFFSET, rask_mono::abi::RESULT_PAYLOAD_OFFSET),
        Wrapper::Option => (rask_mono::abi::OPTION_TAG_OFFSET, rask_mono::abi::OPTION_PAYLOAD_OFFSET),
    };
    // Tag 0 is the ok side (`Some` for an option), tag 1 the error side.
    for (tag_value, side) in [(0u64, ok), (1u64, err)] {
        let mut arm = Vec::new();
        describe(side, base + payload_offset as i32, layouts, enums, names, depth + 1, &mut arm)?;
        if arm.is_empty() {
            continue;
        }
        out.push(tag_guard(base + tag_offset as i32, tag_value, arm.len(), 8)?);
        out.extend(arm);
    }
    Some(())
}

/// One guard plus its entries per variant that owns anything.
///
/// `None` rather than a partial list: an arm that can't be described has to
/// take the whole element with it, because a guard whose count is wrong makes
/// the walk read the following entries as belonging to the wrong variant.
fn enum_arms(
    layout: &EnumLayout,
    base: i32,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
    depth: u32,
    self_name: Option<&str>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    let (tag_size, _) = rask_mono::type_size_align(&layout.tag_ty, &Default::default());
    let tag_offset = base + layout.tag_offset as i32;
    for variant in &layout.variants {
        let mut arm = Vec::new();
        let fields: Vec<FieldLayout> = variant
            .fields
            .iter()
            .map(|f| FieldLayout { offset: variant.payload_offset + f.offset, ..f.clone() })
            .collect();
        flatten(&fields, base, layouts, enums, names, depth + 1, self_name, &mut arm)?;
        if arm.is_empty() {
            continue;
        }
        out.push(tag_guard(tag_offset, variant.tag, arm.len(), tag_size)?);
        out.extend(arm);
    }
    Some(())
}

fn flatten(
    fields: &[FieldLayout],
    base: i32,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
    depth: u32,
    self_name: Option<&str>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return Some(());
    }
    for f in fields {
        let at = base + f.offset as i32;
        if let Some(payload) = heap_payload(&f.ty) {
            let payload = layout_name(payload, layouts, enums);
            // Only where a self-reference has a name to match. The
            // container-element path passes `None`, because a `retain` would
            // have to copy the block and a block carries no size to copy — so
            // an element's `Heap` stays nobody's, as a boxed value's contents
            // are.
            let Some(sn) = self_name else { continue };
            let mut body = Vec::new();
            if payload == sn {
                body.push(entry(0, KIND_SELF));
            } else {
                describe_named(&payload, 0, layouts, enums, names, depth + 1, Some(&payload), &mut body)?;
            }
            out.push(heap_entry(at, body.len())?);
            out.extend(body);
            continue;
        }
        if let Some(kind) = container_kind(&f.ty, names) {
            // The nested container carries its own element list, set when it was
            // built, so freeing it walks its elements without this one knowing
            // what they are.
            out.push(entry(at, kind));
            continue;
        }
        match &f.ty {
            RaskType::String => out.push(entry(at, KIND_STRING)),
            RaskType::UnresolvedNamed(_) | RaskType::UnresolvedGeneric { .. } => {
                // A nested struct flattens into the same list. A nested *enum*
                // contributes its own guards, at this field's offset.
                let name = &layout_name(&f.ty, layouts, enums);
                if let Some(l) = layouts.iter().find(|l| &l.name == name) {
                    let nested = l.fields.clone();
                    flatten(&nested, at, layouts, enums, names, depth + 1, self_name, out)?;
                } else if let Some(l) = enums.iter().find(|l| &l.name == name) {
                    enum_arms(l, at, layouts, enums, names, depth + 1, self_name, out)?;
                }
            }
            _ => {}
        }
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
pub fn heap_field_descriptor(
    ty: &RaskType,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
) -> Option<Vec<i32>> {
    let payload = layout_name(heap_payload(ty)?, layouts, enums);
    let mut out = Vec::new();
    describe_named(&payload, 0, layouts, enums, names, 0, Some(&payload), &mut out)?;
    Some(out)
}

/// A user generic field type as the layout it's laid out by: `Tasks<T>` in
/// the shared `Group` layout is the `Tasks` layout, `Tasks<string>` in an
/// instance is `Tasks$string`. `None` for anything else, including a generic
/// no layout answers to (the stdlib's opaque containers).
pub fn generic_as_layout(
    ty: &RaskType,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
) -> Option<RaskType> {
    if !matches!(ty, RaskType::UnresolvedGeneric { .. }) {
        return None;
    }
    let name = layout_name(ty, layouts, enums);
    let known = layouts.iter().any(|l| l.name == name) || enums.iter().any(|l| l.name == name);
    known.then(|| RaskType::UnresolvedNamed(name))
}

/// The layout name a written type goes by — see `rask_mono::layout_name_for`.
fn layout_name(ty: &RaskType, layouts: &[StructLayout], enums: &[EnumLayout]) -> String {
    rask_mono::layout_name_for(ty, |n| {
        layouts.iter().any(|l| l.name == n) || enums.iter().any(|l| l.name == n)
    })
}

/// What a value of the named type owns, relative to `base`. A name that isn't a
/// layout owns nothing describable — a scalar, or a type this pass can't see.
fn describe_named(
    name: &str,
    base: i32,
    layouts: &[StructLayout],
    enums: &[EnumLayout],
    names: &HashMap<TypeId, String>,
    depth: u32,
    self_name: Option<&str>,
    out: &mut Vec<i32>,
) -> Option<()> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(l) = layouts.iter().find(|l| l.name == name) {
        let fields = l.fields.clone();
        return flatten(&fields, base, layouts, enums, names, depth, self_name, out);
    }
    if let Some(l) = enums.iter().find(|l| l.name == name) {
        return enum_arms(l, base, layouts, enums, names, depth, self_name, out);
    }
    Some(())
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

/// Is this field a container the element owns, and which one.
///
/// Read off the type's head, as `container_free_for` does: an optional or a
/// result is a wrapper around the handle rather than the handle, and a `Rack`
/// is an arena whose nodes outlive any one node (mem.racks).
fn container_kind(ty: &RaskType, names: &HashMap<TypeId, String>) -> Option<i32> {
    // A closure a field holds is the aggregate's — storing one moves it in, and
    // the frame stops dropping it the moment it does, the same rule
    // `container_free_for` states for a frame's own walk. Without it the
    // *element* walk had nothing to say about a `Vec<Handler>` whose `Handler`
    // holds a `func(i64) -> i64`: one 32-byte block per element, left to
    // nobody (#1228).
    if matches!(ty, RaskType::Fn { .. }) {
        return Some(KIND_CLOSURE);
    }
    match ty.head_name(names)? {
        "Vec" => Some(KIND_VEC),
        // A `Set<T>` is a struct holding a `Map<T, bool>`, so it reaches this
        // through the nested-struct arm above.
        "Map" => Some(KIND_MAP),
        _ => None,
    }
}
