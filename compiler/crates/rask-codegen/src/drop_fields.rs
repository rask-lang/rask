// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What frees a field, by the field's declared type.
//!
//! One reader: the release walk in `builder.rs`, for a value dying in a frame.
//! A value dying inside a trait object used to have a second answer here, a
//! per-type list the vtable's drop slot pointed at — it went away with the slot
//! when the frame became the owner of a boxed value's contents (mem.boxes,
//! #1144).

use rask_types::Type as RaskType;

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
