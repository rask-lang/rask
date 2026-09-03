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
/// The constructors are `elem_strs::CTORS` — the same list lowering appends
/// element tags to and codegen builds C signatures from. The free is that
/// type's own, and it now takes nothing but the container: it knows what its
/// elements are.
fn free_for(ctor: &str) -> Option<&'static str> {
    match crate::elem_strs::CTORS.iter().find(|(c, _, _)| *c == ctor)?.0 {
        c if c.starts_with("Vec_") || c.starts_with("rask_vec_") => Some("Vec_free"),
        c if c.starts_with("Map_") => Some("Map_free"),
        c if c.starts_with("Rack_") => Some("Rack_free"),
        c if c.starts_with("Pool_") => Some("Pool_free"),
        _ => None,
    }
}

pub fn insert_container_drops(fns: &mut [MirFunction]) {
    let handing_over = functions_that_hand_a_container_back(fns);
    let kept = params_a_callee_keeps(fns);
    for func in fns.iter_mut() {
        insert_for_function(func, &handing_over, &kept);
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
fn params_a_callee_keeps(fns: &[MirFunction]) -> HashMap<String, Vec<bool>> {
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
fn call_keeps_argument(
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
fn functions_that_hand_a_container_back(fns: &[MirFunction]) -> HashMap<String, &'static str> {
    let mut handing: HashMap<String, &'static str> = HashMap::new();
    loop {
        let mut grew = false;
        for func in fns {
            if handing.contains_key(&func.name) {
                continue;
            }
            let fresh = collect_fresh_containers_with(func, &handing);
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
            let mut any = false;
            for b in &func.blocks {
                let MirTerminatorKind::Return { value: Some(v), .. } = &b.terminator.kind else {
                    continue;
                };
                any = true;
                match v {
                    MirOperand::Local(id) => match fresh.get(id) {
                        Some(f) => free_fn = Some(f),
                        None => all_fresh = false,
                    },
                    _ => all_fresh = false,
                }
            }
            if let (true, true, Some(free)) = (any, all_fresh, free_fn) {
                handing.insert(func.name.clone(), free);
                grew = true;
            }
        }
        if !grew {
            return handing;
        }
    }
}

fn insert_for_function(
    func: &mut MirFunction,
    handing_over: &HashMap<String, &'static str>,
    kept: &HashMap<String, Vec<bool>>,
) {
    let fresh = collect_fresh_containers_with(func, handing_over);
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
    for group in value_groups(func, &fresh) {
        let survivors = group.iter().filter(|id| droppable.contains_key(id)).count();
        if survivors > 1 {
            for id in &group {
                droppable.remove(id);
            }
        }
    }

    if droppable.is_empty() {
        return;
    }
    insert_drops(func, &droppable);
}

/// Locals holding a container this frame owns, mapped to how to free it: the
/// destination of a constructor, and of a call to a function that builds one
/// and hands it back.
fn collect_fresh_containers_with(
    func: &MirFunction,
    handing_over: &HashMap<String, &'static str>,
) -> HashMap<LocalId, &'static str> {
    let mut fresh: HashMap<LocalId, &'static str> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Call { dst: Some(dst), func: fref, .. } = &stmt.kind {
                let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
                // A monomorphized name carries a `$` suffix.
                let base = head.split('$').next().unwrap_or(head);
                if let Some(free) = free_for(base) {
                    fresh.insert(*dst, free);
                } else if let Some(free) = handing_over.get(&fref.name) {
                    // The callee's own constructor decided which free this is.
                    fresh.insert(*dst, free);
                }
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    let made_here: HashSet<LocalId> = fresh.keys().copied().collect();

    // The same value under a new name: follow it, so the name still holding it
    // at its death is the one that gets freed.
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
            break;
        }
    }

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

/// Free before every return the container's definition dominates, and before
/// every loop back-edge it was built inside. Same placement rules as
/// `trait_drop.rs`, and for the same reasons — see the comments there on why
/// dominance is what decides it rather than block order.
fn insert_drops(func: &mut MirFunction, droppable: &HashMap<LocalId, &'static str>) {
    let dom = crate::analysis::dominators::DominatorTree::build(func);

    let mut defined_in_block: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
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
                &mut to_insert, block_idx, block.id, *target, &func.blocks, &dom, &defined_in_block,
            ),
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *then_block, &func.blocks, &dom,
                    &defined_in_block,
                );
                backedge_drops(
                    &mut to_insert, block_idx, block.id, *else_block, &func.blocks, &dom,
                    &defined_in_block,
                );
            }
            _ => {}
        }
    }

    for (block_idx, locals) in to_insert {
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

fn backedge_drops(
    out: &mut Vec<(usize, Vec<LocalId>)>,
    block_idx: usize,
    source: BlockId,
    target: BlockId,
    blocks: &[MirBlock],
    dom: &crate::analysis::dominators::DominatorTree,
    defined_in_block: &HashMap<LocalId, usize>,
) {
    if !dom.dominates(target, source) {
        return;
    }
    let drops: Vec<LocalId> = defined_in_block
        .iter()
        .filter(|(_, &def_idx)| {
            let def = blocks[def_idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
        .map(|(&id, _)| id)
        .collect();
    if !drops.is_empty() {
        out.push((block_idx, drops));
    }
}
