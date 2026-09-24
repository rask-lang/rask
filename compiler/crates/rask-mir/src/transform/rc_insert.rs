// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! String RC insertion — adds explicit `RcInc` and `RcDec` operations for
//! string-typed locals.
//!
//! Runs after SSA conversion. For each string-typed local:
//! - Insert `RcInc` after each copy (assignment from another string local)
//! - Insert `RcDec` at each last-use point (from liveness analysis)
//!
//! This makes refcount operations explicit in MIR so subsequent passes
//! (rc_elide) can analyze and eliminate them.
//!
//! See `comp.architecture/RC1-RC2` and `comp.string-refcount-elision`.

use std::collections::{HashMap, HashSet};

use crate::analysis::addr_alias::AddrAliases;
use crate::analysis::cfg;
use crate::analysis::dominators::DominatorTree;
use crate::analysis::liveness;
use crate::analysis::uses;
use crate::{
    MirBlock, MirTerminator,
    BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminatorKind, MirType,
};

/// The runtime entry points that answer with the address of a string's own
/// buffer rather than with a value.
///
/// Everything else a string method returns is either a scalar or a string of
/// its own; these two hand out an interior pointer, and the string is what
/// keeps the storage behind it alive.
const HANDS_OUT_THE_BUFFER: &[&str] = &["string_as_ptr", "string_as_mut_ptr"];

/// Does this statement hand out the address of `local`'s buffer?
///
/// Two spellings, one meaning. `s as i64` is the cast only `unsafe` code can
/// ask for; `s.as_ptr()` is the method, and it lowers to a call. Both read as
/// the string's last use, because nothing afterwards names the string — what
/// continues is the address. Releasing on that reading frees the buffer while
/// the address is still in flight.
fn hands_out_the_buffer(stmt: &MirStmt, local: LocalId) -> bool {
    match &stmt.kind {
        MirStmtKind::Assign { rvalue: MirRValue::Cast { value, target_ty }, .. } => {
            matches!(value, MirOperand::Local(id) if *id == local)
                && matches!(
                    target_ty,
                    MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64 | MirType::I128
                        | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
                        | MirType::U128 | MirType::Ptr
                )
        }
        // `let p = unsafe s.as_ptr()` — the method form, which the cast rule
        // never covered. `strlen(s.as_ptr())` in a frame that doesn't name `s`
        // again read the buffer after it had been freed, and got a wrong answer
        // out of libc whenever the free had written over the bytes (#1118).
        MirStmtKind::Call { func, args, .. } => {
            HANDS_OUT_THE_BUFFER.contains(&func.name.as_str())
                && args.iter().any(|a| matches!(a, MirOperand::Local(id) if *id == local))
        }
        _ => false,
    }
}

/// Insert explicit RcInc/RcDec for all string-typed locals in a function.
///
/// `kept` says, per callee, which of its parameters it holds on to — see
/// `insert_aggregate_release`, which is the only part that needs it.
pub fn insert_rc_ops(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    own: &HashSet<String>,
) {
    let string_locals: Vec<LocalId> = func.locals_of_type(&MirType::String);

    // The three string steps only have work when there is a string. The
    // aggregate walk does not: a struct holding a `Vec` and no string needs its
    // release just the same, and bailing out here meant whether that happened
    // depended on whether the function *happened* to mention a string —
    // `println("{h.items[0]}")` released the vector and
    // `assert h.items[0] == 7` leaked it, in bodies that are otherwise the same.
    if !string_locals.is_empty() {
        // Insert RcInc after string copies
        insert_rc_inc(func, &string_locals);

        // Insert RcDec at last-use points
        insert_rc_dec(func, &string_locals);

        // A returned parameter is handed out, not owned — take a reference for it.
        retain_returned_params(func, &string_locals);

        // And a write through a captured variable's address gives back what
        // that variable held.
        release_replaced_captures(func, &string_locals);
    }

    // And the aggregates: a struct field or a wrapper's payload owns a string —
    // or a container — just as much as a local does.
    insert_aggregate_release(func, kept, own);
}

/// Release what a captured string variable held, where a closure writes a new
/// one over it.
///
/// ```text
/// mut seen = ""
/// upto(2).for_each(|x| { seen = "{seen}{prefix}:{x};" })
/// ```
///
/// The closure captures `seen` by reference, so the body holds the address of
/// the frame's variable and the write is a store through it
/// (mem.closures/MC1). The new string is retained for the slot and the one it
/// replaced was released by nobody — one buffer per turn but the last (#1162).
///
/// No aliasing guard, unlike the container case: a string is refcounted, so
/// every name holds a count of its own and giving back the slot's is right
/// whoever else is reading. And a capture points at a variable the frame
/// established before building the closure, so a store through one is always a
/// replacement — there is no "is this the first write" to get wrong.
fn release_replaced_captures(func: &mut MirFunction, string_locals: &[LocalId]) {
    let strings: HashSet<LocalId> = string_locals.iter().copied().collect();
    // Addresses of a captured variable, as the body sees them.
    let capture_refs: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture { dst, access, .. } if access.is_addressed() => Some(*dst),
            _ => None,
        })
        .collect();

    for block_idx in 0..func.blocks.len() {
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();
        for (si, stmt) in func.blocks[block_idx].statements.iter().enumerate() {
            let MirStmtKind::Store { addr, value, offset: 0, .. } = &stmt.kind else {
                continue;
            };
            if !capture_refs.contains(addr) || !strings.contains(addr) {
                continue;
            }
            // Writing the slot back to itself replaces nothing.
            if uses::operand_local(value) == Some(*addr) {
                continue;
            }
            // A `Call`, not an `RcDec`. A dec on this name reads as "give back
            // the reference this name took", and a capture read takes none —
            // so `rc_elide` strips it, correctly, along with the one this
            // wants. What is being released is the reference the *slot* holds,
            // which is a different thing wearing the same local.
            insertions.push((
                si,
                MirStmt::new(
                    MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal(
                            "string_free_replaced".to_string(),
                        ),
                        args: vec![MirOperand::Local(*addr)],
                    },
                    stmt.span,
                ),
            ));
        }
        for (idx, stmt) in insertions.into_iter().rev() {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
    }
}

/// Insert `RcInc` after each assignment that copies a string local.
///
/// Pattern: `dst = src` where both are string-typed → insert `RcInc { local: dst }`
/// after the assignment. The inc goes on `dst` because dst is the new reference
/// sharing the same string data.
fn insert_rc_inc(func: &mut MirFunction, string_locals: &[LocalId]) {
    let string_set: std::collections::HashSet<LocalId> = string_locals.iter().copied().collect();

    for block_idx in 0..func.blocks.len() {
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        for (si, stmt) in func.blocks[block_idx].statements.iter().enumerate() {
            match &stmt.kind {
                // Copy from another string local
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if string_set.contains(dst) && string_set.contains(src) =>
                {
                    insertions.push((si + 1, MirStmt::new(
                        MirStmtKind::RcInc { local: *dst },
                        stmt.span,
                    )));
                }
                // Phi producing a string — the incoming value already has a refcount,
                // but the phi creates a new name that needs to be tracked. The actual
                // inc happens at the copy site in the predecessor. No inc here.
                MirStmtKind::Phi { dst, .. } if string_set.contains(dst) => {}

                // Call returning a string — new allocation, refcount starts at 1. No inc.
                MirStmtKind::Call { dst: Some(dst), .. } if string_set.contains(dst) => {}

                // Field access extracting a string — this is a copy of the string
                // from a struct field, needs inc.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Field { .. } }
                    if string_set.contains(dst) =>
                {
                    insertions.push((si + 1, MirStmt::new(
                        MirStmtKind::RcInc { local: *dst },
                        stmt.span,
                    )));
                }

                // Stored into memory — a struct field, a Result payload. That
                // location now holds its own reference, so it needs its own
                // count. Without the inc, the dec at the local's last use freed
                // a buffer the field still points at: `Body { error: "no route
                // for {path}" }` printed whatever the next allocation put
                // there (#501).
                MirStmtKind::Store { value: MirOperand::Local(src), .. }
                    if string_set.contains(src) =>
                {
                    insertions.push((si, MirStmt::new(
                        MirStmtKind::RcInc { local: *src },
                        stmt.span,
                    )));
                }

                // Copied into a heap closure's environment, which is the same
                // thing one step further out: the environment can outlive this
                // frame, so its copy needs a reference of its own. The release
                // is in the environment's drop glue
                // (`container_drop::env_drop_glue`).
                //
                // Without the retain, the dec at the string's last use — which
                // *is* this statement, nothing after it names the string — freed
                // the buffer while the closure still pointed at it, and
                // `fns.push(|x| "{prefix}:{x}")` printed an empty line (#1160).
                //
                // A stack environment needs neither: it dies with the frame, so
                // the frame's own reference covers it, and it has no glue to
                // release from.
                MirStmtKind::ClosureCreate { captures, heap: true, .. } => {
                    for cap in captures.iter().filter(|c| !c.by_ref) {
                        if string_set.contains(&cap.local_id) {
                            insertions.push((si, MirStmt::new(
                                MirStmtKind::RcInc { local: cap.local_id },
                                stmt.span,
                            )));
                        }
                    }
                }

                _ => {}
            }

        }

        // Apply insertions in reverse to preserve indices
        for (idx, stmt) in insertions.into_iter().rev() {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
    }
}

/// Release the strings an aggregate holds when the aggregate dies.
///
/// `Holder { text: "…" }` retains the string on the way into the field, and
/// nothing gave it back: the local's own release covers the local, not the
/// field. Same for a `T?` payload and a `T or E` payload — the wrapper owns a
/// reference and the wrapper going out of scope was silent. That was the last
/// ~7 MB per 200k turns after the plain cases were fixed (#1024).
///
/// This pass has no layouts, so it can't tell which aggregates hold strings.
/// It marks the death of every one it's sure about and lets codegen decide;
/// codegen has the layouts and emits nothing for the majority that hold none.
///
/// Deliberately conservative about *which* deaths it marks. An aggregate that
/// is returned, stored, or handed to a call may be keeping the string alive
/// somewhere this pass can't see, and releasing it there is a use-after-free
/// rather than a leak. Only a local nothing else can reach gets the release.
/// Container handles this frame read out of an aggregate, and their copies.
///
/// A `Vec` field's slot holds the handle, so reading it gives a bare `Ptr` that
/// the aggregate analysis can't see: it isn't an aggregate, and the group
/// union's `Field` arm skips it because a `Ptr` can't hold a string. But it
/// names storage the aggregate owns — releasing the aggregate frees what the
/// handle points at — so the two die together and liveness has to know it.
///
/// They join the group; they never become the release *target*, because the
/// release walks an aggregate apart field by field and a bare handle is not one.
fn container_handles_from(
    func: &MirFunction,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
    own: &HashSet<String>,
) -> (HashMap<LocalId, LocalId>, HashMap<LocalId, LocalId>) {
    let mut from: HashMap<LocalId, LocalId> = HashMap::new();
    // What a call handed back that points into a container this group holds:
    // `inv.orders[1]` is an `Order` *inside* the vector's buffer, not a copy of
    // one. Reading `.items` off it and indexing that is still reaching through
    // the Inventory, so the release can't run until those reads are done —
    //
    //     _64 = Vec_index(_63, 1)
    //     rc_dec_contents(_43)     // frees the Inventory, and the Vec inside it
    //     _65 = _64.0              // reads the handle that just went away
    //
    // which segfaulted on `inv.orders[1].items[1].qty` once a nested container
    // started being freed. Before that it read a freed buffer that happened to
    // still hold the right bytes.
    let mut views: HashMap<LocalId, LocalId> = HashMap::new();
    // A fixpoint: `_29 = _27` after `_27 = _25.0` is still the same handle, and
    // a view's own field read is a handle into the same group.
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                // A call that hands back a view into its receiver's storage.
                if let MirStmtKind::Call { func: fref, args, dst: Some(dst), .. } = &stmt.kind {
                    if crate::own_names::returns_a_view(&fref.name, own) {
                        let root = args
                            .first()
                            .and_then(uses::operand_local)
                            .and_then(|recv| from.get(&recv).or_else(|| views.get(&recv)))
                            .copied();
                        if let Some(root) = root {
                            if views.insert(*dst, root).is_none() {
                                changed = true;
                            }
                        }
                    }
                }
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } => {
                        let Some(base) = uses::operand_local(base) else { continue };
                        // A container handle out of an aggregate this frame
                        // holds. The read has to be a pointer — that is what
                        // tells a handle from an ordinary scalar field. A plain
                        // `m.size` admitted here joins the group and can block
                        // its release, which turns this into a leak somewhere
                        // else.
                        //
                        // `Heap<T>` is the same read: the block belongs to the
                        // aggregate, so `*h.inner` is reading through it and
                        // the release has to wait. It used to arrive as a bare
                        // `Ptr` and be covered by that; once the type said
                        // `heap<i64>` instead, the release landed between the
                        // field read and the load and `*h.inner` read freed
                        // memory (#1256).
                        if aggregates.contains(&base)
                            && matches!(
                                ty_of.get(dst),
                                Some(MirType::Ptr) | Some(MirType::Heap(_))
                            )
                        {
                            if from.insert(*dst, base).is_none() {
                                changed = true;
                            }
                            continue;
                        }
                        // A field read off something already known to point
                        // into a group is still pointing into it, whatever MIR
                        // types the base. The rule above needs the base to be
                        // an aggregate, and a `T?` holding a container handle
                        // isn't one — so
                        //
                        //     _41 = Vec_get_opt(_40, 0)  // a view into h.nested
                        //     _44 = _41.0                // the inner Vec's handle
                        //     rc_dec_contents(_0)        // frees h, and _44 with it
                        //     _45 = Vec_len(_44)         // reads what just went
                        //
                        // gave `first.len()` = 5775375445721207872 for
                        // `h.nested.get(0)? as first`. Recorded as a view,
                        // which holds the release back without letting this
                        // local's own verdict decide the container's fate.
                        let root = views
                            .get(&base)
                            .or_else(|| from.get(&base))
                            // A *wrapper* read off an aggregate. `h.v` on a
                            // `Vec<i64>?` field gives the tag and the handle
                            // together, and `h.v!` reaches the handle through
                            // it — so neither is a bare pointer off the struct
                            // and the release ran before the reads. As a view
                            // it only delays the release; a group member would
                            // block it.
                            //
                            // Wrappers only. Every scalar field read admitted
                            // here pushes the release to that local's last use,
                            // which cost four suite files a small leak each.
                            //
                            // A trait object joins them: it is a 16-byte fat
                            // pointer read out of the struct's own storage, not
                            // a scalar copied out of it, and `r.inner` passed
                            // on to something else is read long after the read
                            // that produced it.
                            .or((aggregates.contains(&base)
                                && matches!(
                                    ty_of.get(dst),
                                    Some(MirType::Option(_))
                                        | Some(MirType::Result { .. })
                                        | Some(MirType::TraitObject { .. })
                                ))
                            .then_some(&base))
                            .copied();
                        if let Some(root) = root {
                            if views.insert(*dst, root).is_none() {
                                changed = true;
                            }
                        }
                    }
                    // A handle parked in a buffer so a call can point at it.
                    // The buffer holds a copy of the handle, so whoever reads
                    // the buffer is still reading through the aggregate and the
                    // release has to wait for them. `json.encode(p)` on a
                    // `struct { counts: Map }` hands the encoder the field's
                    // handle exactly this way; without this the release landed
                    // between the store and the call and the map read empty.
                    //
                    // A store *into* an aggregate is the opposite — that's how
                    // one is built — so those are left alone.
                    MirStmtKind::Store { addr, value, .. } => {
                        let Some(src) = uses::operand_local(value) else { continue };
                        if aggregates.contains(addr) {
                            continue;
                        }
                        let root = from.get(&src).or_else(|| views.get(&src)).copied();
                        if let Some(root) = root {
                            if views.insert(*addr, root).is_none() {
                                changed = true;
                            }
                        }
                    }
                    MirStmtKind::Assign {
                        dst,
                        rvalue: MirRValue::Use(MirOperand::Local(src)),
                    // A *copy* of a known handle stays one whatever MIR types
                    // it: `_43: ptr` then `_45 = _43` with `_45: i64` is what
                    // gets emitted. Requiring `Ptr` here dropped the copy out of
                    // the group, so the release landed before its own uses —
                    //
                    //     _43 = _40.1
                    //     _45 = _43
                    //     rc_dec_contents(_40)     // frees the Vec
                    //     _46 = Vec_len(_45)       // reads it: 0
                    } => {
                        if let Some(&root) = from.get(src) {
                            if from.insert(*dst, root).is_none() {
                                changed = true;
                            }
                        } else if let Some(&root) = views.get(src) {
                            if views.insert(*dst, root).is_none() {
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (from, views)
}

fn insert_aggregate_release(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    own: &HashSet<String>,
) {
    let ty_of: HashMap<LocalId, MirType> =
        func.locals.iter().map(|l| (l.id, l.ty.clone())).collect();
    let aggregates: HashSet<LocalId> = func
        .locals
        .iter()
        // A wrapper around a container holds something worth releasing even
        // when nothing in it is a string: `Vec<i64>?` is a tag beside a handle,
        // and the vector behind that tag was nobody's. The kind is on the local
        // rather than in the type — `MirType::Container` says why.
        .filter(|l| aggregate_may_hold_string(&l.ty) || l.container.is_some())
        .map(|l| l.id)
        .collect();
    if aggregates.is_empty() {
        return;
    }
    let (handles, views) = container_handles_from(func, &aggregates, &ty_of, own);

    // Closures this frame drops, and the aggregates they hold.
    //
    // `container_drop::insert_closure_drops` emits a drop only for a closure
    // the frame owns, and this pass runs after it — so the drop's presence is
    // the answer to "does the frame outlive this closure".
    //
    // A closure that holds an aggregate used to block its whole group, and
    // blocking leaks: a struct with a `Vec` field, handed to a closure the
    // frame also drops, was released by nobody. `h.walk()` on a
    // `struct Holder { items: Vec<i64> }` leaked the vector on every sequence
    // built over a struct field. The closure is a name that *reaches* the
    // group instead, exactly like a handle read out of it — it counts for
    // placement and is never the name released, so the group stays live until
    // the `closure_drop` and the release lands after it.
    let dropped_closures: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::ClosureDrop { closure } => Some(*closure),
            _ => None,
        })
        .collect();
    let mut holding_closures: Vec<(LocalId, LocalId)> = Vec::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::ClosureCreate { dst, captures, heap: true, .. } = &stmt.kind else {
            continue;
        };
        if !dropped_closures.contains(dst) {
            continue;
        }
        for cap in captures.iter().filter(|c| aggregates.contains(&c.local_id)) {
            holding_closures.push((*dst, cap.local_id));
        }
    }
    let holds_one: HashSet<LocalId> = holding_closures.iter().map(|(c, _)| *c).collect();

    // A trait box the frame drops doesn't take the value away either, and for
    // the same reason: `TraitDrop` is what `trait_drop` emits for a box the
    // frame owns, this pass runs after it, so the drop's presence answers "does
    // the frame outlive this box".
    //
    // The frame owns a boxed value's contents; the box borrows them
    // (mem.shared-rack-heap, #1144). `TraitBox` copies the value *shallowly*, so the box
    // and the frame's own local hold the same container handle, and two boxes
    // of one value hold it twice — a free has to happen exactly once and the
    // box is not a place where "exactly once" can be arranged. Calling the
    // boxing a hand-over left the contents to the box's drop glue, which can't
    // do it; so the frame keeps them, one release however many boxes exist.
    //
    // A box the frame *doesn't* drop can outlive the frame, and releasing then
    // is a use-after-free rather than a leak — so that one still blocks.
    let dropped_boxes: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::TraitDrop { trait_object } => Some(*trait_object),
            _ => None,
        })
        .collect();
    // The drop is rarely on the boxing site's own name. Inlining copies the box
    // into the callee's parameter local and the drop lands there, so
    // `describe_one(one)` boxes into `_22` and drops `_32`. Follow the copies
    // forward from the box and ask whether any name it reaches is dropped.
    let mut copied_into: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } =
            &stmt.kind
        {
            copied_into.entry(*src).or_default().push(*dst);
        }
    }
    // Every name the box reaches that the frame drops. The *dropped* name is
    // what has to hold the group live, not the boxing site's: `_22`'s last use
    // is the copy into `_32`, so registering `_22` put the release after the
    // first `TraitDrop` while a later box of the same value was still reading
    // it — `two.counts.len()` came back 12209367259287946116.
    let drops_reached = |start: LocalId| {
        let mut seen: HashSet<LocalId> = HashSet::new();
        let mut found: Vec<LocalId> = Vec::new();
        let mut frontier = vec![start];
        while let Some(id) = frontier.pop() {
            if !seen.insert(id) {
                continue;
            }
            if dropped_boxes.contains(&id) {
                found.push(id);
            }
            if let Some(next) = copied_into.get(&id) {
                frontier.extend(next.iter().copied());
            }
        }
        found
    };
    let mut holding_boxes: Vec<(LocalId, LocalId)> = Vec::new();
    let mut boxes_one: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::TraitBox { dst, value, .. } = &stmt.kind else { continue };
        let dropped = drops_reached(*dst);
        if dropped.is_empty() {
            continue;
        }
        if let Some(id) = uses::operand_local(value) {
            if aggregates.contains(&id) {
                boxes_one.insert(*dst);
                for d in dropped {
                    // The dropped name is a fat pointer too, so it can't be the
                    // name the release walks either — and it is the one the
                    // placement below sees, because it's what keeps the group
                    // live. Naming it emitted `rc_dec_contents(_40)` on an
                    // `any Describes`, which the walk has no case for and
                    // silently does nothing about: the `Vec` inside the boxed
                    // value leaked exactly as before the change.
                    boxes_one.insert(d);
                    holding_boxes.push((d, id));
                }
            }
        }
    }

    // One group per value. SSA renames an aggregate at every copy, and a
    // payload read out of a wrapper names the same bytes rather than copying
    // them — so `r`, `r.0`, and every SSA name of either are one thing that
    // dies once. Splitting them was how the wrapper's release ended up running
    // while a view into its payload was still live.
    let mut groups = aggregate_value_groups(func, &aggregates, &ty_of);
    for (handle, base) in &handles {
        for g in groups.iter_mut() {
            if g.contains(base) {
                g.insert(*handle);
                break;
            }
        }
    }
    // Neither a handle nor a view is a name the release can walk — the release
    // takes an aggregate apart field by field, and both of these point *into*
    // one.
    let not_a_name = |l: &LocalId| {
        handles.contains_key(l)
            || views.contains_key(l)
            || holds_one.contains(l)
            || boxes_one.contains(l)
    };

    /// The aggregate a chain of views and handles ultimately reads out of.
    fn resolve_root(
        local: LocalId,
        handles: &HashMap<LocalId, LocalId>,
        views: &HashMap<LocalId, LocalId>,
    ) -> LocalId {
        let mut cur = local;
        // Bounded rather than trusting the chain to be acyclic.
        for _ in 0..64 {
            match views.get(&cur).or_else(|| handles.get(&cur)) {
                Some(&next) if next != cur => cur = next,
                _ => break,
            }
        }
        cur
    }

    // Anything that might keep the value alive elsewhere disqualifies its whole
    // group. Releasing there is a use-after-free rather than a leak, and this
    // pass can't see far enough to tell.
    let mut blocked: HashSet<usize> = HashSet::new();
    let group_of: HashMap<LocalId, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(gi, g)| g.iter().map(move |l| (*l, gi)))
        .collect();
    let block_local = |blocked: &mut HashSet<usize>, id: &LocalId| {
        if let Some(gi) = group_of.get(id) {
            blocked.insert(*gi);
        }
    };

    // A parameter is the caller's aggregate, not this frame's.
    for param in &func.params {
        block_local(&mut blocked, &param.id);
    }

    // And an allow-list for where the value came from. Releasing something this
    // frame doesn't own is a use-after-free, so the question is answered the
    // safe way round: a group is releasable only when every one of its names
    // was produced by something that hands ownership over.
    for block in &func.blocks {
        for stmt in &block.statements {
            let Some(dst) = uses::stmt_def(stmt) else { continue };
            if !aggregates.contains(&dst) {
                continue;
            }
            let owns = match &stmt.kind {
                // A copy or a payload read — the group's own members, already
                // unioned together.
                MirStmtKind::Assign { rvalue, .. } => matches!(
                    rvalue,
                    MirRValue::Use(MirOperand::Local(_)) | MirRValue::Field { .. }
                ),
                MirStmtKind::Phi { .. } => true,
                // A call gives up what it returns — unless it hands back a
                // view into storage its receiver keeps, the way `v.get(i)`
                // points into the vector's own buffer. The declaration says
                // which (`mir_metadata::returns_a_view`).
                MirStmtKind::Call { func: fref, .. } => {
                    !crate::own_names::returns_a_view(&fref.name, own)
                }
                // A pool element, a capture, a global, a dynamic call: all
                // views into storage somebody else keeps.
                _ => false,
            };
            if !owns {
                block_local(&mut blocked, &dst);
            }
        }
    }

    // Where a group's value was handed over on *this* path, rather than
    // everywhere. A `try` on a `Container or E` inside a function that returns
    // `_ or E` reads the error out of the wrapper and stores it into the
    // Result being returned — so the wrapper's group was blocked outright, and
    // the vector on its *ok* side, which that path never produced, went with
    // it. `Buffer.read_text` leaked the byte vector it decodes from, every
    // call.
    //
    // The hand-over happens at a point, so the release is refused from there
    // on and allowed everywhere else. Same shape as `container_drop`'s
    // `blocks_past_a_consume`, and the same reason for the "may" answer:
    // refusing where the value is still ours only leaks.
    let mut handed_over_in: HashMap<usize, HashSet<BlockId>> = HashMap::new();

    for block in &func.blocks {
        for (si, stmt) in block.statements.iter().enumerate() {
            match &stmt.kind {
                // Handed to something else, which may keep it.
                MirStmtKind::Call { func: fref, args, .. } => {
                    let borrows_recv =
                        rask_stdlib::mir_metadata::borrows_receiver(&fref.name);
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        // `h.items[0]` is `Vec_index(items, 0)`: the receiver is
                        // borrowed, so the call keeps nothing. Only for a
                        // handle read out of an aggregate — a *struct* reaching
                        // a call is one whose fields might now be somebody
                        // else's, whatever the callee does with argument zero.
                        if i == 0 && borrows_recv && handles.contains_key(&id) {
                            continue;
                        }
                        // Giving back what a field held, right before the field
                        // holds something else. Argument zero is the handle
                        // that was in the slot; the aggregate is untouched and
                        // still needs its own release, for whatever ends up in
                        // there (#1198).
                        if i == 0
                            && rask_stdlib::mir_metadata::frees_a_replaced_slot(&fref.name)
                        {
                            continue;
                        }
                        // A callee whose body this pass can read, and which
                        // demonstrably doesn't hold on to the aggregate, leaves
                        // it to this frame. Both sides refusing is how a `take
                        // self` struct's `Vec` came to be freed by nobody: the
                        // caller called it handed over, the callee called it the
                        // caller's, and `os.Command.spawn` leaked the builder's
                        // two vectors on every call. It only looked fixed when
                        // the callee was small enough to inline, which put the
                        // release in the caller by accident.
                        //
                        // Only for a callee in `kept`. A runtime helper or a
                        // native has no body to read, and the declared metadata
                        // answers "doesn't keep" for anything outside a family
                        // it accounts for — `rask_vec_from_static` copies an
                        // array literal's bytes into a new vector and owns the
                        // strings afterwards, so releasing the array here freed
                        // what the vector now holds.
                        if kept.get(&fref.name).is_some_and(|v| !v.get(i).copied().unwrap_or(true))
                        {
                            continue;
                        }
                        // A bodiless runtime helper whose line in
                        // `INTERNAL_SPELLINGS` says outright that it keeps
                        // none of what it is handed. That is a written-down
                        // claim rather than the "nobody accounted for this"
                        // default `keeps_argument` returns, which is why it
                        // can be trusted where that one can't.
                        //
                        // `Link_register_struct(h)` is the reason: a rack has
                        // to be told which of a struct's fields hold links, so
                        // the whole struct goes to the runtime — and a struct
                        // reaching any call at all was reason enough to stop
                        // releasing it. Every struct with a rack in it leaked
                        // the arena and its nodes.
                        if rask_stdlib::mir_metadata::keeps_no_arguments(&fref.name) {
                            continue;
                        }
                        block_local(&mut blocked, &id);
                    }
                }
                // Copied whole into memory — the destination owns it now.
                // Storing *into* an aggregate is the opposite: that's how one is
                // built, and the retain on the value is already there.
                MirStmtKind::Store { addr, value, .. } => {
                    if let Some(id) = uses::operand_local(value) {
                        // Unless what's stored is a handle read out of an
                        // aggregate and the destination is somewhere this pass
                        // never releases — a scratch word parked so a call can
                        // point at it. Nothing there can free the container a
                        // second time, and calling it a hand-over stopped the
                        // struct that owns the container from being released at
                        // all: `json.encode(p)` on a `struct { counts: Map }`
                        // leaked the map, because the encoder is handed the
                        // field's handle through exactly such a buffer.
                        if handles.contains_key(&id) && !aggregates.contains(addr) {
                            continue;
                        }
                        // Or the value went into its own successor, which is
                        // not leaving the frame at all. See `moved_within_the_group`.
                        if moved_within_the_group(func, block, si, *addr, id, &group_of) {
                            continue;
                        }
                        if let Some(gi) = group_of.get(&id) {
                            handed_over_in.entry(*gi).or_default().insert(block.id);
                        }
                    }
                }
                MirStmtKind::ArrayStore { value, .. } => {
                    if let Some(id) = uses::operand_local(value) {
                        block_local(&mut blocked, &id);
                    }
                }
                // A box the frame drops leaves the value the frame's — see
                // `holding_boxes` above. One it doesn't own can outlive the
                // frame, so that still blocks.
                MirStmtKind::TraitBox { dst, value, .. } => {
                    if boxes_one.contains(dst) {
                        continue;
                    }
                    let _ = dst;
                    if let Some(id) = uses::operand_local(value) {
                        block_local(&mut blocked, &id);
                    }
                }
                // A closure the frame drops doesn't take the aggregate
                // away — see `holding_closures` above. One it doesn't own can
                // outlive the frame, and releasing then is a use-after-free
                // rather than a leak.
                MirStmtKind::ClosureCreate { dst, captures, .. } => {
                    if holds_one.contains(dst) {
                        continue;
                    }
                    for cap in captures {
                        block_local(&mut blocked, &cap.local_id);
                    }
                }
                // `Ref` hands out the address.
                MirStmtKind::Assign { rvalue: MirRValue::Ref(src), .. } => {
                    block_local(&mut blocked, src);
                }
                _ => {}
            }
        }
        // Returned: ownership moves to the caller.
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                block_local(&mut blocked, id);
            }
            _ => {}
        }
    }

    // Filtering renumbers the groups, so the hand-over map has to be
    // renumbered with it or a release would be refused in another group's
    // blocks.
    let mut gone: Vec<HashSet<BlockId>> = Vec::new();
    let groups: Vec<HashSet<LocalId>> = groups
        .into_iter()
        .enumerate()
        .filter(|(gi, _)| !blocked.contains(gi))
        .map(|(gi, g)| {
            gone.push(match handed_over_in.get(&gi) {
                Some(sites) => blocks_past_a_handover(func, sites),
                None => HashSet::new(),
            });
            g
        })
        .collect();
    if groups.is_empty() {
        return;
    }

    // Locals that read *through* a group without being one of its names.
    //
    // `inv.orders[1]` is an `Order` inside the vector's buffer, not a copy of
    // one, so `.items` off it and an index into that are still reads of the
    // Inventory. They can't be group members: a view's own verdict — "not owned
    // here" — belongs to the view, and letting it reach the container took the
    // protection off `scene.nodes.get(h)? as n` and released a pool element's
    // contents. So they count for placement and for nothing else:
    //
    //     _64 = Vec_index(_63, 1)
    //     rc_dec_contents(_43)     // frees the Inventory, and the Vec inside it
    //     _65 = _64.0              // reads the handle that just went away
    //
    // which segfaulted `inv.orders[1].items[1].qty` once a nested container
    // started being freed; before that it read a buffer that was gone and
    // happened to still hold the right bytes.
    let mut reaches: Vec<HashSet<LocalId>> = vec![HashSet::new(); groups.len()];
    {
        let member_of: HashMap<LocalId, usize> = groups
            .iter()
            .enumerate()
            .flat_map(|(gi, g)| g.iter().map(move |l| (*l, gi)))
            .collect();
        for local in views.keys().chain(handles.keys()) {
            let root = resolve_root(*local, &handles, &views);
            if root == *local {
                continue;
            }
            if let Some(&gi) = member_of.get(&root) {
                if !groups[gi].contains(local) {
                    reaches[gi].insert(*local);
                }
            }
        }
        // And a trait box holding one of the group's names, so the group stays
        // live until the `TraitDrop` and the release lands after it.
        for (boxed, member) in &holding_boxes {
            if let Some(&gi) = member_of.get(member) {
                if !groups[gi].contains(boxed) {
                    reaches[gi].insert(*boxed);
                }
            }
        }
        // And a closure holding one of the group's names, for the reason above.
        for (closure, member) in &holding_closures {
            if let Some(&gi) = member_of.get(member) {
                if !groups[gi].contains(closure) {
                    reaches[gi].insert(*closure);
                }
            }
        }
    }

    let (live_in, live_out) = aggregate_liveness(func, &groups, &reaches);

    // Which aggregate each one was read out of. A group can hold both a struct
    // and a struct *inside* it — `p.home` on a `Rec { home: Address, counts:
    // Vec<i64> }` is one storage read at an offset, which is why the two are
    // grouped at all. The release then has to name the outer one: it walks
    // every field, the inner's included, where naming the inner walks a strict
    // subset and leaves the outer's containers to nobody.
    let enclosing = enclosing_aggregates(func);

    // A group that only stays live because of a branch that doesn't end it
    // needs its release on the branch that does. The normal placement below
    // anchors a release to the group's last *use* in a block where it dies —
    // and a block can have neither. `for it in self.items` is exactly that: the
    // loop header keeps the vector live for the body, the exit block never
    // mentions it, so there was no release anywhere and the container in the
    // field was never freed. An early `return` out of a function that reads the
    // field later is the same shape.
    let edge_releases =
        aggregate_edge_releases(func, &groups, &handles, &views, &live_in, &live_out, &gone);

    // Groups something else already placed a release for, read once — the loop
    // below adds releases of its own, and re-reading the function would let one
    // block's release suppress another block's.
    let released_already: Vec<bool> =
        groups.iter().map(|g| already_released(func, g)).collect();

    for block_idx in 0..func.blocks.len() {
        let stmts_len = func.blocks[block_idx].statements.len();
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        for (gi, group) in groups.iter().enumerate() {
            if live_out[block_idx][gi] || gone[gi].contains(&func.blocks[block_idx].id) {
                continue;
            }
            // Somebody already said where this one dies. `drop(p)` on a
            // `Heap<T>` releases the payload's contents before giving the block
            // back — the block is where they live, and after `rask_free` there
            // is nothing left to walk — so lowering emits the release itself.
            // A second one here is a second release of the same strings.
            if released_already[gi] {
                continue;
            }
            let mut last = None;
            let mut local = None;
            for si in 0..stmts_len {
                let stmt = &func.blocks[block_idx].statements[si];
                if matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                    continue;
                }
                // A store *into* the aggregate is one field of a value being
                // built, not the end of one — and a release placed right after
                // it runs on a slot whose other fields nobody has written yet.
                // `try dto.validate()` in a `-> string or ApiError` function
                // released between the tag store and the payload store, so
                // `release_either` took the err branch and freed a string
                // header made of stack garbage (#1122).
                if matches!(&stmt.kind, MirStmtKind::Store { addr, .. } if group.contains(addr)) {
                    continue;
                }
                // Lowest id among the ones this statement touches, and lowest
                // among the group's own names — a group is a `HashSet`, so
                // `find` picked a different member per process and two compiles
                // of one program emitted the release on different locals. It
                // showed up as a leak that appeared in half the runs.
                let touched = group
                    .iter()
                    .chain(reaches[gi].iter())
                    .copied()
                    .filter(|id| {
                        uses::stmt_reads(stmt, *id) || uses::stmt_def(stmt) == Some(*id)
                    })
                    .min_by_key(|id| id.0);
                if let Some(id) = touched {
                    last = Some(si);
                    // The release walks an aggregate apart field by field, so a
                    // bare handle is never the thing to name — but its use
                    // still moves the release later.
                    let nameable = group
                        .iter()
                        .copied()
                        .filter(|l| !not_a_name(l))
                        .min_by_key(|l| l.0);
                    if !not_a_name(&id) {
                        // The name has to be one this statement actually
                        // touches. Taking the group's lowest instead named a
                        // local the path never wrote, and the `IoError` message
                        // in `fs.metadata(missing) catch e => …` stopped being
                        // released.
                        local = Some(id);
                    } else if local.is_none() {
                        local = nameable;
                    }
                }
            }
            let (Some(si), Some(local)) = (last, local) else { continue };
            // Up to the outermost member of this group. Reading `p.home` means
            // `p` was written first — you can't read a field of something that
            // was never established — so the base is always a valid name here.
            let mut local = local;
            while let Some(&base) = enclosing.get(&local) {
                if !group.contains(&base) || not_a_name(&base) {
                    break;
                }
                local = base;
            }
            let local = &local;
            let span = func.blocks[block_idx].statements[si].span;
            // Step over the retains already sitting here. The last use of a
            // wrapper is usually the read that pulls its payload out, and the
            // retain on that payload is the next statement — releasing first
            // frees the buffer the retain is about to touch.
            let mut at = si + 1;
            while at < stmts_len
                && matches!(
                    func.blocks[block_idx].statements[at].kind,
                    MirStmtKind::RcInc { .. }
                )
            {
                at += 1;
            }
            insertions.push((
                at,
                MirStmt::new(MirStmtKind::RcDecContents { local: *local }, span),
            ));
        }

        insertions.sort_by(|a, b| b.0.cmp(&a.0));
        for (idx, stmt) in insertions {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
    }

    // After the placement loop, for the same reason the string version is: a
    // release sitting at the top of a block reads the group, so the loop above
    // would have counted it as a use and put a second one behind it.
    for (block_id, local) in edge_releases {
        if let Some(b) = func.blocks.iter_mut().find(|b| b.id == block_id) {
            let span = b.terminator.span;
            b.statements
                .insert(0, MirStmt::new(MirStmtKind::RcDecContents { local }, span));
        }
    }
}

/// Where a group's release belongs when no block holds both its last use and
/// its death.
///
/// The group is live out of `B` and dead on entry to one of `B`'s successors,
/// so that edge is where it ends. Three guards, and each of them is a leak
/// rather than a double free when it says no:
///
///   - every predecessor of the successor has the group live on the way out, so
///     nothing can arrive there with the value already gone, or having never
///     built it. One predecessor is the easy way to be sure of that and used to
///     be the whole test, which left out every fused adapter loop with two ways
///     out — `r.xs.zip(other)` exits both when the receiver runs out and when
///     the other side does
///   - something that writes the group dominates the successor, so the slot the
///     release walks has been written by the time control gets there
///   - the name is one the release can walk, which a bare container handle is
///     not — it names the aggregate, and the aggregate is what holds the fields
fn aggregate_edge_releases(
    func: &MirFunction,
    groups: &[HashSet<LocalId>],
    handles: &HashMap<LocalId, LocalId>,
    views: &HashMap<LocalId, LocalId>,
    live_in: &[Vec<bool>],
    live_out: &[Vec<bool>],
    gone: &[HashSet<BlockId>],
) -> Vec<(BlockId, LocalId)> {
    let index_of: HashMap<BlockId, usize> =
        func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let preds = cfg::predecessors(func);
    let dom = DominatorTree::build(func);

    // Blocks that write each group, so "has it been built yet" has an answer.
    let mut writes: Vec<Vec<BlockId>> = vec![Vec::new(); groups.len()];
    for block in &func.blocks {
        for (gi, group) in groups.iter().enumerate() {
            let touched = block.statements.iter().any(|st| {
                matches!(&st.kind, MirStmtKind::Store { addr, .. } if group.contains(addr))
                    || uses::stmt_def(st).is_some_and(|d| group.contains(&d))
            });
            if touched {
                writes[gi].push(block.id);
            }
        }
    }

    let mut out: Vec<(BlockId, LocalId)> = Vec::new();
    for (bi, block) in func.blocks.iter().enumerate() {
        for (gi, group) in groups.iter().enumerate() {
            if !live_out[bi][gi] || writes[gi].is_empty() {
                continue;
            }
            let Some(name) = group
                .iter()
                .copied()
                .filter(|l| !handles.contains_key(l) && !views.contains_key(l))
                .min_by_key(|l| l.0)
            else {
                continue;
            };
            for succ in cfg::successors(&block.terminator) {
                let Some(si) = index_of.get(&succ) else { continue };
                if live_in[*si][gi] {
                    continue;
                }
                // Every path into the successor has to be one where the group
                // is live on the way in, or a release at the top of it runs on
                // a path that never built the group or still needs it. This
                // used to demand a single predecessor, which is the easy case
                // of the same rule — and it left out every fused adapter loop
                // with two ways out. `r.xs.zip(other)` on a struct field is
                // one: the exit is reached both when the receiver runs out and
                // when the other side does, so neither edge qualified and the
                // field's vector was freed by nobody.
                let all_live_out = preds
                    .get(&succ)
                    .is_some_and(|ps| {
                        !ps.is_empty()
                            && ps.iter().all(|p| {
                                index_of.get(p).is_some_and(|pi| live_out[*pi][gi])
                            })
                    });
                if !all_live_out {
                    continue;
                }
                if !writes[gi].iter().any(|w| dom.dominates(*w, succ)) {
                    continue;
                }
                if gone[gi].contains(&succ) {
                    continue;
                }
                // And the successor must not still need the value. Liveness
                // answers that per *group*, so a block that builds the next
                // version while reading the current one — two names, one
                // group — has the write hide the read and reads as dead on
                // entry:
                //
                //     bb12:
                //       *(_34+0)  = 1      // the new node: a write
                //       *(_44+0)  = _41    // the old list: a read of another name
                //
                // That put an `rc_dec_contents` at the top of a loop body, on a
                // name nothing had written yet the first time round (#1213).
                // Asking the block itself is cheap and exact where the group
                // answer is not.
                if reads_before_writing(func, succ, group) {
                    continue;
                }
                out.push((succ, name));
            }
        }
    }
    out.sort_by_key(|(b, l)| (b.0, l.0));
    out.dedup_by_key(|(b, l)| (b.0, l.0));
    out
}

/// Does `block` read one of the group's names before writing that same name?
///
/// The precise half of "is the group live on entry here". `aggregate_liveness`
/// answers it for the group as a whole, which is enough for placing a release
/// inside a block and not enough for putting one at the top of one.
fn reads_before_writing(func: &MirFunction, block: BlockId, group: &HashSet<LocalId>) -> bool {
    let Some(b) = func.blocks.iter().find(|b| b.id == block) else { return false };
    let mut written: HashSet<LocalId> = HashSet::new();
    for stmt in &b.statements {
        let stored_into = match &stmt.kind {
            MirStmtKind::Store { addr, .. } if group.contains(addr) => Some(*addr),
            _ => None,
        };
        // A store's destination address is the write, not a read of what was
        // there before; its *value* is an ordinary read.
        let reads = match (&stmt.kind, stored_into) {
            (MirStmtKind::Store { value, .. }, Some(_)) => uses::operand_local(value)
                .is_some_and(|v| group.contains(&v) && !written.contains(&v)),
            _ => group
                .iter()
                .any(|l| !written.contains(l) && uses::stmt_reads(stmt, *l)),
        };
        if reads {
            return true;
        }
        if let Some(addr) = stored_into {
            written.insert(addr);
        }
        if let Some(d) = uses::stmt_def(stmt) {
            if group.contains(&d) {
                written.insert(d);
            }
        }
    }
    group
        .iter()
        .any(|l| !written.contains(l) && uses::terminator_reads(&b.terminator, *l))
}

/// A store that moves a value from one of a group's names to another, rather
/// than out of the frame.
///
/// `out = List.Cons(i, Heap(out))` builds a new node whose tail is the old
/// list, and in MIR that is a copy of the old value into a fresh block:
///
/// ```text
///   bb5:
///     _27 = phi [_25 from bb4, _31 from bb6]
///   bb6:
///     *(_20+0)  = 1
///     *(_20+8)  = _28
///     _30 = rask_alloc(24)
///     *(_30+0)  = _27  [24B]   // the old list, copied into the new block
///     *(_20+16) = _30          // the block goes into the new node
///     _31 = _20                // and the new node is what comes round again
/// ```
///
/// Read that middle store on its own and it is a hand-over: whoever owns the
/// block owns the copy now, so releasing `_27` from there on would free it
/// twice. Read the three together and the block never left the frame — it is
/// inside `_20`, `_20` is the same group as `_27`, and the group's release
/// walks into it and frees the whole chain. Calling it a hand-over left every
/// node of a list built this way freed by nobody (#1213).
///
/// Both halves have to hold:
///
///   - the block lands in a name of the same group, which is what makes that
///     group's release cover the copy;
///   - and that name takes `_27`'s place — straight through, or round the loop
///     as the phi operand arriving from this block. Without it the old value
///     would still have a live name of its own and the release would be a
///     double free rather than a leak, which is the wrong half of the trade.
fn moved_within_the_group(
    func: &MirFunction,
    block: &crate::MirBlock,
    at: usize,
    into: LocalId,
    value: LocalId,
    group_of: &HashMap<LocalId, usize>,
) -> bool {
    let Some(gi) = group_of.get(&value) else { return false };
    let rest = &block.statements[at + 1..];

    // Where the block ends up, followed as far as the rest of this block goes.
    // An enum variant with a struct payload takes three hops — block into the
    // struct's field, struct into the variant's payload, variant into the name
    // that comes round again — and stopping at the first would miss it.
    let mut carries: HashSet<LocalId> = HashSet::from([into]);
    for st in rest {
        match &st.kind {
            MirStmtKind::Store { addr, value: stored, .. } => {
                if uses::operand_local(stored).is_some_and(|v| carries.contains(&v)) {
                    carries.insert(*addr);
                }
            }
            MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                if carries.contains(src) {
                    carries.insert(*dst);
                }
            }
            _ => {}
        }
    }
    // It has to end up inside one of the group's own names, or its release is
    // somebody else's business and this really was a hand-over.
    if !carries.iter().any(|l| group_of.get(l) == Some(gi)) {
        return false;
    }
    let successor_names = carries;

    // Taken over on the spot.
    let reassigned = rest.iter().any(|st| match &st.kind {
        MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
            *dst == value && successor_names.contains(src)
        }
        _ => false,
    });
    if reassigned {
        return true;
    }

    // Or round the loop: the phi that names `value` takes its operand on this
    // block's edge from one of them.
    func.blocks.iter().any(|b| {
        b.statements.iter().any(|st| match &st.kind {
            MirStmtKind::Phi { dst, args } => {
                *dst == value
                    && args.iter().any(|(from, op)| {
                        *from == block.id
                            && uses::operand_local(op)
                                .is_some_and(|l| successor_names.contains(&l))
                    })
            }
            _ => false,
        })
    })
}

/// Every block a group's value might already be gone in: the blocks where it
/// was handed over, and everything reachable from them.
fn blocks_past_a_handover(func: &MirFunction, sites: &HashSet<BlockId>) -> HashSet<BlockId> {
    let mut out: HashSet<BlockId> = sites.clone();
    let mut frontier: Vec<BlockId> = sites.iter().copied().collect();
    while let Some(bid) = frontier.pop() {
        let Some(block) = func.blocks.iter().find(|b| b.id == bid) else { continue };
        for succ in cfg::successors(&block.terminator) {
            if out.insert(succ) {
                frontier.push(succ);
            }
        }
    }
    out
}

/// Group the aggregate locals that name one value.
///
/// Three things put two names on the same bytes: an SSA copy (`b = a`), a phi,
/// and a payload read out of a wrapper (`v = r.0`, which doesn't copy the
/// strings — it points at where they already are). All three go in one group,
/// so the value is released once and not before the last of its names is done.
fn aggregate_value_groups(
    func: &MirFunction,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> Vec<HashSet<LocalId>> {
    let mut parent: HashMap<LocalId, LocalId> = HashMap::new();

    fn find(parent: &mut HashMap<LocalId, LocalId>, x: LocalId) -> LocalId {
        let p = *parent.get(&x).unwrap_or(&x);
        if p == x {
            return x;
        }
        let root = find(parent, p);
        parent.insert(x, root);
        root
    }

    fn union(parent: &mut HashMap<LocalId, LocalId>, a: LocalId, b: LocalId) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if aggregates.contains(dst) && aggregates.contains(src) =>
                {
                    union(&mut parent, *dst, *src);
                }
                // A payload read: only when the payload is itself an aggregate.
                // A *string* read out of one takes its own reference and is
                // released on its own, so it isn't part of this.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } }
                    if aggregates.contains(dst)
                        && ty_of.get(dst).is_some_and(aggregate_may_hold_string) =>
                {
                    if let Some(base) = uses::operand_local(base) {
                        if aggregates.contains(&base) {
                            union(&mut parent, *dst, base);
                        }
                    }
                }
                MirStmtKind::Phi { dst, args } if aggregates.contains(dst) => {
                    for (_, arg) in args {
                        if let MirOperand::Local(src) = arg {
                            if aggregates.contains(src) {
                                union(&mut parent, *dst, *src);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut groups: HashMap<LocalId, HashSet<LocalId>> = HashMap::new();
    for local in aggregates {
        let root = find(&mut parent, *local);
        groups.entry(root).or_default().insert(*local);
    }
    groups.into_values().collect()
}

/// Aggregate field reads: the local a nested aggregate was read out of.
///
/// Only aggregate-to-aggregate, which is the same test the grouping uses — a
/// string read out of a struct takes its own reference and is released on its
/// own.
fn enclosing_aggregates(func: &MirFunction) -> HashMap<LocalId, LocalId> {
    let ty_of: HashMap<LocalId, &MirType> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| (l.id, &l.ty))
        .collect();
    let mut out = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } = &stmt.kind else {
            continue;
        };
        let (Some(base), Some(dst_ty)) = (uses::operand_local(base), ty_of.get(dst)) else {
            continue;
        };
        if dst_ty.passed_by_address() && ty_of.get(&base).is_some_and(|t| t.passed_by_address()) {
            out.insert(*dst, base);
        }
    }
    out
}

/// Does a release for this group already exist in the function?
fn already_released(func: &MirFunction, group: &HashSet<LocalId>) -> bool {
    func.blocks.iter().flat_map(|b| b.statements.iter()).any(|stmt| {
        matches!(&stmt.kind, MirStmtKind::RcDecContents { local } if group.contains(local))
    })
}

/// Which groups are still live at each block's exit.
///
/// The shared liveness analysis is no use here. An aggregate local with its own
/// storage is never *defined* by a statement — it's written through, field by
/// field — so nothing ever kills it and it reads as live from function entry to
/// the last block. Every group came out live at every exit and the pass emitted
/// nothing at all.
///
/// Writing into an aggregate is what starts its life, so a store counts as a
/// definition here. That makes the loop case work: the struct built at the top
/// of the body is dead by the bottom, because the next turn writes it again
/// before reading it.
///
/// Indexed `[block index][group index]`.
/// Per block, per group: live on entry and live on exit.
fn aggregate_liveness(
    func: &MirFunction,
    groups: &[HashSet<LocalId>],
    reaches: &[HashSet<LocalId>],
) -> (Vec<Vec<bool>>, Vec<Vec<bool>>) {
    let n_blocks = func.blocks.len();
    let n_groups = groups.len();
    let index_of: HashMap<BlockId, usize> =
        func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    // Upward-exposed use, and whether the block writes the group at all.
    let mut gen = vec![vec![false; n_groups]; n_blocks];
    let mut kill = vec![vec![false; n_groups]; n_blocks];

    // How big the value each group names is, so "did this block write all of
    // it" has an answer.
    for (bi, block) in func.blocks.iter().enumerate() {
        // Slots this block gives back before writing over them. A store that
        // follows one is a *replacement*, not the end of the value: what was
        // there has just been freed by name, and what lands next is the group's
        // as much as the old one was. Counting it as a kill is what put the
        // release one statement after the literal in `parse_args` (#1198).
        let released_here: HashSet<(LocalId, u32)> = block
            .statements
            .iter()
            .filter_map(|st| match &st.kind {
                MirStmtKind::ReleaseSlot { addr, offset, .. } => Some((*addr, *offset)),
                _ => None,
            })
            .collect();
        for (gi, group) in groups.iter().enumerate() {
            let mut written = false;
            for stmt in &block.statements {
                // A store names the aggregate as its destination address. That
                // is the write, not a use of what was there before — counting
                // it as a read made every group look upward-exposed, so nothing
                // was ever dead and nothing was ever released.
                let stores_into = matches!(
                    &stmt.kind,
                    MirStmtKind::Store { addr, .. } if group.contains(addr)
                );
                let reads = if stores_into {
                    match &stmt.kind {
                        MirStmtKind::Store { value, .. } => uses::operand_local(value)
                            .is_some_and(|v| group.contains(&v)),
                        _ => false,
                    }
                } else {
                    group.iter().any(|l| uses::stmt_reads(stmt, *l))
                        || reaches[gi].iter().any(|l| uses::stmt_reads(stmt, *l))
                };
                if reads && !written {
                    gen[bi][gi] = true;
                }
                let replaced = match &stmt.kind {
                    MirStmtKind::Store { addr, offset, .. } => {
                        released_here.contains(&(*addr, *offset))
                    }
                    _ => false,
                };
                let writes = (stores_into && !replaced && !store_is_narrow(stmt))
                    || uses::stmt_def(stmt).is_some_and(|d| group.contains(&d));
                if writes {
                    written = true;
                    kill[bi][gi] = true;
                }
            }
            if !written && group.iter().any(|l| uses::terminator_reads(&block.terminator, *l)) {
                gen[bi][gi] = true;
            }
        }
    }

    let mut live_in = vec![vec![false; n_groups]; n_blocks];
    let mut live_out = vec![vec![false; n_groups]; n_blocks];
    loop {
        let mut changed = false;
        for bi in 0..n_blocks {
            for gi in 0..n_groups {
                let mut out = false;
                for succ in crate::analysis::cfg::successors(&func.blocks[bi].terminator) {
                    if let Some(si) = index_of.get(&succ) {
                        out |= live_in[*si][gi];
                    }
                }
                if out != live_out[bi][gi] {
                    live_out[bi][gi] = out;
                    changed = true;
                }
                let inn = gen[bi][gi] || (out && !kill[bi][gi]);
                if inn != live_in[bi][gi] {
                    live_in[bi][gi] = inn;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    (live_in, live_out)
}

/// A store too narrow to be replacing anything the release walks.
///
/// The narrowest thing that walk ever frees is a container handle at eight
/// bytes; a string's header is sixteen. So a one-byte store is a flag being
/// written and the strings and containers beside it are exactly where they
/// were — `opts.ignore_case = true` is not the end of the six-field struct
/// around it, and counting it as one put the release inside the loop that
/// writes the flags (#1198).
///
/// Asked of the store's own recorded width rather than the operand's type,
/// because that width is what codegen copies and a `None` there means "as wide
/// as the value", which is not a claim this can act on.
fn store_is_narrow(stmt: &MirStmt) -> bool {
    matches!(&stmt.kind, MirStmtKind::Store { store_size: Some(n), .. } if *n < 8)
}

/// Shapes that can hold a string somewhere inside them. The layouts that would
/// settle it live in codegen, so this only rules out what it can.
fn aggregate_may_hold_string(ty: &MirType) -> bool {
    match ty {
        MirType::Struct(_) | MirType::Enum(_) => true,
        MirType::Tuple(elems) => elems.iter().any(slot_is_releasable),
        MirType::Array { elem, .. } => slot_is_releasable(elem),
        MirType::Option(inner) => slot_is_releasable(inner),
        MirType::Result { ok, err } => slot_is_releasable(ok) || slot_is_releasable(err),
        _ => false,
    }
}

/// Is there something to give back in a slot of this type?
///
/// A `Heap<T>` slot is: storing a block in an aggregate moves it in
/// (mem.heap/HP4), so the aggregate gives it back, and a tuple of them had no
/// release emitted at all. A *bare* `Heap` local is not — that one is the
/// obligation itself and `drop` is what discharges it. Releasing it here as
/// well freed an `own` closure's captured block a second time.
fn slot_is_releasable(ty: &MirType) -> bool {
    *ty == MirType::String
        || matches!(ty, MirType::Heap(_))
        // A closure in a slot is the same case as a block in one. A *bare*
        // closure local isn't — `ClosureDrop` discharges that one, and the
        // escape analysis has already said the frame no longer owns a closure
        // it stored into an aggregate, so this is the only release it gets
        // (#1253).
        || matches!(ty, MirType::FuncPtr(_))
        || aggregate_may_hold_string(ty)
}

/// Take a reference before handing a borrowed parameter back to the caller.
///
/// The caller keeps its own and releases it at its own last use, so `return s`
/// on a `s: string` parameter would give the caller a second name for a buffer
/// with one reference — `let b = id(a)` then frees it twice. Anything else
/// returned is a value this function owns, and returning it moves that
/// ownership out, which is why `insert_rc_dec` skips the release there.
fn retain_returned_params(func: &mut MirFunction, string_locals: &[LocalId]) {
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let strings: HashSet<LocalId> = string_locals.iter().copied().collect();

    for block in &mut func.blocks {
        let returned = match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => *id,
            _ => continue,
        };
        if !params.contains(&returned) || !strings.contains(&returned) {
            continue;
        }
        let span = block.terminator.span;
        block.statements.push(MirStmt::new(MirStmtKind::RcInc { local: returned }, span));
    }
}

/// Release each string where it stops being live.
///
/// A string holds one reference, and the reference is given back at the
/// point its value dies. There are two kinds of place that can be:
///
/// - Inside a block, just after the statement that reads it for the last
///   time, or just after the one that makes it when nothing ever reads it.
/// - On an edge. A block with several successors can have a string live on
///   its way out because one successor reads it, while another doesn't: the
///   value dies on the way into that one. A `mut` string reassigned in a loop
///   and not read after it is the shape: live out of the loop header, dead in
///   the block after the loop. So is a string an `assert` would print if it
///   failed, dead on the branch that carries on.
///
/// Both come from one liveness answer, the edge-aware one: a phi reads its
/// operand on the edge it arrives by, and takes over that operand's reference
/// there rather than copying it (`insert_rc_inc` adds no count for a phi). So
/// a value handed to a phi is not dead on that edge, and one the phi doesn't
/// take is.
///
/// This used to be four rules: a last use in a block, a release before a
/// redefinition, one at a loop's back edge, and one on the branch beside an
/// abort. The last three were each a case of dying on an edge, placed
/// somewhere that happened to work for the shapes that found them; two of
/// them fired for the same death in a loop that overwrote a string, and freed
/// it twice.
fn insert_rc_dec(func: &mut MirFunction, string_locals: &[LocalId]) {
    let live = liveness::analyze_phis_on_edges(func);
    // `s as i64` into an unsafe call hands out the address of `s`, and the
    // native callee reads the buffer through it. Counting only the cast as a
    // use released the buffer one statement before the call read it (#1036).
    let aliases = AddrAliases::build(func);
    // A string parameter is borrowed from the caller, which keeps its own
    // reference and releases it at its own last use. Releasing here as well is
    // two releases for one reference; a callee that needs to outlive the call
    // takes its own reference (storing incs, and returning a parameter incs in
    // `retain_returned_params`).
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let locals: Vec<LocalId> = string_locals.iter().copied().filter(|l| !params.contains(l)).collect();

    let mut in_block: Vec<(usize, usize, LocalId)> = Vec::new();
    for (bi, block) in func.blocks.iter().enumerate() {
        for &local in &locals {
            if let Some(at) = dies_in_block(block, local, live.live_at_exit(block.id, local), &aliases) {
                in_block.push((bi, at, local));
            }
        }
    }

    let edges = edge_deaths(func, &live, &locals);

    // Positions descending within a block, so an insertion doesn't move the
    // next one's index.
    in_block.sort_by(|a, b| (a.0, b.1).cmp(&(b.0, a.1)));
    for (bi, at, local) in in_block {
        let span = func.blocks[bi]
            .statements
            .get(at.saturating_sub(1))
            .map(|s| s.span)
            .unwrap_or(func.blocks[bi].terminator.span);
        func.blocks[bi].statements.insert(at, MirStmt::new(MirStmtKind::RcDec { local }, span));
    }
    release_on_edges(func, edges);
}

/// Where in `block` the value of `local` dies, as the index to insert its
/// release at, or `None` when it doesn't die here: it's live on the way out,
/// it's returned, or this block never had it.
///
/// Scanned backwards from the block's exit, where the value is dead unless
/// `live_out`. The first statement met that reads it, or failing that the one
/// that makes it, is where it dies.
fn dies_in_block(block: &MirBlock, local: LocalId, live_out: bool, aliases: &AddrAliases) -> Option<usize> {
    if live_out {
        return None;
    }
    // A returned string is handed to the caller, not dropped. Decrementing it
    // here freed the buffer while the caller still held the only reference —
    // `return json.encode(v)` from a `string or E` function came back with its
    // first eight bytes overwritten by whatever the caller allocated next
    // (#499).
    let returned = matches!(
        &block.terminator.kind,
        MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
            if id == &local
    );
    if returned {
        return None;
    }
    let n = block.statements.len();
    if aliases.terminator_reads(&block.terminator, local) {
        return Some(n);
    }
    for (si, stmt) in block.statements.iter().enumerate().rev() {
        // A phi reads its operand on the incoming edge, not here, and hands
        // the reference over to its own name there.
        let phi = matches!(stmt.kind, MirStmtKind::Phi { .. });
        let reads = !phi && aliases.stmt_reads(stmt, local);
        let defines = uses::stmt_def(stmt) == Some(local);
        if reads && defines {
            // Reads the old value and writes the new one in one statement, so
            // no point after it names the old one. Lowering puts a fresh
            // local between the two, so this doesn't arise.
            return None;
        }
        if reads {
            // Handing out the buffer's address is not the end of the string's
            // usefulness, but nothing after it mentions the string, so the
            // naive spot is directly after — the release runs, the buffer is
            // freed, and the raw address the callee dereferences is dangling.
            // `write_raw` in `stdlib/http.rk` is exactly this shape, which is
            // how the HTTP server answered with eight bytes of allocator
            // free-list where `HTTP/1.1` should be. Hold the reference to the
            // end of the block, so every use of the address it produced is
            // covered.
            if hands_out_the_buffer(stmt, local) {
                return Some(n);
            }
            return Some(after_increments(block, si + 1));
        }
        if defines {
            // Made and never read: dead as soon as it exists.
            return Some(after_increments(block, si + 1));
        }
    }
    None
}

/// Step over the increments the copy pass put at `at`. At `dst = src`, `src`'s
/// last use is the copy itself, so the naive spot is directly between
/// `dst = src` and `RcInc(dst)` — the release runs first, the buffer hits
/// zero, and the increment that was meant to keep it alive touches freed
/// memory.
fn after_increments(block: &MirBlock, mut at: usize) -> usize {
    while at < block.statements.len() && matches!(block.statements[at].kind, MirStmtKind::RcInc { .. }) {
        at += 1;
    }
    at
}

/// Each `(from, to)` edge a string dies on, with the strings.
///
/// Dead on the edge means live on the way out of `from`, because some
/// successor needs it, and not needed by `to`: not live into it, and not an
/// operand its phis take along this edge.
///
/// Only where the local is certainly assigned on the way out of `from`, so it
/// holds a value whichever path got there. A local SSA renamed has one
/// definition that dominates every use; one it left alone can be written in
/// each arm of a branch and read after the join, with no single definition
/// dominating anything, and one read on a path that never wrote it (a match's
/// impossible default arm) must not be released there.
///
/// Not the edges a `CleanupReturn` names. Those run the `ensure` chain on the
/// way out of the function, and a value returned through one flows through
/// them to the caller.
fn edge_deaths(
    func: &MirFunction,
    live: &liveness::LivenessResults,
    locals: &[LocalId],
) -> Vec<(BlockId, BlockId, Vec<LocalId>)> {
    let mut phi_takes: HashMap<(BlockId, BlockId), HashSet<LocalId>> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Phi { args, .. } = &stmt.kind else { continue };
            for (from, op) in args {
                if let Some(id) = uses::operand_local(op) {
                    phi_takes.entry((*from, block.id)).or_default().insert(id);
                }
            }
        }
    }
    let assigned = assigned_on_exit(func, locals);

    let mut out = Vec::new();
    for block in &func.blocks {
        if matches!(block.terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
            continue;
        }
        let mut succs = cfg::successors(&block.terminator);
        succs.sort_by_key(|b| b.0);
        succs.dedup();
        if succs.len() < 2 {
            continue; // one way out: live out and live in are the same set
        }
        for &succ in &succs {
            let taken = phi_takes.get(&(block.id, succ));
            let dying: Vec<LocalId> = locals
                .iter()
                .copied()
                .filter(|&l| live.live_at_exit(block.id, l))
                .filter(|&l| !live.live_at_entry(succ, l) && !taken.is_some_and(|t| t.contains(&l)))
                .filter(|l| assigned.get(&block.id).is_some_and(|a| a.contains(l)))
                .collect();
            if !dying.is_empty() {
                out.push((block.id, succ, dying));
            }
        }
    }
    out
}

/// For each block, the `locals` certainly assigned on the way out of it: on
/// every path from the entry, something wrote them. Forward, and an
/// intersection over predecessors, so a path that skips the write keeps the
/// local out.
fn assigned_on_exit(func: &MirFunction, locals: &[LocalId]) -> HashMap<BlockId, HashSet<LocalId>> {
    let preds = cfg::predecessors(func);
    let everything: HashSet<LocalId> = locals.iter().copied().collect();
    let writes: HashMap<BlockId, HashSet<LocalId>> = func
        .blocks
        .iter()
        .map(|b| {
            let w = b.statements.iter().filter_map(uses::stmt_def).filter(|d| everything.contains(d)).collect();
            (b.id, w)
        })
        .collect();
    // Start from "everything" and shrink, so a loop's back edge doesn't wipe
    // out what the way in established.
    let mut out: HashMap<BlockId, HashSet<LocalId>> =
        func.blocks.iter().map(|b| (b.id, everything.clone())).collect();
    loop {
        let mut changed = false;
        for block in &func.blocks {
            let mut inn: HashSet<LocalId> = if block.id == func.entry_block {
                HashSet::new()
            } else {
                let mut sets = preds.get(&block.id).into_iter().flatten().filter_map(|p| out.get(p));
                match sets.next() {
                    Some(first) => sets.fold(first.clone(), |acc, s| acc.intersection(s).copied().collect()),
                    None => HashSet::new(), // unreachable: nothing is known assigned
                }
            };
            inn.extend(writes[&block.id].iter().copied());
            if out.get(&block.id) != Some(&inn) {
                out.insert(block.id, inn);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// Put each edge's releases on its edge: at the top of the successor when this
/// is the only way into it, and otherwise in a new block between the two, so
/// the release runs on this edge and no other.
fn release_on_edges(func: &mut MirFunction, edges: Vec<(BlockId, BlockId, Vec<LocalId>)>) {
    if edges.is_empty() {
        return;
    }
    let preds = cfg::predecessors(func);
    let mut next_id = func.blocks.iter().map(|b| b.id.0).max().unwrap_or(0) + 1;
    for (from, to, locals) in edges {
        let only_way_in = preds
            .get(&to)
            .is_some_and(|ps| ps.iter().all(|p| *p == from));
        let span = func
            .blocks
            .iter()
            .find(|b| b.id == from)
            .map(|b| b.terminator.span)
            .unwrap_or(crate::Span::new(0, 0));
        let releases: Vec<MirStmt> =
            locals.iter().map(|&local| MirStmt::new(MirStmtKind::RcDec { local }, span)).collect();
        if only_way_in {
            let Some(block) = func.blocks.iter_mut().find(|b| b.id == to) else { continue };
            let at = block.statements.iter().take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. })).count();
            block.statements.splice(at..at, releases);
            continue;
        }
        let between = BlockId(next_id);
        next_id += 1;
        if let Some(block) = func.blocks.iter_mut().find(|b| b.id == from) {
            retarget(&mut block.terminator, to, between);
        }
        if let Some(block) = func.blocks.iter_mut().find(|b| b.id == to) {
            for stmt in &mut block.statements {
                if let MirStmtKind::Phi { args, .. } = &mut stmt.kind {
                    for (pred, _) in args.iter_mut() {
                        if *pred == from {
                            *pred = between;
                        }
                    }
                }
            }
        }
        func.blocks.push(MirBlock {
            id: between,
            statements: releases,
            terminator: MirTerminator::new(MirTerminatorKind::Goto { target: to }, span),
        });
    }
}

/// Point every arm of `term` that goes to `old` at `new`.
fn retarget(term: &mut MirTerminator, old: BlockId, new: BlockId) {
    let swap = |b: &mut BlockId| {
        if *b == old {
            *b = new;
        }
    };
    match &mut term.kind {
        MirTerminatorKind::Goto { target } => swap(target),
        MirTerminatorKind::Branch { then_block, else_block, .. } => {
            swap(then_block);
            swap(else_block);
        }
        MirTerminatorKind::Switch { cases, default, .. } => {
            cases.iter_mut().for_each(|(_, b)| swap(b));
            swap(default);
        }
        MirTerminatorKind::CleanupReturn { cleanup_chain, .. } => cleanup_chain.iter_mut().for_each(swap),
        MirTerminatorKind::Return { .. } | MirTerminatorKind::Unreachable => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockId, FunctionRef, MirBlock, MirConst, MirLocal, MirOperand, MirRValue, MirStmt,
        MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
    };

    fn local(id: u32) -> LocalId { LocalId(id) }

    fn string_local(id: u32, name: &str) -> MirLocal {
        MirLocal { id: local(id), name: Some(name.into()), ty: MirType::String, is_param: false, container: None }
    }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn has_rc_inc(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::RcInc { local } if *local == target))
    }

    fn has_rc_dec(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::RcDec { local } if *local == target))
    }

    fn count_rc_inc(stmts: &[MirStmt]) -> usize {
        stmts.iter().filter(|s| matches!(&s.kind, MirStmtKind::RcInc { .. })).count()
    }

    fn count_rc_dec(stmts: &[MirStmt]) -> usize {
        stmts.iter().filter(|s| matches!(&s.kind, MirStmtKind::RcDec { .. })).count()
    }

    /// A string parameter gets no release at all, and storing it takes a
    /// reference of its own.
    ///
    /// It used to get one, which was already one too many: the caller keeps its
    /// own reference and releases it at its own last use, so a callee that
    /// releases as well is two releases for one reference. (Before that it got
    /// *two*, because the pass walked `params` and `locals` back to back and
    /// visited every parameter twice — #698. That was the visible half of the
    /// same mistake; the elision pass hid the other half by deleting both.)
    ///
    /// `self.last = title` still needs the increment: the field outlives the
    /// call, so it takes a reference the caller isn't going to give up.
    #[test]
    fn string_param_is_borrowed_and_stores_retain() {
        let param = MirLocal {
            id: local(1),
            name: Some("title".into()),
            ty: MirType::String,
            is_param: true,
            container: None,
        };
        let mut f = MirFunction {
            name: "put".to_string(),
            params: vec![param.clone()],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("self".into()), ty: MirType::Ptr, is_param: true, container: None },
                param,
            ],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Store {
                    addr: local(0),
                    offset: 0,
                    value: MirOperand::Local(local(1)),
                    store_size: Some(16),
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        let stmts = &f.blocks[0].statements;
        assert_eq!(count_rc_inc(stmts), 1, "one inc for the store: {stmts:?}");
        assert_eq!(count_rc_dec(stmts), 0, "a parameter is borrowed: {stmts:?}");
    }

    /// `unsafe { native_fn(fd, s as i64) }` — the release belongs after the
    /// call, not after the cast.
    ///
    /// The cast is the last statement that names `s`, so the naive last-use
    /// scan put the release between the cast and the call. The buffer hit zero
    /// and the allocator wrote its free-list link into the first eight bytes,
    /// which the native call then wrote to the socket: every HTTP response
    /// started with eight bytes of garbage (#1036).
    #[test]
    fn release_follows_the_call_that_reads_the_address() {
        let mut f = make_fn(
            vec![
                string_local(0, "s"),
                MirLocal { id: local(1), name: Some("addr".into()), ty: MirType::I64, is_param: false, container: None },
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("build".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Cast {
                            value: MirOperand::Local(local(0)),
                            target_ty: MirType::I64,
                        },
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::extern_c("rask_io_write_string".into()),
                        args: vec![
                            MirOperand::Constant(MirConst::Int(1)),
                            MirOperand::Local(local(1)),
                        ],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());

        let stmts = &f.blocks[0].statements;
        let dec = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::RcDec { local: l } if *l == local(0)))
            .expect("string is released somewhere: {stmts:?}");
        let write = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::Call { func, .. } if func.name == "rask_io_write_string"))
            .unwrap();
        assert!(dec > write, "release must follow the native write: {stmts:?}");
    }

    /// `unsafe { strlen(s.as_ptr()) }` — the method form of the same thing.
    ///
    /// `as_ptr` is a call, not a cast, so the rule above never covered it: the
    /// release landed between taking the address and using it, and libc read a
    /// freed buffer. Most of the time freed memory still holds the same bytes,
    /// which is why this went unseen — `strlen` on a string with a NUL at byte
    /// 3 answered 21 (#1118).
    #[test]
    fn release_follows_the_call_that_reads_a_pointer_from_as_ptr() {
        let mut f = make_fn(
            vec![
                string_local(0, "s"),
                MirLocal { id: local(1), name: Some("p".into()), ty: MirType::Ptr, is_param: false, container: None },
                MirLocal { id: local(2), name: Some("n".into()), ty: MirType::U64, is_param: false, container: None },
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("build".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(1)),
                        func: FunctionRef::internal("string_as_ptr".into()),
                        args: vec![MirOperand::Local(local(0))],
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(2)),
                        func: FunctionRef::extern_c("strlen".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());

        let stmts = &f.blocks[0].statements;
        let dec = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::RcDec { local: l } if *l == local(0)))
            .expect("string is released somewhere");
        let read = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::Call { func, .. } if func.name == "strlen"))
            .unwrap();
        assert!(dec > read, "release must follow the read through the pointer: {stmts:?}");
    }

    /// A string the aborting branch builds is not the surviving branch's to
    /// release.
    ///
    /// The edge release exists for a value whose only remaining reader is a
    /// branch that panics — releasing it on the branch that carries on is the
    /// point. It is wrong when that branch is also where the value is *built*:
    /// on the surviving side the slot was never written, and
    /// `rask_string_free` read an uninitialised header. `combined("42", 2)!`
    /// succeeded and still did it, because the panic branch's
    /// `"parse error: …"` was released on the success branch (#1121).
    #[test]
    fn a_string_built_only_where_it_panics_is_not_released_where_it_does_not() {
        let mut f = make_fn(
            vec![
                MirLocal { id: local(0), name: Some("c".into()), ty: MirType::Bool, is_param: false, container: None },
                string_local(1, "msg"),
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(0)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                },
                // Carries on. Never sees `msg`.
                MirBlock {
                    id: BlockId(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
                // Builds the message and dies.
                MirBlock {
                    id: BlockId(2),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(local(1)),
                            func: FunctionRef::internal("build_message".into()),
                            args: vec![],
                        }),
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::internal("panic_forced_error".into()),
                            args: vec![MirOperand::Local(local(1))],
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Unreachable),
                },
            ],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());

        let surviving = &f.blocks[1].statements;
        assert!(
            !has_rc_dec(surviving, local(1)),
            "the branch that carries on never had the string: {surviving:?}",
        );
    }

    /// A half-built aggregate is not a dead one.
    ///
    /// `insert_aggregate_release` puts the release after the group's last use
    /// in a block, and a `Store` into the aggregate was counted as one — so a
    /// `try` that wraps its error released the outer Result after its tag had
    /// been written and before its payload had. `release_either` read the tag,
    /// took the err branch, and freed a string header made of stack garbage
    /// (#1122).
    #[test]
    fn a_store_into_an_aggregate_is_not_a_place_to_release_it() {
        let mut f = make_fn(
            vec![
                MirLocal {
                    id: local(0),
                    name: Some("r".into()),
                    ty: MirType::Result {
                        ok: Box::new(MirType::String),
                        err: Box::new(MirType::String),
                    },
                    is_param: false,
                    container: None,
                },
                string_local(1, "payload"),
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(local(1)),
                            func: FunctionRef::internal("build".into()),
                            args: vec![],
                        }),
                        // The tag, and nothing else — the payload lands in the
                        // next block.
                        MirStmt::dummy(MirStmtKind::Store {
                            addr: local(0),
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(1)),
                            store_size: None,
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto {
                        target: BlockId(1),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![MirStmt::dummy(MirStmtKind::Store {
                        addr: local(0),
                        offset: 24,
                        value: MirOperand::Local(local(1)),
                        store_size: None,
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(local(0))),
                    }),
                },
            ],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());

        let building = &f.blocks[0].statements;
        assert!(
            !building
                .iter()
                .any(|s| matches!(&s.kind, MirStmtKind::RcDecContents { local: l } if *l == local(0))),
            "nothing may release a Result whose payload isn't written yet: {building:?}",
        );
    }

    #[test]
    fn copy_inserts_rc_inc() {
        // dst = src (both strings) → RcInc on dst
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    // Use dst so it's live somewhere
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("print_string".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        assert!(has_rc_inc(&f.blocks[0].statements, local(1)));
    }

    #[test]
    fn call_result_no_rc_inc() {
        // dst = call(...) returning string → no RcInc (new allocation)
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(local(0)),
                    func: FunctionRef::internal("string_new".into()),
                    args: vec![],
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        assert_eq!(count_rc_inc(&f.blocks[0].statements), 0);
    }

    #[test]
    fn last_use_inserts_rc_dec() {
        // src used, then dead → RcDec
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        // Both src and dst should get RcDec (src after copy, dst after block)
        assert!(has_rc_dec(&f.blocks[0].statements, local(0)));
        assert!(has_rc_dec(&f.blocks[0].statements, local(1)));
    }

    /// At `dst = src` the increment on the copy has to happen before the
    /// release of the original. Placed the other way round, a refcount of one
    /// hits zero, the buffer is freed, and the increment meant to keep it alive
    /// lands on freed memory.
    #[test]
    fn copy_increments_before_it_releases() {
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        let stmts = &f.blocks[0].statements;
        let (src, dst) = (local(0), local(1));
        let inc = stmts.iter().position(|s|
            matches!(&s.kind, MirStmtKind::RcInc { local } if *local == dst));
        let dec = stmts.iter().position(|s|
            matches!(&s.kind, MirStmtKind::RcDec { local } if *local == src));
        assert!(inc.is_some() && dec.is_some(), "expected both ops: {stmts:?}");
        assert!(inc < dec, "inc on the copy must precede the release: {stmts:?}");
    }

    /// A phi reads its argument on the incoming edge, not in the phi's own
    /// block. Treating it as a use there put the release at the top of a loop
    /// header, where it runs on the first iteration — before anything has
    /// written the local it releases.
    #[test]
    fn phi_argument_is_not_a_use_in_the_header() {
        let header = MirBlock {
            id: BlockId(1),
            statements: vec![MirStmt::dummy(MirStmtKind::Phi {
                dst: local(0),
                args: vec![
                    (BlockId(0), MirOperand::Constant(MirConst::String("".into()))),
                    (BlockId(2), MirOperand::Local(local(1))),
                ],
            })],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(2) }),
        };
        let body = MirBlock {
            id: BlockId(2),
            statements: vec![MirStmt::dummy(MirStmtKind::Call {
                dst: Some(local(1)),
                func: FunctionRef::internal("string_new".into()),
                args: vec![],
            })],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
        };
        let entry = MirBlock {
            id: BlockId(0),
            statements: vec![],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
        };
        let mut f = make_fn(
            vec![string_local(0, "carried"), string_local(1, "fresh")],
            vec![entry, header, body],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        let header = f.blocks.iter().find(|b| b.id == BlockId(1)).unwrap();
        assert!(
            !has_rc_dec(&header.statements, local(1)),
            "no release for a phi argument in the header: {:?}", header.statements
        );
    }

    #[test]
    fn no_ops_for_non_string_locals() {
        let mut f = make_fn(
            vec![MirLocal { id: local(0), name: Some("x".into()), ty: MirType::I64, is_param: false, container: None, }],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: local(0),
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(42))),
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashSet::new());
        assert_eq!(count_rc_inc(&f.blocks[0].statements), 0);
        assert_eq!(count_rc_dec(&f.blocks[0].statements), 0);
    }
}
