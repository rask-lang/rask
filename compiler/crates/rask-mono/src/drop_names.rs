// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What frees a handle, by the type of the slot holding it.
//!
//! Here rather than in codegen because two passes ask it and they must agree.
//! Codegen asks for a field of a dying aggregate; MIR asks for the payload of a
//! `Heap<T>` being dropped, which is the same question about the same handle.
//! They used to be two tables, and the MIR one was shorter — a `Heap<Vec<i64>>`
//! gave its buffer back and a `Heap<func(i64) -> i64>` didn't (#1256).

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
    // A closure a field holds is the aggregate's: storing one moves it in, and
    // the frame stops dropping it the moment it does. The block describes
    // itself — `rask_closure_free` reads its size and its environment glue out
    // of the header words — so this needs nothing type-specific, and a bare
    // function used as a value is wrapped in a block like any other closure.
    if matches!(ty, RaskType::Fn { .. }) {
        return Some("rask_closure_free");
    }
    container_free_for_rendered(&format!("{}", ty))
}

/// The same question asked with the type already written out.
///
/// The rendering is what the answer is read off, and a caller that has better
/// names than `Display` does should render its own: a resolved `Type::Generic`
/// carries TypeIds, so `Shared<i64, Local>` comes out without the word `Local`
/// in it and the strategy below can't be told apart (#1256).
pub fn container_free_for_rendered(rendered: &str) -> Option<&'static str> {
    // Only the container itself. `Vec<i64>?` renders with the same head and is
    // a different thing: the slot holds a tag and a payload, the handle is
    // behind the tag, and MIR reaches it through the wrapper rather than
    // straight off the struct — so freeing it here ran before the reads
    // (`h.v!.len()` gave 1361822157891490808).
    if rendered.ends_with('?') || rendered.contains(" or ") {
        return None;
    }
    let head = rendered.split('<').next().unwrap_or(rendered).trim();
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
        // A box in a field. The release is a decrement, so it is right whether
        // or not somebody else still holds one — which is what makes a box safe
        // to hand to a task and still free here.
        //
        // Which decrement depends on the strategy, because each builds its own
        // runtime object: `Shared<T, Local>` is a cell, `Shared<T, Mutex>` is a
        // mutex, and a bare `Shared<T>` is `Readers` (conc.sync/SH2).
        // `io.Buffer` keeps its read position in a `Shared<i64, Local>` and
        // leaked two allocations per buffer.
        "Shared" | "Cell" | "Mutex" => Some(box_release_for(rendered)),
        // One-word handles onto a heap block the runtime made. Not containers —
        // they hold no elements — but the same ownership: the field owns the
        // block, and nothing else was going to give it back. `Random.from_seed`
        // in a struct field leaked its state on every construction, and an
        // `Atomic` counter in one leaked eight bytes.
        //
        // `StringBuilder` is deliberately not here. `build()` takes the builder
        // away, so a builder reached through a field and built would leave this
        // release pointing at a block that is already gone — which is a worse
        // answer than the leak.
        "Random" => Some("rask_rng_free"),
        "Atomic" => Some("rask_atomic_int_free"),
        "cstring" => Some("rask_cstring_free"),
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
