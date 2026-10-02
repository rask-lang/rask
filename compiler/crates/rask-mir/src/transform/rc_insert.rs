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
use crate::analysis::liveness;
use crate::analysis::ownership;
use crate::analysis::uses;
use crate::{
    MirBlock,
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
/// Where each one dies, and whether it is still this frame's there, is
/// `analysis::ownership`'s answer, per path. This function says what each
/// statement does to the aggregates: makes one, renames one, reads into one,
/// or hands one over. A value is released only where it is certainly ours
/// and certainly finished with; anything this can't see clearly is left
/// alone, because releasing what somebody else still holds is a
/// use-after-free and leaving it is a leak.
fn insert_aggregate_release(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    own: &HashSet<String>,
) {
    let ty_of: HashMap<LocalId, MirType> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| (l.id, l.ty.clone()))
        .collect();
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

    // Closures this frame drops, and boxes it drops.
    //
    // `closures::insert_drops` and `interface_drop` emit a drop
    // only for a closure or box the frame owns, and this pass runs after both,
    // so the drop's presence answers "does the frame outlive it". One the
    // frame drops reads into what it holds and keeps it needed until the drop;
    // one it doesn't can outlive the frame and takes what it holds away.
    //
    // The frame owns a boxed value's contents; the box borrows them
    // (mem.shared-rack-heap, #1144). `InterfaceBox` copies the value
    // *shallowly*, so the box and the frame's own local hold the same
    // container handle, and two boxes of one value hold it twice: a free has
    // to happen exactly once, and the frame is where that can be arranged.
    let boxes_dropped: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::InterfaceDrop { interface_object } => Some(*interface_object),
            _ => None,
        })
        .collect();
    // A closure's drop goes under whichever copy still holds it, and says
    // which create built it.
    let closures_dropped: HashSet<LocalId> = crate::closures::closure_drops_by_create(func)
        .into_iter()
        .map(|(create, _, _)| create)
        .collect();
    // The drop is rarely on the boxing site's own name. Inlining copies the box
    // into the callee's parameter local and the drop lands there, so
    // `describe_one(one)` boxes into `_22` and drops `_32`.
    let mut copied_into: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } =
            &stmt.kind
        {
            copied_into.entry(*src).or_default().push(*dst);
        }
    }
    let reaches_a_drop = |start: LocalId| {
        let mut seen: HashSet<LocalId> = HashSet::new();
        let mut frontier = vec![start];
        while let Some(id) = frontier.pop() {
            if !seen.insert(id) {
                continue;
            }
            if boxes_dropped.contains(&id) {
                return true;
            }
            if let Some(next) = copied_into.get(&id) {
                frontier.extend(next.iter().copied());
            }
        }
        false
    };

    // Names that may come to name an aggregate or read into one: the
    // aggregates, and whatever is copied, read or parked out of them.
    let mut tracked: HashSet<LocalId> = aggregates.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let reached = match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                    tracked.contains(src).then_some(*dst)
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } => {
                    uses::operand_local(base)
                        .filter(|b| tracked.contains(b))
                        .and_then(|b| {
                            (part_of_field(*dst, b, &aggregates, &ty_of)
                                || view_of_field(*dst, b, &aggregates, &ty_of))
                            .then_some(*dst)
                        })
                }
                MirStmtKind::Call { func: fref, args, dst: Some(dst), .. } => {
                    (crate::own_names::returns_a_view(&fref.name, own)
                        && args.first().and_then(uses::operand_local).is_some_and(|r| tracked.contains(&r)))
                    .then_some(*dst)
                }
                MirStmtKind::Store { addr, value, .. } => uses::operand_local(value)
                    .filter(|v| tracked.contains(v) && !aggregates.contains(v) && !aggregates.contains(addr))
                    .map(|_| *addr),
                MirStmtKind::ClosureCreate { dst, .. } | MirStmtKind::InterfaceBox { dst, .. } => {
                    Some(*dst)
                }
                MirStmtKind::Phi { dst, args } => args
                    .iter()
                    .any(|(_, op)| uses::operand_local(op).is_some_and(|l| tracked.contains(&l)))
                    .then_some(*dst),
                _ => None,
            };
            if let Some(d) = reached {
                if tracked.insert(d) {
                    changed = true;
                }
            }
        }
    }

    let params: Vec<LocalId> = func.params.iter().map(|p| p.id).collect();
    let mut facts = ownership::Facts {
        names: tracked.iter().copied().collect(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: params.iter().copied().filter(|p| tracked.contains(p)).collect(),
    };

    for block in &func.blocks {
        // Slots this block gives back before writing over them. A store that
        // follows one is a *replacement*, not the end of the value: what was
        // there has just been freed by name, and what lands next is the value's
        // as much as the old one was (#1198).
        let released_here: HashSet<(LocalId, u32)> = block
            .statements
            .iter()
            .filter_map(|st| match &st.kind {
                MirStmtKind::ReleaseSlot { addr, offset, .. } => Some((*addr, *offset)),
                _ => None,
            })
            .collect();
        let mut events = Vec::with_capacity(block.statements.len());
        let mut reads = Vec::with_capacity(block.statements.len());
        let mut kills = Vec::with_capacity(block.statements.len());
        for stmt in &block.statements {
            let mut ev: Vec<ownership::Event> = Vec::new();
            let is_tracked = |l: &LocalId| tracked.contains(l);
            let hand_over = |ev: &mut Vec<ownership::Event>, l: LocalId| {
                if tracked.contains(&l) {
                    ev.push(ownership::Event::HandOver(l));
                }
            };
            match &stmt.kind {
                MirStmtKind::Phi { .. } => {}
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if is_tracked(dst) =>
                {
                    if is_tracked(src) {
                        ev.push(ownership::Event::Alias { dst: *dst, src: *src });
                    } else if aggregates.contains(dst) {
                        // Copied in from a local this pass can't see into: a
                        // wrapper lowering didn't mark as holding a container
                        // (`_19 = _40` for `maybe_words(1)? as words`). Taken
                        // over here, as anything an aggregate is built from is.
                        ev.push(ownership::Event::Make(*dst));
                    } else {
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } if is_tracked(dst) => {
                    match uses::operand_local(base).filter(|b| is_tracked(b)) {
                        Some(b) if part_of_field(*dst, b, &aggregates, &ty_of) => {
                            ev.push(ownership::Event::Part { dst: *dst, base: b })
                        }
                        Some(b) if view_of_field(*dst, b, &aggregates, &ty_of) => {
                            ev.push(ownership::Event::View { dst: *dst, base: b })
                        }
                        _ => ev.push(ownership::Event::Other(*dst)),
                    }
                }
                // `Ref` hands out the address.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } => {
                    hand_over(&mut ev, *src);
                    if is_tracked(dst) {
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                MirStmtKind::Call { func: fref, args, dst } => {
                    let borrows_recv = rask_stdlib::mir_metadata::borrows_receiver(&fref.name);
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if !is_tracked(&id) {
                            continue;
                        }
                        // `h.items[0]` is `Vec_index(items, 0)`: the receiver
                        // is borrowed, so the call keeps nothing. Only for a
                        // handle read out of an aggregate. A *struct* reaching
                        // a call is one whose fields might now be somebody
                        // else's, whatever the callee does with argument zero.
                        if i == 0 && borrows_recv && !aggregates.contains(&id) {
                            continue;
                        }
                        // Giving back what a field held, right before the field
                        // holds something else. Argument zero is the handle
                        // that was in the slot; the aggregate is untouched
                        // (#1198).
                        if i == 0 && rask_stdlib::mir_metadata::frees_a_replaced_slot(&fref.name) {
                            continue;
                        }
                        // A callee whose body this pass can read, and which
                        // demonstrably doesn't hold on to the aggregate, leaves
                        // it to this frame. Both sides refusing is how a `take
                        // self` struct's `Vec` came to be freed by nobody
                        // (`os.Command.spawn`). Only for a callee in `kept`: a
                        // runtime helper has no body to read.
                        if kept.get(&fref.name).is_some_and(|v| !v.get(i).copied().unwrap_or(true)) {
                            // It may still write into it: a `mutate`
                            // parameter is the caller's slot, by address.
                            if aggregates.contains(&id) {
                                ev.push(ownership::Event::WriteThrough(id));
                            }
                            continue;
                        }
                        // A runtime helper whose line in `INTERNAL_SPELLINGS`
                        // says outright that it keeps none of what it is
                        // handed. `Link_register_struct(h)` is the reason: the
                        // whole struct goes to the runtime so a rack can find
                        // its link fields.
                        // A runtime helper whose line in `INTERNAL_SPELLINGS`
                        // says outright that it keeps none of what it is
                        // handed. `Link_register_struct(h)` is the reason: the
                        // whole struct goes to the runtime so a rack can find
                        // its link fields.
                        //
                        // And a struct handed to a stdlib method that declares
                        // the parameter borrowed: `m.get(k)` with a struct key
                        // only reads it, and handing it over left the key's
                        // strings to nobody (#1394). Declared methods only — a
                        // runtime helper's silence isn't a promise.
                        if rask_stdlib::mir_metadata::keeps_no_arguments(&fref.name)
                            || (aggregates.contains(&id)
                                && rask_stdlib::mir_metadata::borrows_argument(&fref.name, i))
                        {
                            if aggregates.contains(&id) {
                                ev.push(ownership::Event::WriteThrough(id));
                            }
                            continue;
                        }
                        ev.push(ownership::Event::HandOver(id));
                    }
                    if let Some(dst) = dst.filter(|d| is_tracked(d)) {
                        // A call gives up what it returns, unless it hands back
                        // a view into storage its receiver keeps, the way
                        // `v.get(i)` points into the vector's own buffer.
                        if crate::own_names::returns_a_view(&fref.name, own) {
                            match args.first().and_then(uses::operand_local).filter(|r| is_tracked(r)) {
                                Some(recv) => ev.push(ownership::Event::View { dst, base: recv }),
                                None => ev.push(ownership::Event::Other(dst)),
                            }
                        } else if aggregates.contains(&dst) {
                            ev.push(ownership::Event::Make(dst));
                        } else {
                            ev.push(ownership::Event::Other(dst));
                        }
                    }
                }
                MirStmtKind::Store { addr, offset, value, .. } => {
                    if let Some(v) = uses::operand_local(value).filter(|v| is_tracked(v)) {
                        if !aggregates.contains(&v) && !aggregates.contains(addr) {
                            // A handle parked in a buffer so a call can point
                            // at it: whoever reads the buffer reads through the
                            // aggregate. `json.encode(p)` on a `struct { counts:
                            // Map }` hands the encoder the field's handle this
                            // way.
                            ev.push(ownership::Event::ViewAlso { dst: *addr, base: v });
                        } else {
                            // Copied whole into memory: whatever the
                            // destination is, it holds the value now.
                            ev.push(ownership::Event::HandOver(v));
                        }
                    }
                    if aggregates.contains(addr)
                        && !released_here.contains(&(*addr, *offset))
                        && !store_is_narrow(stmt)
                    {
                        ev.push(ownership::Event::Fill(*addr));
                    }
                }
                MirStmtKind::ArrayStore { value, .. } => {
                    if let Some(v) = uses::operand_local(value) {
                        hand_over(&mut ev, v);
                    }
                }
                MirStmtKind::InterfaceBox { dst, value, .. } => {
                    let v = uses::operand_local(value).filter(|v| is_tracked(v));
                    if reaches_a_drop(*dst) {
                        match v {
                            Some(v) => ev.push(ownership::Event::View { dst: *dst, base: v }),
                            None => ev.push(ownership::Event::Other(*dst)),
                        }
                    } else {
                        if let Some(v) = v {
                            hand_over(&mut ev, v);
                        }
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                MirStmtKind::ClosureCreate { dst, captures, heap, .. } => {
                    let caps: Vec<LocalId> =
                        captures.iter().map(|c| c.local_id).filter(|c| is_tracked(c)).collect();
                    if *heap && closures_dropped.contains(dst) {
                        if caps.is_empty() {
                            ev.push(ownership::Event::Other(*dst));
                        }
                        for c in caps {
                            ev.push(ownership::Event::View { dst: *dst, base: c });
                        }
                    } else {
                        for c in caps {
                            hand_over(&mut ev, c);
                        }
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                // Released already, by whoever lowered it: `drop(p)` on a
                // `Heap<T>` releases the payload's contents before giving the
                // block back.
                MirStmtKind::RcDecContents { local } => hand_over(&mut ev, *local),
                _ => {
                    if let Some(d) = uses::stmt_def(stmt).filter(|d| is_tracked(d)) {
                        ev.push(ownership::Event::Other(d));
                    }
                }
            }

            // Reads and writes, for liveness.
            let store_into = match &stmt.kind {
                MirStmtKind::Store { addr, value, .. } => Some((*addr, value)),
                _ => None,
            };
            let mut r = Vec::new();
            let mut k = Vec::new();
            if !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                for name in &facts.names {
                    let reads = match store_into {
                        Some((addr, value)) if addr == *name => {
                            uses::operand_local(value) == Some(*name)
                        }
                        _ => uses::stmt_reads(stmt, *name),
                    };
                    if reads {
                        r.push(*name);
                    }
                    // A store into a scratch slot writes it; one into an
                    // aggregate is a `Fill`, which the analysis decides.
                    let writes = match store_into {
                        Some((addr, _)) => addr == *name && !aggregates.contains(name),
                        None => uses::stmt_def(stmt) == Some(*name),
                    };
                    if writes {
                        k.push(*name);
                    }
                }
            }
            events.push(ev);
            reads.push(r);
            kills.push(k);
        }
        facts.events.push(events);
        facts.reads.push(reads);
        facts.kills.push(kills);
        let mut term_ev = Vec::new();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } =
            &block.terminator.kind
        {
            if tracked.contains(id) {
                term_ev.push(ownership::Event::HandOver(*id));
            }
        }
        facts.terminator_events.push(term_ev);
        facts.terminator_reads.push(
            facts
                .names
                .iter()
                .copied()
                .filter(|n| uses::terminator_reads(&block.terminator, *n))
                .collect(),
        );
    }

    let plan = ownership::plan(func, &facts, ownership::Placement::LastUse);

    // Insert back to front so earlier indices stay put. Step over the retains
    // already sitting at the spot: the last use of a wrapper is usually the
    // read that pulls its payload out, and the retain on that payload is the
    // next statement — releasing first frees the buffer the retain is about to
    // touch.
    let mut by_block: HashMap<usize, Vec<(usize, LocalId)>> = HashMap::new();
    let mut on_edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan {
        match r {
            ownership::Release::At { block, at, name, .. } => {
                let stmts = &func.blocks[block].statements;
                let mut at = at;
                while at < stmts.len() && matches!(stmts[at].kind, MirStmtKind::RcInc { .. }) {
                    at += 1;
                }
                by_block.entry(block).or_default().push((at, name));
            }
            ownership::Release::OnEdge { from, to, name, .. } => {
                let span = func
                    .blocks
                    .iter()
                    .find(|b| b.id == from)
                    .map(|b| b.terminator.span)
                    .unwrap_or(crate::Span::new(0, 0));
                on_edges.push((from, to, vec![MirStmt::new(MirStmtKind::RcDecContents { local: name }, span)]));
            }
        }
    }
    for (bi, mut list) in by_block {
        list.sort_by(|a, b| b.0.cmp(&a.0).then(b.1 .0.cmp(&a.1 .0)));
        for (at, name) in list {
            let block = &mut func.blocks[bi];
            let span = block
                .statements
                .get(at.saturating_sub(1))
                .map(|s| s.span)
                .unwrap_or(block.terminator.span);
            block.statements.insert(at, MirStmt::new(MirStmtKind::RcDecContents { local: name }, span));
        }
    }
    // After the in-block ones: those index into the blocks as they were.
    ownership::insert_on_edges(func, on_edges);
}

/// Whether reading a field of `base` into `dst` gives a part of what `base`
/// holds, so that handing `dst` on hands the whole on.
///
/// - A payload that is itself an aggregate names the same bytes: `v = r.0`
///   doesn't copy the strings, it points at where they already are.
/// - A container handle or a `Heap` block out of an aggregate is the
///   container the aggregate owns. `*h.inner` read after the release read
///   freed memory (#1256).
fn part_of_field(
    dst: LocalId,
    base: LocalId,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> bool {
    if !aggregates.contains(&base) {
        return false;
    }
    match ty_of.get(&dst) {
        Some(t) if aggregates.contains(&dst) && aggregate_may_hold_string(t) => true,
        Some(MirType::Ptr) | Some(MirType::Heap(_)) => true,
        _ => false,
    }
}

/// Whether reading a field of `base` into `dst` reaches into `base`'s storage
/// without being a part of it, rather than copying a scalar out of it.
///
/// - A wrapper or an interface object read off one carries a handle with it:
///   `h.v!` on a `Vec<i64>?` field reaches the vector through it.
/// - Anything read off something that already reads into an aggregate is
///   still reading into it, whatever MIR types it as: `h.nested.get(0)? as
///   first` read `first.len()` after the release had freed `h` and gave
///   5775375445721207872.
///
/// A plain scalar is none of these, and admitting one holds the release back
/// to that scalar's last use for nothing.
fn view_of_field(
    dst: LocalId,
    base: LocalId,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> bool {
    if !aggregates.contains(&base) {
        return true;
    }
    matches!(
        ty_of.get(&dst),
        Some(MirType::Option(_)) | Some(MirType::Result { .. }) | Some(MirType::InterfaceObject { .. })
    )
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
    let edges = edges
        .into_iter()
        .map(|(from, to, locals)| {
            let span = func
                .blocks
                .iter()
                .find(|b| b.id == from)
                .map(|b| b.terminator.span)
                .unwrap_or(crate::Span::new(0, 0));
            let releases =
                locals.iter().map(|&local| MirStmt::new(MirStmtKind::RcDec { local }, span)).collect();
            (from, to, releases)
        })
        .collect();
    ownership::insert_on_edges(func, edges);
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
