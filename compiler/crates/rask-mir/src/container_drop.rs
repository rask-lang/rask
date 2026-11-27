// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Container drop insertion.
//!
//! `Vec.new()` allocates a `RaskVec` and, on the first push, a data array.
//! Nothing in the pipeline ever freed either (#1027) — a vector built in a
//! loop leaked the handle, the array, and every heap string in it, once per
//! turn. `Map` and `Rack` are the same — and for the rack that sentence stayed
//! aspirational until #1048: its free existed in the runtime and no constructor
//! of its own was on the list this pass reads.
//!
//! Modelled on `interface_drop.rs`, which solves the same problem for interface
//! objects, and shares its rules: track only *fresh* allocations — the
//! destinations of the constructors below, plus whatever a chain of moves or
//! phis carries forward — and drop only what never hands ownership to anything
//! else. A container read back out of a struct field, another container, or a
//! call's return value is somebody else's, and freeing it double-frees.
//!
//! One rule differs, and it has to. A container's own methods take it as their
//! first argument, so the "any call argument escapes" rule of the interface pass
//! would mark every vector escaping the moment anything was pushed onto it, and
//! nothing would ever be dropped. A container-prefixed call borrows its
//! *receiver* — argument zero — and escapes the rest: `v.push(inner)` hands
//! `inner` over and keeps `v`.

use std::collections::{BTreeSet, HashMap, HashSet};
use rask_ast::ty::ParamMode;

use crate::closure_reach::ClosureReach;
use crate::closure_targets::ClosureTargets;
use crate::{
    BlockId, FunctionRef, LocalId, MirFunction, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminatorKind, MirType,
};

/// What frees a value fresh out of a call to `ctor`, or `None` if the call
/// made nothing the caller owns. The call's shape says: a stdlib declaration's
/// gives it (`Vec.new()`, `Random.new()`), and a call lowering makes up states
/// it where it is emitted (`rask_vec_from_static`, a channel's ends).
pub(crate) fn free_for(ctor: &FunctionRef) -> Option<&'static str> {
    ctor.shape.and_then(|s| s.fresh_release())
}

/// The call that gives back what `free` frees. A handle's declared release is
/// a runtime symbol called as itself; anything else is a stdlib name codegen
/// dispatches.
pub(crate) fn free_call(free: &str) -> FunctionRef {
    if rask_stdlib::handles::is_declared_release(free) {
        FunctionRef::runtime(free.to_string())
    } else {
        FunctionRef::releasing(free)
    }
}

/// What puts a reference back on a handle its free gives up.
///
/// Only the handles that have both halves in the runtime. A `Receiver` is
/// single-owner, so it doesn't belong here — and leaving one out leaks where
/// getting it wrong closes a channel under a task still sending into it.
fn retain_for(free: &str) -> Option<&'static str> {
    match free {
        "rask_sender_drop_i64" => Some("Sender_clone"),
        "rask_shared_drop_i64" => Some("rask_shared_clone_i64"),
        "rask_mutex_drop" => Some("rask_mutex_clone"),
        "rask_cell_free" => Some("rask_cell_clone"),
        _ => None,
    }
}

/// `own`-capture a refcounted handle by taking a reference to it (#1139).
///
/// A `Sender<T>` is one word, so `spawn(own || …)` copied the handle into the
/// environment and nothing bumped the channel's sender count: N tasks shared
/// one sender, and whichever glue dropped it first closed the channel under
/// the rest. The conservative answer — don't let a glue own a capture defined
/// outside the loop that builds it — is what `t_channel_spawned_producer.rk`
/// records, and it leaks the handles instead.
///
/// Cloning at the create site is what the program means there: N senders, one
/// per task. It also makes every rule below land right with no special case —
/// the clone is defined where the closure is built, so it is that capture's
/// only capturer, it is made each turn, and `Sender_clone` is a constructor
/// the pass already knows, which makes the clone fresh under its own name. The
/// frame keeps owning the handle it started with, because the environment no
/// longer names it.
///
/// Only where the frame still has the handle afterwards. `own` on the last use
/// is a *move*, and that is what one program in three relies on:
///
/// ```text
/// spawn(own || { for i in 1..n { tx.send(i) }  tx.close() })
/// ```
///
/// Closing an end is dropping it, and the point of moving the only sender in
/// is that closing it there closes the channel. Clone that one and the frame
/// keeps a sender open, the receiver never learns the channel is done, and the
/// program hangs — which is what the first cut of this did to
/// `t_channel_spawned_producer.rk`.
fn retain_handle_captures(func: &mut MirFunction) {
    let mut made: HashMap<LocalId, &'static str> = HashMap::new();
    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let live = crate::analysis::liveness::analyze(func, &dom);
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
            if let Some(free) = free_for(fref) {
                made.insert(*dst, free);
            }
        }
    }
    if made.is_empty() {
        return;
    }
    // A copy of a handle is the same handle. `let tx = tx0.clone()` with
    // `tx0` unused after is a move, which clone elision leaves as a plain
    // copy, so the task captured a name this pass didn't know and got no
    // reference of its own: in a loop it was moved once per turn (#2005).
    let mut grew = true;
    while grew {
        grew = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
                if let Some(free) = made.get(src).copied() {
                    if made.insert(*dst, free).is_none() {
                        grew = true;
                    }
                }
            }
        }
    }
    let mut next_id = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| l.id.0)
        .max()
        .map_or(0, |m| m + 1);

    for bi in 0..func.blocks.len() {
        // (statement index, the clone to run before it, captures to rewrite)
        let mut work: Vec<(usize, Vec<(usize, LocalId, LocalId, &'static str, &'static str)>)> =
            Vec::new();
        for (si, stmt) in func.blocks[bi].statements.iter().enumerate() {
            let MirStmtKind::ClosureCreate { captures, heap: true, .. } = &stmt.kind else {
                continue;
            };
            let mut here = Vec::new();
            for (ci, c) in captures.iter().enumerate() {
                if c.by_ref {
                    continue;
                }
                let Some((free, retain)) =
                    made.get(&c.local_id).and_then(|f| Some((*f, retain_for(f)?)))
                else {
                    continue;
                };
                // Still the frame's after this? Then the environment needs a
                // reference of its own. Otherwise the capture is the last use
                // and `own` is the move it reads as.
                let block = &func.blocks[bi];
                let kept_after = live.live_at_exit(block.id, c.local_id)
                    || block.statements[si + 1..]
                        .iter()
                        .any(|st| crate::analysis::uses::stmt_reads(st, c.local_id))
                    || crate::analysis::uses::terminator_reads(&block.terminator, c.local_id);
                if !kept_after {
                    continue;
                }
                let fresh_id = LocalId(next_id);
                next_id += 1;
                here.push((ci, c.local_id, fresh_id, retain, free));
            }
            if !here.is_empty() {
                work.push((si, here));
            }
        }
        for (si, here) in work.into_iter().rev() {
            let mut clones = Vec::new();
            for (ci, from, to, retain, free) in here {
                let ty = func.local_ty(from).cloned().unwrap_or(MirType::Ptr);
                func.locals.push(crate::MirLocal {
                    id: to,
                    name: None,
                    ty,
                    is_param: false,
                });
                clones.push(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(to),
                    // A new reference, given back the way the original is.
                    func: FunctionRef::borrowing(retain).handing_back(free),
                    args: vec![MirOperand::Local(from)],
                }));
                if let MirStmtKind::ClosureCreate { captures, .. } =
                    &mut func.blocks[bi].statements[si].kind
                {
                    captures[ci].local_id = to;
                }
            }
            for stmt in clones.into_iter().rev() {
                func.blocks[bi].statements.insert(si, stmt);
            }
        }
    }
}

pub fn insert_container_drops(fns: &mut Vec<MirFunction>) {
    // Before anything reads the MIR: an `own` capture of a refcounted handle
    // becomes a capture of a fresh reference to it (#1139).
    for func in fns.iter_mut() {
        retain_handle_captures(func);
    }
    // Which bodies a call through a closure can reach, so the by-name answer
    // below covers those calls too (#943). Built first: it reads only the MIR,
    // and the "hands a container back" fixed point needs it.
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    let reach = ClosureReach::build(fns);
    let kept = params_a_callee_keeps(fns);
    let handing_over = functions_that_hand_a_container_back(fns, &kept, &targets);
    let interface_kept = interface_method_args_kept(fns, &kept);
    let keeps = Keeps { by_name: &kept, interface: &interface_kept, targets: &targets, reach: &reach };
    let interface_handing = interface_methods_that_hand_back(fns, &handing_over);
    // A snapshot, because tracing a container through a capture cell has to
    // read the closure that captured it while the frame it belongs to is being
    // rewritten.
    let snapshot: Vec<MirFunction> = fns.to_vec();
    let closure_slots = closure_cell_writers(&snapshot, &targets);
    for func in fns.iter_mut() {
        insert_for_function(
            func, &snapshot, &handing_over, &interface_handing, &keeps, &targets,
        );
        release_before_body_stores(func, &closure_slots);
    }
    leave_what_bodies_give_away(fns, &kept, &targets, &reach);
}

/// Clear `owns` on each capture its closure body gives away itself, so the
/// environment doesn't release it a second time when it dies.
///
/// A closure's body can only give away a Copy capture (`mem.closures/CM4`),
/// and a channel end is one:
///
/// ```text
/// let f = || { for i in 1..n { tx.send(i) }  tx.close() }
/// ```
///
/// `close` takes the sender away (closing an end is dropping it), so releasing
/// it again on the way out freed it twice. A task block runs once and may give
/// away anything it captured (conc.async/S6).
///
/// Per closure body, so every site that builds one gives the same answer. A
/// body that gives a capture away on only one path still counts: that leaks
/// on the other path rather than freeing twice.
fn leave_what_bodies_give_away(
    fns: &mut [MirFunction],
    kept: &HashMap<String, Vec<bool>>,
    targets: &ClosureTargets,
    reach: &ClosureReach,
) {
    let gone: HashMap<String, HashSet<u32>> = fns
        .iter()
        .map(|f| {
            let mut gone = captures_the_body_consumes(f);
            gone.extend(task_captures_given_away(f, kept, targets, reach));
            (f.name.clone(), gone)
        })
        .filter(|(_, gone)| !gone.is_empty())
        .collect();
    if gone.is_empty() {
        return;
    }
    for stmt in fns.iter_mut().flat_map(|f| f.blocks.iter_mut()).flat_map(|b| b.statements.iter_mut()) {
        let MirStmtKind::ClosureCreate { func_name, captures, .. } = &mut stmt.kind else { continue };
        let Some(offsets) = gone.get(func_name.as_str()) else { continue };
        for c in captures.iter_mut().filter(|c| offsets.contains(&c.offset)) {
            c.owns = None;
        }
    }
}

/// What a frame's capture cell holds, and so how the frame releases it.
enum InCell {
    /// A container handle, freed by value with this function.
    Handle(&'static str),
    /// A value released in place (`ReleaseSlot`): a string, whose 16-byte
    /// header is the slot, or a heap closure.
    Slot(MirType),
}

/// The capture offsets this function's body takes away itself — loaded out of
/// the environment and handed to something declared `take self`.
///
/// Conservative on purpose: a body that consumes a capture on only one path
/// still counts, because the glue runs on every path and freeing twice is
/// worse than not freeing at all.
fn captures_the_body_consumes(func: &MirFunction) -> HashSet<u32> {
    // Which capture each local came from. A capture is loaded once and then
    // copied around, so the copies have to carry the offset with them.
    let mut from_capture: HashMap<LocalId, u32> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            match &stmt.kind {
                MirStmtKind::LoadCapture { dst, offset, .. } => {
                    if from_capture.insert(*dst, *offset).is_none() {
                        changed = true;
                    }
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Use(MirOperand::Local(src)),
                } => {
                    if let Some(&off) = from_capture.get(src) {
                        if from_capture.insert(*dst, off).is_none() {
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { continue };
        if !fref.consumes_receiver() {
            continue;
        }
        let Some(recv) = args.first().and_then(crate::analysis::uses::operand_local) else { continue };
        if let Some(&off) = from_capture.get(&recv) {
            out.insert(off);
        }
    }
    out
}

/// The capture offsets a task block's body gives away: returns, stores, hands
/// to something that keeps it, or carries into a task of its own. A closure
/// value it captured counts whatever it does with it: the body frees that one
/// itself, like any closure it holds.
///
/// A task block runs once, so it owns what it captured (conc.async/S6), and
/// what it gives away is not the environment's to free when the task ends.
/// `local_is_kept_by` answers it for each capture the block loads. A capture
/// given away on one path counts, which leaks on the other path rather than
/// freeing twice.
pub(crate) fn task_captures_given_away(
    func: &MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    targets: &ClosureTargets,
    reach: &ClosureReach,
) -> HashSet<u32> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            // A closure value the block frees itself (`closures::insert_drops`).
            MirStmtKind::LoadCapture { dst, offset, access: crate::CaptureAccess::Taken, .. }
                if matches!(func.local_ty(*dst), Some(MirType::FuncPtr(_))) =>
            {
                Some(*offset)
            }
            MirStmtKind::LoadCapture { dst, offset, access: crate::CaptureAccess::Taken, .. } => {
                local_is_kept_by(func, *dst, kept, targets, reach).then_some(*offset)
            }
            _ => None,
        })
        .collect()
}

/// The containers a task block captured and gives away on some path, loaded
/// out of its environment, with the free for each.
///
/// The environment leaves those alone when the task ends
/// (`task_captures_given_away`), so the block frees each one on the paths
/// that don't give it away, the way a function frees a `take` parameter it
/// keeps on only one branch. `if flag { out.push(v) }` in a task block leaked
/// `v` whenever `flag` was false (#2043).
fn task_captures_the_body_frees(func: &MirFunction, keeps: &Keeps) -> Vec<(LocalId, &'static str)> {
    let given = task_captures_given_away(func, keeps.by_name, keeps.targets, keeps.reach);
    if given.is_empty() {
        return Vec::new();
    }
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture { dst, offset, access: crate::CaptureAccess::Taken, .. }
                if given.contains(offset) =>
            {
                match func.local_ty(*dst) {
                    Some(MirType::Container(kind)) => Some((*dst, container_free(*kind))),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

/// Give each `for` body over a sequence the item mode its body calls for:
/// Take where it keeps the item, so the sequence hands the item over instead of
/// releasing it after the call (type.sequence/SEQ47).
///
/// Ownership can't say this. It tracks bindings by name, so a body-local
/// that shadows the item read as the item, and `return x`, `keep = x` or a
/// field store keep a value without passing through a consume. Reading the
/// body lowering made sees every one.
///
/// Grows to a fixed point: a body may keep its item by handing it to a call
/// through a closure that reaches another loop body.
pub(crate) fn settle_loop_item_modes(fns: &mut [MirFunction]) {
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    let reach = ClosureReach::build(fns);
    let index: HashMap<String, usize> = fns.iter().enumerate().map(|(i, f)| (f.name.clone(), i)).collect();
    // Loop bodies a yield call nobody can place may reach. That call lends
    // its item and frees it once the body returns, so a body behind it can't
    // take the item: one that keeps it keeps a copy instead
    // (`closures::clone_lent_params_handed_on`). A closure item looks like a
    // sequence's yield to the reach analysis, so `map(keep_i).to_vec()` had
    // `to_vec`'s body take each closure while `map` freed it. Only a closure
    // item: that copy exists for closures alone, and a `Vec` item a body keeps
    // without taking would be pushed as-is while the caller frees it.
    let item_is_closure = |f: &MirFunction| {
        f.params.get(1).is_some_and(|p| matches!(f.local_ty(p.id), Some(crate::MirType::FuncPtr(_))))
    };
    let mut lent_to = vec![false; fns.len()];
    for f in fns.iter() {
        for stmt in f.blocks.iter().flat_map(|b| b.statements.iter()) {
            let MirStmtKind::ClosureCall { closure, args, .. } = &stmt.kind else { continue };
            if args.len() != 1 || targets.known(&f.name, *closure).is_some() {
                continue;
            }
            let bodies: Vec<usize> =
                reach.may_hold(&f.name, *closure).into_iter().filter_map(|n| index.get(n).copied()).collect();
            if !bodies.iter().all(|i| fns[*i].item_mode_from_body) {
                for i in bodies {
                    lent_to[i] |= item_is_closure(&fns[i]);
                }
            }
        }
    }
    loop {
        let kept = params_a_callee_keeps(fns);
        let mut keeps: HashSet<usize> = fns
            .iter()
            .enumerate()
            .filter(|(i, f)| f.item_mode_from_body && f.param_modes.get(1) == Some(&ParamMode::Borrow) && !lent_to[*i])
            .filter(|(_, f)| local_is_kept_by(f, f.params[1].id, &kept, &targets, &reach))
            .map(|(i, _)| i)
            .collect();
        // One yield call hands its item over or lends it, whichever body is
        // behind it, so every body one call may reach has to take the item if
        // any of them does. Two `for` loops over the same sequence share its
        // closure: the inner one kept its item and the outer one only read
        // its own, so the yield handed every item over and nobody freed the
        // outer loop's (#2009). A body taking an item it doesn't keep frees it.
        for f in fns.iter() {
            for stmt in f.blocks.iter().flat_map(|b| b.statements.iter()) {
                let MirStmtKind::ClosureCall { closure, args, .. } = &stmt.kind else { continue };
                if args.len() != 1 {
                    continue;
                }
                let bodies: Vec<usize> = match targets.known(&f.name, *closure) {
                    Some(b) => b.iter().filter_map(|n| index.get(n).copied()).collect(),
                    None => reach.may_hold(&f.name, *closure).into_iter().filter_map(|n| index.get(n).copied()).collect(),
                };
                // Only a call every target of which is a loop body. A closure
                // taken out of a vector may reach every one-argument closure in
                // the program; reading that as a yield made the `for` bodies in
                // it take their items, and then the call itself looked like it
                // kept its argument, so `h(v)` left `v` to nobody.
                if bodies.is_empty() || !bodies.iter().all(|i| fns[*i].item_mode_from_body) {
                    continue;
                }
                let item_takers = |i: &usize| fns[*i].param_modes.get(1) == Some(&ParamMode::Take) || keeps.contains(i);
                if !bodies.iter().any(item_takers) {
                    continue;
                }
                let lenders: Vec<usize> = bodies
                    .iter()
                    .copied()
                    .filter(|i| fns[*i].item_mode_from_body && fns[*i].param_modes.get(1) == Some(&ParamMode::Borrow))
                    .collect();
                keeps.extend(lenders);
            }
        }
        if keeps.is_empty() {
            return;
        }
        for i in keeps {
            fns[i].param_modes[1] = ParamMode::Take;
        }
    }
}

/// Per function, which of its parameters it *keeps* rather than just reads:
/// the ones written `take`.
///
/// Borrow is the default (`mem.parameters/PM1`): `func first(v: Vec<i32>)`
/// reads the vector and the caller still owns it. A borrowed Copy value the
/// callee stores is a copy with references of its own
/// (`rc_insert::retains_what_it_hands_on`), a borrowed non-Copy one can't be
/// given away (PM6), and a `mutate` slot the callee moves out of comes back
/// refilled (PM7), the new value the caller's to release (#1885).
///
/// Every function says how it takes each parameter (`BlockBuilder::add_param`),
/// lowering's own as well, and a loop body's item once `settle_loop_item_modes`
/// has run. This was once worked out from every function's body, grown to a
/// fixed point, though a declared function's mode already said it (PM6).
pub(crate) fn params_a_callee_keeps(fns: &[MirFunction]) -> HashMap<String, Vec<bool>> {
    fns.iter()
        .map(|f| {
            let kept = (0..f.params.len())
                .map(|i| f.param_modes.get(i) == Some(&ParamMode::Take))
                .collect();
            (f.name.clone(), kept)
        })
        .collect()
}

/// Every function a vtable can hold, with the method it answers for.
///
/// A vtable is built per (concrete type, interface) pair some `InterfaceBox`
/// boxes, and its entries are that type's `{concrete}_{method}` functions
/// (`collect_vtables` in rask-compiler). So what can sit behind an interface
/// call is the methods of the types the program boxes.
///
/// Any `Something_method` used to count. A `Chunk.read_bytes` nothing ever
/// boxed then had a say in what `reader.read_bytes()` hands back, and since
/// it didn't hand back a container it made, the `Vec` every real reader
/// returns was nobody's and leaked.
fn vtable_candidates(fns: &[MirFunction]) -> Vec<(&str, &MirFunction)> {
    let boxed: HashSet<&str> = fns
        .iter()
        .flat_map(|f| f.blocks.iter().flat_map(|b| b.statements.iter()))
        .filter_map(|st| match &st.kind {
            MirStmtKind::InterfaceBox { concrete_type, .. } => Some(concrete_type.as_str()),
            _ => None,
        })
        .collect();
    fns.iter()
        .filter_map(|func| {
            let head = func.name.rsplit("::").next().unwrap_or(&func.name);
            let method = boxed
                .iter()
                .filter_map(|ty| head.strip_prefix(*ty)?.strip_prefix('_'))
                .find(|m| !m.is_empty())?;
            Some((method, func))
        })
        .collect()
}

/// Per interface method, which of its arguments an implementation keeps.
///
/// The same question `params_a_callee_keeps` answers, asked of a call that has
/// no function name to ask about — an interface call names a method and a vtable
/// slot. The answer is taken across every function that could be behind that
/// slot (`vtable_candidates`), and a call is a borrow only when all of them
/// borrow. A method with no candidate at all keeps every argument: a leak,
/// where the other way is a double free.
///
/// `self` isn't in an interface call's argument list, so the implementation's
/// parameters are read one to the right.
fn interface_method_args_kept(
    fns: &[MirFunction],
    kept: &HashMap<String, Vec<bool>>,
) -> HashMap<String, Vec<bool>> {
    let mut candidates: HashMap<&str, Vec<&[bool]>> = HashMap::new();
    for (method, func) in vtable_candidates(fns) {
        let Some(flags) = kept.get(&func.name) else { continue };
        if flags.is_empty() {
            continue;
        }
        candidates.entry(method).or_default().push(&flags[1..]);
    }
    candidates
        .into_iter()
        .map(|(method, impls)| {
            let len = impls.iter().map(|f| f.len()).max().unwrap_or(0);
            let mut merged = vec![false; len];
            for flags in &impls {
                for (i, slot) in merged.iter_mut().enumerate() {
                    *slot = *slot || flags.get(i).copied().unwrap_or(true);
                }
            }
            (method.to_string(), merged)
        })
        .collect()
}

/// Per interface method, how an implementation hands a container back.
///
/// Same shape as the closure answer below it: the method is only a
/// constructor when every function that could be behind the vtable slot is
/// one, with the same free and the same wrapping. One implementation that
/// hands back somebody else's container is the whole set's answer.
///
/// Without it a `Vec<u8>` read out of `reader.read_bytes()` was nobody's —
/// nothing named the callee, so nothing knew the caller owned it (#1199).
fn interface_methods_that_hand_back(
    fns: &[MirFunction],
    handing: &HashMap<String, HandBack>,
) -> HashMap<String, HandBack> {
    let mut per_method: HashMap<&str, Option<HandBack>> = HashMap::new();
    for (method, func) in vtable_candidates(fns) {
        // Only a container the implementation built: a vtable call has no
        // argument list this pass reads the way `pass_edges` needs.
        let mine = handing.get(&func.name).filter(|b| b.made().is_some()).cloned();
        let entry = per_method.entry(method).or_insert(mine.clone());
        let agrees = match (entry.as_ref(), mine.as_ref()) {
            (Some(a), Some(b)) => a.free == b.free && a.wrapped == b.wrapped,
            _ => false,
        };
        if !agrees {
            *entry = None;
        }
    }
    per_method
        .into_iter()
        .filter_map(|(method, back)| back.map(|b| (method.to_string(), b)))
        .collect()
}

/// Does `func` hold on to the value in `param`: returned, stored, captured by
/// a closure that outlives the frame, or handed to a parameter written `take`?
///
/// Asked of the two values that arrive without a declared mode: what a task
/// block loads from its captures, and the item a `for` body over a sequence is
/// handed (`settle_loop_item_modes`). Both are code lowering made, so reading
/// it is reading the declaration.
fn local_is_kept_by(
    func: &MirFunction,
    param: LocalId,
    kept: &HashMap<String, Vec<bool>>,
    targets: &ClosureTargets,
    reach: &ClosureReach,
) -> bool {
    // Follow the value through renames: lowering copies a parameter into a
    // local before doing anything with it often enough that reading only the
    // parameter's own id saw nothing.
    //
    // Not into a copy that took references of its own: that one is a new
    // value, and handing it on hands on its references, not the parameter.
    // `return p` on a Copy aggregate parameter returns such a copy (#1447).
    let owns_copy: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::RcIncContents { local, for_hand_off: false } => Some(*local),
            _ => None,
        })
        .collect();
    // Which names hold the parameter at each point. A name stops holding it
    // where a whole new value is made in its slot: `mut x = first` then
    // `x = second`, with `x` reassigned in place because the rack records its
    // links, is `first` until the write and the frame's own value after it,
    // so `return x` hands back the new value and `first` is still the
    // caller's to free.
    let preds = crate::analysis::cfg::predecessors(func);
    let step = |names: &mut HashSet<LocalId>, stmt: &MirStmt| match &stmt.kind {
        MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
            if names.contains(src) && !owns_copy.contains(dst) =>
        {
            names.insert(*dst);
        }
        MirStmtKind::Phi { dst, args } => {
            if args.iter().any(|(_, op)| matches!(op, MirOperand::Local(s) if names.contains(s))) {
                names.insert(*dst);
            }
        }
        MirStmtKind::ZeroAggregate { local } if *local != param => {
            names.remove(local);
        }
        // A parameter a closure captures by reference lives in a memory cell,
        // and the body reads it back with `x = *param`. That read is the
        // parameter's value until something new is written into the cell.
        // Missed, `for v in s { let g = || v.len(); if c { keep = v } }`
        // stored the item without the sequence knowing, so it freed it after
        // the call (#2008).
        MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(src)) }
            if *src == param && names.contains(&param) && func.local_ty(*dst) == func.local_ty(param) =>
        {
            names.insert(*dst);
        }
        MirStmtKind::Store { addr, offset: 0, .. } if *addr == param => {
            names.remove(&param);
        }
        _ => {}
    };
    let mut entry: HashMap<BlockId, HashSet<LocalId>> = HashMap::new();
    let mut exit: HashMap<BlockId, HashSet<LocalId>> = HashMap::new();
    loop {
        let mut changed = false;
        for (bi, block) in func.blocks.iter().enumerate() {
            let mut names: HashSet<LocalId> = if bi == 0 { HashSet::from([param]) } else { HashSet::new() };
            for p in preds.get(&block.id).into_iter().flatten() {
                if let Some(out) = exit.get(p) {
                    names.extend(out.iter().copied());
                }
            }
            if entry.get(&block.id) != Some(&names) {
                entry.insert(block.id, names.clone());
                changed = true;
            }
            for stmt in &block.statements {
                step(&mut names, stmt);
            }
            if exit.get(&block.id) != Some(&names) {
                exit.insert(block.id, names);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for block in &func.blocks {
        let mut names = entry.get(&block.id).cloned().unwrap_or_default();
        for stmt in &block.statements {
            let holds = |op: &MirOperand| matches!(op, MirOperand::Local(id) if names.contains(id));
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if holds(arg) && call_keeps_argument(fref, i, kept) {
                            return true;
                        }
                    }
                }
                MirStmtKind::ClosureCall { closure, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if holds(arg)
                            && closure_call_keeps_argument(targets, reach, kept, &func.name, *closure, i, args.len())
                        {
                            return true;
                        }
                    }
                }
                MirStmtKind::InterfaceCall { args, .. } => {
                    if args.iter().any(holds) {
                        return true;
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::InterfaceBox { value, .. } => {
                    if holds(value) {
                        return true;
                    }
                }
                // Carried in by value: the environment owns it now (CM2). A
                // lent capture is held for the caller (CM3), and one by
                // reference points at the frame's variable.
                MirStmtKind::ClosureCreate { captures, .. } => {
                    if captures.iter().any(|c| names.contains(&c.local_id) && !c.lent && !c.by_ref) {
                        return true;
                    }
                }
                _ => {}
            }
            step(&mut names, stmt);
        }
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(op) }
            | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
                if matches!(op, MirOperand::Local(id) if names.contains(id)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}


/// Does this call keep the argument at `index`?
///
/// A function this pass can see answers for itself. Anything else — a runtime
/// function, a stdlib method — falls back to the declared metadata, whose own
/// unmapped default leans to leaking rather than to a double free.
pub(crate) fn call_keeps_argument(
    fref: &FunctionRef,
    index: usize,
    kept: &HashMap<String, Vec<bool>>,
) -> bool {
    if let Some(v) = kept.get(&fref.name) {
        return v.get(index).copied().unwrap_or(true);
    }
    fref.gives(index)
}

/// Does `closure(args)` in `func` keep the argument at `index`?
///
/// A closure call has no name to look up, but `ClosureTargets` says which
/// bodies it reaches, and each of those answers for itself. The call borrows
/// only when every one of them does. Argument `i` is the body's parameter
/// `i + 1`, after the environment.
///
/// Reading every closure call as a hand-over left the argument to nobody: the
/// caller stopped owing it and the body, which only borrows, never freed it
/// (#1468).
///
/// When `ClosureTargets` can't say which bodies — a closure loaded out of a
/// `Vec` — the call asks every body `ClosureReach` says it might hold, the way
/// an interface call asks every implementation. Answering "kept" there instead
/// leaked the argument (#1469).
pub(crate) fn closure_call_keeps_argument(
    targets: &ClosureTargets,
    reach: &ClosureReach,
    kept: &HashMap<String, Vec<bool>>,
    func: &str,
    closure: LocalId,
    index: usize,
    arg_count: usize,
) -> bool {
    let body_keeps = |b: &str| kept.get(b).and_then(|v| v.get(index + 1)).copied().unwrap_or(true);
    match targets.known(func, closure) {
        Some(bodies) => bodies.iter().any(|b| body_keeps(b)),
        None => {
            // A body taking a different number of arguments can't be behind
            // it; one this pass has no MIR for could be anything.
            let fits = |b: &&str| kept.get(*b).map_or(true, |v| v.len() == arg_count + 1);
            let bodies: Vec<&str> = reach.may_hold(func, closure).into_iter().filter(fits).collect();
            // Nothing at all could be behind it: nothing is known either.
            bodies.is_empty() || bodies.into_iter().any(body_keeps)
        }
    }
}

/// What a call does with each argument, for every kind of call: by name, through
/// a vtable slot, or through a closure.
struct Keeps<'a> {
    by_name: &'a HashMap<String, Vec<bool>>,
    interface: &'a HashMap<String, Vec<bool>>,
    targets: &'a ClosureTargets,
    reach: &'a ClosureReach,
}

impl Keeps<'_> {
    /// `self` isn't in an interface call's argument list, so this is already
    /// shifted to match it (`interface_method_args_kept`).
    fn interface_call(&self, method: &str, index: usize) -> bool {
        self.interface.get(method).and_then(|v| v.get(index)).copied().unwrap_or(true)
    }

    fn closure_call(&self, func: &str, closure: LocalId, index: usize, arg_count: usize) -> bool {
        closure_call_keeps_argument(self.targets, self.reach, self.by_name, func, closure, index, arg_count)
    }
}

/// Functions whose caller owns what they return.
///
/// That is a function where every returning path hands back either a container
/// it built, bare or inside a wrapper (`-> Vec<i64>?`, `-> Vec<i64> or E`), or
/// one of its own `take` arguments.
///
/// Without this a container only ever belonged to the frame that called the
/// constructor. `make_vec()` returning a `Vec<string>` was freed by nobody:
/// the callee saw it escape through the return and left it alone, and the
/// caller saw a call result, which is somebody else's by default. Nine
/// allocations a call, silently.
///
/// A `take` argument handed back is the caller's for the same reason: the
/// caller gave it up at the call, and nobody else holds it. Asking for every
/// path to be the *same* argument, or every path to be fresh, left the result
/// of `if c { return Vec.new() }  return p` to nobody (#1502, #1504), and so did
/// returning `a` on one path and `b` on another (#1505). The caller owns that
/// result exactly when it owned every argument that might come back, which is
/// what `Event::Remake` says at the call.
///
/// A path returning anything else — a field, a map's own vector, a borrowed
/// parameter's contents — means the whole function is left out:
/// `lookup` returns `index.get(word) ?? Vec.new()`, and calling that "the
/// caller's" frees a vector the map still holds.
///
/// It's a fixed point because handing one back is transitive — a wrapper that
/// returns what `make_vec` gave it is handing one back too, and so is
/// `return stash(q)`. Only functions this pass can see count: a container from
/// the runtime (`split`, `map.keys`) still has no owner named here, because
/// reading an element out of one doesn't take a reference — #1035.
fn functions_that_hand_a_container_back(
    fns: &[MirFunction],
    kept: &HashMap<String, Vec<bool>>,
    targets: &crate::closure_targets::ClosureTargets,
) -> HashMap<String, HandBack> {
    let mut handing: HashMap<String, HandBack> = HashMap::new();
    loop {
        let mut grew = false;
        for func in fns {
            if handing.contains_key(&func.name) {
                continue;
            }
            if let Some(back) = what_comes_back(func, fns, &handing, kept, targets) {
                handing.insert(func.name.clone(), back);
                grew = true;
            }
        }
        if !grew {
            return handing;
        }
    }
}

/// What one returning path hands back.
enum Path {
    /// A container this frame made, and its free.
    Made(&'static str),
    /// One of the arguments at these indices.
    Args(BTreeSet<usize>),
}

/// `func`'s answer for `functions_that_hand_a_container_back`, given what is
/// already known about its callees.
fn what_comes_back(
    func: &MirFunction,
    fns: &[MirFunction],
    handing: &HashMap<String, HandBack>,
    kept: &HashMap<String, Vec<bool>>,
    targets: &ClosureTargets,
) -> Option<HandBack> {
    // No interface answer yet — it is built from this one, and a half-built
    // map would make the fixed point depend on iteration order. An interface
    // call's result is nobody's here, which is what the whole map said before
    // it existed.
    let fresh = collect_fresh_containers_with(func, fns, handing, &HashMap::new(), targets);
    // A forwarder makes nothing of its own — `return s.to_cstring()` hands its
    // caller the wrapper it was just given, untouched. The answer is the
    // callee's answer.
    let forwarded = forwarded_wrappers(func, handing);
    let edges = pass_edges(func, handing, targets);
    // Which parameters' owners each name has. Asked before `fresh`: what
    // `stash(p)` hands back is this frame's, since a frame owns the `take`
    // parameters it keeps (`params_this_frame_owns`) — but to *this*
    // function's caller it is still the argument it passed, which it may not
    // have owned.
    let param_of = owned_like_params(func, fns, &edges, &fresh);

    let mut paths: Vec<Path> = Vec::new();
    let mut wrapped = false;
    let mut any = false;
    for b in &func.blocks {
        // A function with an `ensure` returns through the cleanup chain.
        // Reading only `Return` left every such function handing nothing back,
        // so `ensure g.detach(); return g.join_all()` gave its caller a vector
        // nobody freed.
        let (MirTerminatorKind::Return { value: Some(v), .. }
        | MirTerminatorKind::CleanupReturn { value: Some(v), .. }) = &b.terminator.kind
        else {
            continue;
        };
        any = true;
        let MirOperand::Local(id) = v else {
            return None;
        };
        // Empty when only containers this frame made reach it: that is `fresh`.
        if let Some(from) = param_of.get(id).filter(|from| !from.is_empty()) {
            paths.push(Path::Args(from.clone()));
            continue;
        }
        if let Some(f) = fresh.get(id) {
            paths.push(Path::Made(f));
            continue;
        }
        if let Some(f) = container_moved_out_of_owned_struct(func, *id, kept) {
            paths.push(Path::Made(f));
            continue;
        }
        if let Some(f) = forwarded.get(id) {
            paths.push(Path::Made(f));
            wrapped = true;
            continue;
        }
        // The container may be *inside* what is returned. `-> Vec<i64>?` and
        // `-> Vec<i64> or E` return the wrapper aggregate, and the vector is a
        // slot in it — so the returned local is never the fresh one, and
        // nobody freed the vector the caller unwrapped and used (#1117).
        //
        // An argument handed back inside a wrapper isn't one of these: the
        // wrapper is `rc_insert`'s to release with what it holds, whoever
        // filled it, and the payload read out of it is a part of that.
        match container_stored_into(func, *id, &fresh) {
            WrapperHoldings::Fresh(f) => {
                paths.push(Path::Made(f));
                wrapped = true;
            }
            // The error path of a `T or E` stores no container at all. That is
            // not a path handing one back, and not a path handing back
            // somebody else's either.
            WrapperHoldings::None => {}
            WrapperHoldings::Foreign => return None,
        }
    }
    if !any {
        return None;
    }
    let mut free: Option<&'static str> = None;
    let mut args: BTreeSet<usize> = BTreeSet::new();
    for p in paths {
        match p {
            Path::Made(f) => free = Some(f),
            Path::Args(from) => args.extend(from),
        }
    }
    // A fresh container needs a free to be named here; an argument handed
    // back is freed the way the caller's argument is.
    match (args.is_empty(), free) {
        (true, None) => None,
        (false, _) if wrapped => None,
        _ => Some(HandBack { free, wrapped, args }),
    }
}

/// The free of a container read out of a field of a struct the frame owns,
/// when `local` is one: a `take` parameter (`return self.items`) or a struct
/// the function built (`let t = Pt(…); return t.items`).
///
/// A `take` argument was given up at the call, and everything in it with it:
/// the field is the callee's to hand on, and what comes back is the caller's
/// whatever it owned before. A struct built here holds what was moved into
/// it. Either way the rest of the struct is the callee's to release
/// (`rc_insert`), which empties the returned field's slot first.
fn container_moved_out_of_owned_struct(
    func: &MirFunction,
    local: LocalId,
    kept: &HashMap<String, Vec<bool>>,
) -> Option<&'static str> {
    if func.is_extern_c {
        return None;
    }
    let defs_of = |l: LocalId| {
        func.blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter(move |st| crate::analysis::uses::stmt_def(st) == Some(l))
    };
    // The one definition of a name, through copies.
    let sole = |mut l: LocalId| -> Option<&MirStmt> {
        for _ in 0..func.locals.len() + func.params.len() {
            let mut defs = defs_of(l);
            let def = defs.next()?;
            if defs.next().is_some() {
                return None;
            }
            match &def.kind {
                MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(src)), .. } => l = *src,
                _ => return Some(def),
            }
        }
        None
    };
    let MirStmtKind::Assign { rvalue: MirRValue::Field { base: MirOperand::Local(root), .. }, dst, .. } =
        &sole(local)?.kind
    else {
        return None;
    };
    let Some(MirType::Container(kind)) = func.local_ty(*dst).cloned() else { return None };
    if !matches!(func.local_ty(*root), Some(MirType::Struct(_))) {
        return None;
    }
    // The struct is a parameter, or a copy of one.
    let mut at = *root;
    for _ in 0..func.locals.len() + func.params.len() {
        if let Some(i) = func.params.iter().position(|p| p.id == at) {
            let taken = kept.get(&func.name).and_then(|f| f.get(i)).copied().unwrap_or(false);
            return taken.then(|| container_free(kind));
        }
        let mut defs = defs_of(at);
        let Some(def) = defs.next() else {
            // Or a struct this frame built field by field: what went into
            // its slots was moved there, so it is the frame's to hand on.
            return built_here(func, at).then(|| container_free(kind));
        };
        if defs.next().is_some() {
            return None;
        }
        match &def.kind {
            MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(src)), .. } => at = *src,
            _ => return None,
        }
    }
    None
}

/// `local` is a struct this frame builds in place: zeroed, then filled by
/// stores, and never assigned whole.
fn built_here(func: &MirFunction, local: LocalId) -> bool {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .any(|st| matches!(&st.kind, MirStmtKind::ZeroAggregate { local: l } if *l == local))
}

/// What a call that may hand back one of its arguments was passed there: per
/// call destination, each such argument's index and the local passed for it.
type Passes = HashMap<LocalId, Vec<(usize, LocalId)>>;

/// The calls in `func` that may hand back one of their arguments.
///
/// What comes back is the caller's exactly when every one of those arguments
/// was, so each frame reads such a call as `Event::Remake`, and follows the
/// container's free from the arguments to the result.
fn pass_edges(func: &MirFunction, handing: &HashMap<String, HandBack>, targets: &ClosureTargets) -> Passes {
    let bare_args = |name: &str| handing.get(name).filter(|b| !b.wrapped).map(|b| &b.args);
    let mut out = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let (dst, indices, args): (LocalId, BTreeSet<usize>, _) = match &stmt.kind {
            MirStmtKind::Call { dst: Some(dst), func: fref, args } => {
                let Some(from) = bare_args(&fref.name).filter(|from| !from.is_empty()) else { continue };
                (*dst, from.clone(), args)
            }
            // Every body the closure can be hands back what its caller owns.
            // Any argument one of them may hand back counts. Argument `i` is
            // the body's parameter `i + 1`, after the environment.
            MirStmtKind::ClosureCall { dst: Some(dst), closure, args, .. } => {
                let Some(bodies) = targets.known(&func.name, *closure) else { continue };
                let mut from = BTreeSet::new();
                let all = !bodies.is_empty()
                    && bodies.iter().all(|b| match bare_args(b) {
                        Some(ps) if !ps.contains(&0) => {
                            from.extend(ps.iter().map(|p| p - 1));
                            true
                        }
                        _ => false,
                    });
                if !all || from.is_empty() {
                    continue;
                }
                (*dst, from, args)
            }
            _ => continue,
        };
        let srcs: Option<Vec<(usize, LocalId)>> = indices
            .into_iter()
            .map(|i| match args.get(i) {
                Some(MirOperand::Local(src)) => Some((i, *src)),
                _ => None,
            })
            .collect();
        if let Some(srcs) = srcs {
            out.insert(dst, srcs);
        }
    }
    out
}

/// Every name whose owner is certainly whoever owned what arrived in some of
/// `func`'s parameters, and which ones: copies, phis whose every input is one,
/// and what a call that may hand one back returned (`pass_edges`). A phi of
/// `a` and `b` is owned like both.
///
/// A container this frame made (`fresh`) counts too, owned like no parameter:
/// it is the caller's once handed back, whatever the caller owned.
///
/// So does a load out of a parameter a closure borrows. The closure makes the
/// parameter memory-resident (`transform::addr_taken`), and `return v` reads
/// it back out of its cell. That read looked like nobody's, so
/// `func f(take v: Vec<T>) -> Vec<T> { let n = || v.len() … return v }` handed
/// back nothing as far as its callers knew, and they never freed the result
/// (#2043). Only while the cell can hold nothing else: this frame never stores
/// into it, and every closure that captured it leaves it the frame's.
///
/// Every definition of the name has to be one of those — lowering assigns a
/// loop variable in more than one place. Not a copy that took references of
/// its own — that is a new value (#1447).
fn owned_like_params(
    func: &MirFunction,
    all: &[MirFunction],
    edges: &Passes,
    fresh: &HashMap<LocalId, &'static str>,
) -> HashMap<LocalId, BTreeSet<usize>> {
    let in_memory = crate::transform::addr_taken::analyze(func).owned;
    let stored: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Store { addr, .. } => Some(*addr),
            _ => None,
        })
        .collect();
    let param_cells: HashSet<LocalId> = func
        .params
        .iter()
        .map(|p| p.id)
        .filter(|id| in_memory.contains(id) && !stored.contains(id))
        .filter(|id| cell_stays_owned_through_closures(func, all, *id))
        .collect();
    let owns_copy: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::RcIncContents { local, for_hand_off: false } => Some(*local),
            _ => None,
        })
        .collect();
    let mut from: HashMap<LocalId, BTreeSet<usize>> =
        fresh.keys().map(|id| (*id, BTreeSet::new())).collect();
    for (i, p) in func.params.iter().enumerate() {
        from.insert(p.id, BTreeSet::from([i]));
    }
    let seeds: HashSet<LocalId> = from.keys().copied().collect();
    // What one definition says `d` is owned like, when it says anything.
    let def_from = |from: &HashMap<LocalId, BTreeSet<usize>>, stmt: &MirStmt, d: LocalId| {
        let union = |srcs: &mut dyn Iterator<Item = Option<LocalId>>| {
            let mut out = BTreeSet::new();
            for s in srcs {
                out.extend(from.get(&s?)?.iter().copied());
            }
            Some(out)
        };
        match &stmt.kind {
            MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(src)), .. } => {
                if owns_copy.contains(&d) {
                    return None;
                }
                from.get(src).cloned()
            }
            MirStmtKind::Assign { rvalue: MirRValue::Deref(MirOperand::Local(cell)), .. }
                if param_cells.contains(cell) =>
            {
                from.get(cell).cloned()
            }
            MirStmtKind::Phi { args, .. } if !args.is_empty() => union(&mut args.iter().map(|(_, op)| match op {
                MirOperand::Local(s) => Some(*s),
                _ => None,
            })),
            MirStmtKind::Phi { .. } => None,
            _ => union(&mut edges.get(&d)?.iter().map(|(_, s)| Some(*s))),
        }
    };
    let defs = || {
        func.blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|stmt| Some((stmt, crate::analysis::uses::stmt_def(stmt)?)))
            .filter(|(_, d)| !seeds.contains(d))
    };
    // Grown on "some definition is one"; the sets only widen, so this ends.
    loop {
        let mut grew = false;
        for (stmt, d) in defs() {
            let Some(more) = def_from(&from, stmt, d) else { continue };
            match from.get_mut(&d) {
                Some(cur) if more.is_subset(cur) => {}
                Some(cur) => {
                    cur.extend(more);
                    grew = true;
                }
                None => {
                    from.insert(d, more);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    // Keep only names where every definition is one.
    loop {
        let doomed: Vec<LocalId> = defs()
            .filter(|(stmt, d)| from.contains_key(d) && def_from(&from, stmt, *d).is_none())
            .map(|(_, d)| d)
            .collect();
        if doomed.is_empty() {
            return from;
        }
        for d in doomed {
            from.remove(&d);
        }
    }
}

/// Wrappers this frame was handed and passes straight on: the destination of a
/// call to a function that hands a container back *inside* what it returns.
///
/// `func terminated_copy_of(s: string) -> cstring or NullByteError { return
/// s.to_cstring() }` is the whole shape — one call, one return, nothing in
/// between. The container is the callee's to make and the caller's to free, and
/// this frame is neither.
fn forwarded_wrappers(
    func: &MirFunction,
    handing: &HashMap<String, HandBack>,
) -> HashMap<LocalId, &'static str> {
    let mut out = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
            if let Some(back) = handing.get(&fref.name) {
                if let (true, Some(free)) = (back.wrapped, back.made()) {
                    out.insert(*dst, free);
                }
            }
        }
    }
    out
}

/// How a function hands a container to its caller.
#[derive(Clone)]
struct HandBack {
    /// What frees it, when a path builds one. Always there when `args` is
    /// empty.
    free: Option<&'static str>,
    /// The container is a slot inside what is returned (`-> Vec<i64>?`), not
    /// the returned value itself. The caller owns what it reads out of that
    /// slot, not the wrapper. Never with `args`.
    wrapped: bool,
    /// Some paths hand back one of the arguments at these indices instead of
    /// building one. The caller owns the result exactly when it owned every
    /// one of them.
    args: BTreeSet<usize>,
}

impl HandBack {
    /// The free, when every path builds what it returns.
    fn made(&self) -> Option<&'static str> {
        if !self.args.is_empty() {
            return None;
        }
        self.free
    }
}

/// What the aggregate `wrapper` holds in its container-shaped slots.
enum WrapperHoldings {
    /// A container this frame made — the caller's to free.
    Fresh(&'static str),
    /// Nothing container-shaped was written into it here.
    None,
    /// Something whose owner is elsewhere. Freeing it would be a double free.
    Foreign,
}

/// Read the stores into `wrapper` and say whose container came out of them.
///
/// A pointer-typed value stored into the aggregate being returned is the
/// payload; anything else in there (a tag, a scalar) is not a container and
/// says nothing. `Foreign` wins over `Fresh` — one store of somebody else's
/// container is enough to make the whole answer unsafe.
fn container_stored_into(
    func: &MirFunction,
    wrapper: LocalId,
    fresh: &HashMap<LocalId, &'static str>,
) -> WrapperHoldings {
    let mut found: Option<&'static str> = None;
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &stmt.kind else {
            continue;
        };
        // Pointer-shaped, so it *might* be somebody's container — or a name
        // this frame is already known to own one under, whatever MIR typed it.
        // A `cstring` is an opaque handle lowered to `i64`, so the shape test
        // alone never saw the one `to_cstring` stores into the wrapper it
        // returns, and every successful conversion leaked its copy (#1117).
        if *addr != wrapper || (!is_container_shaped(func, *v) && !fresh.contains_key(v)) {
            continue;
        }
        match fresh.get(v) {
            Some(f) => found = Some(f),
            None => return WrapperHoldings::Foreign,
        }
    }
    match found {
        Some(f) => WrapperHoldings::Fresh(f),
        None => WrapperHoldings::None,
    }
}

/// A container handle is an opaque pointer, and so is nothing else this pass
/// tracks. Asked of the declared local type rather than guessed from the name.
/// Is this read the container out of a wrapper known to be carrying one?
///
/// The payload sits in slot zero. Everything else in there says nothing: the
/// tag has its own read, and a `T or E` whose error arm has fields reads those
/// out of the later slots — `_28 = _16.1` on the refusal path of `to_cstring`
/// is a byte offset, and freeing it as a `cstring` frees an address that was
/// never one.
///
/// Slot zero is read on the error path too, and on every other variant of an
/// enum, so the read has to have the type the container comes back as: the
/// wrapper's success type, or a handle when the wrapper is an enum of the
/// program's own. "One machine word" used to be the test, and a `u64` that an
/// enum's other variant carried in slot zero was freed as a vector once the
/// ownership plan stopped leaking it (#1636). An opaque handle like `cstring`
/// is an `i64` and is still a container whose free the caller owns, which the
/// success type says.
fn holds_the_payload(func: &MirFunction, local: LocalId, wrapper: LocalId, field_index: u32) -> bool {
    if field_index != 0 {
        return false;
    }
    let Some(ty) = func.local_ty(local) else { return false };
    match func.local_ty(wrapper) {
        Some(MirType::Option(inner)) => **inner == *ty,
        Some(MirType::Result { ok, .. }) => **ok == *ty,
        _ => matches!(ty, MirType::Ptr | MirType::Container(_)),
    }
}

fn is_container_shaped(func: &MirFunction, local: LocalId) -> bool {
    func.locals
        .iter()
        .chain(func.params.iter())
        .find(|l| l.id == local)
        .is_some_and(|l| matches!(l.ty, MirType::Ptr | MirType::Container(_)))
}

/// The `take` container parameters this frame owns, with the free for each.
///
/// A parameter the body keeps is one every caller gave up at the call
/// (`params_a_callee_keeps`, read the same way on both sides), so it is this
/// frame's on every path, not just the one that keeps it.
/// `if c { return Vec.new() }  return p` returns `p` on one path and drops it
/// on the other, and nobody freed it there (#1504).
///
/// A closure body owns its `take` parameters the same way. A call through a
/// function value hands over what the bodies it may reach take
/// (`closure_call_keeps_argument`), and nothing else can call one: a
/// `func(take T)` value won't pass where a `func(T)` is asked for, so the
/// runtime is never handed one. Leaving them out meant `|take v| { … }` freed
/// `v` on no path, and a `for` body that kept its item on one branch leaked it
/// on the other.
///
/// Not an exported function: C calls those, and hands over nothing.
fn params_this_frame_owns(
    func: &MirFunction,
    kept: &HashMap<String, Vec<bool>>,
) -> Vec<(LocalId, &'static str)> {
    if func.is_extern_c {
        return Vec::new();
    }
    let Some(flags) = kept.get(&func.name) else { return Vec::new() };
    // A caller doesn't hand over a container method's receiver, kept or not
    // (`container_facts`). A `take self` receiver is handed over by every
    // caller whether or not the body keeps it, so it is this frame's to free.
    // Asking only whether the body kept it left `Vec.consume` freeing nothing
    // and its caller freeing nothing. Both read the way the method declared
    // `self`.
    let receiver_mode = func
        .params
        .first()
        .filter(|p| p.name.as_deref() == Some("self"))
        .and_then(|_| func.param_modes.first().copied());
    let receiver_lent = matches!(receiver_mode, Some(ParamMode::Borrow | ParamMode::Mutate));
    let receiver_taken = receiver_mode == Some(ParamMode::Take);
    func.params
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let kept = flags.get(*i).copied().unwrap_or(false) || (*i == 0 && receiver_taken);
            kept && !(*i == 0 && receiver_lent)
        })
        .filter_map(|(_, p)| match func.local_ty(p.id).cloned() {
            Some(MirType::Container(kind)) => Some((p.id, container_free(kind))),
            _ => None,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn insert_for_function(
    func: &mut MirFunction,
    all: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    interface_handing: &HashMap<String, HandBack>,
    keeps: &Keeps,
    targets: &ClosureTargets,
) {
    let kept = keeps.by_name;
    // Every name that may hold a container this frame made, and where each
    // is made. Whether a given name holds one of this frame's on a given path
    // is the ownership analysis's question, not a property of the name:
    // `index.get(word) ?? Vec.new()` holds the map's vector on one path and a
    // fresh one on the other, and only the second is this frame's.
    let (mut tracked, mut made_here) =
        containers_and_makers(func, all, handing_over, interface_handing, targets);
    let captured = task_captures_the_body_frees(func, keeps);
    for (id, free) in &captured {
        tracked.insert(*id, free);
        made_here.insert(*id);
    }
    let owned = params_this_frame_owns(func, kept);
    if made_here.is_empty() && owned.is_empty() {
        // A string cell needs no container of the frame's own: a function
        // that made none still releases the string an `ensure` body reads.
        let cells = cells_this_frame_frees(func, all, &HashMap::new(), keeps, &HashMap::new());
        if !cells.is_empty() {
            insert_cell_drops(func, &cells);
        }
        return;
    }
    let passed = pass_edges(func, handing_over, targets);
    // A capture is followed like a parameter: `mut w = v` in the task block
    // moves it, and `return w` hands it on. Untracked, `w` read as somebody
    // else's and `v` was freed under the vector the task returned.
    if !owned.is_empty() || !captured.is_empty() {
        for (id, free) in &owned {
            tracked.insert(*id, free);
        }
        follow_copies_and_passes(func, &mut tracked, &passed);
    }
    // The pruned answer, for the two rules below that still key on a name:
    // a container captured by a closure the frame drops, and one in a cell.
    let fresh = collect_fresh_containers_with(func, all, handing_over, interface_handing, targets);
    let ensured = consumed_by_an_ensure(func, all, &fresh, keeps);

    let owned: HashSet<LocalId> = owned.into_iter().map(|(id, _)| id).collect();
    let lent = lent_slots(func, &mut tracked, kept, &owned);
    let facts =
        container_facts(func, &tracked, &made_here, &ensured, &lent, keeps, &passed, &owned);
    let plan = crate::analysis::ownership::plan(
        func,
        &facts,
        crate::analysis::ownership::Placement::ScopeEnd,
    );

    // A container in a capture cell is reached through a store, which reads
    // as handing it over — so its free goes in separately, keyed on the cell
    // rather than on a name.
    let cells = cells_this_frame_frees(func, all, &fresh, keeps, &lent.slots);

    let guards = crate::analysis::ownership::Guards::new(func, &plan);
    // A release under a lent slot frees what the slot holds *now*: the callee
    // may have put a different container there, so the handle is loaded at the
    // release rather than taken from any earlier name.
    //
    // A variable a closure reaches by address is the same: `addr_taken` ran
    // before this pass and made it a stack cell, so the local now holds the
    // cell's address. `func look(take v: Vec<i64>) { each(|x| x == v[0]) }`
    // freed the address of the cell instead of the vector in it.
    let in_memory = crate::transform::addr_taken::analyze(func).owned;
    let mut next_local = func.locals.iter().map(|l| l.id.0).max().unwrap_or(0) + 1;
    let mut release = |func: &mut MirFunction, name: LocalId| -> Vec<MirStmt> {
        let free = FunctionRef::releasing(tracked[&name]);
        if !lent.slots.contains_key(&name) && !in_memory.contains(&name) {
            return vec![MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: free,
                args: vec![MirOperand::Local(name)],
            })];
        }
        let tmp = LocalId(next_local);
        next_local += 1;
        func.locals.push(crate::MirLocal {
            id: tmp,
            name: None,
            ty: MirType::Ptr,
            is_param: false,
        });
        vec![
            MirStmt::dummy(MirStmtKind::Assign {
                dst: tmp,
                rvalue: MirRValue::Deref(MirOperand::Local(name)),
            }),
            MirStmt::dummy(MirStmtKind::Call { dst: None, func: free, args: vec![MirOperand::Local(tmp)] }),
        ]
    };
    // Ahead of the releases, which go at block ends and on edges: the marks
    // index into the blocks as planned.
    let (shifted, unwind_edges) = crate::analysis::ownership::place_unwind(
        func,
        plan.unwind,
        &guards,
        &mut |func: &mut MirFunction, name: LocalId, _| release(func, name),
    );
    let mut in_block: Vec<(usize, usize, LocalId, Option<usize>)> = Vec::new();
    let mut on_edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan.releases {
        match r {
            crate::analysis::ownership::Release::At { block, at, name, guard, .. } => {
                in_block.push((block, at, name, guard))
            }
            crate::analysis::ownership::Release::OnEdge { from, to, name, guard, .. } => {
                let stmts = guards.wrap(guard, release(func, name));
                on_edges.push((from, to, stmts));
            }
        }
    }
    in_block.sort_by_key(|(b, at, l, _)| (*b, *at, l.0));
    let in_block: Vec<(usize, usize, Vec<MirStmt>)> = in_block
        .into_iter()
        .map(|(block, at, name, guard)| (block, at, guards.wrap(guard, release(func, name))))
        .collect();
    shifted.insert_releases(func, in_block);
    let flag_edges = guards.edges(func);
    crate::analysis::ownership::insert_on_edges(
        func,
        crate::analysis::ownership::merge_edges(
            crate::analysis::ownership::merge_edges(unwind_edges, on_edges),
            flag_edges,
        ),
    );
    guards.finish(func);
    if !cells.is_empty() {
        insert_cell_drops(func, &cells);
    }
}

/// Stack closures that copied one of the frame's containers into their
/// environment, and the names they are copied into.
///
/// Lowering carries a closure's captures by value when the closure looks like
/// it leaves its frame, and `closures::decide_allocation` puts the environment
/// back on the stack once inlining shows it doesn't: `consume(f)` with
/// `consume` inlined calls `f` right there. The container is still the frame's
/// then, and reading the capture as a hand-over gave it to an environment that
/// frees nothing.
/// A stack closure reaching a parameter through its address. A captured
/// local that is written becomes a cell, freed by what was stored into it
/// (`cells_this_frame_frees`). A parameter is never stored into, so the
/// closure only looks at the frame's value: a `take` parameter a loop body
/// reads, `each(|x| x == v[0])`, read as handed to the closure and nobody
/// freed it (#2009).
fn param_by_address(func: &MirFunction, cap: &crate::ClosureCapture) -> bool {
    cap.by_ref && func.params.iter().any(|p| p.id == cap.local_id)
}

fn stack_environments(
    func: &MirFunction,
    tracked: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut envs: HashMap<LocalId, &'static str> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ClosureCreate { dst, captures, heap: false, .. }
                if captures.iter().any(|c| {
                    (!c.by_ref || param_by_address(func, c)) && tracked.contains_key(&c.local_id)
                }) =>
            {
                Some((*dst, ""))
            }
            _ => None,
        })
        .collect();
    follow_copies(func, &mut envs);
    envs.into_keys().collect()
}

/// What each statement does to the containers this frame may hold.
///
/// Given away: returned, stored, captured, boxed, handed to a call that keeps
/// it, taken by a `take self` method, freed already by the lowering, or
/// consumed by an `ensure`. A container method borrows its receiver; every
/// other argument is asked whether the callee keeps it, because borrow is the
/// default (`mem.parameters/PM1`) and treating a read as a hand-over left the
/// container to nobody (#1047).
#[allow(clippy::too_many_arguments)]
fn container_facts(
    func: &MirFunction,
    tracked: &HashMap<LocalId, &'static str>,
    made_here: &HashSet<LocalId>,
    ensured: &HashSet<LocalId>,
    lent: &LentSlots,
    keeps: &Keeps,
    passed: &Passes,
    owned: &HashSet<LocalId>,
) -> crate::analysis::ownership::Facts {
    use crate::analysis::ownership::Event;
    use crate::analysis::uses;
    let envs = stack_environments(func, tracked);
    let names: std::collections::BTreeSet<LocalId> =
        tracked.keys().chain(envs.iter()).copied().collect();
    let is = |l: &LocalId| tracked.contains_key(l);
    let env = |l: &LocalId| envs.contains(l);
    let mut facts = crate::analysis::ownership::Facts {
        moved_parts: Default::default(),
        names: names.clone(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: func.params.iter().map(|p| p.id).filter(|p| is(p) && !owned.contains(p)).collect(),
        owned: func.params.iter().map(|p| p.id).filter(|p| is(p) && owned.contains(p)).collect(),
        caller_slots: Vec::new(),
    };
    // The first name each wrapper's payload was read into. See the `Field`
    // arm below.
    let mut first_read: HashMap<(LocalId, u32), LocalId> = HashMap::new();
    for block in &func.blocks {
        let mut events = Vec::new();
        let mut reads = Vec::new();
        let mut kills = Vec::new();
        for stmt in &block.statements {
            let mut ev: Vec<Event> = Vec::new();
            // A closure that borrows a lent slot doesn't outlive the frame,
            // so whatever it is given to, it is only lent.
            let give = |ev: &mut Vec<Event>, op: &MirOperand| {
                if let MirOperand::Local(id) = op {
                    if is(id) && !lent.closures.contains(id) {
                        ev.push(Event::HandOver(*id));
                    }
                }
            };
            // A call that may hand back one of its arguments: the result is
            // ours exactly when every one of them was (`Event::Remake`). Only
            // when all ends are tracked — otherwise the arguments are handed
            // over as before, and the result is nobody's.
            let pass = match &stmt.kind {
                MirStmtKind::Call { dst: Some(d), .. } | MirStmtKind::ClosureCall { dst: Some(d), .. }
                    if is(d) && !made_here.contains(d) =>
                {
                    passed.get(d).filter(|srcs| srcs.iter().all(|(_, s)| is(s))).map(|srcs| (srcs, *d))
                }
                _ => None,
            };
            let passes_arg = |i: usize| pass.is_some_and(|(srcs, _)| srcs.iter().any(|(p, _)| *p == i));
            let remake = |srcs: &[(usize, LocalId)], d: LocalId| Event::Remake {
                dst: d,
                srcs: srcs.iter().map(|(_, s)| *s).collect(),
            };
            match &stmt.kind {
                MirStmtKind::Phi { .. } => {}
                MirStmtKind::Call { func: fref, args, dst } => {
                    let borrows = fref.borrows_receiver() && !args.is_empty();
                    let takes = fref.consumes_receiver() && !args.is_empty();
                    for (i, arg) in args.iter().enumerate() {
                        if passes_arg(i) {
                            continue;
                        }
                        if i == 0 && takes {
                            give(&mut ev, arg);
                            continue;
                        }
                        if i == 0 && borrows {
                            continue;
                        }
                        if call_keeps_argument(fref, i, keeps.by_name) {
                            give(&mut ev, arg);
                        }
                    }
                    if let Some((srcs, d)) = pass {
                        ev.push(remake(srcs, d));
                    } else if let Some(d) = dst.filter(|d| is(d)) {
                        ev.push(if made_here.contains(&d) { Event::Make(d) } else { Event::Other(d) });
                    }
                }
                // An interface call names a method and a vtable slot, so the
                // by-name answer has nothing to look up. `self` isn't in the
                // argument list, so an implementation's parameters sit one to
                // the right (#1199).
                MirStmtKind::InterfaceCall { method_name, args, dst, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if keeps.interface_call(method_name, i) {
                            give(&mut ev, arg);
                        }
                    }
                    if let Some(d) = dst.filter(|d| is(d)) {
                        ev.push(if made_here.contains(&d) { Event::Make(d) } else { Event::Other(d) });
                    }
                }
                MirStmtKind::ClosureCall { closure, args, dst, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if !passes_arg(i) && keeps.closure_call(&func.name, *closure, i, args.len()) {
                            give(&mut ev, arg);
                        }
                    }
                    if let Some((srcs, d)) = pass {
                        ev.push(remake(srcs, d));
                    } else if let Some(d) = dst.filter(|d| is(d)) {
                        ev.push(if made_here.contains(&d) { Event::Make(d) } else { Event::Other(d) });
                    }
                }
                // Filling a lent slot: the slot is the variable, so it holds
                // the value from here on rather than taking it away.
                MirStmtKind::Store { addr, value, .. } if lent.slots.contains_key(addr) => {
                    match uses::operand_local(value).filter(|v| is(v)) {
                        Some(src) => ev.push(Event::Alias { dst: *addr, src }),
                        None => ev.push(Event::Other(*addr)),
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::InterfaceBox { value, .. } => give(&mut ev, value),
                // A loop body's `return` took it.
                MirStmtKind::MovedOut { local } if is(local) => ev.push(Event::HandOver(*local)),
                // The slot's address, lent to a `mutate` parameter. Whatever
                // reaches the slot through it keeps the value needed, and the
                // callee may put a new container there, so from here the slot
                // is the only name that certainly holds what is in it.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } if lent.slots.contains_key(src) => {
                    ev.push(Event::View { dst: *dst, base: *src });
                    ev.push(Event::WriteThrough(*src));
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(src)) }
                    if lent.slots.contains_key(src) =>
                {
                    ev.push(Event::Alias { dst: *dst, src: *src });
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base: MirOperand::Local(base), .. }
                        | MirRValue::Deref(MirOperand::Local(base)),
                } if lent.refs.contains(base) && is(dst) => {
                    ev.push(Event::View { dst: *dst, base: *base });
                }
                // A second read of a payload already read out is the same
                // container under another name. `let files = r is Vec<T> as f
                // else { … }` reads the result's payload once for `f` and once
                // for `files`, and each read was freed (#1494). Keyed by the
                // field as well as the base: two fields are two containers.
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base: MirOperand::Local(base), field_index, .. },
                } if is(dst) && made_here.contains(dst) => {
                    match first_read.get(&(*base, *field_index)) {
                        Some(first) => ev.push(Event::Alias { dst: *dst, src: *first }),
                        None => {
                            first_read.insert((*base, *field_index), *dst);
                        }
                    }
                }
                // A capture carried by value moves into the environment, which
                // frees it when the closure dies (`mem.closures/CM2`).
                //
                // A lent slot captured by reference is the variable lent to
                // the closure: the closure reaches into it, and running the
                // closure may write a new container there.
                MirStmtKind::ClosureCreate { dst, captures, heap, .. } => {
                    for cap in captures.iter().filter(|c| is(&c.local_id)) {
                        if cap.by_ref && lent.slots.contains_key(&cap.local_id) {
                            ev.push(Event::View { dst: *dst, base: cap.local_id });
                            ev.push(Event::WriteThrough(cap.local_id));
                        } else if (!*heap && env(dst) && param_by_address(func, cap)) || cap.lent {
                            // A lent capture is viewed, not handed over: the
                            // closure holds the caller's value for it (CM3).
                            ev.push(Event::View { dst: *dst, base: cap.local_id });
                        } else {
                            ev.push(Event::HandOver(cap.local_id));
                        }
                    }
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } if env(dst) => {
                    ev.push(if env(src) { Event::Alias { dst: *dst, src: *src } } else { Event::Other(*dst) });
                }
                MirStmtKind::EnsureHookRegister { captures, .. } => {
                    for cap in captures {
                        if ensured.contains(&cap.local_id) {
                            ev.push(Event::HandOver(cap.local_id));
                        }
                    }
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } => {
                    if is(src) && !lent_for_the_call(func, *dst, keeps.by_name) {
                        ev.push(Event::HandOver(*src));
                    }
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } if is(dst) => {
                    ev.push(if is(src) { Event::Alias { dst: *dst, src: *src } } else { Event::Other(*dst) });
                }
                _ => {}
            }
            // A statement handed a lent slot's address — a call given it, a
            // closure that captured it being run or passed on — may write a
            // new container into the slot before it's done. Copying the
            // address and reading through it don't.
            let through_address = !matches!(
                &stmt.kind,
                MirStmtKind::Assign {
                    rvalue: MirRValue::Use(_) | MirRValue::Deref(_) | MirRValue::Field { .. } | MirRValue::Ref(_),
                    ..
                } | MirStmtKind::ClosureCreate { .. }
            );
            if through_address {
                let mut written: Vec<LocalId> = lent
                    .slot_of
                    .iter()
                    .filter(|(r, _)| uses::stmt_reads(stmt, **r))
                    .map(|(_, s)| *s)
                    .collect();
                written.sort_by_key(|l| l.0);
                written.dedup();
                for slot in written {
                    if !ev.iter().any(|e| matches!(e, Event::WriteThrough(s) if *s == slot)) {
                        ev.push(Event::WriteThrough(slot));
                    }
                }
            }
            // Any other definition of a tracked name: made here (a container
            // read out of a wrapper, a load out of a cell) or not ours.
            if let Some(d) = uses::stmt_def(stmt).filter(|d| is(d)) {
                let bound = ev.iter().any(|e| {
                    matches!(e, Event::Make(n) | Event::Other(n) if *n == d)
                        || matches!(e, Event::Alias { dst, .. } | Event::View { dst, .. } | Event::Remake { dst, .. } if *dst == d)
                });
                if !bound && !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                    ev.push(if made_here.contains(&d) { Event::Make(d) } else { Event::Other(d) });
                }
            }
            let (mut r, mut k) = (Vec::new(), Vec::new());
            if !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                // A store into a lent slot writes the variable; it reads
                // nothing of what was there.
                let fills = match &stmt.kind {
                    MirStmtKind::Store { addr, value, .. } if lent.slots.contains_key(addr) => {
                        Some((*addr, value))
                    }
                    _ => None,
                };
                for n in &names {
                    let reads = match fills {
                        Some((slot, value)) if slot == *n => uses::operand_local(value) == Some(*n),
                        _ => uses::stmt_reads(stmt, *n),
                    };
                    if reads {
                        r.push(*n);
                    }
                    let writes = match fills {
                        Some((slot, _)) => slot == *n,
                        None => uses::stmt_def(stmt) == Some(*n),
                    };
                    if writes {
                        k.push(*n);
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
        let mut term = Vec::new();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } =
            &block.terminator.kind
        {
            if is(id) {
                term.push(Event::HandOver(*id));
            }
        }
        facts.terminator_events.push(term);
        facts
            .terminator_reads
            .push(names.iter().copied().filter(|n| uses::terminator_reads(&block.terminator, *n)).collect());
    }
    facts
}

/// Locals holding a container this frame owns, mapped to how to free it: the
/// destination of a constructor, and of a call to a function that builds one
/// and hands it back.
fn collect_fresh_containers_with(
    func: &MirFunction,
    all: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    interface_handing: &HashMap<String, HandBack>,
    targets: &crate::closure_targets::ClosureTargets,
) -> HashMap<LocalId, &'static str> {
    let (mut fresh, made_here) =
        containers_and_makers(func, all, handing_over, interface_handing, targets);
    let edges = pass_edges(func, handing_over, targets);

    // A name is only this frame's if *every* way of reaching it is. Following
    // a copy forwards says "one path put a fresh container here"; it doesn't
    // say the other path didn't put somebody else's there.
    //
    //     func lookup(index: Map<string, Vec<i64>>, word: string) -> Vec<i64> {
    //         return index.get(word) ?? Vec.new()
    //     }
    //
    // One name holds the map's own vector on one path and a fresh one on the
    // other. Calling that name this frame's freed the map's vector, and the
    // next lookup read memory that was already gone.
    loop {
        let doomed: Vec<LocalId> = fresh
            .keys()
            .copied()
            .filter(|id| !made_here.contains(id) && !every_def_is_fresh(func, *id, &fresh, &edges))
            .collect();
        if doomed.is_empty() {
            break;
        }
        for id in doomed {
            fresh.remove(&id);
        }
    }
    fresh
}

/// The free that gives back a container of this kind.
fn container_free(kind: crate::ContainerKind) -> &'static str {
    match kind {
        crate::ContainerKind::Vec => "rask_vec_free",
        crate::ContainerKind::Map => "rask_map_free",
        crate::ContainerKind::Rack => "rask_rack_free",
        crate::ContainerKind::Handle(release) => release,
    }
}

/// Every name that may hold a container this frame made, with the free that
/// matches it, and the names a container is made under: a constructor's
/// destination, a container read out of a wrapper a callee handed back, a
/// load out of a cell only this frame filled.
fn containers_and_makers(
    func: &MirFunction,
    all: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    interface_handing: &HashMap<String, HandBack>,
    targets: &crate::closure_targets::ClosureTargets,
) -> (HashMap<LocalId, &'static str>, HashSet<LocalId>) {
    let mut fresh: HashMap<LocalId, &'static str> = HashMap::new();
    // Calls whose result is a wrapper holding the container, rather than the
    // container: the caller owns what it unwraps, and the wrapper is a value.
    let mut unwrap_for: HashMap<LocalId, &'static str> = HashMap::new();
    // Calls that may hand back the argument they took. The result is this
    // frame's exactly when the argument was, so it is followed below with the
    // copies.
    let edges = pass_edges(func, handing_over, targets);
    for block in &func.blocks {
        for stmt in &block.statements {
            if matches!(&stmt.kind,
                MirStmtKind::Call { dst: Some(d), .. } | MirStmtKind::ClosureCall { dst: Some(d), .. }
                    if edges.contains_key(d))
            {
                continue;
            }
            if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
                if let Some(free) = free_for(fref) {
                    fresh.insert(*dst, free);
                } else if fref.moves_out() {
                    // A container taken out of a container — `vv.remove(i)`,
                    // `vv.pop()`, the binding of `for v in vv.take_all()` — is
                    // the caller's now, like one a constructor made. Nothing
                    // listed it, so a `Vec<Vec<T>>` element that was removed
                    // and never moved on was never freed. `pop` wraps it in an
                    // optional, which the unwrap below reads it out of.
                    match func.local_ty(*dst).cloned() {
                        Some(MirType::Container(kind)) => {
                            fresh.insert(*dst, container_free(kind));
                        }
                        // `rx.receive()` hands its element over inside a
                        // `T or ReceiveError` (#2000).
                        Some(MirType::Option(inner)) | Some(MirType::Result { ok: inner, .. }) => {
                            if let MirType::Container(kind) = *inner {
                                unwrap_for.insert(*dst, container_free(kind));
                            }
                        }
                        _ => {}
                    }
                } else if let Some(free) = fref.shape.and_then(|s| s.fresh_wrapped()) {
                    unwrap_for.insert(*dst, free);
                } else if let Some((back, free)) =
                    handing_over.get(&fref.name).and_then(|b| Some((b, b.made()?)))
                {
                    // The callee's own constructor decided which free this is.
                    if back.wrapped {
                        unwrap_for.insert(*dst, free);
                    } else {
                        fresh.insert(*dst, free);
                    }
                }
            }
            // A call through a vtable, once every implementation agrees it
            // hands one back — the same rule as the closure case below, and
            // for the same reason: there is no callee name to look up.
            if let MirStmtKind::InterfaceCall { dst: Some(dst), method_name, .. } = &stmt.kind {
                if let Some((back, free)) =
                    interface_handing.get(method_name).and_then(|b| Some((b, b.made()?)))
                {
                    if back.wrapped {
                        unwrap_for.insert(*dst, free);
                    } else {
                        fresh.insert(*dst, free);
                    }
                }
            }
            // A call through a closure, once every body it can reach agrees it
            // hands one back. `flat_map`'s closure builds a `Vec` per element,
            // and nothing owned it — the name-keyed answer above has no name to
            // look up (#943). One body that hands back somebody else's is the
            // whole set's answer, which is what keeps `|k| lookup()` returning
            // a const's vector from being freed per key.
            if let MirStmtKind::ClosureCall { dst: Some(dst), closure, .. } = &stmt.kind {
                if let Some(bodies) = targets.known(&func.name, *closure) {
                    let mut agreed: Option<&'static str> = None;
                    let all_hand_back = bodies.iter().all(|body| {
                        match handing_over.get(body).and_then(|b| Some((b, b.made()?))) {
                            // A wrapper needs the unwrap step below, which is
                            // keyed on the call's own destination — one closure
                            // returning a bare container and another a wrapped
                            // one have no single answer, so neither gets one.
                            Some((back, free)) if !back.wrapped => {
                                let same = agreed.is_none_or(|f| f == free);
                                agreed = Some(free);
                                same
                            }
                            _ => false,
                        }
                    });
                    if let (true, Some(free)) = (all_hand_back, agreed) {
                        fresh.insert(*dst, free);
                    }
                }
            }
        }
    }
    unwrap_for.extend(wrappers_filled_here(func, &fresh, &edges));
    // What comes out of such a wrapper is the container, and it is this frame's
    // now. Read as a field of the returned aggregate — the tag beside it is not
    // pointer-shaped, so the payload is the only slot this can pick up (#1117).
    //
    // Only a read the wrapper isn't used after: the payload then has no other
    // holder. `if o? as x { … }` followed by `o!` reads it twice, and freeing
    // it as `x` died left `o` holding freed memory; so does a read inside a
    // loop of a wrapper filled before it.
    let mut unwrapped: HashSet<LocalId> = HashSet::new();
    if !unwrap_for.is_empty() {
        let copies = wrapper_copies(func);
        for block in &func.blocks {
            for (i, stmt) in block.statements.iter().enumerate() {
                let MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base, field_index, .. },
                } = &stmt.kind
                else {
                    continue;
                };
                let MirOperand::Local(src) = base else { continue };
                let Some(free) = unwrap_for.get(src) else { continue };
                if !holds_the_payload(func, *dst, *src, *field_index) {
                    continue;
                }
                let class = copies.get(src).cloned().unwrap_or_else(|| HashSet::from([*src]));
                if class.iter().all(|w| dead_after(func, block, i, *w)) {
                    fresh.insert(*dst, free);
                    unwrapped.insert(*dst);
                }
            }
        }
    }
    if fresh.is_empty() {
        return (fresh, HashSet::new());
    }

    // The same value under a new name: follow it, so the name still holding it
    // at its death is the one that gets freed.
    follow_copies_and_passes(func, &mut fresh, &edges);

    // A container the loop body touches lives in a cell, and the frame reads it
    // back out with a load. Asked after the copies are followed, because the
    // value that goes *into* the cell is usually a copy of the constructor's
    // result rather than the result itself — and then followed again, because
    // what comes back out gets copied on in turn.
    let through_cells = fresh_through_cells(func, all, &fresh);
    let from_cells: HashSet<LocalId> = through_cells.iter().map(|(id, _)| *id).collect();
    for (dst, free) in through_cells {
        fresh.insert(dst, free);
    }
    if !from_cells.is_empty() {
        follow_copies_and_passes(func, &mut fresh, &edges);
    }

    // Every name reached without a copy — a constructor's own destination, or a
    // load out of a cell only this frame filled. The pruning below asks how a
    // *copy* was reached, and has no question to ask of these.
    let made_here: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            // A call through a closure counts the same way: its destination is
            // the definition, not a copy of some other name (#943). So does
            // one through a vtable, whose implementations all agreed to hand
            // a container back: pruned as a copy it was never freed.
            MirStmtKind::Call { dst: Some(dst), .. }
            | MirStmtKind::ClosureCall { dst: Some(dst), .. }
            | MirStmtKind::InterfaceCall { dst: Some(dst), .. } => Some(*dst),
            _ => None,
        })
        // What a call that may hand back its argument returns follows the
        // argument, like a copy.
        .filter(|id| !edges.contains_key(id))
        .filter(|id| fresh.contains_key(id))
        .chain(from_cells)
        // A container read out of the wrapper a callee handed back is reached
        // without a copy too. The pruning below asks how a *copy* was reached,
        // and the answer for these is "it wasn't" — a Field read is the
        // definition, not a copy of some other name.
        .chain(unwrapped)
        .collect();
    (fresh, made_here)
}

/// Each wrapper local with every wrapper local a copy connects it to, in
/// either direction: the names one value goes by.
fn wrapper_copies(func: &MirFunction) -> HashMap<LocalId, HashSet<LocalId>> {
    let is_wrapper = |l: LocalId| matches!(func.local_ty(l), Some(MirType::Option(_) | MirType::Result { .. }));
    let mut parent: HashMap<LocalId, LocalId> = HashMap::new();
    fn root(parent: &HashMap<LocalId, LocalId>, mut l: LocalId) -> LocalId {
        while let Some(p) = parent.get(&l) {
            if *p == l {
                break;
            }
            l = *p;
        }
        l
    }
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind else { continue };
        if !is_wrapper(*dst) || !is_wrapper(*src) {
            continue;
        }
        let (a, b) = (root(&parent, *dst), root(&parent, *src));
        if a != b {
            parent.insert(a, b);
        }
    }
    let members: Vec<LocalId> = parent.keys().copied().chain(parent.values().copied()).collect();
    let mut classes: HashMap<LocalId, HashSet<LocalId>> = HashMap::new();
    for l in &members {
        classes.entry(root(&parent, *l)).or_default().insert(*l);
    }
    members.iter().map(|l| (*l, classes[&root(&parent, *l)].clone())).collect()
}

/// Whether nothing reads `local` after statement `at` of `block`, on any path,
/// before it is written again. A release of the wrapper is no read: what it
/// releases is what the wrapper's own cleanup already accounts for.
fn dead_after(func: &MirFunction, block: &crate::MirBlock, at: usize, local: LocalId) -> bool {
    use crate::analysis::uses::{stmt_def, stmt_reads, terminator_reads};
    let releases = |st: &MirStmt| match &st.kind {
        MirStmtKind::RcDec { local: l } | MirStmtKind::RcDecContents { local: l } => *l == local,
        MirStmtKind::UnwindArm { value, .. } => *value == local,
        _ => false,
    };
    // `Some(true)`: read on this stretch; `Some(false)`: written first.
    let scan = |stmts: &[MirStmt]| {
        for st in stmts {
            if releases(st) {
                continue;
            }
            // A store into the wrapper refills it: the frame builds a `T?`
            // field by field.
            let refills = matches!(&st.kind, MirStmtKind::Store { addr, value, .. }
                if *addr == local && !crate::analysis::uses::operand_reads(value, local));
            if refills {
                return Some(false);
            }
            if stmt_reads(st, local) {
                return Some(true);
            }
            if stmt_def(st) == Some(local) {
                return Some(false);
            }
        }
        None
    };
    let by_id: HashMap<crate::BlockId, &crate::MirBlock> = func.blocks.iter().map(|b| (b.id, b)).collect();
    match scan(&block.statements[at + 1..]) {
        Some(read) => return !read,
        None if terminator_reads(&block.terminator, local) => return false,
        None => {}
    }
    let mut seen: HashSet<crate::BlockId> = HashSet::new();
    let mut work = crate::analysis::cfg::successors(&block.terminator);
    while let Some(id) = work.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(b) = by_id.get(&id) else { continue };
        match scan(&b.statements) {
            Some(true) => return false,
            Some(false) => continue,
            None => {}
        }
        if terminator_reads(&b.terminator, local) {
            return false;
        }
        work.extend(crate::analysis::cfg::successors(&b.terminator));
    }
    true
}

/// `T?` and `T or E` wrappers this frame built around a container it made,
/// and the names that copy one, with that container's free.
///
/// The same wrapper a callee hands back (`HandBack::wrapped`) once the callee
/// is inlined: `return v` became a store of `v` into a slot of the frame's
/// own. Called, what the caller unwraps is the caller's; inlined, it read as a
/// part of the wrapper, and a loop that carried it past the turn that refilled
/// the slot leaked it every turn (#2060).
///
/// Only where every container stored as the payload is one this frame made,
/// and every definition of a copy is such a wrapper: one path handing back
/// somebody else's makes the payload somebody else's on that path.
fn wrappers_filled_here(
    func: &MirFunction,
    fresh: &HashMap<LocalId, &'static str>,
    edges: &Passes,
) -> HashMap<LocalId, &'static str> {
    /// What a wrapper may hold as its payload.
    #[derive(Clone, Copy, PartialEq)]
    enum Holds {
        /// Nothing: a `none` or the error side.
        Empty,
        Made(&'static str),
        Foreign,
    }
    let join = |a: Holds, b: Holds| match (a, b) {
        (Holds::Empty, x) | (x, Holds::Empty) => x,
        (Holds::Made(f), Holds::Made(g)) if f == g => Holds::Made(f),
        _ => Holds::Foreign,
    };
    let payload_of = |l: LocalId| match func.local_ty(l) {
        Some(MirType::Option(inner)) => Some((**inner).clone()),
        Some(MirType::Result { ok, .. }) => Some((**ok).clone()),
        _ => None,
    };
    let mut made = fresh.clone();
    follow_copies_and_passes(func, &mut made, edges);
    let stmts = || func.blocks.iter().flat_map(|b| b.statements.iter());
    let defined: HashSet<LocalId> = stmts().filter_map(crate::analysis::uses::stmt_def).collect();
    // A slot the frame fills field by field. Assigned whole, it is a copy.
    let mut holds: HashMap<LocalId, Holds> = HashMap::new();
    for st in stmts() {
        let MirStmtKind::Store { addr, value, .. } = &st.kind else { continue };
        if defined.contains(addr) {
            continue;
        }
        let Some(payload @ MirType::Container(_)) = payload_of(*addr) else { continue };
        let this = match value {
            MirOperand::Local(v) if func.local_ty(*v) == Some(&payload) => match made.get(v) {
                Some(f) => Holds::Made(f),
                None => Holds::Foreign,
            },
            _ => Holds::Empty,
        };
        let slot = holds.entry(*addr).or_insert(Holds::Empty);
        *slot = join(*slot, this);
    }
    if holds.is_empty() {
        return HashMap::new();
    }
    // A copy holds what every wrapper copied into it may hold.
    for _ in 0..=func.locals.len() {
        let mut next: HashMap<LocalId, Holds> = HashMap::new();
        for st in stmts() {
            let Some(d) = crate::analysis::uses::stmt_def(st) else { continue };
            if payload_of(d).is_none() {
                continue;
            }
            let this = match &st.kind {
                MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(src)), .. } => {
                    holds.get(src).copied().unwrap_or(Holds::Foreign)
                }
                _ => Holds::Foreign,
            };
            let slot = next.entry(d).or_insert(Holds::Empty);
            *slot = join(*slot, this);
        }
        let mut changed = false;
        for (d, h) in next {
            if holds.get(&d) != Some(&h) {
                holds.insert(d, h);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    holds
        .into_iter()
        .filter_map(|(w, h)| match h {
            Holds::Made(f) => Some((w, f)),
            _ => None,
        })
        .collect()
}

/// `follow_copies`, also through calls that may hand back their argument.
fn follow_copies_and_passes(
    func: &MirFunction,
    fresh: &mut HashMap<LocalId, &'static str>,
    edges: &Passes,
) {
    loop {
        follow_copies(func, fresh);
        let mut added = false;
        for (dst, srcs) in edges {
            let frees: Option<Vec<&'static str>> = srcs.iter().map(|(_, s)| fresh.get(s).copied()).collect();
            if let Some(&free) = frees.as_deref().and_then(|f| f.first()) {
                if fresh.insert(*dst, free).is_none() {
                    added = true;
                }
            }
        }
        if !added {
            return;
        }
    }
}

/// Does every definition of `local` put a container this frame made into it?
/// Carry each fresh container forward through copies and phis, to a fixpoint.
fn follow_copies(func: &MirFunction, fresh: &mut HashMap<LocalId, &'static str>) {
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                        if let Some(free) = fresh.get(src).copied() {
                            if fresh.insert(*dst, free).is_none() {
                                added = true;
                            }
                        }
                    }
                    MirStmtKind::Phi { dst, args } => {
                        for (_, op) in args {
                            if let MirOperand::Local(src) = op {
                                if let Some(free) = fresh.get(src).copied() {
                                    if fresh.insert(*dst, free).is_none() {
                                        added = true;
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !added {
            return;
        }
    }
}

/// Containers this frame owns that it can only reach through a stack cell.
///
/// A local the loop body writes to isn't a local any more: `for x in seq`
/// compiles the body to a closure, and anything it captures moves into
/// addressable storage (`transform::addr_taken`). So a sequence terminal builds
/// its result, stores it into a cell, and returns a *load* from that cell — and
/// a load is somebody else's by default, which is why `to_vec` and `to_map`
/// were freed by nobody (#1060).
///
/// Only where the cell can hold nothing else. Every store this frame makes into
/// it holds one of the frame's own containers, all freed the same way, and no
/// closure that captured it puts anything else there. A store of anything else
/// is a container somebody else may still hold, and calling it this frame's
/// would free it under them. `v = Vec.new()` after a closure captured `v` is
/// a second store of the frame's own, and the caller freed neither (#2034).
///
/// And only for a load this frame *returns*. The question this answers is
/// "does this frame hand the caller a container to free" — nothing else. A load
/// the frame goes on to use itself is already handled, and calling it fresh
/// changed answers it had no business changing: `let count = || items.len()`
/// reads `items` out of its cell three times, and putting all three in one
/// value group made a vector that used to be freed once stop being freed at all
/// (t26 went from 4 leaked allocations to 6, t31 from 21 to 40).
///
/// A load reaches the return through plain copies and joins when a loop hands
/// it out: `break v` lowers to `_24 = *cell; _25 = _24; … return _26`, and
/// the caller of such a function freed nothing (#2034).
fn fresh_through_cells(
    func: &MirFunction,
    all: &[MirFunction],
    fresh: &HashMap<LocalId, &'static str>,
) -> Vec<(LocalId, &'static str)> {
    // addr → what got stored there. A cell holding one container is only ever
    // written whole, at offset 0; a write anywhere else is part of something
    // bigger, and stands for a value that isn't one of this frame's.
    let mut stores: HashMap<LocalId, Vec<MirOperand>> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Store { addr, offset, value, .. } = &stmt.kind {
                let value = if *offset == 0 { value.clone() } else { MirOperand::Constant(crate::MirConst::Int(0)) };
                stores.entry(*addr).or_default().push(value);
            }
        }
    }

    let handed_back = handed_back_locals(func, &stores);
    if handed_back.is_empty() {
        return Vec::new();
    }

    // A load found fresh can be what the next cell holds, so this runs until
    // no cell gives up another.
    let mut known = fresh.clone();
    let mut out: Vec<(LocalId, &'static str)> = Vec::new();
    loop {
        let found = out.len();
        for (cell, values) in &stores {
            let Some(free) = one_free_for_all(&stores_of_locals(values), values.len(), &known) else { continue };
            if !cell_stays_owned_through_closures(func, all, *cell) {
                continue;
            }
            for block in &func.blocks {
                for stmt in &block.statements {
                    if let MirStmtKind::Assign {
                        dst,
                        rvalue: MirRValue::Deref(MirOperand::Local(addr)),
                    } = &stmt.kind
                    {
                        if addr == cell && handed_back.contains(dst) && !known.contains_key(dst) {
                            known.insert(*dst, free);
                            out.push((*dst, free));
                        }
                    }
                }
            }
        }
        if out.len() == found {
            return out;
        }
        follow_copies(func, &mut known);
    }
}

/// The locals a cell's stores put there; empty when one stores anything else.
fn stores_of_locals(values: &[MirOperand]) -> Vec<LocalId> {
    let locals: Vec<LocalId> = values
        .iter()
        .filter_map(|v| match v {
            MirOperand::Local(l) => Some(*l),
            _ => None,
        })
        .collect();
    if locals.len() == values.len() { locals } else { Vec::new() }
}

/// How a cell's contents are freed, when every store into it is a container
/// this frame made and all of them are freed the same way.
fn one_free_for_all(
    srcs: &[LocalId],
    stores: usize,
    fresh: &HashMap<LocalId, &'static str>,
) -> Option<&'static str> {
    if srcs.is_empty() || srcs.len() != stores {
        return None;
    }
    let free = *fresh.get(&srcs[0])?;
    srcs.iter().all(|s| fresh.get(s) == Some(&free)).then_some(free)
}

/// Every local whose value the frame returns, or wraps for the return, through
/// nothing but copies, joins and cells holding that one value.
fn handed_back_locals(func: &MirFunction, stores: &HashMap<LocalId, Vec<MirOperand>>) -> HashSet<LocalId> {
    let returned: HashSet<LocalId> = func
        .blocks
        .iter()
        .filter_map(|b| match &b.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn {
                value: Some(MirOperand::Local(id)),
                ..
            } => Some(*id),
            _ => None,
        })
        .collect();
    if returned.is_empty() {
        return HashSet::new();
    }

    // Or put into a wrapper that is: `return out` from a `-> Vec<T> or E`
    // stores the load into the result's payload first. `parse_query` builds
    // its vector in a cell its `split` loop borrows, and its callers never
    // freed what it handed them.
    let wrapped: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } if returned.contains(addr) => Some(*v),
            _ => None,
        })
        .collect();

    // Every local whose value goes on to be returned, or wrapped for the
    // return, through nothing but copies and joins. Two `break`s meet in a
    // phi before the return: `let r = loop { … if k { break v } break
    // Vec.new() }` returned neither as this frame's, and the caller freed
    // nothing either way.
    let mut handed_back: HashSet<LocalId> = returned.union(&wrapped).copied().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let srcs: Vec<LocalId> = match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if handed_back.contains(dst) =>
                {
                    vec![*src]
                }
                MirStmtKind::Phi { dst, args } if handed_back.contains(dst) => args
                    .iter()
                    .filter_map(|(_, op)| match op {
                        MirOperand::Local(src) => Some(*src),
                        _ => None,
                    })
                    .collect(),
                // And back through a cell the values it holds: `r` captured
                // after `let r = loop { … break v }` puts the load out of
                // `v`'s cell into `r`'s, and returns a load out of that.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(addr)) }
                    if handed_back.contains(dst) =>
                {
                    stores_of_locals(stores.get(addr).map(Vec::as_slice).unwrap_or(&[]))
                }
                _ => continue,
            };
            for src in srcs {
                changed |= handed_back.insert(src);
            }
        }
    }
    handed_back
}

/// Containers this frame owns that live in a capture cell and never leave.
///
/// A closure that borrows a variable makes it memory-resident
/// (`transform::addr_taken`), so `mut log: Vec<i64>` stops being a local and
/// becomes a stack cell the closure holds the address of. The container is then
/// reached only through a store into that cell — and a store is handing the
/// value over, so nothing freed it:
///
/// ```text
/// mut log: Vec<i64> = Vec.new()
/// let record = |x| { log.push(x) }     // `log` moves into a cell
/// record(1)
/// // rask: 2 allocations never released
/// ```
///
/// The same body without the closure was freed correctly, which is what made
/// this look like a closure bug rather than a cell one.
///
/// A store into an aggregate really is handing it over; a store into this
/// frame's own variable cell is not, because the cell *is* the variable and
/// dies with the frame. Telling the two apart is what the by-ref capture says:
/// only `addr_taken` makes these, and only for a variable of this frame.
///
/// The conditions are `fresh_through_cells`' — one store, holding one of this
/// frame's own fresh containers, and no closure that replaces it — plus the two
/// this direction needs: the frame must not hand the container back, and must
/// not hand the cell's address anywhere the closures can't be read.
fn cells_this_frame_frees(
    func: &MirFunction,
    all: &[MirFunction],
    fresh: &HashMap<LocalId, &'static str>,
    keeps: &Keeps,
    lent: &HashMap<LocalId, &'static str>,
) -> Vec<(LocalId, InCell, BlockId)> {
    let kept = keeps.by_name;
    let mut by_ref_cells: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::ClosureCreate { captures, .. } => Some(captures),
            _ => None,
        })
        .flatten()
        .filter(|c| c.by_ref)
        .map(|c| c.local_id)
        .collect();
    // An `ensure` body that touches a variable makes the same kind of cell: the
    // hook's environment holds the slot's address so the body — which may run
    // on a panic, long after this frame stopped executing — reads the live
    // value rather than a copy. `ensure v.push(2)` was therefore a `Vec` in a
    // slot nobody freed, and `ensure s.set(1)` a `Shared` box nobody released
    // (#1224). The hook is not a `ClosureCreate`, so none of the reasoning
    // below had ever been pointed at it.
    by_ref_cells.extend(
        func.blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|stmt| match &stmt.kind {
                MirStmtKind::EnsureHookRegister { captures, .. } => Some(captures),
                _ => None,
            })
            .flatten()
            .filter(|c| c.by_ref)
            .map(|c| c.local_id),
    );
    // A slot only lent to `mutate` parameters is released by the ownership
    // plan, which knows where each value in it dies (`lent_slots`).
    by_ref_cells.extend(slots_whose_address_is_taken(func));
    // A lent slot is followed by the ownership plan, which knows where each
    // value in it dies (`lent_slots`).
    by_ref_cells.retain(|s| !lent.contains_key(s));
    if by_ref_cells.is_empty() {
        return Vec::new();
    }

    let mut stores: HashMap<LocalId, Vec<(MirOperand, BlockId)>> = HashMap::new();
    let mut loaded: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    // The address itself going somewhere this pass can't follow. A
    // `ClosureCreate` is the one that made the cell and is read through
    // `cell_stays_owned_through_closures` instead.
    let handed_on = addresses_handed_on(func);
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Store { addr, value, .. } => {
                    stores.entry(*addr).or_default().push((value.clone(), block.id));
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Deref(MirOperand::Local(addr)),
                } => {
                    loaded.entry(*addr).or_default().push(*dst);
                }
                _ => {}
            }
        }
    }

    let returned: HashSet<LocalId> = func
        .blocks
        .iter()
        .filter_map(|b| match &b.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                Some(*id)
            }
            _ => None,
        })
        .collect();

    // The store names whatever lowering last copied the constructor's result
    // into, not the result itself — `_9 = StringBuilder_new(); _10 = _9;
    // *(_1+0) = _10`. Asking about `_10` alone answered "not this frame's".
    let mut fresh_by_copy = fresh.clone();
    follow_copies(func, &mut fresh_by_copy);

    // A cell whose contents a call takes away. For a string cell this replaces
    // the blunter "the address went to a call at all" test below: a string's
    // header *is* the slot, so `println(joined)` passes the cell's address for
    // an ordinary borrow, and reading that as a hand-off left every one of them
    // out.
    let mut kept_by_a_call: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::Call { func: fref, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    if let MirOperand::Local(id) = arg {
                        if call_keeps_argument(fref, i, kept) {
                            kept_by_a_call.insert(*id);
                        }
                    }
                }
            }
            MirStmtKind::InterfaceCall { method_name, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    if let MirOperand::Local(id) = arg {
                        if keeps.interface_call(method_name, i) {
                            kept_by_a_call.insert(*id);
                        }
                    }
                }
            }
            MirStmtKind::ClosureCall { closure, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    if let MirOperand::Local(id) = arg {
                        if keeps.closure_call(&func.name, *closure, i, args.len()) {
                            kept_by_a_call.insert(*id);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let mut out = Vec::new();
    for cell in &by_ref_cells {
        let Some(values) = stores.get(cell) else { continue };
        // A string an `ensure` body reads, filled once. That body runs in the
        // cleanup chain, after the frame's last use, so the string can't be
        // released at its last use the way every other string variable is
        // (`rc_insert::places_as_definitions`); it's released on the way out
        // instead (#1207). A string is refcounted, so being wrong about a
        // second owner costs a leak rather than a double free.
        let is_string = |value: &MirOperand| match value {
            MirOperand::Constant(c) if c.static_buffer().is_some() => true,
            MirOperand::Local(v) => func.local_ty(*v) == Some(&MirType::Buffer),
            _ => false,
        };
        // The store every other one follows fills the cell; the rest replace
        // what it holds. Block numbers don't say which.
        let leader = values
            .iter()
            .map(|(_, b)| *b)
            .find(|first| values.iter().all(|(_, b)| dom.dominates(*first, *b)));
        if let Some(ty) = closure_cell_is_a_variable(func, all, keeps.targets, *cell, values, &handed_on, &dom, &mut Vec::new()) {
            out.push((*cell, InCell::Slot(ty), leader.expect("a closure variable has a leading store")));
            continue;
        }
        if values.iter().any(|(v, _)| is_string(v)) {
            // `insert_cell_drops` releases what each later store covers. With
            // no leading store there is no telling which write makes the value
            // and which replaces it.
            if let Some(store_block) = leader {
                if ensure_captured(func, *cell) && !kept_by_a_call.contains(cell) {
                    out.push((*cell, InCell::Slot(MirType::Buffer), store_block));
                }
            }
            continue;
        }
        if handed_on.contains(cell) {
            continue;
        }
        let [(MirOperand::Local(src), store_block)] = values.as_slice() else { continue };
        let Some(free) = fresh_by_copy.get(src).copied() else { continue };
        if !cell_stays_owned_through_closures(func, all, *cell) {
            continue;
        }
        // A load the frame returns is the caller's to free.
        if loaded.get(cell).is_some_and(|dsts| dsts.iter().any(|d| returned.contains(d))) {
            continue;
        }
        // And so is one it hands to something that keeps it. `consume(log)` on
        // a `take` parameter takes the container with it, and the cell it came
        // out of must not free it a second time. The same rules the frame's own
        // locals get, asked of what comes out of the cell.
        let mut carried: HashMap<LocalId, &'static str> = HashMap::new();
        for dst in loaded.get(cell).into_iter().flatten() {
            carried.insert(*dst, free);
        }
        follow_copies(func, &mut carried);
        // Consumed counts the same here as escaping: a cell's free runs on the
        // way out of the frame, so a value already taken away must not be
        // named at all. The path-sensitivity below is for a frame's own locals,
        // which have a placement to be sensitive about.
        let (escapes, taken) = find_escaping(func, &carried, keeps);
        if !escapes.is_empty() || !taken.is_empty() {
            continue;
        }
        out.push((*cell, InCell::Handle(free), *store_block));
    }
    out
}

/// A closure variable a closure body writes: `mut f = || 0; for v in vs { f = || v[2] }`.
///
/// The body writes the variable through its address, so the variable is a
/// cell and every closure stored into it is a heap one (`find_escaping_closures`
/// sees the write). Storing hands the closure over, so nothing frees it by
/// name; the cell owns the closure it holds, the body releases the one it
/// replaces, and this frame releases the last one on the way out unless it
/// returns it. Each store before that had leaked.
///
/// Only where the variable is nothing but a variable: filled by one store every
/// other follows, and holding closures this frame owns (`owned_closures`). A
/// closure read out and kept somewhere took a reference of its own at the read
/// (`closures::retain_loads_handed_on`), so what is read out may go anywhere.
fn closure_cell_is_a_variable(
    func: &MirFunction,
    all: &[MirFunction],
    targets: &ClosureTargets,
    cell: LocalId,
    values: &[(MirOperand, BlockId)],
    handed_on: &HashSet<LocalId>,
    dom: &crate::analysis::dominators::DominatorTree,
    reached: &mut Vec<Reached>,
) -> Option<MirType> {
    let ty = match values.first()? {
        (MirOperand::Local(v), _) => func.local_ty(*v)?.clone(),
        _ => return None,
    };
    if !matches!(ty, MirType::FuncPtr(_)) || handed_on.contains(&cell) || ensure_captured(func, cell) {
        return None;
    }
    // A cell read out of the environment is the enclosing frame's variable,
    // which that frame releases. A sequence body that handed it on to an inner
    // body's closure took it for its own and freed it on the way out.
    let from_env = func.blocks.iter().flat_map(|b| b.statements.iter()).any(|st| {
        matches!(&st.kind, MirStmtKind::LoadCapture { dst, .. } if *dst == cell)
    });
    if from_env {
        return None;
    }
    let owned = owned_closures(func, all);
    if !values.iter().all(|(v, _)| matches!(v, MirOperand::Local(l) if owned.contains(l))) {
        return None;
    }
    // Some store has to come before all the others. Block numbers don't say
    // which: an `if` that writes the variable can sit in a lower-numbered
    // block than the declaration's.
    if !values.iter().any(|(_, first)| values.iter().all(|(_, b)| dom.dominates(*first, *b))) {
        return None;
    }
    let mut addrs = HashSet::from([cell]);
    if !cell_used_as_variable(func, all, targets, &mut addrs, reached, &None, 0) {
        return None;
    }
    Some(ty)
}

/// Where a closure variable's address reaches another frame that writes it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum CellReach {
    /// A body that captured it by reference, at this environment offset.
    Capture(String, u32),
    /// A `mutate` parameter, by function and index, from the call at
    /// (caller, block, statement).
    Param { callee: String, index: usize, site: (String, BlockId, usize) },
}

/// One frame a closure variable reaches, and the `mutate` parameter the
/// traversal came in through, if any: a frame reached through a parameter is
/// only sure of what it holds when every caller of that parameter is.
#[derive(Clone, Debug)]
struct Reached {
    at: CellReach,
    through: Option<(String, usize)>,
}

/// Where a store replaces a closure that a variable owns, so the store
/// releases it first: a body's environment slot, or a `mutate` parameter.
#[derive(Default)]
struct CellWriters {
    /// (closure body, offset).
    captures: HashSet<(String, u32)>,
    /// (function, parameter index).
    params: HashSet<(String, usize)>,
}

/// The frames that write a closure variable `closure_cell_is_a_variable` says
/// its frame owns.
///
/// A body that captured the variable has one creator, so that creator's answer
/// settles it. A `mutate` parameter has as many callers as the function has
/// calls, and the callee releases what it replaces for all of them, so every
/// call has to hand it something that owns its closure: such a variable, a
/// field or collection element (`lends_owned_slot`), or a parameter that was
/// itself handed one. A function a call can reach without naming it (a
/// function value, a vtable entry, a closure body) has callers nobody can
/// list, and never releases.
fn closure_cell_writers(fns: &[MirFunction], targets: &ClosureTargets) -> CellWriters {
    let mut reached: Vec<Reached> = Vec::new();
    for func in fns {
        let mut cells: HashSet<LocalId> = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|st| match &st.kind {
                MirStmtKind::ClosureCreate { captures, .. } => Some(captures),
                _ => None,
            })
            .flatten()
            .filter(|c| c.by_ref)
            .map(|c| c.local_id)
            .collect();
        cells.extend(slots_whose_address_is_taken(func));
        cells.retain(|c| matches!(func.local_ty(*c), Some(MirType::FuncPtr(_))));
        if cells.is_empty() {
            continue;
        }
        let handed_on = addresses_handed_on(func);
        let dom = crate::analysis::dominators::DominatorTree::build(func);
        let mut cells: Vec<LocalId> = cells.into_iter().collect();
        cells.sort_by_key(|c| c.0);
        for cell in cells {
            let values: Vec<(MirOperand, BlockId)> = func
                .blocks
                .iter()
                .flat_map(|b| b.statements.iter().map(move |st| (b.id, st)))
                .filter_map(|(b, st)| match &st.kind {
                    MirStmtKind::Store { addr, value, .. } if *addr == cell => Some((value.clone(), b)),
                    _ => None,
                })
                .collect();
            let mut found = Vec::new();
            if closure_cell_is_a_variable(func, fns, targets, cell, &values, &handed_on, &dom, &mut found).is_some() {
                reached.extend(found);
            }
        }
    }
    // Each scalar `mutate` parameter taken as a variable of its own frame:
    // what the function stores through it has to be a closure it owns,
    // whoever called it.
    let mut writers: HashSet<(String, usize)> = HashSet::new();
    for func in fns {
        for (i, p) in func.params.iter().enumerate() {
            if func.param_modes.get(i) != Some(&rask_ast::ty::ParamMode::Mutate) || !matches!(p.ty, MirType::Ptr) {
                continue;
            }
            let through = Some((func.name.clone(), i));
            let mut found = Vec::new();
            if cell_used_as_variable(func, fns, targets, &mut HashSet::from([p.id]), &mut found, &through, 0) {
                writers.insert((func.name.clone(), i));
                reached.extend(found);
            }
        }
    }

    let unnamed: HashSet<&str> = fns
        .iter()
        .flat_map(|f| f.blocks.iter().flat_map(|b| b.statements.iter()))
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign { rvalue: MirRValue::FuncAddr(name), .. } => Some(name.as_str()),
            _ => None,
        })
        .chain(fns.iter().flat_map(|f| f.blocks.iter().flat_map(|b| b.statements.iter())).filter_map(|st| {
            match &st.kind {
                MirStmtKind::ClosureCreate { func_name, .. } => Some(func_name.as_str()),
                _ => None,
            }
        }))
        .chain(vtable_candidates(fns).into_iter().map(|(_, f)| f.name.as_str()))
        .collect();
    let covered: HashSet<(&(String, BlockId, usize), usize)> = reached
        .iter()
        .filter_map(|r| match &r.at {
            CellReach::Param { site, index, .. } => Some((site, *index)),
            _ => None,
        })
        .collect();
    let mut unsure: HashSet<(String, usize)> = HashSet::new();
    for func in fns {
        for block in &func.blocks {
            for (si, st) in block.statements.iter().enumerate() {
                let MirStmtKind::Call { func: callee, args, .. } = &st.kind else { continue };
                let Some(target) = fns.iter().find(|f| f.name == callee.name) else { continue };
                let site = (func.name.clone(), block.id, si);
                for (i, mode) in target.param_modes.iter().enumerate() {
                    if *mode != rask_ast::ty::ParamMode::Mutate || covered.contains(&(&site, i)) {
                        continue;
                    }
                    if !args.get(i).is_some_and(|a| lends_owned_slot(func, a)) {
                        unsure.insert((target.name.clone(), i));
                    }
                }
            }
        }
    }
    // A closure body is reached through calls with no name. When every one of
    // them can be listed, each is held to the same test as a call by name;
    // otherwise the body has callers nobody can see (#2017). A function whose
    // address is taken, or that sits in a vtable, never has its calls listed.
    let listed = targets.calls_reaching(fns);
    let by_address: HashSet<&str> = fns
        .iter()
        .flat_map(|f| f.blocks.iter().flat_map(|b| b.statements.iter()))
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign { rvalue: MirRValue::FuncAddr(name), .. } => Some(name.as_str()),
            _ => None,
        })
        .chain(vtable_candidates(fns).into_iter().map(|(_, f)| f.name.as_str()))
        .collect();
    for f in fns.iter().filter(|f| unnamed.contains(f.name.as_str())) {
        let sites = listed
            .as_ref()
            .filter(|_| !by_address.contains(f.name.as_str()))
            .and_then(|l| l.get(&f.name));
        for (i, mode) in f.param_modes.iter().enumerate() {
            if *mode != rask_ast::ty::ParamMode::Mutate {
                continue;
            }
            let all_lend_owned = sites.is_some_and(|sites| {
                sites.iter().all(|site| {
                    if covered.contains(&(site, i)) {
                        return true;
                    }
                    let Some(caller) = fns.iter().find(|c| c.name == site.0) else { return false };
                    let Some(block) = caller.blocks.iter().find(|b| b.id == site.1) else { return false };
                    let Some(MirStmtKind::ClosureCall { args, .. }) = block.statements.get(site.2).map(|st| &st.kind) else {
                        return false;
                    };
                    i >= 1 && args.get(i - 1).is_some_and(|a| lends_owned_slot(caller, a))
                })
            });
            if !all_lend_owned {
                unsure.insert((f.name.clone(), i));
            }
        }
    }
    // A parameter handed on from an unsure one is unsure too.
    loop {
        let before = unsure.len();
        for r in &reached {
            if let (CellReach::Param { callee, index, .. }, Some(t)) = (&r.at, &r.through) {
                if unsure.contains(t) {
                    unsure.insert((callee.clone(), *index));
                }
            }
        }
        if unsure.len() == before {
            break;
        }
    }

    let mut out = CellWriters::default();
    for r in reached {
        if r.through.as_ref().is_some_and(|t| unsure.contains(t)) {
            continue;
        }
        match r.at {
            CellReach::Capture(body, offset) => {
                out.captures.insert((body, offset));
            }
            CellReach::Param { .. } => {}
        }
    }
    out.params = writers.into_iter().filter(|w| !unsure.contains(w)).collect();
    out
}

/// Is `arg` the address of a slot inside something that owns what the slot
/// holds: a field of an aggregate (`mutate h.f`), or an element a collection
/// lends (`mutate v[1]`)? A closure in either is a heap one with a reference
/// of its own, released when the owner goes.
fn lends_owned_slot(func: &MirFunction, arg: &MirOperand) -> bool {
    let MirOperand::Local(mut at) = arg else { return false };
    for _ in 0..16 {
        if matches!(
            func.local_ty(at),
            Some(MirType::Struct(_) | MirType::Tuple(_) | MirType::Array { .. })
        ) {
            return true;
        }
        let def = func.blocks.iter().flat_map(|b| b.statements.iter()).find_map(|st| match &st.kind {
            MirStmtKind::Assign { dst, rvalue } if *dst == at => Some(Ok(rvalue)),
            MirStmtKind::Call { dst: Some(dst), func: callee, .. } if *dst == at => Some(Err(callee)),
            _ => None,
        });
        match def {
            Some(Ok(MirRValue::Use(MirOperand::Local(src)))) => at = *src,
            Some(Ok(MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(base),
                ..
            })) => at = *base,
            Some(Err(callee)) => return callee.returns_a_view(),
            _ => return false,
        }
    }
    false
}

/// Release what a closure variable held before a store from another frame
/// replaces it (`closure_cell_writers`).
fn release_before_body_stores(func: &mut MirFunction, writers: &CellWriters) {
    let mut addrs: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture { dst, offset, .. }
                if writers.captures.contains(&(func.name.clone(), *offset)) =>
            {
                Some(*dst)
            }
            _ => None,
        })
        .collect();
    let lent: Vec<LocalId> = func
        .params
        .iter()
        .enumerate()
        .filter(|(i, _)| writers.params.contains(&(func.name.clone(), *i)))
        .map(|(_, p)| p.id)
        .collect();
    addrs.extend(lent);
    if addrs.is_empty() {
        return;
    }
    extend_by_copies(func, &mut addrs);
    let tys: HashMap<LocalId, MirType> = func.locals.iter().map(|l| (l.id, l.ty.clone())).collect();
    for block in func.blocks.iter_mut() {
        let mut at = 0;
        while at < block.statements.len() {
            if let MirStmtKind::Store { addr, offset: 0, value: MirOperand::Local(v), .. } = &block.statements[at].kind {
                if addrs.contains(addr) {
                    let (addr, ty) = (*addr, tys[v].clone());
                    block.statements.insert(at, MirStmt::dummy(MirStmtKind::ReleaseSlot { addr, offset: 0, ty }));
                    at += 1;
                }
            }
            at += 1;
        }
    }
}

/// Locals whose value goes to a call as an argument: for a cell, its address
/// leaving for somewhere this pass can't follow.
fn addresses_handed_on(func: &MirFunction) -> HashSet<LocalId> {
    let mut out = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::Call { args, .. }
            | MirStmtKind::InterfaceCall { args, .. }
            | MirStmtKind::ClosureCall { args, .. } => {
                out.extend(args.iter().filter_map(|a| match a {
                    MirOperand::Local(id) => Some(*id),
                    _ => None,
                }));
            }
            _ => {}
        }
    }
    out
}

/// Closures `func` owns outright: heap ones it builds or clones, ones a call
/// hands back, and the copies made of them. A function that returns a closure
/// always hands over its own (the checker refuses to return a lent one or a
/// capture), and so does a call through a closure. A runtime call is not one
/// of those: `fs[0]` reads a closure the vector still holds, unless the
/// declaration says it moves out.
fn owned_closures(func: &MirFunction, all: &[MirFunction]) -> HashSet<LocalId> {
    let returns_closure = |name: &str| {
        all.iter().any(|f| f.name == name && !f.is_extern_c && matches!(f.ret_ty, MirType::FuncPtr(_)))
    };
    let mut out: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ClosureCreate { dst, heap: true, .. } => Some(*dst),
            MirStmtKind::Call { dst: Some(dst), func: callee, .. }
                if callee.name == "rask_closure_clone"
                    || returns_closure(&callee.name)
                    || (callee.moves_out() && matches!(func.local_ty(*dst), Some(MirType::FuncPtr(_)))) =>
            {
                Some(*dst)
            }
            MirStmtKind::ClosureCall { dst: Some(dst), .. } if matches!(func.local_ty(*dst), Some(MirType::FuncPtr(_))) => {
                Some(*dst)
            }
            _ => None,
        })
        .collect();
    extend_by_copies(func, &mut out);
    out
}

fn extend_by_copies(func: &MirFunction, set: &mut HashSet<LocalId>) {
    loop {
        let before = set.len();
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &st.kind {
                if set.contains(src) {
                    set.insert(*dst);
                }
            }
        }
        if set.len() == before {
            return;
        }
    }
}

/// Does `func` treat the closure cell at `addrs` as a plain variable: store a
/// closure it owns, release, read, hand the address to another body or
/// a `mutate` parameter that does the same?
fn cell_used_as_variable(
    func: &MirFunction,
    all: &[MirFunction],
    targets: &ClosureTargets,
    addrs: &mut HashSet<LocalId>,
    reached: &mut Vec<Reached>,
    through: &Option<(String, usize)>,
    depth: u32,
) -> bool {
    if depth > 8 {
        return false;
    }
    // `&g` is the cell's address too, the one `mutate g` hands over.
    loop {
        let before = addrs.len();
        extend_by_copies(func, addrs);
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } = &st.kind {
                if addrs.contains(src) {
                    addrs.insert(*dst);
                }
            }
        }
        if addrs.len() == before {
            break;
        }
    }
    let owned = owned_closures(func, all);
    for block in &func.blocks {
        for (si, st) in block.statements.iter().enumerate() {
            if !addrs.iter().any(|a| crate::analysis::uses::stmt_reads(st, *a)) {
                continue;
            }
            let fine = match &st.kind {
                MirStmtKind::Store { addr, offset: 0, value: MirOperand::Local(v), .. } => {
                    addrs.contains(addr) && owned.contains(v)
                }
                MirStmtKind::ReleaseSlot { addr, .. } | MirStmtKind::ZeroAggregate { local: addr } => {
                    addrs.contains(addr)
                }
                MirStmtKind::Assign { rvalue: MirRValue::Deref(MirOperand::Local(a)), .. }
                | MirStmtKind::Assign {
                    rvalue: MirRValue::Field { base: MirOperand::Local(a), byte_offset: Some(0), .. },
                    ..
                } => addrs.contains(a),
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(_)) | MirRValue::Ref(_) } => {
                    addrs.contains(dst)
                }
                MirStmtKind::Call { func: callee, args, .. } => {
                    let target = all.iter().find(|f| f.name == callee.name);
                    args.iter().enumerate().all(|(i, a)| {
                        let MirOperand::Local(a) = a else { return true };
                        if !addrs.contains(a) {
                            return true;
                        }
                        let Some(target) = target else { return false };
                        if target.param_modes.get(i) != Some(&rask_ast::ty::ParamMode::Mutate) {
                            return false;
                        }
                        let Some(param) = target.params.get(i) else { return false };
                        reached.push(Reached {
                            at: CellReach::Param {
                                callee: target.name.clone(),
                                index: i,
                                site: (func.name.clone(), block.id, si),
                            },
                            through: through.clone(),
                        });
                        let mut inner = HashSet::from([param.id]);
                        let into = Some((target.name.clone(), i));
                        cell_used_as_variable(target, all, targets, &mut inner, reached, &into, depth + 1)
                    })
                }
                // The same through a closure, to each body it is known to
                // reach: argument `i` is the body's parameter `i + 1`, after
                // the environment (#2017).
                MirStmtKind::ClosureCall { closure, args, .. } if !addrs.contains(closure) => {
                    let Some(bodies) = targets.known(&func.name, *closure) else { return false };
                    args.iter().enumerate().all(|(i, a)| {
                        let MirOperand::Local(a) = a else { return true };
                        if !addrs.contains(a) {
                            return true;
                        }
                        bodies.iter().all(|b| {
                            let Some(body) = all.iter().find(|f| f.name == *b) else { return false };
                            let index = i + 1;
                            if body.param_modes.get(index) != Some(&rask_ast::ty::ParamMode::Mutate) {
                                return false;
                            }
                            let Some(param) = body.params.get(index) else { return false };
                            reached.push(Reached {
                                at: CellReach::Param {
                                    callee: body.name.clone(),
                                    index,
                                    site: (func.name.clone(), block.id, si),
                                },
                                through: through.clone(),
                            });
                            let mut inner = HashSet::from([param.id]);
                            let into = Some((body.name.clone(), index));
                            cell_used_as_variable(body, all, targets, &mut inner, reached, &into, depth + 1)
                        })
                    })
                }
                MirStmtKind::ClosureCreate { func_name, captures, .. } => {
                    captures.iter().filter(|c| addrs.contains(&c.local_id)).all(|c| {
                        let Some(body) = all.iter().find(|f| f.name == *func_name) else { return false };
                        let mut inner: HashSet<LocalId> = body
                            .blocks
                            .iter()
                            .flat_map(|b| b.statements.iter())
                            .filter_map(|st| match &st.kind {
                                MirStmtKind::LoadCapture { dst, offset, .. } if *offset == c.offset => Some(*dst),
                                _ => None,
                            })
                            .collect();
                        reached.push(Reached {
                            at: CellReach::Capture(func_name.clone(), c.offset),
                            through: through.clone(),
                        });
                        c.by_ref && cell_used_as_variable(body, all, targets, &mut inner, reached, through, depth + 1)
                    })
                }
                _ => false,
            };
            if !fine {
                return false;
            }
        }
        if addrs.iter().any(|a| crate::analysis::uses::terminator_reads(&block.terminator, *a)) {
            return false;
        }
    }
    true
}

/// An `ensure` body runs after the exits release the cell, so a cell it reads
/// is left to the rules that know about that.
fn ensure_captured(func: &MirFunction, cell: LocalId) -> bool {
    func.blocks.iter().flat_map(|b| b.statements.iter()).any(|st| {
        matches!(&st.kind, MirStmtKind::EnsureHookRegister { captures, .. } if captures.iter().any(|c| c.local_id == cell))
    })
}


/// Container variables the frame lends by address and nothing else:
/// `touch(mutate v)`.
///
/// `mutate` makes the variable memory-resident (`transform::addr_taken`), so
/// `mut v = Vec.new()` is a store into a slot and every later `v` a load. The
/// cell rule frees a slot once, on the way out of the frame, and only where
/// the store dominates the exit — which a variable declared inside a loop body
/// never does. Every turn's vector leaked (#1423).
///
/// A slot like this is just the variable, so the ownership plan can follow it
/// like any other name: the store binds the slot to the value, a load is a
/// copy, and the address is a view that keeps the value needed. The plan then
/// frees the old value where the variable's scope ends, on the back edge
/// before the next turn's store.
///
/// Only when the address provably stays in this frame: copied, read through,
/// written through, or handed to a call that doesn't keep it. Anything else is
/// left to the cell rule.
struct LentSlots {
    /// The slot, and what frees the container in it.
    slots: HashMap<LocalId, &'static str>,
    /// The slots' addresses and their copies.
    refs: HashSet<LocalId>,
    /// Which slot each address is of.
    slot_of: HashMap<LocalId, LocalId>,
    /// The addresses that are closures: a closure that captured the slot by
    /// reference holds its address in its environment.
    closures: HashSet<LocalId>,
}

fn lent_slots(
    func: &MirFunction,
    tracked: &mut HashMap<LocalId, &'static str>,
    kept: &HashMap<String, Vec<bool>>,
    owned_params: &HashSet<LocalId>,
) -> LentSlots {
    let mut out = LentSlots {
        slots: HashMap::new(),
        refs: HashSet::new(),
        slot_of: HashMap::new(),
        closures: HashSet::new(),
    };
    // A closure that captures the variable by reference lends it the same
    // way: its environment holds the slot's address, and whatever the closure
    // is given to may run it and write the variable. Lowering only leaves a
    // capture by reference on a closure that doesn't outlive the frame
    // (`closures::closures_handed_on` turns the rest into copies), so that
    // address stays here by construction.
    //
    // Not one an `ensure` body reads: that body runs in the cleanup chain,
    // after the frame's last use, and the cell rule frees those instead. Nor
    // one a closure copied in.
    let mut lent_to_closures: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    let mut not_lent: HashSet<LocalId> = HashSet::new();
    let mut lent_to_ensures: Vec<LocalId> = Vec::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::ClosureCreate { dst, captures, .. } => {
                for c in captures {
                    if c.by_ref {
                        lent_to_closures.entry(c.local_id).or_default().push(*dst);
                    } else {
                        not_lent.insert(c.local_id);
                    }
                }
            }
            MirStmtKind::EnsureHookRegister { captures, .. } => {
                for c in captures.iter().filter(|c| c.by_ref) {
                    lent_to_ensures.push(c.local_id);
                }
            }
            _ => {}
        }
    }
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let stmts: Vec<&MirStmt> = func.blocks.iter().flat_map(|b| b.statements.iter()).collect();

    let mut candidates: Vec<LocalId> = slots_whose_address_is_taken(func).into_iter().collect();
    candidates.extend(lent_to_closures.keys().copied());
    candidates.extend(lent_to_ensures);
    candidates.sort_by_key(|l| l.0);
    candidates.dedup();
    for slot in candidates {
        if not_lent.contains(&slot) || !is_container_shaped(func, slot) {
            continue;
        }
        // An owned parameter a closure captures by reference is a slot the
        // call fills on entry. Followed as one, a load out of it is the
        // parameter, so storing that load gives the parameter away: a loop
        // body that captures its item and keeps it on one branch frees it
        // only on the other (#2008).
        let entry_fill = owned_params.contains(&slot).then(|| tracked.get(&slot).copied()).flatten();
        if params.contains(&slot) && entry_fill.is_none() {
            continue;
        }
        // Every store fills the whole slot with a local, and at least one of
        // them is a container this frame made.
        let mut free = entry_fill;
        let mut whole = true;
        for stmt in &stmts {
            if let MirStmtKind::Store { addr, offset, value, .. } = &stmt.kind {
                if *addr != slot {
                    continue;
                }
                match crate::analysis::uses::operand_local(value) {
                    Some(v) if *offset == 0 => {
                        if let Some(f) = tracked.get(&v) {
                            free = Some(*f);
                        }
                    }
                    _ => whole = false,
                }
            }
        }
        let (true, Some(free)) = (whole, free) else { continue };

        // The addresses, followed through copies, and checked to stay here.
        let mut refs: HashSet<LocalId> = stmts
            .iter()
            .filter_map(|s| match &s.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } if *src == slot => Some(*dst),
                _ => None,
            })
            .collect();
        loop {
            let before = refs.len();
            for s in &stmts {
                if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &s.kind {
                    if refs.contains(src) {
                        refs.insert(*dst);
                    }
                }
            }
            if refs.len() == before {
                break;
            }
        }
        if !refs.iter().all(|r| address_stays_here(func, &stmts, *r, &refs, kept)) {
            continue;
        }
        let mut closures: HashMap<LocalId, &'static str> = lent_to_closures
            .get(&slot)
            .into_iter()
            .flatten()
            .map(|c| (*c, free))
            .collect();
        follow_copies(func, &mut closures);

        // What comes back out of the slot is the same container, under a new
        // name, and so is a read through one of its addresses.
        tracked.insert(slot, free);
        for s in &stmts {
            if let MirStmtKind::Assign { dst, rvalue } = &s.kind {
                let from = match rvalue {
                    MirRValue::Deref(MirOperand::Local(src)) => *src == slot || refs.contains(src),
                    MirRValue::Field { base: MirOperand::Local(src), .. } => refs.contains(src),
                    _ => false,
                };
                if from && is_container_shaped(func, *dst) {
                    tracked.insert(*dst, free);
                }
            }
        }
        for r in refs.iter().chain(closures.keys()) {
            tracked.insert(*r, free);
            out.slot_of.insert(*r, slot);
        }
        out.slots.insert(slot, free);
        out.refs.extend(refs);
        out.closures.extend(closures.into_keys());
    }
    if !out.slots.is_empty() {
        follow_copies(func, tracked);
    }
    out
}

/// Does the address in `r` stay in this frame? Copied into another of `refs`,
/// read through, written through, or lent to a call.
fn address_stays_here(
    func: &MirFunction,
    stmts: &[&MirStmt],
    r: LocalId,
    refs: &HashSet<LocalId>,
    kept: &HashMap<String, Vec<bool>>,
) -> bool {
    use crate::analysis::uses;
    for stmt in stmts {
        if !uses::stmt_reads(stmt, r) {
            continue;
        }
        let ok = match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(_)) } => refs.contains(dst),
            MirStmtKind::Assign {
                rvalue: MirRValue::Deref(MirOperand::Local(_)) | MirRValue::Field { base: MirOperand::Local(_), .. },
                ..
            } => true,
            MirStmtKind::Store { addr, value, .. } => *addr == r && uses::operand_local(value) != Some(r),
            MirStmtKind::Call { func: fref, args, .. } => args
                .iter()
                .enumerate()
                .filter(|(_, a)| uses::operand_local(a) == Some(r))
                .all(|(i, _)| !call_keeps_argument(fref, i, kept)),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    !func.blocks.iter().any(|b| uses::terminator_reads(&b.terminator, r))
}

/// The other way a variable becomes a cell: something took its address.
///
/// Lowering makes a `Ref` for a `mutate` argument and for a closure capture;
/// the capture half is already in the set above, so in practice this is the
/// `mutate` ones. Same shape and the same reasoning either way. The store into
/// the slot hands the value to the slot, and the `&` on the slot puts the slot
/// itself out of reach, so no name is left holding it — a `StringBuilder`
/// passed to a `mutate` parameter was freed by nobody (#1203). A slot whose
/// address only ever goes to `mutate` parameters is `lent_slots`' instead, and
/// is left out of the cells.
///
/// What makes the slot safe to free at every exit is mem.parameters/PM2: a
/// `mutate` callee has to leave something valid there, so whether it replaced
/// the value or not, the slot names a live one when it returns. The frame
/// consuming it itself is the case that isn't safe — `b.build()` takes the
/// builder — and the `find_escaping` check on what comes out of the cell is
/// what stands the release down there.
fn slots_whose_address_is_taken(func: &MirFunction) -> HashSet<LocalId> {
    let mut addr_of: HashMap<LocalId, LocalId> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } = &stmt.kind {
            addr_of.insert(*dst, *src);
        }
    }
    addr_of.into_values().collect()
}

/// Free what a capture cell holds, on the way out of the frame.
///
/// The cell holds the container's handle rather than being it, so this is a
/// load and then the free — unlike a plain local, whose name already is the
/// thing to hand over.
///
/// Only where the store dominates the exit. A cell filled in one arm of an
/// `if` holds nothing on the other, and the load would free whatever the stack
/// had there — reliably zero on a fresh frame, and 0xAAAA… under
/// `RASK_POISON_STACK=1`, which is the point of that flag.
fn insert_cell_drops(func: &mut MirFunction, cells: &[(LocalId, InCell, BlockId)]) {
    release_overwritten_slot_cells(func, cells);
    empty_closure_cells_at_entry(func, cells);
    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let exits: Vec<Vec<(usize, HashSet<LocalId>)>> = cells.iter().map(|(cell, _, _)| exit_blocks(func, *cell)).collect();
    let slots = arm_cells(func, cells);

    let mut next = func.locals.iter().map(|l| l.id.0).max().unwrap_or(0) + 1;
    // `return s` with ensure bodies still to run: the bodies read the cell and
    // may write it, and the cell's release comes at the end of the last one
    // that does. The caller gets a reference of its own, taken at the return,
    // so the string it gets is the one `s` held there.
    for (cell, holds, _) in cells {
        if !matches!(holds, InCell::Slot(MirType::Buffer)) {
            continue;
        }
        for block in func.blocks.iter_mut() {
            let returns_cell = matches!(
                &block.terminator.kind,
                MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(v)), cleanup_chain }
                    if v == cell && !cleanup_chain.is_empty()
            );
            if !returns_cell {
                continue;
            }
            block.statements.push(MirStmt::dummy(MirStmtKind::RcIncContents { local: *cell, for_hand_off: false }));
        }
    }
    for (ci, (cell, holds, store_block)) in cells.iter().enumerate() {
        for (block_idx, consumed_here) in exits[ci].clone() {
            let exit = func.blocks[block_idx].id;
            if !starts_empty(holds) && !dom.dominates(*store_block, exit) {
                continue;
            }
            // Already handed away on the path that reaches this exit.
            if consumed_here.contains(cell) {
                continue;
            }
            if let Some(slot) = slots.get(cell) {
                func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::UnwindDisarm { slot: *slot }));
            }
            // A string's 16-byte header *is* the slot, so the release takes the
            // cell's address as it stands — the same shape a struct field's
            // string gets, and the statement that already says it.
            let free = match holds {
                InCell::Slot(ty) => {
                    // Returning the variable hands its reference to the caller.
                    // A closure is returned as a load out of the cell, which
                    // took a reference of its own (`retain_loads_handed_on`).
                    let returns_cell = matches!(
                        &func.blocks[block_idx].terminator.kind,
                        MirTerminatorKind::Return { value: Some(MirOperand::Local(v)) }
                            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(v)), .. }
                            if v == cell
                    );
                    if returns_cell {
                        continue;
                    }
                    func.blocks[block_idx].statements.push(MirStmt::dummy(
                        MirStmtKind::ReleaseSlot {
                            addr: *cell,
                            offset: 0,
                            ty: ty.clone(),
                        },
                    ));
                    continue;
                }
                InCell::Handle(free) => *free,
            };
            let tmp = LocalId(next);
            next += 1;
            func.locals.push(crate::MirLocal {
                id: tmp,
                name: None,
                ty: MirType::Ptr,
                is_param: false,
            });
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::Assign {
                dst: tmp,
                rvalue: MirRValue::Deref(MirOperand::Local(*cell)),
            }));
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: free_call(free),
                args: vec![MirOperand::Local(tmp)],
            }));
        }
    }
}

/// A store into a string or closure cell after the one that filled it
/// replaces what the cell owns, so the old one is released first. A variable
/// an `ensure` reads is such a cell, and `s = "…"` after the ensure (or in its
/// body) left every string it covered unreleased (#1917).
fn release_overwritten_slot_cells(func: &mut MirFunction, cells: &[(LocalId, InCell, BlockId)]) {
    for (cell, holds, fill_block) in cells {
        let InCell::Slot(ty) = holds else { continue };
        let is_store = |s: &MirStmt| matches!(&s.kind, MirStmtKind::Store { addr, offset: 0, .. } if addr == cell);
        let is_zero = |s: &MirStmt| matches!(&s.kind, MirStmtKind::ZeroAggregate { local } if local == cell);
        for block in func.blocks.iter_mut() {
            let mut skip_first = block.id == *fill_block && !starts_empty(holds);
            // A whole write of an addressed cell starts by zeroing it
            // (`addr_taken`), so the old string has to go before the zero,
            // not before the store that follows it.
            let mut zeroed_at: Option<usize> = None;
            let mut at = 0;
            while at < block.statements.len() {
                if is_zero(&block.statements[at]) {
                    zeroed_at = Some(at);
                } else if is_store(&block.statements[at]) {
                    let write_starts = zeroed_at.take().unwrap_or(at);
                    if skip_first {
                        skip_first = false;
                    } else {
                        block.statements.insert(
                            write_starts,
                            MirStmt::dummy(MirStmtKind::ReleaseSlot { addr: *cell, offset: 0, ty: ty.clone() }),
                        );
                        at += 1;
                    }
                }
                at += 1;
            }
        }
    }
}

/// A closure cell holds no closure until something is stored in it. Emptied on
/// entry, every store can release what the cell held and every exit can
/// release what it holds, whether or not the store ran: a release of an empty
/// closure does nothing. Without that, a variable filled in a loop body
/// (`let h = make(); let k = || h()`) was released by none of its turns, since
/// the fill neither came first nor ran on every path to the exit.
fn starts_empty(holds: &InCell) -> bool {
    matches!(holds, InCell::Slot(MirType::FuncPtr(_)))
}

fn empty_closure_cells_at_entry(func: &mut MirFunction, cells: &[(LocalId, InCell, BlockId)]) {
    let Some(entry) = func.blocks.first_mut() else { return };
    for (cell, holds, _) in cells {
        if !starts_empty(holds) {
            continue;
        }
        entry.statements.insert(
            0,
            MirStmt::dummy(MirStmtKind::Store {
                addr: *cell,
                offset: 0,
                value: MirOperand::Constant(crate::MirConst::Int(0)),
                store_size: 8,
                field: None,
            }),
        );
    }
}

/// Arm each cell's unwind slot right after the store that fills it: from there
/// on a panic releases what it holds, the way the frame's last exit would
/// (ctrl.panic/U6). The release reads the cell when it runs, so the slot holds
/// the cell's address.
///
/// Not a cell an `ensure`'s resource names: on the path where that resource is
/// consumed the cell's contents were handed away, and nothing here can see
/// when that happened to disarm it.
fn arm_cells(func: &mut MirFunction, cells: &[(LocalId, InCell, BlockId)]) -> HashMap<LocalId, u32> {
    let registered: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ResourceRegister { slot: Some(slot), .. } => Some(*slot),
            _ => None,
        })
        .collect();
    let mut next_slot = crate::analysis::ownership::next_unwind_slot(func);
    let mut next_local = func.locals.iter().map(|l| l.id.0).max().unwrap_or(0) + 1;
    let mut slots = HashMap::new();
    for (cell, holds, store_block) in cells {
        if registered.contains(cell) {
            continue;
        }
        let Some(bi) = func.blocks.iter().position(|b| b.id == *store_block) else { continue };
        let Some(si) = func.blocks[bi]
            .statements
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::Store { addr, offset: 0, .. } if addr == cell))
        else {
            continue;
        };
        let stmts = match holds {
            InCell::Slot(ty) => vec![MirStmt::dummy(MirStmtKind::ReleaseSlot { addr: *cell, offset: 0, ty: ty.clone() })],
            InCell::Handle(free) => {
                let tmp = LocalId(next_local);
                next_local += 1;
                func.locals.push(crate::MirLocal { id: tmp, name: None, ty: MirType::Ptr, is_param: false });
                vec![
                    MirStmt::dummy(MirStmtKind::Assign { dst: tmp, rvalue: MirRValue::Deref(MirOperand::Local(*cell)) }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: free_call(free),
                        args: vec![MirOperand::Local(tmp)],
                    }),
                ]
            }
        };
        let slot = next_slot;
        next_slot += 1;
        func.blocks[bi].statements.insert(
            si + 1,
            MirStmt::dummy(MirStmtKind::UnwindArm {
                slot,
                value: *cell,
                release: crate::UnwindRelease { placeholder: *cell, stmts },
            }),
        );
        slots.insert(*cell, slot);
    }
    slots
}

/// Where a free belongs: the blocks that are the last thing to run before the
/// frame returns.
///
/// For a plain `return` that is the block itself. A `cleanup_return` is not:
/// it names a chain of blocks that run *after* it, which is where an `ensure`
/// body lives, and appending the free to the `cleanup_return` block frees the
/// value the ensure body is about to use. So for those, follow the chain to its
/// ends. The chain's bodies run one after another: codegen turns the end of
/// each into a jump to the next, and only the last body's ends into the actual
/// return. A cell is freed at the end of the last body that reads it, or of
/// the last body if none does. Freeing at the end of every body freed a `Vec`
/// two `ensure`s read before the second one read it. Freeing only at the end
/// of the chain lost a value made after an early return: that end is shared
/// with the early return's chain, where the value doesn't exist, so the store
/// doesn't dominate it.
///
/// Each exit also carries the slots that must not be freed *there*: the ones
/// an `ensure`'s registered resource names, on the arm reached because that
/// resource turned out to be consumed. Freeing on that arm would be a second
/// free of something already handed away.
///
/// Both blunter answers are wrong, and each was tried. Skipping the whole arm
/// stands down every release in the function, not just the one at issue —
/// `ensure out.close()` beside an explicit `out.close()` takes that arm on
/// every run, and an unrelated `Vec` in the same function ended up freed by
/// nobody. Freeing on it regardless leans on `find_escaping` having stood the
/// cell down for any value anything hands away, which is true today and is
/// still an argument rather than a check. `ResourceRegister` names its slot, so
/// this asks the question directly: this arm, this slot, no free.
fn exit_blocks(func: &MirFunction, cell: LocalId) -> Vec<(usize, HashSet<LocalId>)> {
    let index: HashMap<BlockId, usize> =
        func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    // The runtime token each registration hands back, and the slot it is about.
    let registered: HashMap<LocalId, LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ResourceRegister { dst, slot: Some(slot), .. } => Some((*dst, *slot)),
            _ => None,
        })
        .collect();
    // `_n = rask_resource_is_consumed(tok)`: the flag, and the slot it answers
    // for. Its `then` arm is the one that must not free that slot.
    let consumed_flag: HashMap<LocalId, LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Call { dst: Some(dst), func: fref, args }
                if fref.name == crate::RESOURCE_IS_CONSUMED =>
            {
                let tok = args.first().and_then(crate::analysis::uses::operand_local)?;
                Some((*dst, *registered.get(&tok)?))
            }
            _ => None,
        })
        .collect();

    let mut out: Vec<(usize, HashSet<LocalId>)> = Vec::new();
    let mut seen: HashSet<BlockId> = HashSet::new();
    // A block, what was consumed on the way, and the bodies still to run
    // after the one this block is in.
    let mut queue: Vec<(BlockId, HashSet<LocalId>, Vec<BlockId>)> = Vec::new();
    for block in &func.blocks {
        match &block.terminator.kind {
            MirTerminatorKind::Return { .. } => {
                if seen.insert(block.id) {
                    out.push((index[&block.id], HashSet::new()));
                }
            }
            MirTerminatorKind::CleanupReturn { cleanup_chain, .. } => {
                if cleanup_chain.is_empty() {
                    if seen.insert(block.id) {
                        out.push((index[&block.id], HashSet::new()));
                    }
                } else {
                    let last_reader = cleanup_chain
                        .iter()
                        .rposition(|head| ensure_body_reads(func, *head, cell))
                        .unwrap_or(cleanup_chain.len() - 1);
                    let run = &cleanup_chain[..=last_reader];
                    queue.push((run[0], HashSet::new(), run[1..].to_vec()));
                }
            }
            _ => {}
        }
    }

    let mut walked: HashSet<(BlockId, bool, Vec<BlockId>)> = HashSet::new();
    while let Some((bid, consumed, rest)) = queue.pop() {
        // Keyed on whether anything was consumed on the way, so a block reached
        // both ways is walked both ways and keeps the narrower answer.
        if !walked.insert((bid, !consumed.is_empty(), rest.clone())) {
            continue;
        }
        let Some(block) = func.blocks.iter().find(|b| b.id == bid) else { continue };
        let next: Vec<(BlockId, HashSet<LocalId>)> = match &block.terminator.kind {
            MirTerminatorKind::Branch { cond, then_block, else_block } => {
                let taken = crate::analysis::uses::operand_local(cond).and_then(|c| consumed_flag.get(&c));
                let mut then_set = consumed.clone();
                if let Some(slot) = taken {
                    then_set.insert(*slot);
                }
                vec![(*then_block, then_set), (*else_block, consumed.clone())]
            }
            _ => crate::analysis::cfg::successors(&block.terminator)
                .into_iter()
                .map(|b| (b, consumed.clone()))
                .collect(),
        };
        if next.is_empty() {
            if let Some((&body, later)) = rest.split_first() {
                queue.push((body, consumed, later.to_vec()));
                continue;
            }
            match out.iter_mut().find(|(i, _)| *i == index[&bid]) {
                // Reached twice: free only what neither path consumed.
                Some((_, skip)) => skip.retain(|s| consumed.contains(s)),
                None => {
                    seen.insert(bid);
                    out.push((index[&bid], consumed));
                }
            }
        } else {
            queue.extend(next.into_iter().map(|(b, c)| (b, c, rest.clone())));
        }
    }
    out
}

/// Whether the `ensure` body starting at `head` reads `cell`.
fn ensure_body_reads(func: &MirFunction, head: BlockId, cell: LocalId) -> bool {
    let mut seen: HashSet<BlockId> = HashSet::new();
    let mut stack = vec![head];
    while let Some(bid) = stack.pop() {
        if !seen.insert(bid) {
            continue;
        }
        let Some(block) = func.blocks.iter().find(|b| b.id == bid) else { continue };
        if block.statements.iter().any(|st| crate::analysis::uses::stmt_reads(st, cell)) {
            return true;
        }
        if !matches!(
            block.terminator.kind,
            MirTerminatorKind::Unreachable | MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. }
        ) {
            stack.extend(crate::analysis::cfg::successors(&block.terminator));
        }
    }
    false
}

/// Does `cell` still hold a container this frame owns after every closure
/// that captured it has run?
///
/// The capture arrives in the closure as the cell's address, so a write back
/// into it is a `Store` through whatever that address was copied into. A
/// closure that only reads the cell leaves it as it was. One that writes it
/// keeps it the frame's when the write releases what was there first and
/// stores something the closure owned: a `take` parameter or a container it
/// just made. `keep = v` in a body that takes `v` is that, and asking for read
/// only left whatever the last write stored to nobody (#2008).
///
/// Handing the address to another function fails it: what happens on the far
/// side isn't visible here. So does a closure this pass can't find, which is
/// the safe answer: a leak rather than a free of something the closure
/// replaced.
fn cell_stays_owned_through_closures(
    func: &MirFunction,
    all: &[MirFunction],
    cell: LocalId,
) -> bool {
    for block in &func.blocks {
        for stmt in &block.statements {
            // An ensure hook's thunk is a closure body in everything that
            // matters here: it reaches the cell through the same address and
            // can write back through it.
            let (func_name, captures) = match &stmt.kind {
                MirStmtKind::ClosureCreate { func_name, captures, .. } => (func_name, captures),
                MirStmtKind::EnsureHookRegister { thunk, captures } => (thunk, captures),
                _ => continue,
            };
            for cap in captures.iter().filter(|c| c.local_id == cell) {
                let Some(callee) = all.iter().find(|f| f.name == *func_name) else {
                    return false;
                };
                if !slot_stays_owned(all, callee, cap.offset, &mut HashSet::new()) {
                    return false;
                }
            }
        }
    }
    true
}

/// Within one closure body: is the capture at `offset` only loaded from, or
/// overwritten only with a release of the old value and a value the body owns?
///
/// A closure the body builds that captures the same address is asked the same
/// question: two nested `for` loops over sequences hand the outer variable to
/// the inner body that way, and stopping there left a vector built across both
/// loops to nobody.
fn slot_stays_owned(
    all: &[MirFunction],
    callee: &MirFunction,
    offset: u32,
    seen: &mut HashSet<(String, u32)>,
) -> bool {
    if !seen.insert((callee.name.clone(), offset)) {
        return true;
    }
    // Every name the capture's address reaches, copies included.
    let mut addrs: HashSet<LocalId> = HashSet::new();
    for block in &callee.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::LoadCapture { dst, offset: o, .. } = &stmt.kind {
                if *o == offset {
                    addrs.insert(*dst);
                }
            }
        }
    }
    if addrs.is_empty() {
        return true;
    }
    loop {
        let mut grew = false;
        for block in &callee.blocks {
            for stmt in &block.statements {
                if let MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Use(MirOperand::Local(src)),
                } = &stmt.kind
                {
                    if addrs.contains(src) && addrs.insert(*dst) {
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            break;
        }
    }
    let owned = values_the_body_owns(callee);
    for block in &callee.blocks {
        let mut released = false;
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::ReleaseSlot { addr, offset: 0, .. } if addrs.contains(addr) => released = true,
                MirStmtKind::Store { addr, offset: 0, value: MirOperand::Local(v), .. }
                    if addrs.contains(addr) && released && owned.contains(v) =>
                {
                    released = false;
                }
                MirStmtKind::Store { addr, .. } if addrs.contains(addr) => return false,
                MirStmtKind::ArrayStore { base, .. } if addrs.contains(base) => return false,
                MirStmtKind::Call { args, .. } | MirStmtKind::ClosureCall { args, .. } => {
                    if args.iter().any(|a| {
                        matches!(a, MirOperand::Local(id) if addrs.contains(id))
                    }) {
                        return false;
                    }
                }
                MirStmtKind::ClosureCreate { func_name, captures, .. } => {
                    for c in captures.iter().filter(|c| addrs.contains(&c.local_id)) {
                        let Some(inner) = all.iter().find(|f| f.name == *func_name) else {
                            return false;
                        };
                        if !c.by_ref || !slot_stays_owned(all, inner, c.offset, seen) {
                            return false;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// The locals of a closure body holding a value the body may hand on: its
/// `take` parameters and the containers it makes, through copies.
fn values_the_body_owns(callee: &MirFunction) -> HashSet<LocalId> {
    let mut owned: HashSet<LocalId> = callee
        .params
        .iter()
        .zip(&callee.param_modes)
        .filter(|(_, m)| **m == ParamMode::Take)
        .map(|(p, _)| p.id)
        .collect();
    for stmt in callee.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Call { dst: Some(d), func, .. } = &stmt.kind {
            if free_for(func).is_some() {
                owned.insert(*d);
            }
        }
    }
    // A `take` parameter a closure in the body captures by reference lives in
    // a cell, and the body reads it back with `x = *p`: that read is the
    // parameter (#2008).
    let taken: HashSet<LocalId> = owned.clone();
    loop {
        let mut grew = false;
        for stmt in callee.blocks.iter().flat_map(|b| b.statements.iter()) {
            let src = match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => Some((dst, src)),
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(src)) }
                    if taken.contains(src) && callee.local_ty(*dst) == callee.local_ty(*src) =>
                {
                    Some((dst, src))
                }
                _ => None,
            };
            if let Some((dst, src)) = src {
                if owned.contains(src) && owned.insert(*dst) {
                    grew = true;
                }
            }
        }
        if !grew {
            return owned;
        }
    }
}

fn every_def_is_fresh(
    func: &MirFunction,
    local: LocalId,
    fresh: &HashMap<LocalId, &'static str>,
    edges: &Passes,
) -> bool {
    let mut any = false;
    for block in &func.blocks {
        for stmt in &block.statements {
            if crate::analysis::uses::stmt_def(stmt) != Some(local) {
                continue;
            }
            any = true;
            let ok = match &stmt.kind {
                MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(src)), .. } => {
                    fresh.contains_key(src)
                }
                MirStmtKind::Phi { args, .. } => args.iter().all(|(_, op)| {
                    matches!(op, MirOperand::Local(src) if fresh.contains_key(src))
                }),
                // A call that may hand back one of its arguments is owned like
                // all of them.
                _ => edges.get(&local).is_some_and(|srcs| srcs.iter().all(|(_, s)| fresh.contains_key(s))),
            };
            if !ok {
                return false;
            }
        }
    }
    any
}


/// Returned, stored, captured, or handed to something that keeps it.
///
/// A container method borrows its receiver; every other argument is asked
/// whether the callee actually keeps it, because borrow is the default
/// (`mem.parameters/PM1`) and treating a read as a handover left the container
/// to nobody (#1047).
/// Is this address only ever handed to a call that doesn't keep it?
///
/// Taking a container's address marks it escaping, because an address handed
/// out could be kept. For a `mutate` parameter it demonstrably isn't:
/// mem.parameters/PM1 says a parameter is borrowed and PM2 says the value is
/// still the caller's when the call returns, so the address dies with the call.
/// `params_a_callee_keeps` is what answers that per parameter, and it is
/// already what the rest of this pass asks about a container passed by value.
///
/// Without this, passing a container by address would leak it in every frame
/// that does — which is what stopped `mutate v: Vec<i32>` from being fixed at
/// all: replacing a whole `mutate` container writes into the vector's header
/// instead of the caller's slot, and the only way to fix that is to pass the
/// address (#1197).
///
/// A whitelist, so an address used a way nobody thought about still counts as
/// escaping: that answer leaks, and the other one frees something a callee
/// kept.
fn lent_for_the_call(
    func: &MirFunction,
    addr: LocalId,
    kept: &HashMap<String, Vec<bool>>,
) -> bool {
    let mut handed_to_a_call = false;
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let reads_it = crate::analysis::uses::stmt_reads(stmt, addr);
        if !reads_it {
            continue;
        }
        match &stmt.kind {
            MirStmtKind::Call { func: fref, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    if crate::analysis::uses::operand_local(arg) != Some(addr) {
                        continue;
                    }
                    if call_keeps_argument(fref, i, kept) {
                        return false;
                    }
                    handed_to_a_call = true;
                }
            }
            _ => return false,
        }
    }
    for block in &func.blocks {
        if crate::analysis::uses::terminator_reads(&block.terminator, addr) {
            return false;
        }
    }
    handed_to_a_call
}

fn find_escaping(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
    keeps: &Keeps,
) -> (HashSet<LocalId>, HashMap<LocalId, HashSet<BlockId>>) {
    let kept = keeps.by_name;
    let mut escaping = HashSet::new();
    // Where a container is handed to a stdlib method declared `take self`.
    //
    // That is not the same as escaping, and treating it as such is why
    // `string.from_utf8` leaked a `StringBuilder` for every byte sequence it
    // rejected: `out.build()` takes the builder away, so every name for it was
    // marked escaping and no path got a release — including the eight that
    // return a `Utf8Error` before `build()` is ever reached.
    //
    // So it is recorded per block instead, and the placement below frees only
    // where no path in has consumed it yet.
    let mut consumed: HashMap<LocalId, HashSet<BlockId>> = HashMap::new();
    let mark = |op: &MirOperand, escaping: &mut HashSet<LocalId>| {
        if let MirOperand::Local(id) = op {
            if containers.contains_key(id) {
                escaping.insert(*id);
            }
        }
    };

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    let skip_receiver = fref.borrows_receiver() && !args.is_empty();
                    let takes_receiver = fref.consumes_receiver() && !args.is_empty();
                    for (i, arg) in args.iter().enumerate() {
                        if skip_receiver && i == 0 {
                            continue;
                        }
                        if takes_receiver && i == 0 {
                            if let MirOperand::Local(id) = arg {
                                if containers.contains_key(id) {
                                    consumed.entry(*id).or_default().insert(block.id);
                                }
                            }
                            continue;
                        }
                        if call_keeps_argument(fref, i, kept) {
                            mark(arg, &mut escaping);
                        }
                    }
                }
                // An interface call names a method and a vtable slot, so the
                // by-name answer above has nothing to look up — every argument
                // read as handed over. `io.copy` reads a `Vec<u8>` out of one
                // interface call and passes it to another, and that second call
                // left it to nobody (#1199). `self` isn't in the argument list,
                // so an implementation's parameters sit one to the right.
                MirStmtKind::InterfaceCall { method_name, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if keeps.interface_call(method_name, i) {
                            mark(arg, &mut escaping);
                        }
                    }
                }
                MirStmtKind::ClosureCall { closure, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if keeps.closure_call(&func.name, *closure, i, args.len()) {
                            mark(arg, &mut escaping);
                        }
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::InterfaceBox { value, .. } => {
                    mark(value, &mut escaping);
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
                    for cap in captures {
                        if containers.contains_key(&cap.local_id) {
                            escaping.insert(cap.local_id);
                        }
                    }
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } => {
                    if containers.contains_key(src) && !lent_for_the_call(func, *dst, kept) {
                        escaping.insert(*src);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(op) }
            | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
                mark(op, &mut escaping);
            }
            _ => {}
        }
    }
    (escaping, consumed)
}

/// Containers an `ensure` body gives away: `ensure tx.close()`.
///
/// Either the ensure consumes it at scope exit or something else did first and
/// the ensure stood down (mem.linear/L6); one way or the other it is consumed
/// once, so the frame has no free of its own to add. It added one anyway: the
/// scope-exit free ran before the cleanup chain, then the ensure closed the
/// sender again — a double free for any `ensure tx.close()`.
///
/// The ensure body is a thunk, and the same question `find_escaping` asks of a
/// frame is asked of it: does the capture escape, or reach a `take self`?
fn consumed_by_an_ensure(
    func: &MirFunction,
    all: &[MirFunction],
    fresh: &HashMap<LocalId, &'static str>,
    keeps: &Keeps,
) -> HashSet<LocalId> {
    let mut out = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::EnsureHookRegister { thunk, captures } = &stmt.kind else { continue };
        let Some(body) = all.iter().find(|f| f.name == *thunk) else { continue };
        for cap in captures.iter().filter(|c| !c.by_ref && fresh.contains_key(&c.local_id)) {
            // What the capture is called inside the thunk, and every copy of it.
            let mut names: HashMap<LocalId, &'static str> = HashMap::new();
            for st in body.blocks.iter().flat_map(|b| b.statements.iter()) {
                match &st.kind {
                    MirStmtKind::LoadCapture { dst, offset, .. } if *offset == cap.offset => {
                        names.insert(*dst, fresh[&cap.local_id]);
                    }
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                        if names.contains_key(src) =>
                    {
                        names.insert(*dst, fresh[&cap.local_id]);
                    }
                    _ => {}
                }
            }
            let (escaping, consumed) = find_escaping(body, &names, keeps);
            if !escaping.is_empty() || !consumed.is_empty() {
                out.insert(cap.local_id);
            }
        }
    }
    out
}

