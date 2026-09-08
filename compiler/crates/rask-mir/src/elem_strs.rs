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
//!   2            the element *is* a Vec
//!   3            the element *is* a Map
//!   4 + index    a struct with that layout

use crate::MirType;

pub const ELEM_NONE: i64 = 0;
pub const ELEM_STRING: i64 = 1;
pub const ELEM_VEC: i64 = 2;
pub const ELEM_MAP: i64 = 3;
pub const ELEM_STRUCT_BASE: i64 = 4;

/// The tag for an element that *is* a container.
///
/// MIR types a nested container as `Ptr`, which is what every pointer is — so
/// `tag_of` can't tell `Map<string, Vec<i32>>`'s values from a raw address and
/// answered "owns nothing". The checker's type knows, so this takes the
/// rendered name. `Vec<i64>?` and `Vec<i64> or E` are wrappers around the
/// handle rather than the handle, and a `Pool` or `Rack` is an arena whose
/// contents outlive any one element (mem.pools, mem.racks).
pub fn container_tag(rendered: &str) -> Option<i64> {
    if rendered.ends_with('?') || rendered.contains(" or ") {
        return None;
    }
    match rendered.split('<').next().unwrap_or(rendered).trim() {
        "Vec" => Some(ELEM_VEC),
        "Map" => Some(ELEM_MAP),
        _ => None,
    }
}

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
    // `entries` is the fourth of that family and was the one left out. Its
    // elements are (handle, value) pairs copied out of the slots with no
    // element map, so freeing it gives back the byte store and leaves the
    // strings to the pool — which is what the other three do too.
    ("Pool_entries", 0, 0, "Vec_free"),
    // `rack.nodes()` walks the directory and pushes each node's address into a
    // fresh Vec. The elements are links, which own nothing — freeing the
    // vector doesn't touch a node. `for n in s.nodes()` leaked one vector per
    // call, which is most of what the snapshot files were carrying.
    ("Rack_nodes", 0, 0, "Vec_free"),
    // Clone the elements into a new vector and clear the source, so the result
    // owns them and carries the source's element map. The source keeps its own
    // allocation, empty.
    ("Vec_take_all", 0, 0, "Vec_free"),
    // Not `fs.read_lines`, `fs.read_bytes` or `File.lines`, even though the
    // runtime has a function for each: those three are written in Rask now, so
    // these names reach MIR as ordinary functions with bodies and the pass
    // works out the answer itself — including that each hands its vector back
    // *inside* a `Vec<T> or IoError`, which a line here can't say. Listing
    // them overrode that with "a bare Vec" and the caller freed the wrapper as
    // one: `fs.read_lines(p) catch _ => Vec.new()` tripped the borrow guard.
    //
    // `chars()` yields scalars and `graphemes()` copies each cluster into a
    // fresh string. Neither points into the source — unlike the splitters
    // below, which look identical from here and are not.
    ("string_chars", 0, 0, "Vec_free"),
    ("string_graphemes", 0, 0, "Vec_free"),
    // The three the runtime builds from the OS: each copies what it found into
    // fresh strings and carries the element map, so the vector it hands back is
    // the caller's to free — elements and all. They were the largest single
    // leak left in the suite once the closures were fixed: 150 strings for one
    // `os.env_vars()`, and `t_os_env.rk` and `t41_os.rk` between them held 304.
    //
    // Unlike the string splitters below, nothing here is a view into a source
    // the caller still holds.
    ("os_env_vars", 0, 0, "Vec_free"),
    ("os_args", 0, 0, "Vec_free"),
    ("fs_list_dir", 0, 0, "Vec_free"),
    // A `Shared` box carries no element tag — its payload is opaque bytes it
    // was handed, the same as a pool slot. It is here for the same reason
    // `Rack_new` is: `rask_shared_free` has existed all along with nothing
    // calling it, so `Shared.new(0)` and nothing else leaked the box and its
    // payload — two allocations, whatever the payload was (#1099).
    //
    // The free is a release, not a free: `Shared_drop` decrements and frees at
    // zero, and `Shared_clone` is what incremented. So a box handed to a task
    // outlives the frame that made it, which is the point of the type.
    ("Shared_new", 0, 0, "Shared_drop"),
    // A cstring owns the NUL-terminated copy it made, and it is the caller's to
    // free — that is what makes it different from `string.as_ptr()`, which
    // points into a buffer the string still holds (#949).
    ("string_copy_terminated", 0, 0, "cstring_free"),
    // The other two strategies build their own runtime object, so each needs
    // its own release: `Shared.mutex(0)` is `Mutex_new` and `Shared.local(0)`
    // is `Cell_new`, and neither is `Shared_new`. Only the third was listed, so
    // two of the three constructors leaked the box and its payload — the same
    // two allocations #1099 measured, for the two spellings it didn't test.
    ("Mutex_new", 0, 0, "Mutex_drop"),
    ("Cell_new", 0, 0, "Cell_drop"),
    // The clones. These were absent because `clone_elision` can decide a clone
    // is unnecessary and leave the caller's own container in the slot, and
    // freeing that is a double free — `return v.clone()` printed the right
    // length and died on the way out. The drop pass now runs *after* elision,
    // so what it sees is what runs: an elided clone is an assignment, not a
    // call, and never looks like a fresh container at all (#1050, #1045).
    ("Vec_clone", 0, 0, "Vec_free"),
    ("Map_clone", 0, 0, "Map_free"),
    // `bytes()` builds a fresh Vec of the string's bytes and hands it over —
    // no view into the source, so nothing reads it after the frame ends. The
    // splitters below are the family it belongs to and stay out for the reason
    // written there; this one was measured on its own.
    ("string_bytes", 0, 0, "Vec_free"),
    // Same shape on the C side: the bytes up to the terminator, copied into a
    // fresh Vec that `string.from_utf8` reads and nobody else holds (#949).
    ("cstring_bytes", 0, 0, "Vec_free"),
    //
    // The string splitters — `string_split`, `string_lines` and friends — are
    // absent for a nearer reason: each does hand back a fresh Vec, and
    // registering them clears the leak, but `simple_grep` then finds nothing
    // and `markdown_renderer` aborts on `malloc(): unaligned tcache chunk`.
    // Their result is a Vec of *views into the source string*, so the elements
    // outlive the free. `bytes()` copies instead, which is why it is listed
    // above and they are not. Measured both ways on #1050.
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
