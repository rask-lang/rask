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
/// An enum is `ELEM_NONE`: where its string sits depends on its tag, so a flat
/// list of offsets can't describe one. Codegen walks the tag branches for an
/// enum reached any other way, so what is uncovered is narrow — an enum nested
/// inside a container element.
pub fn tag_of(ty: Option<&MirType>) -> i64 {
    match ty {
        Some(MirType::String) => ELEM_STRING,
        Some(MirType::Struct(id)) => ELEM_STRUCT_BASE + id.id as i64,
        _ => ELEM_NONE,
    }
}

/// Every call that hands back a container the caller owns: how many size
/// arguments come first, how many element tags follow them, and what frees the
/// result.
///
/// One list, read by everything that needs it: lowering appends that many tags,
/// codegen's dispatch table builds the C signature from it and expands the tags
/// into offset pointers, the pre-pass that registers those offset blobs finds
/// the tags with it, and the drop pass knows a fresh container when it sees one
/// come out of a call to one of these.
///
/// The free function is spelled out rather than guessed from the name, because
/// the two part company: `Map_keys` is a `Map_` call that hands back a `Vec`,
/// and freeing that with `Map_free` would read a Vec as a hash table.
pub const CTORS: &[(&str, u8, u8, &str)] = &[
    ("Vec_new", 1, 1, "Vec_free"),
    // `mut v: Vec<T> = []` and `Vec.from([...])`: the elements come from a
    // static blob, but anything pushed later does not.
    ("rask_vec_from_static", 3, 1, "Vec_free"),
    ("Vec_with_capacity", 2, 1, "Vec_free"),
    ("Vec_fixed", 2, 1, "Vec_free"),
    // `skip`/`take` outside a fused chain call the runtime, which hands back a
    // freshly allocated Vec. No size arguments and no element tags — the source
    // Vec already carries both, and the runtime copies them across — so these
    // are here only to tell the drop pass the result is the caller's to free.
    ("Vec_skip", 0, 0, "Vec_free"),
    ("Vec_take", 0, 0, "Vec_free"),
    // `chunks` hands back a fresh `Vec<Vec<T>>`. Freeing it releases the outer
    // Vec only — the inner ones are elements, and `Vec_free` frees a byte
    // store, not what its elements point at. That nested half is #943.
    ("Vec_chunks", 0, 0, "Vec_free"),
    ("Map_new", 2, 2, "Map_free"),
    ("Map_new_string_keys", 2, 2, "Map_free"),
    // `keys`, `values` and `entries` walk a map and hand back a fresh Vec of
    // what they found — a `Map_` name with a `Vec` result, which is why the
    // free is written down rather than read off the prefix.
    ("Map_keys", 0, 0, "Vec_free"),
    ("Map_values", 0, 0, "Vec_free"),
    ("Map_entries", 0, 0, "Vec_free"),
    // Racks and pools carry no element tag: a rack is told about its fields
    // separately, through `Link_register_*`, and a pool's slots are opaque
    // bytes. They are here so the drop pass recognises one coming out of a
    // constructor — `rask_rack_free` and `rask_pool_free` have existed all
    // along with nothing calling them, so `Rack.new()` with nothing in it
    // leaked (#1048).
    ("Rack_new", 0, 0, "Rack_free"),
    ("Rack_snapshot", 1, 0, "Rack_free"),
    ("Pool_new", 1, 0, "Pool_free"),
    ("Pool_with_capacity", 2, 0, "Pool_free"),
    // `handles`, `drain` and `values` walk the pool and hand back a fresh Vec —
    // another family whose name and result type disagree. `values` is declared
    // `Iterator<T>` in the stdlib but `rask_pool_values` builds a plain
    // `RaskVec`, the same as its two neighbours, so it frees the same way.
    ("Pool_handles", 0, 0, "Vec_free"),
    ("Pool_drain", 0, 0, "Vec_free"),
    ("Pool_values", 0, 0, "Vec_free"),
    // `Vec_clone` and `Map_clone` are absent on purpose. `clone_elision` can
    // decide a clone is unnecessary and leave the caller's own container in the
    // slot, and freeing that is a double free — `return v.clone()` printed the
    // right length and died on the way out. It needs the drop pass and the
    // elision to agree on what an elided clone left behind (#1050, #1045).
    //
    // The string splitters — `string_split`, `string_lines`, `string_bytes`
    // and friends — are absent for a nearer reason: each does hand back a
    // fresh Vec, and registering them clears the leak, but `simple_grep` then
    // finds nothing and `markdown_renderer` aborts on `malloc(): unaligned
    // tcache chunk`. Their result is read after the drop pass frees it.
    // Measured both ways on #1050.
];

/// `(leading sizes, element tags)` for a container constructor, by the name MIR
/// calls it — monomorphization's `$` suffix and any module path stripped.
pub fn ctor_shape(name: &str) -> Option<(usize, usize)> {
    entry(name).map(|(_, l, t, _)| (*l as usize, *t as usize))
}

/// What frees the container this call handed back, or `None` if it isn't one.
pub fn free_fn(name: &str) -> Option<&'static str> {
    entry(name).map(|(_, _, _, free)| *free)
}

fn entry(name: &str) -> Option<&'static (&'static str, u8, u8, &'static str)> {
    let head = name.rsplit("::").next().unwrap_or(name);
    let base = head.split('$').next().unwrap_or(head);
    CTORS.iter().find(|(n, _, _, _)| *n == base)
}
