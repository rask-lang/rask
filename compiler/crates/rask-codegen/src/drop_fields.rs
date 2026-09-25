// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What frees a field, by the field's declared type.
//!
//! Two readers, and they release different things. The walk in `builder.rs`
//! handles a value dying in a frame. `owned_fields` below feeds the vtable's
//! `owned_release`, for a value dying inside a box that owns it — which is any
//! box the value was *moved* into (a container element, a struct field, a
//! return), as against one borrowed for a call.
//!
//! The old version of the second reader is what #1144 was about: it ran for
//! every box and released the value's strings, so a borrowed box decremented a
//! buffer the frame still held. Owning the value is what makes the release
//! right, and the checker is what says the value was moved.

use rask_types::Type as RaskType;

pub use rask_mono::drop_names::{box_release_for, container_free_for};

/// How a field's release is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseShape {
    /// The slot *is* the value's header, so the release takes the slot's
    /// address — a `string`.
    ByAddress,
    /// The slot holds a handle, so the release loads it and passes the pointer
    /// — every container and every box.
    ByHandle,
}

/// A field to release when a value dies: where it sits, what frees it, and how
/// that free is called.
#[derive(Debug, Clone, Copy)]
pub struct DropField {
    pub offset: u32,
    pub free_fn: &'static str,
    pub shape: ReleaseShape,
}

/// Every field of `type_name` a box that owns the value has to release, as
/// offsets from the start of the value.
///
/// Containers *and* strings, unlike the frame's walk — a box the value was
/// moved into owns all of it, so there is no second holder for either kind. The
/// old list was strings only, which is the half that made a borrowed box
/// double-decrement.
///
/// Flat on purpose: this feeds a straight list of calls with no value to branch
/// on. An enum field is therefore left out — where its contents sit depends on
/// its tag, and saying so needs the guard encoding the container element
/// descriptors use. A `T?` or `T or E` field is out for the same reason, and
/// each omission is a leak rather than a double free.
pub fn owned_fields(
    type_name: &str,
    base_offset: u32,
    struct_layouts: &[rask_mono::StructLayout],
    visited: &mut std::collections::HashSet<String>,
) -> Vec<DropField> {
    if !visited.insert(type_name.to_string()) {
        return Vec::new();
    }
    let Some(layout) = struct_layouts.iter().find(|s| s.name == type_name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for field in &layout.fields {
        let at = base_offset + field.offset;
        if let Some(free_fn) = container_free_for(&field.ty) {
            out.push(DropField { offset: at, free_fn, shape: ReleaseShape::ByHandle });
            continue;
        }
        match &field.ty {
            RaskType::String => out.push(DropField {
                offset: at,
                free_fn: "rask_string_free",
                shape: ReleaseShape::ByAddress,
            }),
            RaskType::UnresolvedNamed(name) => {
                out.extend(owned_fields(name, at, struct_layouts, visited));
            }
            _ => {}
        }
    }
    out
}

/// Is this a trait object, however the type happens to be spelled?
///
/// A field written `any Interface` reaches the layout as a *name* rather than a
/// parsed `TraitObject` (#474), so asking for the parsed form alone answers no
/// for every field — which is exactly where the question matters.
pub fn is_trait_object(ty: &RaskType) -> bool {
    match ty {
        RaskType::TraitObject { .. } => true,
        RaskType::UnresolvedNamed(name) => name.starts_with("any "),
        _ => false,
    }
}

