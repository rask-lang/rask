// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Turning a container's element tag into what one element owns and where.
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

use rask_mir::elem_strs::{ELEM_MAP, ELEM_STRING, ELEM_STRUCT_BASE, ELEM_VEC};
use rask_mono::{FieldLayout, StructLayout};
use rask_types::Type as RaskType;

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

fn entry(offset: i32, kind: i32) -> i32 {
    offset | (kind << KIND_SHIFT)
}

/// What one element of a container tagged `tag` owns, and where, or `None` when
/// it owns nothing.
pub fn string_offsets_for_tag(tag: i64, layouts: &[StructLayout]) -> Option<Vec<i32>> {
    match tag {
        ELEM_STRING => Some(vec![0]),
        // The element is the container. Its own list travels with it, so this
        // level says "a Vec lives at offset 0" and stops there.
        ELEM_VEC => Some(vec![entry(0, KIND_VEC)]),
        ELEM_MAP => Some(vec![entry(0, KIND_MAP)]),
        n if n >= ELEM_STRUCT_BASE => {
            let idx = usize::try_from(n - ELEM_STRUCT_BASE).ok()?;
            let layout = layouts.get(idx)?;
            let mut out = Vec::new();
            flatten(&layout.fields, 0, layouts, 0, &mut out);
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

fn flatten(
    fields: &[FieldLayout],
    base: i32,
    layouts: &[StructLayout],
    depth: u32,
    out: &mut Vec<i32>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for f in fields {
        let at = base + f.offset as i32;
        if let Some(kind) = container_kind(&f.ty) {
            // The nested container carries its own element list, set when it was
            // built, so freeing it walks its elements without this one knowing
            // what they are.
            out.push(entry(at, kind));
            continue;
        }
        match &f.ty {
            RaskType::String => out.push(entry(at, KIND_STRING)),
            // A nested struct flattens into the same list. A nested *enum*
            // doesn't — where its string is depends on the tag.
            RaskType::UnresolvedNamed(name) => {
                if let Some(l) = layouts.iter().find(|l| &l.name == name) {
                    let nested = l.fields.clone();
                    flatten(&nested, at, layouts, depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

/// Is this field a container the element owns, and which one.
///
/// The same head-of-the-rendered-type test `container_free_for` uses, and for
/// the same reasons: `Vec<i64>?` is a wrapper around the handle rather than the
/// handle, and a `Pool` or a `Rack` is an arena whose contents outlive any one
/// element (mem.pools, mem.racks).
fn container_kind(ty: &RaskType) -> Option<i32> {
    let rendered = format!("{}", ty);
    if rendered.ends_with('?') || rendered.contains(" or ") {
        return None;
    }
    match rendered.split('<').next().unwrap_or(&rendered).trim() {
        "Vec" => Some(KIND_VEC),
        // A `Set<T>` is a struct holding a `Map<T, bool>`, so it reaches this
        // through the nested-struct arm above.
        "Map" => Some(KIND_MAP),
        _ => None,
    }
}
