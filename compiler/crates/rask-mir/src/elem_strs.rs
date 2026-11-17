// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What a container's elements are, and which calls build one.
//!
//! A container is a byte store. It knows how big an element is and nothing
//! else, so it can't tell a sixteen-byte string from a sixteen-byte struct —
//! and `free` has to know, or the strings inside never come back (#1027).
//!
//! The answer is settled once, where a container is constructed, by the only
//! place that has it: lowering, reading the checker's type. It travels as the
//! element's own type (`MirConst::Elem`), with its containers still named;
//! codegen turns it into the offsets of what one element owns, and the runtime
//! keeps that list on the container itself. Nothing downstream re-derives it —
//! not the drop pass, not the caller of a function that hands a container
//! back, not an inlined copy.
//!
//! It used to travel as one `i64`: small numbers for a string or a container,
//! a range for struct layouts, another for enums, a flag bit for wrappers. A
//! tuple fit nowhere in that, so `Vec<(string, i64)>` described its elements
//! as owning nothing and every string in it leaked (#1395).

use crate::{MirConst, MirOperand, MirType};

/// The constructor argument describing elements of type `ty`. Pass the type
/// with its containers named (`payload_to_mir`), or a `Vec<Vec<i64>>` frees
/// none of its inner vectors.
pub fn elem(ty: MirType) -> MirOperand {
    MirOperand::Constant(MirConst::Elem(ty))
}

/// Which `Map` constructor a key type wants.
///
/// A string key hashes and compares by its contents. A *link* key compares by
/// its word — two links are equal when they name the same node — but hashing
/// that word hashes the address the allocator handed out, which moves with
/// whatever the program allocated before the rack was built. Sim replays the
/// map seed so a replay walks the buckets in the same order (determinism/D7),
/// and an address in the hash input is the one thing that doesn't replay, so a
/// link key buckets by its node's slot instead (#1268). Anything else buckets
/// by its word.
///
/// One place, because three call sites used to spell the string case by hand
/// and a fourth kind would have had to find all of them.
pub fn map_ctor_for(key_ty: &MirType) -> &'static str {
    match key_ty {
        MirType::Buffer => "Map_new_string_keys",
        MirType::Link(_) => "Map_new_link_keys",
        MirType::Option(inner) if matches!(**inner, MirType::Link(_)) => "Map_new_link_keys",
        _ => "Map_new",
    }
}

/// The same decision for `Map.with_capacity(n)`, which takes the capacity
/// between the sizes and the tags but buckets by exactly the same rule.
///
/// Derived from `map_ctor_for` rather than repeating the match, so a fourth key
/// kind is added in one place and this follows.
pub fn map_ctor_with_capacity(key_ty: &MirType) -> &'static str {
    match map_ctor_for(key_ty) {
        "Map_new_string_keys" => "Map_with_capacity_string_keys",
        "Map_new_link_keys" => "Map_with_capacity_link_keys",
        _ => "Map_with_capacity",
    }
}

/// A call to a map constructor, by any of the spellings `map_ctor_for` picks.
/// Each is the declared `Map.new` (or `Map.with_capacity`) over a table keyed
/// another way, so it does what that declaration says.
pub fn map_ctor_ref(name: impl Into<String>) -> crate::FunctionRef {
    let name = name.into();
    let declared = match name.as_str() {
        "Map_with_capacity_string_keys" | "Map_with_capacity_link_keys" | "Map_with_capacity_keyed" => {
            Some("Map_with_capacity")
        }
        "Map_new_string_keys" | "Map_new_link_keys" | "Map_new_keyed" => Some("Map_new"),
        _ => None,
    };
    match declared {
        Some(declared) => crate::FunctionRef::like(name, declared),
        None => crate::FunctionRef::internal(name),
    }
}
