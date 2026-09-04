// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Every `Struct`/`Enum`/`Link` id in the program names a layout that exists
//! and agrees with what the type carries.
//!
//! A `StructLayoutId` is an index into one flat table plus the size and
//! alignment the type was minted with. Nothing checked that the two agree, so
//! an id built from a layout that was never laid out — or built against one
//! table and read against another — indexed whatever happened to sit at that
//! position. Codegen then walked *that* type's fields.
//!
//! `Metadata_compare` is the case that surfaced it (#1062): three scalar fields
//! on the declaration, and a generated compare that read a `Heap<Node>` off the
//! user's first enum. The error it produced named a type the program never
//! mentions, which is why it read as a compiler bug — it was one. The lucky
//! part is that the foreign field happened to be unorderable, so it refused. A
//! wrong layout whose fields are all orderable compiles and compares the wrong
//! bytes.
//!
//! Cheap enough to run on every build: one walk of each function's locals.

use std::collections::HashSet;

use crate::function::MirFunction;
use crate::types::{EnumLayoutId, MirType, StructLayoutId};
use rask_mono::{EnumLayout, StructLayout};

/// One id that doesn't match the layout it points at.
pub struct LayoutIdMismatch {
    /// The function the type was found in.
    pub function: String,
    /// What went wrong, in words.
    pub detail: String,
}

/// Check every layout id the program's functions mention. Empty when they all
/// agree, which is the normal case.
pub fn check_layout_ids(
    functions: &[MirFunction],
    struct_layouts: &[StructLayout],
    enum_layouts: &[EnumLayout],
) -> Vec<LayoutIdMismatch> {
    let mut problems = Vec::new();
    for f in functions {
        let mut seen = HashSet::new();
        let mut check = |ty: &MirType, problems: &mut Vec<LayoutIdMismatch>| {
            walk(ty, &mut |t| {
                let detail = match t {
                    MirType::Struct(id) | MirType::Link(id) => {
                        struct_detail(id, struct_layouts)
                    }
                    MirType::Enum(id) => enum_detail(id, enum_layouts),
                    _ => None,
                };
                if let Some(detail) = detail {
                    if seen.insert(detail.clone()) {
                        problems.push(LayoutIdMismatch {
                            function: f.name.clone(),
                            detail,
                        });
                    }
                }
            });
        };
        check(&f.ret_ty, &mut problems);
        for local in f.locals.iter().chain(f.params.iter()) {
            check(&local.ty, &mut problems);
        }
    }
    problems
}

fn struct_detail(id: &StructLayoutId, layouts: &[StructLayout]) -> Option<String> {
    match layouts.get(id.id as usize) {
        None => Some(format!(
            "struct layout #{} doesn't exist — the table holds {}",
            id.id,
            layouts.len()
        )),
        // The size and alignment travel with the id, so a mismatch means the
        // id and the table were built from different pictures of the program.
        Some(l) if l.size != id.byte_size || l.align != id.align => Some(format!(
            "struct layout #{} is `{}` ({} bytes, align {}), but the type says {} bytes, align {}",
            id.id, l.name, l.size, l.align, id.byte_size, id.align
        )),
        Some(_) => None,
    }
}

fn enum_detail(id: &EnumLayoutId, layouts: &[EnumLayout]) -> Option<String> {
    match layouts.get(id.id as usize) {
        None => Some(format!(
            "enum layout #{} doesn't exist — the table holds {}",
            id.id,
            layouts.len()
        )),
        Some(l) if l.size != id.byte_size || l.align != id.align => Some(format!(
            "enum layout #{} is `{}` ({} bytes, align {}), but the type says {} bytes, align {}",
            id.id, l.name, l.size, l.align, id.byte_size, id.align
        )),
        Some(_) => None,
    }
}

/// Every type inside `ty`, itself included.
fn walk(ty: &MirType, f: &mut impl FnMut(&MirType)) {
    f(ty);
    match ty {
        MirType::Array { elem, .. }
        | MirType::Slice(elem)
        | MirType::Option(elem)
        | MirType::SimdVector { elem, .. } => walk(elem, f),
        MirType::Result { ok, err } => {
            walk(ok, f);
            walk(err, f);
        }
        MirType::Tuple(parts) | MirType::Union(parts) => {
            for p in parts {
                walk(p, f);
            }
        }
        _ => {}
    }
}
