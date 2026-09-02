// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Turning a container's element tag into the byte offsets of the strings
//! inside one element.
//!
//! Lowering says *what* the elements are (`rask_mir::elem_strs`); only codegen
//! has the layouts to say *where* the strings sit. Two places need the answer —
//! the pass that registers the offset lists as read-only data, and the call
//! adapter that points a constructor at one — so the walk lives here rather
//! than once in each.

use rask_mir::elem_strs::{ELEM_STRING, ELEM_STRUCT_BASE};
use rask_mono::{FieldLayout, StructLayout};
use rask_types::Type as RaskType;

/// How deep to look for a string inside a type before giving up. A recursive
/// type reaches MIR through a pointer, which this walk doesn't follow, so the
/// bound is only there so a pathological nesting can't turn a compile into a
/// hang.
const MAX_DEPTH: u32 = 8;

/// Where the strings sit inside one element of a container tagged `tag`, or
/// `None` when the elements own none.
pub fn string_offsets_for_tag(tag: i64, layouts: &[StructLayout]) -> Option<Vec<i32>> {
    match tag {
        ELEM_STRING => Some(vec![0]),
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
        match &f.ty {
            RaskType::String => out.push(at),
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
