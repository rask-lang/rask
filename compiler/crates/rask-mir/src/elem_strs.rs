// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a container's elements are, as one number.
//!
//! A container is a byte store. It knows how big an element is and nothing
//! else, so it can't tell a sixteen-byte string from a sixteen-byte struct —
//! and `free` has to know, or the strings inside never come back (#1027).
//!
//! The answer is settled once, where a container is constructed, by the only
//! place that has it: lowering, reading the checker's type. It travels as this
//! tag, codegen turns it into the byte offsets of the strings inside one
//! element, and the runtime keeps that map on the container itself. Nothing
//! downstream re-derives it — not the drop pass, not the caller of a function
//! that hands a container back, not an inlined copy.
//!
//! Encoding, shared by the one place that writes it and the one that reads it:
//!
//!   0            the elements own nothing
//!   1            the element *is* a string
//!   2 + index    a struct with that layout

use crate::MirType;

pub const ELEM_NONE: i64 = 0;
pub const ELEM_STRING: i64 = 1;
pub const ELEM_STRUCT_BASE: i64 = 2;

/// The tag for `ty`, or `ELEM_NONE` if it owns no strings this can point at.
///
/// An enum is `ELEM_NONE` on purpose: where its string sits depends on its tag,
/// so a flat list of offsets can't describe it. That case is #1027.
pub fn tag_of(ty: Option<&MirType>) -> i64 {
    match ty {
        Some(MirType::String) => ELEM_STRING,
        Some(MirType::Struct(id)) => ELEM_STRUCT_BASE + id.id as i64,
        _ => ELEM_NONE,
    }
}

/// The container constructors, with how many size arguments come first and how
/// many element tags follow them.
///
/// One list, read by everything that needs it: lowering appends that many tags,
/// codegen's dispatch table builds the C signature from it and expands the tags
/// into offset pointers, the pre-pass that registers those offset blobs finds
/// the tags with it, and the drop pass knows a fresh container when it sees one
/// come out of a call to one of these.
pub const CTORS: &[(&str, u8, u8)] = &[
    ("Vec_new", 1, 1),
    ("Vec_with_capacity", 2, 1),
    ("Vec_fixed", 2, 1),
    ("Map_new", 2, 2),
    ("Map_new_string_keys", 2, 2),
];

/// `(leading sizes, element tags)` for a container constructor, by the name MIR
/// calls it — monomorphization's `$` suffix and any module path stripped.
pub fn ctor_shape(name: &str) -> Option<(usize, usize)> {
    let head = name.rsplit("::").next().unwrap_or(name);
    let base = head.split('$').next().unwrap_or(head);
    CTORS
        .iter()
        .find(|(n, _, _)| *n == base)
        .map(|(_, l, t)| (*l as usize, *t as usize))
}
