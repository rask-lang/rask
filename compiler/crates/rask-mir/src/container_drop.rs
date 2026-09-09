// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Container drop insertion.
//!
//! `Vec.new()` allocates a `RaskVec` and, on the first push, a data array.
//! Nothing in the pipeline ever freed either (#1027) — a vector built in a
//! loop leaked the handle, the array, and every heap string in it, once per
//! turn. `Map`, `Rack` and `Pool` are the same — and for the last two that
//! sentence stayed aspirational until #1048: their frees existed in the runtime
//! and no constructor of theirs was on the list this pass reads.
//!
//! Modelled on `trait_drop.rs`, which solves the same problem for trait
//! objects, and shares its rules: track only *fresh* allocations — the
//! destinations of the constructors below, plus whatever a chain of moves or
//! phis carries forward — and drop only what never hands ownership to anything
//! else. A container read back out of a struct field, another container, or a
//! call's return value is somebody else's, and freeing it double-frees.
//!
//! One rule differs, and it has to. A container's own methods take it as their
//! first argument, so the "any call argument escapes" rule of the trait pass
//! would mark every vector escaping the moment anything was pushed onto it, and
//! nothing would ever be dropped. A container-prefixed call borrows its
//! *receiver* — argument zero — and escapes the rest: `v.push(inner)` hands
//! `inner` over and keeps `v`.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockId, FunctionRef, LocalId, MirBlock, MirConst, MirFunction, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminatorKind, MirType,
};

/// What frees a container made by `ctor`, or `None` if this isn't one.
///
/// `elem_strs::CTORS` names both — the calls that hand a container back and
/// the free that matches each. The free takes nothing but the container: it
/// knows what its elements are.
fn free_for(ctor: &str) -> Option<&'static str> {
    crate::elem_strs::free_fn(ctor)
}

pub fn insert_container_drops(fns: &mut Vec<MirFunction>) {
    // Which bodies a call through a closure can reach, so the by-name answer
    // below covers those calls too (#943). Built first: it reads only the MIR,
    // and the "hands a container back" fixed point needs it.
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    let handing_over = functions_that_hand_a_container_back(fns, &targets);
    let kept = params_a_callee_keeps(fns);
    // A snapshot, because tracing a container through a capture cell has to
    // read the closure that captured it while the frame it belongs to is being
    // rewritten.
    let snapshot: Vec<MirFunction> = fns.to_vec();
    for func in fns.iter_mut() {
        insert_for_function(func, &snapshot, &handing_over, &kept, &targets);
    }
    let glue = env_drop_glue(fns, &handing_over, &targets);
    fns.extend(glue);
}

/// The suffix a closure's environment-drop function carries.
///
/// Codegen looks the name up rather than being told: a closure block is freed
/// by whichever frame ends up holding it, which is usually not the one that
/// built it, so the block has to carry how to release what it owns. The name is
/// the only thing the two sides need to agree on.
pub const ENV_DROP_SUFFIX: &str = "__env_drop";

/// One function per closure that *owns* a container it captured, freeing what
/// the environment holds.
///
/// A heap closure with a by-value capture owns that value — `own` moves it in,
/// and `find_escaping` below keeps the frame from freeing it as well. Nothing
/// then released it: `closure_drop` gave back the block and left the vector
/// inside it, which is the leak #1045 closed around ("the block would need drop
/// glue next to its size"). Every adapter chain captures its source, so this is
/// most of what the sequence files leak.
///
/// A *by-ref* capture is not this: the slot holds an address into the frame that
/// built the closure, and that frame still owns the value.
fn env_drop_glue(
    fns: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    targets: &crate::closure_targets::ClosureTargets,
) -> Vec<MirFunction> {
    // How many escaping closures capture each container by value, per frame.
    // Two means the container has two candidate owners and the answer is to
    // leave it alone: two glues freeing one vector is a use-after-free, where
    // none is a leak. `v.map(f)` twice off one vector is exactly that shape —
    // the receiver is *borrowed* (`mem.parameters/PM1`), so the frame owns it
    // and neither chain may release it.
    let mut capturers: HashMap<(String, LocalId), usize> = HashMap::new();
    for func in fns {
        for block in &func.blocks {
            for stmt in &block.statements {
                let MirStmtKind::ClosureCreate { captures, heap: true, .. } = &stmt.kind else {
                    continue;
                };
                for c in captures.iter().filter(|c| !c.by_ref) {
                    *capturers.entry((func.name.clone(), c.local_id)).or_default() += 1;
                }
            }
        }
    }

    // What each closure body gives up by itself, by capture offset. A capture
    // the body consumes is not the glue's to free:
    //
    //     spawn(own || { for i in 1..n { tx.send(i) }  tx.close() })
    //
    // `close` takes the sender away — closing an end *is* dropping it — so the
    // glue freeing it again on the way out aborted the process on a double
    // free. Nothing had noticed because until channels were released at all,
    // no capture was both owned and consumable.
    let consumed: HashMap<&str, HashSet<u32>> = fns
        .iter()
        .map(|f| (f.name.as_str(), captures_the_body_consumes(f)))
        .collect();

    // One glue per closure *function*, because the block header holds a
    // function address and the name is all codegen has to find it by. So every
    // site that builds this closure has to agree about what its environment
    // owns — inlining copies a create site into each caller, and a site that
    // owns nothing must not get a glue that frees something.
    let mut answers: HashMap<String, Vec<Vec<(u32, &'static str)>>> = HashMap::new();
    let mut order: Vec<(String, Option<String>)> = Vec::new();
    for func in fns {
        let fresh = collect_fresh_containers_with(func, fns, handing_over, targets);
        let reach = strict_reach(func);
        let def_block = defining_blocks(func);
        for block in &func.blocks {
            for stmt in &block.statements {
                let MirStmtKind::ClosureCreate { func_name, captures, heap: true, .. } = &stmt.kind
                else {
                    continue;
                };
                // One create site can still run many times. A loop is how that
                // happens, and `capturers` counts sites, so it can't see it —
                //
                //     for id in 0..n { spawn(own || { tx.send(x) }).detach() }
                //
                // handed every task's glue the same sender, and the second
                // drop closed the channel: the tutorial's `estimate_pi` began
                // reading "receive on closed channel". A capture *defined
                // inside the same loop* is the opposite case and the one the
                // glue exists for — `.map()` in a loop builds a fresh
                // environment each turn and each closure owns its own.
                let create_repeats = reach.get(&block.id).is_some_and(|r| r.contains(&block.id));
                let made_each_turn = |c: &crate::ClosureCapture| {
                    !create_repeats
                        || def_block.get(&c.local_id).is_some_and(|d| {
                            reach.get(&block.id).is_some_and(|r| r.contains(d))
                                && reach.get(d).is_some_and(|r| r.contains(&block.id))
                        })
                };
                let mut owned: Vec<(u32, &'static str)> = captures
                    .iter()
                    .filter(|c| !c.by_ref)
                    .filter(|c| {
                        capturers
                            .get(&(func.name.clone(), c.local_id))
                            .copied()
                            .unwrap_or(0)
                            == 1
                    })
                    .filter(|c| made_each_turn(c))
                    .filter(|c| {
                        !consumed
                            .get(func_name.as_str())
                            .is_some_and(|offs| offs.contains(&c.offset))
                    })
                    .filter_map(|c| fresh.get(&c.local_id).map(|free| (c.offset, *free)))
                    .collect();
                owned.sort();
                if !answers.contains_key(func_name) {
                    order.push((func_name.clone(), func.source_file.clone()));
                }
                answers.entry(func_name.clone()).or_default().push(owned);
            }
        }
    }

    let mut out = Vec::new();
    for (name, source_file) in order {
        let sites = &answers[&name];
        let first = &sites[0];
        if first.is_empty() || sites.iter().any(|s| s != first) {
            continue;
        }
        out.push(build_env_drop(&name, first, source_file));
    }
    out
}

/// Which blocks each block can reach in one step or more.
///
/// One step *or more* is the point: a block that appears in its own set is on
/// a cycle, which is how "this statement runs many times" is asked here.
fn strict_reach(func: &MirFunction) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut reach: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for block in &func.blocks {
        reach.insert(
            block.id,
            crate::analysis::cfg::successors(&block.terminator).into_iter().collect(),
        );
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            let onward: HashSet<BlockId> = reach[&block.id]
                .iter()
                .filter_map(|s| reach.get(s))
                .flatten()
                .copied()
                .collect();
            let set = reach.get_mut(&block.id).unwrap();
            let before = set.len();
            set.extend(onward);
            changed |= set.len() != before;
        }
    }
    reach
}

/// Where each local is written. SSA, so one place each — a phi's destination
/// belongs to the block holding the phi.
fn defining_blocks(func: &MirFunction) -> HashMap<LocalId, BlockId> {
    let mut out = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                out.entry(dst).or_insert(block.id);
            }
        }
    }
    out
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
        if !rask_stdlib::mir_metadata::consumes_receiver(&fref.name) {
            continue;
        }
        let Some(recv) = args.first().and_then(crate::analysis::uses::operand_local) else { continue };
        if let Some(&off) = from_capture.get(&recv) {
            out.insert(off);
        }
    }
    out
}

/// `<closure>__env_drop(env: ptr)` — load each owned container out of the
/// environment and free it.
///
/// `LoadCapture` is the same statement the closure's own body reads a capture
/// with, so the offsets can't drift from how they were written.
fn build_env_drop(
    closure_name: &str,
    owned: &[(u32, &'static str)],
    source_file: Option<String>,
) -> MirFunction {
    let env = LocalId(0);
    let mut locals = vec![crate::MirLocal {
        id: env,
        name: Some("__env".to_string()),
        ty: MirType::Ptr,
        is_param: true,
    }];
    let mut statements = Vec::new();
    for (i, (offset, free)) in owned.iter().enumerate() {
        let held = LocalId(i as u32 + 1);
        locals.push(crate::MirLocal {
            id: held,
            name: None,
            ty: MirType::Ptr,
            is_param: false,
        });
        statements.push(MirStmt::dummy(MirStmtKind::LoadCapture {
            dst: held,
            env_ptr: env,
            offset: *offset,
            access: crate::CaptureAccess::Value,
        }));
        statements.push(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(free.to_string()),
            args: vec![MirOperand::Local(held)],
        }));
    }
    let entry = BlockId(0);
    MirFunction {
        name: format!("{closure_name}{ENV_DROP_SUFFIX}"),
        params: vec![crate::MirLocal {
            id: env,
            name: Some("__env".to_string()),
            ty: MirType::Ptr,
            is_param: true,
        }],
        ret_ty: MirType::Void,
        locals,
        blocks: vec![MirBlock {
            id: entry,
            statements,
            terminator: crate::MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
        }],
        entry_block: entry,
        is_extern_c: false,
        source_file,
    }
}

/// Per function, which of its parameters it *keeps* rather than just reads.
///
/// Borrow is the default (`mem.parameters/PM1`): `func first(v: Vec<i32>)`
/// reads the vector and the caller still owns it. `find_escaping` treated every
/// argument as given away, so `first(v)` left the vector to nobody — the caller
/// had handed it over, and the callee never built it, so neither freed it
/// (#1047).
///
/// Read off the body rather than the declaration, because MIR doesn't carry
/// parameter modes and `closures.rs` already answers the same question about
/// closure arguments the same way. A parameter that is returned, stored,
/// captured, or passed on to something that keeps it counts as kept; anything
/// else is a borrow.
///
/// That reads like an approximation of `take` and, for a program that compiles,
/// isn't one: PM6 makes giving away a borrow a compile error, so a body that
/// keeps a parameter has `take` on the declaration and a body that doesn't,
/// doesn't. `dst.push(v)` on a plain `v: Vec<i32>` is rejected with "cannot give
/// away `v` — it's borrowed, not owned". The two answers can't disagree.
///
/// Where it could still drift is MIR the checker never saw, or a body this pass
/// can't read. Both land on "kept", which leaves a container to nobody — a leak,
/// where the other direction would be a double free.
///
/// Grows to a fixed point because "passed on to something that keeps it" is
/// itself one of these answers.
pub(crate) fn params_a_callee_keeps(fns: &[MirFunction]) -> HashMap<String, Vec<bool>> {
    let mut kept: HashMap<String, Vec<bool>> =
        fns.iter().map(|f| (f.name.clone(), vec![false; f.params.len()])).collect();

    loop {
        let mut grew = false;
        for func in fns {
            for (i, param) in func.params.iter().enumerate() {
                if kept.get(&func.name).is_some_and(|v| v[i]) {
                    continue;
                }
                if param_is_kept_by(func, param.id, &kept) {
                    if let Some(v) = kept.get_mut(&func.name) {
                        v[i] = true;
                        grew = true;
                    }
                }
            }
        }
        if !grew {
            return kept;
        }
    }
}

/// Does `func` hold on to what arrived in `param`?
fn param_is_kept_by(
    func: &MirFunction,
    param: LocalId,
    kept: &HashMap<String, Vec<bool>>,
) -> bool {
    // Follow the value through renames: lowering copies a parameter into a
    // local before doing anything with it often enough that reading only the
    // parameter's own id saw nothing.
    let mut names: HashSet<LocalId> = HashSet::from([param]);
    loop {
        let before = names.len();
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                        if names.contains(src) =>
                    {
                        names.insert(*dst);
                    }
                    MirStmtKind::Phi { dst, args } => {
                        if args.iter().any(|(_, op)| matches!(op, MirOperand::Local(s) if names.contains(s))) {
                            names.insert(*dst);
                        }
                    }
                    _ => {}
                }
            }
        }
        if names.len() == before {
            break;
        }
    }

    let holds = |op: &MirOperand| matches!(op, MirOperand::Local(id) if names.contains(id));

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        if holds(arg) && call_keeps_argument(fref, i, kept) {
                            return true;
                        }
                    }
                }
                MirStmtKind::ClosureCall { args, .. } | MirStmtKind::TraitCall { args, .. } => {
                    if args.iter().any(holds) {
                        return true;
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::TraitBox { value, .. } => {
                    if holds(value) {
                        return true;
                    }
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
                    if captures.iter().any(|c| names.contains(&c.local_id)) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(op) }
            | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
                if holds(op) {
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
    let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
    rask_stdlib::mir_metadata::keeps_argument(head, index)
}

/// Functions that build a container and return it, so the caller owns what
/// comes back.
///
/// Without this a container only ever belonged to the frame that called the
/// constructor. `make_vec()` returning a `Vec<string>` was freed by nobody:
/// the callee saw it escape through the return and left it alone, and the
/// caller saw a call result, which is somebody else's by default. Nine
/// allocations a call, silently.
///
/// It's a fixed point because handing one back is transitive — a wrapper that
/// returns what `make_vec` gave it is handing one back too. Only functions
/// this pass can see count: a container from the runtime (`split`, `map.keys`)
/// still has no owner named here, because reading an element out of one
/// doesn't take a reference — #1035.
fn functions_that_hand_a_container_back(
    fns: &[MirFunction],
    targets: &crate::closure_targets::ClosureTargets,
) -> HashMap<String, HandBack> {
    let mut handing: HashMap<String, HandBack> = HashMap::new();
    loop {
        let mut grew = false;
        for func in fns {
            if handing.contains_key(&func.name) {
                continue;
            }
            let fresh = collect_fresh_containers_with(func, fns, &handing, targets);
            if fresh.is_empty() {
                continue;
            }
            // Every returning path has to hand back one this frame made. One
            // that doesn't is the whole risk here: `lookup` returns
            // `index.get(word) ?? Vec.new()` — a fresh vector on one path and
            // the map's own on the other — and calling that "the caller's"
            // frees a vector the map still holds.
            let mut free_fn: Option<&'static str> = None;
            let mut all_fresh = true;
            let mut wrapped = false;
            let mut any = false;
            for b in &func.blocks {
                let MirTerminatorKind::Return { value: Some(v), .. } = &b.terminator.kind else {
                    continue;
                };
                any = true;
                let MirOperand::Local(id) = v else {
                    all_fresh = false;
                    continue;
                };
                if let Some(f) = fresh.get(id) {
                    free_fn = Some(f);
                    continue;
                }
                // The container may be *inside* what is returned. `-> Vec<i64>?`
                // and `-> Vec<i64> or E` return the wrapper aggregate, and the
                // vector is a slot in it — so the returned local is never the
                // fresh one, `all_fresh` was false, and nobody freed the vector
                // the caller unwrapped and used (#1117).
                match container_stored_into(func, *id, &fresh) {
                    WrapperHoldings::Fresh(f) => {
                        free_fn = Some(f);
                        wrapped = true;
                    }
                    // The error path of a `T or E` stores no container at all.
                    // That is not a path handing one back, and not a path
                    // handing back somebody else's either.
                    WrapperHoldings::None => {}
                    WrapperHoldings::Foreign => all_fresh = false,
                }
            }
            if let (true, true, Some(free)) = (any, all_fresh, free_fn) {
                handing.insert(func.name.clone(), HandBack { free, wrapped });
                grew = true;
            }
        }
        if !grew {
            return handing;
        }
    }
}

/// How a function hands a container to its caller.
#[derive(Clone, Copy)]
struct HandBack {
    /// What frees it.
    free: &'static str,
    /// The container is a slot inside what is returned (`-> Vec<i64>?`), not
    /// the returned value itself. The caller owns what it reads out of that
    /// slot, not the wrapper.
    wrapped: bool,
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
        if *addr != wrapper || !is_container_shaped(func, *v) {
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
fn is_container_shaped(func: &MirFunction, local: LocalId) -> bool {
    func.locals
        .iter()
        .chain(func.params.iter())
        .find(|l| l.id == local)
        .is_some_and(|l| matches!(l.ty, MirType::Ptr))
}

fn insert_for_function(
    func: &mut MirFunction,
    all: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    kept: &HashMap<String, Vec<bool>>,
    targets: &crate::closure_targets::ClosureTargets,
) {
    let fresh = collect_fresh_containers_with(func, all, handing_over, targets);
    if fresh.is_empty() {
        return;
    }
    let escaping = find_escaping(func, &fresh, kept);
    let moved_away = find_moved_away(func, &fresh);
    let already_freed = find_already_freed(func, &fresh);
    let fresh: HashMap<LocalId, &'static str> = fresh
        .into_iter()
        .filter(|(id, _)| !already_freed.contains(id))
        .collect();
    if fresh.is_empty() {
        return;
    }

    let mut droppable: HashMap<LocalId, &'static str> = fresh
        .iter()
        .filter(|(id, _)| !escaping.contains(id) && !moved_away.contains(id))
        .map(|(id, f)| (*id, *f))
        .collect();

    // The moved-away rule assumes a chain: `a` into `b` into `c`, where only
    // the last name still holds the value. Inlining breaks that. `v.min()` and
    // `v.max()` both copy the same vector into their own parameter local, so
    // one `Vec.new()` reaches two surviving names — and each one freed it.
    //
    // A value that fans out like that is left alone. Leaking it is the wrong
    // answer; freeing it twice is a worse one.
    //
    // "Surviving" means a name that would actually get a free emitted, which is
    // why the placement is worked out first. A name with nowhere to put one
    // isn't a second free — it's no free. `for x in v` inside a loop copies `v`
    // into the loop body, and that copy's only candidate site was the
    // back-edge, which it no longer qualifies for; counting it anyway made the
    // group look like it fanned out and `v` was freed nowhere at all (#1071).
    let groups = value_groups(func, &fresh);

    // A container packed into the aggregate this function returns is gone to
    // the caller, under every name it has here. `for x in v` copies the vector
    // into a loop-local, and one line later the original is stored into the
    // `T or E` being returned — so one name was marked escaping and the other
    // wasn't, and the free landed on the one that wasn't. It freed the vector
    // the caller was then handed, and `v.len()` on it read whatever the
    // allocator had written there (#1119).
    //
    //     func build(n: i64) -> Vec<i64> or Refused {
    //         …
    //         for x in v { total = total + x }   // _36 = _27
    //         return v                           // *(_23+24) = _27
    //     }                                      // Vec_free(_36)  ← the same one
    //
    // The same body returning a plain `Vec<i64>` was fine, which is what made
    // this look like a wrapper bug rather than a grouping one. Only this
    // relation propagates across a group; escaping in general does not, because
    // most of what it marks is a store into an aggregate that never leaves.
    let handed_over = packed_into_a_returned_aggregate(func, &fresh);
    for group in &groups {
        if group.iter().any(|id| handed_over.contains(id)) {
            for id in group {
                droppable.remove(id);
            }
        }
    }

    let placed = placed_locals(func, &droppable, &groups);
    for group in &groups {
        let survivors = group
            .iter()
            .filter(|id| droppable.contains_key(id) && placed.contains(id))
            .count();
        if survivors > 1 {
            for id in group {
                droppable.remove(id);
            }
        }
    }
    droppable.retain(|id, _| placed.contains(id));

    // A container in a capture cell is reached through a store, which the rule
    // above reads as handing it over — so it never becomes droppable and its
    // free goes in separately, keyed on the cell rather than on a name.
    let cells = cells_this_frame_frees(func, all, &fresh, kept);
    for (cell, _, _) in &cells {
        for group in &groups {
            if group.iter().any(|id| stores_into(func, *id, *cell)) {
                for id in group {
                    droppable.remove(id);
                }
            }
        }
    }

    if !droppable.is_empty() {
        insert_drops(func, &droppable, &groups);
    }
    if !cells.is_empty() {
        insert_cell_drops(func, &cells);
    }
}

/// Is `value` what gets stored into `cell`?
fn stores_into(func: &MirFunction, value: LocalId, cell: LocalId) -> bool {
    func.blocks.iter().flat_map(|b| b.statements.iter()).any(|stmt| {
        matches!(
            &stmt.kind,
            MirStmtKind::Store { addr, value: MirOperand::Local(v), .. }
                if *addr == cell && *v == value
        )
    })
}

/// Locals holding a container this frame owns, mapped to how to free it: the
/// destination of a constructor, and of a call to a function that builds one
/// and hands it back.
fn collect_fresh_containers_with(
    func: &MirFunction,
    all: &[MirFunction],
    handing_over: &HashMap<String, HandBack>,
    targets: &crate::closure_targets::ClosureTargets,
) -> HashMap<LocalId, &'static str> {
    let mut fresh: HashMap<LocalId, &'static str> = HashMap::new();
    // Calls whose result is a wrapper holding the container, rather than the
    // container: the caller owns what it unwraps, and the wrapper is a value.
    let mut unwrap_for: HashMap<LocalId, &'static str> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
                let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
                // A monomorphized name carries a `$` suffix.
                let base = head.split('$').next().unwrap_or(head);
                if let Some(free) = free_for(base) {
                    fresh.insert(*dst, free);
                } else if let Some(back) = handing_over.get(&fref.name) {
                    // The callee's own constructor decided which free this is.
                    if back.wrapped {
                        unwrap_for.insert(*dst, back.free);
                    } else {
                        fresh.insert(*dst, back.free);
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
                        match handing_over.get(body) {
                            // A wrapper needs the unwrap step below, which is
                            // keyed on the call's own destination — one closure
                            // returning a bare container and another a wrapped
                            // one have no single answer, so neither gets one.
                            Some(back) if !back.wrapped => {
                                let same = agreed.is_none_or(|f| f == back.free);
                                agreed = Some(back.free);
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
    // What comes out of such a wrapper is the container, and it is this frame's
    // now. Read as a field of the returned aggregate — the tag beside it is not
    // pointer-shaped, so the payload is the only slot this can pick up (#1117).
    let mut unwrapped: HashSet<LocalId> = HashSet::new();
    if !unwrap_for.is_empty() {
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } = &stmt.kind
            else {
                continue;
            };
            let MirOperand::Local(src) = base else { continue };
            if let Some(free) = unwrap_for.get(src) {
                if is_container_shaped(func, *dst) {
                    fresh.insert(*dst, free);
                    unwrapped.insert(*dst);
                }
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    // The same value under a new name: follow it, so the name still holding it
    // at its death is the one that gets freed.
    follow_copies(func, &mut fresh);

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
        follow_copies(func, &mut fresh);
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
            // the definition, not a copy of some other name (#943).
            MirStmtKind::Call { dst: Some(dst), .. }
            | MirStmtKind::ClosureCall { dst: Some(dst), .. } => Some(*dst),
            _ => None,
        })
        .filter(|id| fresh.contains_key(id))
        .chain(from_cells)
        // A container read out of the wrapper a callee handed back is reached
        // without a copy too. The pruning below asks how a *copy* was reached,
        // and the answer for these is "it wasn't" — a Field read is the
        // definition, not a copy of some other name.
        .chain(unwrapped)
        .collect();

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
            .filter(|id| !made_here.contains(id) && !every_def_is_fresh(func, *id, &fresh))
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
/// Only where the cell can hold nothing else. One store reaches it in the whole
/// program — this frame's, plus any closure that captured it — and that store's
/// value is one of this frame's own. A second store is a second possible
/// container, and calling that one this frame's would free something somebody
/// else still holds.
///
/// And only for a load this frame *returns*. The question this answers is
/// "does this frame hand the caller a container to free" — nothing else. A load
/// the frame goes on to use itself is already handled, and calling it fresh
/// changed answers it had no business changing: `let count = || items.len()`
/// reads `items` out of its cell three times, and putting all three in one
/// value group made a vector that used to be freed once stop being freed at all
/// (t26 went from 4 leaked allocations to 6, t31 from 21 to 40).
fn fresh_through_cells(
    func: &MirFunction,
    all: &[MirFunction],
    fresh: &HashMap<LocalId, &'static str>,
) -> Vec<(LocalId, &'static str)> {
    // addr → what got stored there, across every offset. Offsets aren't
    // separated: a cell holding one container is written at one offset, and a
    // second write anywhere in it is enough to give up.
    let mut stores: HashMap<LocalId, Vec<MirOperand>> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Store { addr, value, .. } = &stmt.kind {
                stores.entry(*addr).or_default().push(value.clone());
            }
        }
    }

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
        return Vec::new();
    }

    let mut out = Vec::new();
    for (cell, values) in &stores {
        let [MirOperand::Local(src)] = values.as_slice() else { continue };
        let Some(free) = fresh.get(src).copied() else { continue };
        if !cell_is_read_only_in_closures(func, all, *cell) {
            continue;
        }
        for block in &func.blocks {
            for stmt in &block.statements {
                if let MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Deref(MirOperand::Local(addr)),
                } = &stmt.kind
                {
                    if addr == cell && returned.contains(dst) {
                        out.push((*dst, free));
                    }
                }
            }
        }
    }
    out
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
    kept: &HashMap<String, Vec<bool>>,
) -> Vec<(LocalId, &'static str, BlockId)> {
    let by_ref_cells: HashSet<LocalId> = func
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
    if by_ref_cells.is_empty() {
        return Vec::new();
    }

    let mut stores: HashMap<LocalId, Vec<(MirOperand, BlockId)>> = HashMap::new();
    let mut loaded: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    let mut handed_on: HashSet<LocalId> = HashSet::new();
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
                // The address itself going somewhere this pass can't follow.
                // A `ClosureCreate` is the one that made the cell and is read
                // through `cell_is_read_only_in_closures` instead.
                MirStmtKind::Call { args, .. } | MirStmtKind::TraitCall { args, .. } => {
                    for arg in args {
                        if let MirOperand::Local(id) = arg {
                            handed_on.insert(*id);
                        }
                    }
                }
                MirStmtKind::ClosureCall { args, .. } => {
                    for arg in args {
                        if let MirOperand::Local(id) = arg {
                            handed_on.insert(*id);
                        }
                    }
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

    let mut out = Vec::new();
    for cell in &by_ref_cells {
        if handed_on.contains(cell) {
            continue;
        }
        let Some(values) = stores.get(cell) else { continue };
        let [(MirOperand::Local(src), store_block)] = values.as_slice() else { continue };
        let Some(free) = fresh.get(src).copied() else { continue };
        if !cell_is_read_only_in_closures(func, all, *cell) {
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
        if !find_escaping(func, &carried, kept).is_empty() {
            continue;
        }
        out.push((*cell, free, *store_block));
    }
    out
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
fn insert_cell_drops(func: &mut MirFunction, cells: &[(LocalId, &'static str, BlockId)]) {
    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let return_blocks: Vec<usize> = func
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            matches!(
                b.terminator.kind,
                MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. }
            )
        })
        .map(|(i, _)| i)
        .collect();

    let mut next = func.locals.iter().map(|l| l.id.0).max().unwrap_or(0) + 1;
    for block_idx in return_blocks {
        let exit = func.blocks[block_idx].id;
        for (cell, free, store_block) in cells {
            if !dom.dominates(*store_block, exit) {
                continue;
            }
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
                func: FunctionRef::internal(free.to_string()),
                args: vec![MirOperand::Local(tmp)],
            }));
        }
    }
}

/// Does every closure that captured `cell` only ever read it?
///
/// The capture arrives in the closure as the cell's address, so a write back
/// into it is a `Store` through whatever that address was copied into. Handing
/// the address to another function counts as a write too — what happens on the
/// far side isn't visible here.
///
/// `false` for a closure this pass can't find, which is the safe answer: a leak
/// rather than a free of something the closure replaced.
fn cell_is_read_only_in_closures(
    func: &MirFunction,
    all: &[MirFunction],
    cell: LocalId,
) -> bool {
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::ClosureCreate { func_name, captures, .. } = &stmt.kind else {
                continue;
            };
            for cap in captures.iter().filter(|c| c.local_id == cell) {
                let Some(callee) = all.iter().find(|f| f.name == *func_name) else {
                    return false;
                };
                if !slot_is_read_only(callee, cap.offset) {
                    return false;
                }
            }
        }
    }
    true
}

/// Within one closure body: is the capture at `offset` only ever loaded from?
fn slot_is_read_only(callee: &MirFunction, offset: u32) -> bool {
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
    for block in &callee.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Store { addr, .. } if addrs.contains(addr) => return false,
                MirStmtKind::ArrayStore { base, .. } if addrs.contains(base) => return false,
                MirStmtKind::Call { args, .. } | MirStmtKind::ClosureCall { args, .. } => {
                    if args.iter().any(|a| {
                        matches!(a, MirOperand::Local(id) if addrs.contains(id))
                    }) {
                        return false;
                    }
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
                    if captures.iter().any(|c| addrs.contains(&c.local_id)) {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

fn every_def_is_fresh(
    func: &MirFunction,
    local: LocalId,
    fresh: &HashMap<LocalId, &'static str>,
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
                _ => false,
            };
            if !ok {
                return false;
            }
        }
    }
    any
}

/// Containers the lowering already frees itself.
///
/// The iterator and sort lowerings build a scratch vector and free it when
/// they're done. Adding a second free there is a double free — `sort_by_key`
/// segfaulted on the way out of `main`. Anything with an explicit free on it
/// is somebody else's business.
fn find_already_freed(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut freed = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { continue };
            let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
            let base = head.split('$').next().unwrap_or(head);
            if !base.ends_with("_free") && !base.ends_with("_free_elems") {
                continue;
            }
            if let Some(MirOperand::Local(id)) = args.first() {
                if containers.contains_key(id) {
                    freed.insert(*id);
                }
            }
        }
    }
    freed
}

/// Containers written into an aggregate that one of this function's `return`s
/// hands back.
///
/// Distinct from `find_escaping`'s general store rule, which fires for a store
/// into any aggregate — most of which stay in the frame and are freed here.
fn packed_into_a_returned_aggregate(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let returned: HashSet<LocalId> = func
        .blocks
        .iter()
        .filter_map(|b| match &b.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)), .. } => Some(*id),
            _ => None,
        })
        .collect();
    if returned.is_empty() {
        return HashSet::new();
    }
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::Store { addr, value: MirOperand::Local(v), .. }
                if returned.contains(addr) && containers.contains_key(v) =>
            {
                Some(*v)
            }
            _ => None,
        })
        .collect()
}

/// The names that hold one value: copies and phi merges.
fn value_groups(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
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
                    if containers.contains_key(dst) && containers.contains_key(src) =>
                {
                    union(&mut parent, *dst, *src);
                }
                MirStmtKind::Phi { dst, args } if containers.contains_key(dst) => {
                    for (_, op) in args {
                        if let MirOperand::Local(src) = op {
                            if containers.contains_key(src) {
                                union(&mut parent, *dst, *src);
                            }
                        }
                    }
                }
                // Two loads from one cell are one container under two names,
                // and each name got its own free: `let count = || items.len()`
                // reads `items` out of its cell once per push and once for the
                // closure, and `main` came out with two `Vec_free`s on the same
                // pointer — "free(): invalid pointer" (#1060). The cell is the
                // group's representative, whether or not it holds a container
                // itself.
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Deref(MirOperand::Local(addr)),
                } if containers.contains_key(dst) => {
                    union(&mut parent, *dst, *addr);
                }
                _ => {}
            }
        }
    }

    let mut groups: HashMap<LocalId, HashSet<LocalId>> = HashMap::new();
    for id in containers.keys() {
        let root = find(&mut parent, *id);
        groups.entry(root).or_default().insert(*id);
    }
    groups.into_values().collect()
}

/// Returned, stored, captured, or handed to something that keeps it.
///
/// A container method borrows its receiver; every other argument is asked
/// whether the callee actually keeps it, because borrow is the default
/// (`mem.parameters/PM1`) and treating a read as a handover left the container
/// to nobody (#1047).
fn find_escaping(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
    kept: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    let mut escaping = HashSet::new();
    let mut mark = |op: &MirOperand, escaping: &mut HashSet<LocalId>| {
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
                    let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
                    let skip_receiver = rask_stdlib::mir_metadata::borrows_receiver(head) && !args.is_empty();
                    for (i, arg) in args.iter().enumerate() {
                        if skip_receiver && i == 0 {
                            continue;
                        }
                        if call_keeps_argument(fref, i, kept) {
                            mark(arg, &mut escaping);
                        }
                    }
                }
                MirStmtKind::ClosureCall { args, .. } | MirStmtKind::TraitCall { args, .. } => {
                    for arg in args {
                        mark(arg, &mut escaping);
                    }
                }
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::TraitBox { value, .. } => {
                    mark(value, &mut escaping);
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
                    for cap in captures {
                        if containers.contains_key(&cap.local_id) {
                            escaping.insert(cap.local_id);
                        }
                    }
                }
                MirStmtKind::Assign { rvalue: MirRValue::Ref(src), .. } => {
                    if containers.contains_key(src) {
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
    escaping
}

/// Copied into another local, or merged through a phi: the new name owns it.
fn find_moved_away(
    func: &MirFunction,
    containers: &HashMap<LocalId, &'static str>,
) -> HashSet<LocalId> {
    let mut moved = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if containers.contains_key(src) && src != dst =>
                {
                    moved.insert(*src);
                }
                MirStmtKind::Phi { args, .. } => {
                    for (_, op) in args {
                        if let MirOperand::Local(id) = op {
                            if containers.contains_key(id) {
                                moved.insert(*id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    moved
}

/// Which of `droppable` would actually get a free emitted somewhere.
///
/// The same walk `insert_drops` does, minus the emitting. Split out because
/// the fan-out rule has to count names that get a free, not names that are
/// merely eligible for one.
fn placed_locals(
    func: &MirFunction,
    droppable: &HashMap<LocalId, &'static str>,
    groups: &[HashSet<LocalId>],
) -> HashSet<LocalId> {
    plan_drops(func, droppable, groups)
        .into_iter()
        .flat_map(|(_, locals)| locals)
        .collect()
}

/// Free before every return the container's definition dominates, and before
/// every loop back-edge it was built inside. Same placement rules as
/// `trait_drop.rs`, and for the same reasons — see the comments there on why
/// dominance is what decides it rather than block order.
fn insert_drops(
    func: &mut MirFunction,
    droppable: &HashMap<LocalId, &'static str>,
    groups: &[HashSet<LocalId>],
) {
    for (block_idx, locals) in plan_drops(func, droppable, groups) {
        for local in locals {
            let free = droppable[&local];
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(free.to_string()),
                args: vec![MirOperand::Local(local)],
            }));
        }
    }
}

/// Where each free would go: one entry per block that needs them.
fn plan_drops(
    func: &MirFunction,
    droppable: &HashMap<LocalId, &'static str>,
    groups: &[HashSet<LocalId>],
) -> Vec<(usize, Vec<LocalId>)> {
    let dom = crate::analysis::dominators::DominatorTree::build(func);

    let mut defined_in_block: HashMap<LocalId, usize> = HashMap::new();
    // Every local's defining block, not just the droppable ones: a back-edge
    // has to ask where the *allocation* was made, and the name holding it
    // there is usually a copy of one defined further out.
    let mut def_of_any: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                def_of_any.insert(dst, idx);
                if droppable.contains_key(&dst) {
                    defined_in_block.insert(dst, idx);
                }
            }
        }
    }

    let mut to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        match &block.terminator.kind {
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. } => {
                let drops: Vec<LocalId> = droppable
                    .keys()
                    .copied()
                    .filter(|id| {
                        defined_in_block.get(id).is_some_and(|&def_idx| {
                            dom.dominates(func.blocks[def_idx].id, block.id)
                        })
                    })
                    .collect();
                if !drops.is_empty() {
                    to_insert.push((block_idx, drops));
                }
            }
            MirTerminatorKind::Goto { target } => backedge_drops(
                &mut to_insert, block_idx, block.id, *target, &func.blocks, &dom,
                &defined_in_block, &def_of_any, groups,
            ),
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *then_block, &func.blocks, &dom,
                    &defined_in_block, &def_of_any, groups,
                );
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *else_block, &func.blocks, &dom,
                    &defined_in_block, &def_of_any, groups,
                );
            }
            _ => {}
        }
    }

    to_insert
}

fn backedge_drops(
    out: &mut Vec<(usize, Vec<LocalId>)>,
    block_idx: usize,
    source: BlockId,
    target: BlockId,
    blocks: &[MirBlock],
    dom: &crate::analysis::dominators::DominatorTree,
    defined_in_block: &HashMap<LocalId, usize>,
    def_of_any: &HashMap<LocalId, usize>,
    groups: &[HashSet<LocalId>],
) {
    if !dom.dominates(target, source) {
        return;
    }
    let inside = |id: &LocalId| {
        // No defining statement means a parameter, which the frame was handed
        // and the loop certainly didn't make.
        def_of_any.get(id).is_some_and(|&idx| {
            let def = blocks[idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
    };
    let drops: Vec<LocalId> = defined_in_block
        .iter()
        .filter(|(_, &def_idx)| {
            let def = blocks[def_idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
        // The name is inside the loop; the allocation has to be too. `for x in v`
        // inside a `while` copies `v` into a fresh name in the loop body, and
        // freeing *that* freed `v` — so the second iteration walked memory that
        // was already gone. A plain `for` over a vector, no adapter involved,
        // segfaulted (#1061).
        //
        // One allocation under several names is one group, so the question is
        // whether every name in it was made inside the loop. Any member from
        // outside means the container predates the loop and belongs to the
        // enclosing frame — leaving it alone leaks at worst, where freeing it
        // is a use-after-free.
        .filter(|(id, _)| {
            groups
                .iter()
                .find(|g| g.contains(id))
                .is_none_or(|g| g.iter().all(inside))
        })
        .map(|(&id, _)| id)
        .collect();
    if !drops.is_empty() {
        out.push((block_idx, drops));
    }
}
