// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What frees a handle, by the type of the slot holding it.
//!
//! Here rather than in codegen because two passes ask it and they must agree.
//! Codegen asks for a field of a dying aggregate; MIR asks for the payload of a
//! `Heap<T>` being dropped, which is the same question about the same handle.
//! They used to be two tables, and the MIR one was shorter — a `Heap<Vec<i64>>`
//! gave its buffer back and a `Heap<func(i64) -> i64>` didn't (#1256).

use std::collections::HashMap;

use rask_types::{GenericArg, Type as RaskType, TypeId};

/// The release for a container or a box a field holds, if it holds one.
///
/// A container field's slot holds the *handle*, not the container, so freeing
/// it means loading the pointer and passing it — the opposite shape from a
/// string field, whose slot is the header and whose release takes the slot's
/// address.
///
/// Only the container itself. `Vec<i64>?` is a different thing: the slot holds
/// a tag and a payload, the handle is behind the tag, and MIR reaches it through
/// the wrapper rather than straight off the struct — so freeing it here ran
/// before the reads (`h.v!.len()` gave 1361822157891490808). An optional or a
/// result has no head name, so it answers `None`.
pub fn container_free_for(ty: &RaskType, names: &HashMap<TypeId, String>) -> Option<&'static str> {
    // A closure a field holds is the aggregate's: storing one moves it in, and
    // the frame stops dropping it the moment it does. The block describes
    // itself — `rask_closure_free` reads its size and its environment glue out
    // of the header words — so this needs nothing type-specific, and a bare
    // function used as a value is wrapped in a block like any other closure.
    if matches!(ty, RaskType::Fn { .. }) {
        return Some("rask_closure_free");
    }
    match ty.head_name(names)? {
        // A box in a field. The release is a decrement, so it is right whether
        // or not somebody else still holds one — which is what makes a box safe
        // to hand to a task and still free here.
        "Shared" => Some(box_release_for(ty, names)),
        // Every other handle names its release on its declaration
        // (`@handle(release: …)`). A container's buffer, a rack's arena, a
        // `Random`'s state: the field owns the block, and nothing else was
        // going to give it back. One without a release is closed explicitly
        // or doesn't own what it points at.
        head => rask_stdlib::runtime_handle(head)?.release,
    }
}

/// Which of the three box releases a `Shared` field needs.
///
/// Each strategy builds its own runtime object: `Shared<T, Local>` is a cell,
/// `Shared<T, Mutex>` is a mutex, and a bare `Shared<T>` is `Readers`
/// (conc.sync/SH2). The strategy is the second type argument.
pub fn box_release_for(ty: &RaskType, names: &HashMap<TypeId, String>) -> &'static str {
    let strategy = match ty {
        RaskType::Generic { args, .. } | RaskType::UnresolvedGeneric { args, .. } => match args.get(1) {
            Some(GenericArg::Type(s)) => s.head_name(names),
            _ => None,
        },
        _ => None,
    };
    match strategy {
        Some("Local") => "rask_cell_free",
        Some("Mutex") => "rask_mutex_drop",
        _ => "rask_shared_drop_i64",
    }
}
