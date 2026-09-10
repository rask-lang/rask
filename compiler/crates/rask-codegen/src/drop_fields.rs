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

/// The release for a container or a box a field holds, if it holds one.
///
/// A container field's slot holds the *handle*, not the container, so freeing
/// it means loading the pointer and passing it — the opposite shape from a
/// string field, whose slot is the header and whose release takes the slot's
/// address.
///
/// A field's type in a layout is a resolved `Type::Generic`, which carries a
/// TypeId and no name, and there is no table here to look one up in. Rendering
/// it and taking the head is what works.
pub fn container_free_for(ty: &RaskType) -> Option<&'static str> {
    let rendered = format!("{}", ty);
    // Only the container itself. `Vec<i64>?` renders with the same head and is
    // a different thing: the slot holds a tag and a payload, the handle is
    // behind the tag, and MIR reaches it through the wrapper rather than
    // straight off the struct — so freeing it here ran before the reads
    // (`h.v!.len()` gave 1361822157891490808).
    if rendered.ends_with('?') || rendered.contains(" or ") {
        return None;
    }
    let head = rendered.split('<').next().unwrap_or(&rendered).trim();
    match head {
        "Vec" => Some("rask_vec_free"),
        // A map's tables are the same shape of ownership as a vector's buffer,
        // and 72 suite files were leaking one: `Set<T>` is a struct holding a
        // `Map<T, bool>`, so every set leaked its map too.
        "Map" => Some("rask_map_free"),
        // A rack owns its nodes' lifetime and a pool owns its slots
        // (mem.racks/RK1), so whoever owns the arena frees it. A local already
        // did; a *field* didn't, so every struct with a rack in it leaked the
        // arena and everything in it — 252 allocations across six suite files,
        // `p12_rack_link_churn.rk` alone 128.
        //
        // The links and handles that outlive a field read don't change that:
        // they can't outlive the struct that holds the arena, and this release
        // runs where that struct dies.
        "Rack" => Some("rask_rack_free"),
        "Pool" => Some("rask_pool_free"),
        // A box in a field. The release is a decrement, so it is right whether
        // or not somebody else still holds one — which is what makes a box safe
        // to hand to a task and still free here.
        //
        // Which decrement depends on the strategy, because each builds its own
        // runtime object: `Shared<T, Local>` is a cell, `Shared<T, Mutex>` is a
        // mutex, and a bare `Shared<T>` is `Readers` (conc.sync/SH2).
        // `io.Buffer` keeps its read position in a `Shared<i64, Local>` and
        // leaked two allocations per buffer.
        "Shared" | "Cell" | "Mutex" => Some(box_release_for(&rendered)),
        _ => None,
    }
}

/// Which of the three box releases a `Shared`/`Cell`/`Mutex` field needs, read
/// off the strategy in its type arguments.
pub fn box_release_for(rendered: &str) -> &'static str {
    let args = rendered.split_once('<').map(|(_, rest)| rest).unwrap_or("");
    if args.contains("Local") || rendered.starts_with("Cell") {
        return "rask_cell_free";
    }
    if args.contains("Mutex") || rendered.starts_with("Mutex") {
        return "rask_mutex_drop";
    }
    "rask_shared_drop_i64"
}
