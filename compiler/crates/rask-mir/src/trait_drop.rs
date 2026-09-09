// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Trait-object drop insertion.
//!
//! `TraitBox` heap-allocates and copies a concrete value once to build an
//! `any Trait` fat pointer; `TraitCall` reads through it without consuming.
//! Nothing else in the pipeline ever dropped it (#366) — every trait object
//! leaked its heap allocation unconditionally.
//!
//! Mirrors `closures.rs`'s escape analysis, applied to trait-object-typed
//! locals instead of closure locals: a trait object escapes by being
//! returned, stored, or passed as a call/method argument — any of those hand
//! ownership to something else, so dropping the original name would
//! double-free. A plain move to another local (`dst = Use(src)`, or merging
//! through a `Phi`) is not an escape — it's the same value under a new name,
//! so tracking follows the new name instead (a moved-from name is excluded
//! so only the name still holding the value at its death gets dropped). What's
//! left (created, only ever read through `TraitCall`, never hands off
//! ownership) gets a `TraitDrop` before the function returns and before each
//! loop back-edge it's still alive at.
//!
//! Tracking starts from `TraitBox` destinations only — not every local typed
//! as a trait object. Reading one back out of existing storage (a struct
//! field, a `Vec` element, an `Option` payload) produces a local with the
//! same type but not fresh ownership: it's the same heap pointer the
//! container still holds, so a temp created by `r.inner.handle()` and
//! another by `run(r.inner)` right after are two aliases of the one box, not
//! two owners. Treating both as droppable-because-typed double-freed it
//! (`tests/suite/t62_trait_object_positions.rk`'s struct-field test). Only
//! `TraitBox`, and whatever a chain of moves/phis carries forward from it, is
//! a fresh allocation this pass may decide to free.
//!
//! Unlike the closure pass, this doesn't refine call-argument escapes with
//! per-callee borrow info — any appearance as a call/method argument counts
//! as escaping. That undercounts what could safely be dropped (a value
//! borrowed by a callee and not stored still "escapes" here), but never
//! drops something a callee kept, which is the risk worth avoiding.
//!
//! Function parameters are excluded — a `TraitDrop` needs the block where its
//! value was defined (for the loop back-edge check below), and a param's
//! "definition" is the call site, not a statement in this function's body.
//! A parameter that isn't returned/stored/passed onward still leaks; narrower
//! than the general case, and left for a follow-up.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind,
    MirTerminatorKind, MirType,
};

/// Insert `TraitDrop` for every non-escaping trait-object local, across all functions.
pub fn insert_trait_drops(fns: &mut [MirFunction]) {
    // Which callees keep an argument, read off their bodies — the same map the
    // closure pass uses, and for the same reason. Passing a box to a function
    // used to count as handing it over, so `io.copy(src, dst)` left both
    // buffers to a callee that only reads through them: the boxed value's
    // containers were freed by nobody. It only looked fixed while `io.copy`
    // was small enough to inline, which is why one `io.copy` in a file was
    // clean and two leaked five buffers.
    //
    // `heap_captures_only: false` — a parameter captured by *any* closure
    // counts as escaping here. Erring that way leaks; erring the other way is
    // a double free.
    let callee_escapes = crate::closures::build_callee_escape_map(fns, false);
    // And which functions hand a fresh box *back*, which makes their caller the
    // owner. `return Boom.Bad` in a `-> i64 or Error` boxes the enum and
    // returns it, and the caller read the box out of the wrapper and dropped
    // nothing — a boxed error leaked on every failing call, which is every
    // `or Error` in the language.
    let hands_back = functions_handing_back_a_trait_box(fns);
    for func in fns.iter_mut() {
        insert_for_function(func, &callee_escapes, &hands_back);
    }
}

/// Functions whose return value is a fresh trait box the caller now owns.
///
/// The same question `closures::functions_handing_back_a_closure` asks, and a
/// fixed point for the same reason: handing one back is transitive, so a
/// wrapper that just forwards what it called joins the set on a later pass.
fn functions_handing_back_a_trait_box(fns: &[MirFunction]) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    loop {
        let before = names.len();
        for func in fns {
            if names.contains(&func.name) {
                continue;
            }
            let mut fresh = collect_fresh_trait_locals(func);
            // A box that came back from a call to something already in the set
            // is this frame's, and passing it on hands it over again. Which
            // *name* holds it is the same question the caller's side asks, so
            // ask it the same way: a trait-object destination is the box, and
            // a wrapper destination holds it in its payload.
            fresh.extend(boxes_handed_over(func, &names));
            fresh.extend(boxes_parked_in_a_wrapper(func, &fresh));
            if fresh.is_empty() {
                continue;
            }
            if hands_one_back(func, &fresh) {
                names.insert(func.name.clone());
            }
        }
        if names.len() == before {
            return names;
        }
    }
}

/// Whether a returning path gives the caller a box this frame owns.
///
/// Two shapes, the same two as on the caller's side. Returning the box is one.
/// Returning a *wrapper* with the box in its payload is the other: `-> i64 or
/// Error` hands the box back as the error side, so the returned local is the
/// wrapper and never the box itself. Only the first shape was recognised, so a
/// function that forwards what it called — `let v = try classify(n)`, which
/// reads the box out and repacks it into its own wrapper — was not in the set,
/// and its caller dropped nothing (#1147).
///
/// A wrapper holding a box this frame does *not* own settles the whole function
/// the other way. `return e` for a boxed parameter hands back something whose
/// owner is the caller already; calling that fresh frees it twice. A path that
/// stores no box at all — the ok side of a `T or E` — says nothing either way.
fn hands_one_back(func: &MirFunction, fresh: &HashSet<LocalId>) -> bool {
    let ty_of = local_types(func);
    let mut found = false;
    for block in &func.blocks {
        let returned = match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => *id,
            _ => continue,
        };
        if fresh.contains(&returned) {
            found = true;
            continue;
        }
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &stmt.kind else {
                continue;
            };
            if *addr != returned || !is_box(&ty_of, v) {
                continue;
            }
            if !fresh.contains(v) {
                return false;
            }
            found = true;
        }
    }
    found
}

/// A box parked in one of this frame's own wrappers and read back out.
///
/// `let v = try classify(n)` reads the box out of the callee's wrapper on the
/// error side and repacks it into a wrapper of its own. Once the forwarding
/// function is inlined — and it always is, it's three statements — both
/// wrappers are locals of one frame, so the box's trip through storage happens
/// entirely inside it:
///
/// ```text
/// _38 = _33.0          // the box, out of what classify returned
/// *(_26+24) = _38      // parked in this frame's wrapper
/// _11 = _26            // the wrapper, moved
/// _18 = _11.0          // and the box, out again
/// ```
///
/// `_38` is moved-from — the store hands the box to the aggregate — so
/// dropping there would free it while `_18` still reads through it. `_18` is
/// the name holding it when it dies, which is the name to drop, and nothing
/// said so: a `Field` read is an alias of somebody else's storage by default,
/// for the good reason in the module doc.
///
/// What makes this one different is that the storage is a local the frame
/// controls. So the wrapper has to *stay* here: one that gets returned hands
/// the box to the caller instead (`hands_one_back`), and freeing it here as
/// well is a double free. And one box-typed read per wrapper only, the same
/// discipline `boxes_handed_over` keeps — two reads name one box.
fn boxes_parked_in_a_wrapper(func: &MirFunction, fresh: &HashSet<LocalId>) -> HashSet<LocalId> {
    let ty_of = local_types(func);
    let mut out: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &stmt.kind else {
            continue;
        };
        if fresh.contains(v) && is_box(&ty_of, v) {
            out.extend(box_read_out_of(func, *addr, &ty_of));
        }
    }
    out
}

/// The one name that ends up owning the box in `wrapper`'s payload.
///
/// The wrapper is followed through plain moves first. `_11 = _24` renames the
/// aggregate and the payload read comes off the new name, which is how a plain
/// `return classify(n)` forward stayed leaking after the `try` form was fixed:
/// the call's destination was the only name anyone looked at.
///
/// One box-typed read across the whole group, and the group has to stay in this
/// frame. Two reads name one box and a drop under each frees it twice; a
/// wrapper that leaves hands the box to whoever gets it. Both answer "no
/// owner here", which leaks — the safe half.
fn box_read_out_of(
    func: &MirFunction,
    wrapper: LocalId,
    ty_of: &HashMap<LocalId, MirType>,
) -> Option<LocalId> {
    let carriers = names_the_aggregate_reaches(func, wrapper);
    if !wrapper_stays_here(func, &carriers) {
        return None;
    }
    let reads: Vec<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } => {
                let base = crate::analysis::uses::operand_local(base)?;
                (carriers.contains(&base) && is_box(ty_of, dst)).then_some(*dst)
            }
            _ => None,
        })
        .collect();
    match reads.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Every name an aggregate reaches by a plain move, itself included.
fn names_the_aggregate_reaches(func: &MirFunction, start: LocalId) -> HashSet<LocalId> {
    let mut reached: HashSet<LocalId> = HashSet::new();
    reached.insert(start);
    loop {
        let before = reached.len();
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } =
                &st.kind
            {
                if reached.contains(src) {
                    reached.insert(*dst);
                }
            }
        }
        if reached.len() == before {
            return reached;
        }
    }
}

fn local_types(func: &MirFunction) -> HashMap<LocalId, MirType> {
    func.locals.iter().map(|l| (l.id, l.ty.clone())).collect()
}

fn is_box(ty_of: &HashMap<LocalId, MirType>, id: &LocalId) -> bool {
    matches!(ty_of.get(id), Some(MirType::TraitObject { .. }))
}

/// Whether a wrapper, and every name it moves to, is only ever assembled,
/// read, and moved along — never handed anywhere else.
///
/// A whitelist rather than a list of ways to escape: a shape nobody thought
/// about should read as "handed away", because that answer leaks and the other
/// one double-frees.
fn wrapper_stays_here(func: &MirFunction, carriers: &HashSet<LocalId>) -> bool {
    let touches = |st: &MirStmt| {
        carriers.iter().any(|c| crate::analysis::uses::stmt_reads(st, *c))
    };
    for block in &func.blocks {
        for st in &block.statements {
            if !touches(st) {
                continue;
            }
            let allowed = match &st.kind {
                // Writing a slot of the wrapper. Writing the wrapper itself
                // into something else is a hand-off.
                MirStmtKind::Store { addr, value, .. } => {
                    carriers.contains(addr)
                        && !matches!(
                            crate::analysis::uses::operand_local(value),
                            Some(v) if carriers.contains(&v)
                        )
                }
                // Reading a slot, reading the tag, or moving the whole wrapper
                // to a name that is itself a carrier.
                MirStmtKind::Assign { rvalue: MirRValue::Field { .. }, .. }
                | MirStmtKind::Assign { rvalue: MirRValue::EnumTag { .. }, .. } => true,
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(_)) } => {
                    carriers.contains(dst)
                }
                _ => false,
            };
            if !allowed {
                return false;
            }
        }
        // Returned, or read by a terminator any other way.
        if carriers
            .iter()
            .any(|c| crate::analysis::uses::terminator_reads(&block.terminator, *c))
        {
            return false;
        }
    }
    true
}

/// The box a call handed this frame, when the callee is one of the above.
///
/// Two shapes. A trait-object-typed destination *is* the box. A wrapper
/// destination holds it in its payload — `-> i64 or Error` returns the box as
/// the error side — and the payload read is the name that owns it.
///
/// Which name owns the wrapper's payload is `box_read_out_of`'s question, and
/// the same one for a wrapper this frame assembled itself.
fn boxes_handed_over(func: &MirFunction, hands_back: &HashSet<String>) -> HashSet<LocalId> {
    let ty_of = local_types(func);
    let mut out: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Call { dst: Some(dst), func: callee, .. } = &stmt.kind else { continue };
        if !hands_back.contains(&callee.name) {
            continue;
        }
        if is_box(&ty_of, dst) {
            out.insert(*dst);
        } else if matches!(
            ty_of.get(dst),
            Some(MirType::Option(_)) | Some(MirType::Result { .. })
        ) {
            out.extend(box_read_out_of(func, *dst, &ty_of));
        }
    }
    out
}

fn insert_for_function(
    func: &mut MirFunction,
    callee_escapes: &HashMap<String, Vec<bool>>,
    hands_back: &HashSet<String>,
) {
    let mut trait_locals = collect_fresh_trait_locals(func);
    trait_locals.extend(boxes_handed_over(func, hands_back));
    trait_locals.extend(boxes_parked_in_a_wrapper(func, &trait_locals));
    if trait_locals.is_empty() {
        return;
    }

    let escaping = find_escaping(func, &trait_locals, callee_escapes);
    let moved_away = find_moved_away(func, &trait_locals);

    let droppable: HashSet<LocalId> = trait_locals.iter()
        .filter(|id| !escaping.contains(id) && !moved_away.contains(id))
        .copied()
        .collect();
    if droppable.is_empty() {
        return;
    }

    insert_drops(func, &droppable);
}

/// Locals that hold a fresh trait-object allocation: `TraitBox` destinations,
/// plus anything a chain of plain moves or `Phi` merges carries forward from
/// one. A local typed as a trait object but reached only through a `Field`
/// read, a container access, or a call/method return is deliberately left
/// out — see the module doc for why aliasing one of those as "droppable"
/// double-frees.
fn collect_fresh_trait_locals(func: &MirFunction) -> HashSet<LocalId> {
    // SSA renaming (loop variables especially) can leave a pre-rename local
    // declaration behind in `func.locals` even once nothing in the CFG
    // defines it anymore, so start from actual definition sites, not the
    // declared list — a `TraitDrop` for a local nothing ever writes reads
    // garbage (Cranelift's verifier catches it as a block-argument mismatch).
    let mut fresh: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::TraitBox { dst, .. } = &stmt.kind {
                fresh.insert(*dst);
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    // Propagate through moves and phi-merges to a fixed point: `_4 = _3`
    // (real lowering copies a `TraitBox` result into the source-named local
    // before first use) or a multi-hop chain both carry the same fresh
    // allocation to a new name.
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                        if fresh.contains(src) && !fresh.contains(dst) =>
                    {
                        fresh.insert(*dst);
                        added = true;
                    }
                    MirStmtKind::Phi { dst, args } if !fresh.contains(dst) => {
                        if args.iter().any(|(_, op)| matches!(op, MirOperand::Local(id) if fresh.contains(id))) {
                            fresh.insert(*dst);
                            added = true;
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

    fresh
}

/// A trait object escapes if it's returned, stored, or passed as a call or
/// method argument. Being read through `TraitCall`'s receiver position is a
/// borrow, not an escape.
fn find_escaping(
    func: &MirFunction,
    trait_locals: &HashSet<LocalId>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    let mut escaping = HashSet::new();

    let mark_args = |args: &[MirOperand], escaping: &mut HashSet<LocalId>| {
        for arg in args {
            if let MirOperand::Local(id) = arg {
                if trait_locals.contains(id) {
                    escaping.insert(*id);
                }
            }
        }
    };

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                // A named callee whose body says it keeps nothing of this
                // argument leaves the box to this frame. Anything else — a
                // bodiless native, a call through a closure — has no answer to
                // read, and no answer means it might keep it.
                MirStmtKind::Call { func: callee, args, .. } => {
                    let keeps = callee_escapes.get(&callee.name);
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = crate::analysis::uses::operand_local(arg) else {
                            continue;
                        };
                        if !trait_locals.contains(&id) {
                            continue;
                        }
                        let borrowed = keeps
                            .and_then(|e| e.get(i))
                            .map(|escapes| !escapes)
                            .unwrap_or(false);
                        if !borrowed {
                            escaping.insert(id);
                        }
                    }
                }
                MirStmtKind::ClosureCall { args, .. } => {
                    mark_args(args, &mut escaping);
                }
                MirStmtKind::TraitCall { args, .. } => {
                    mark_args(args, &mut escaping);
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. } => {
                    if trait_locals.contains(id) {
                        escaping.insert(*id);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                if trait_locals.contains(id) {
                    escaping.insert(*id);
                }
            }
            _ => {}
        }
    }

    escaping
}

/// A trait object is "moved away" when it's copied into a different local
/// (a plain move — the new name owns the value now) or merged through a
/// `Phi`. Either way, the old name's death isn't a drop point; whichever
/// name still holds the value when *it* dies is the one that gets dropped.
fn find_moved_away(func: &MirFunction, trait_locals: &HashSet<LocalId>) -> HashSet<LocalId> {
    let mut moved = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if trait_locals.contains(src) && src != dst =>
                {
                    moved.insert(*src);
                }
                MirStmtKind::Phi { args, .. } => {
                    for (_, op) in args {
                        if let MirOperand::Local(id) = op {
                            if trait_locals.contains(id) {
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

/// Insert `TraitDrop` before every return and every loop back-edge, for each
/// droppable trait object still alive at that point.
fn insert_drops(func: &mut MirFunction, droppable: &HashSet<LocalId>) {
    let dom = crate::analysis::dominators::DominatorTree::build(func);

    let mut defined_in_block: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                if droppable.contains(&dst) {
                    defined_in_block.insert(dst, idx);
                }
            }
        }
    }

    let mut drops_to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    // Drop where control leaves the region the definition rules.
    //
    // The return rule above needs the definition to dominate the return, and a
    // definition inside a `match` arm — or a `catch` — dominates none of them:
    // the join block is reachable from the other arms too. Its own comment
    // names the case and stops there, so the box was dropped nowhere.
    // `classify(-1) catch e => -1` binds the error, so the payload read exists
    // and owns the box; it just had no site.
    //
    // A definition rules a region; control leaves it either at a `return`
    // inside it or across an edge out of it, and every path out crosses exactly
    // one of the two. Same rule as `container_drop::exit_edge_drops`, ported
    // rather than re-derived — including both of the guards that cost a
    // segfault there.
    {
        let mut extra: HashMap<usize, Vec<LocalId>> = HashMap::new();
        for id in droppable.iter().copied() {
            let Some(&def_idx) = defined_in_block.get(&id) else { continue };
            let def = func.blocks[def_idx].id;
            // Anything outside the region still naming it would read a value
            // this is about to free — a phi merging this arm's box with
            // another's is the shape that matters.
            let named_outside = func.blocks.iter().any(|b| {
                !dom.dominates(def, b.id)
                    && (b.statements.iter().any(|st| crate::analysis::uses::stmt_reads(st, id))
                        || crate::analysis::uses::terminator_reads(&b.terminator, id))
            });
            if named_outside {
                continue;
            }
            for (idx, block) in func.blocks.iter().enumerate() {
                if !dom.dominates(def, block.id) {
                    continue;
                }
                // Every successor, not any: the drop goes at the end of the
                // block, so a block that can also carry on inside the region
                // would run it and keep going. And a back-edge target is not an
                // exit whatever dominance says — `collect_backedge_drops`
                // already owns those, and dropping in both places is a double
                // free.
                let succs = crate::analysis::cfg::successors(&block.terminator);
                let leaves = !succs.is_empty()
                    && succs
                        .iter()
                        .all(|s| !dom.dominates(def, *s) && !dom.dominates(*s, block.id));
                if leaves {
                    extra.entry(idx).or_default().push(id);
                }
            }
        }
        for (idx, mut locals) in extra {
            locals.sort_by_key(|l| l.0);
            drops_to_insert.push((idx, locals));
        }
    }

    for (block_idx, block) in func.blocks.iter().enumerate() {
        match &block.terminator.kind {
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. } => {
                // A trait object created inside a loop (or either arm of a
                // branch) doesn't reach every return in the function — only
                // a return this local's definition actually dominates can
                // rely on it being live. Dropping it at a return it doesn't
                // dominate reads a local nothing wrote on that path, which
                // is exactly the stale-SSA-name crash this pass had before.
                let to_drop: Vec<LocalId> = droppable.iter()
                    .copied()
                    .filter(|id| {
                        defined_in_block.get(id)
                            .is_some_and(|&def_idx| dom.dominates(func.blocks[def_idx].id, block.id))
                    })
                    .collect();
                if !to_drop.is_empty() {
                    drops_to_insert.push((block_idx, to_drop));
                }
            }
            MirTerminatorKind::Goto { target } => {
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *target, &func.blocks, &dom, &defined_in_block,
                );
            }
            // Nothing to do here; the exit-edge rule below covers every other
            // way control leaves a definition's region.
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *then_block, &func.blocks, &dom, &defined_in_block,
                );
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *else_block, &func.blocks, &dom, &defined_in_block,
                );
            }
            _ => {}
        }
    }

    for (block_idx, locals) in drops_to_insert {
        for trait_object in locals {
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::TraitDrop { trait_object }));
        }
    }
}

fn collect_backedge_drops(
    out: &mut Vec<(usize, Vec<LocalId>)>,
    block_idx: usize,
    source: BlockId,
    target: BlockId,
    blocks: &[MirBlock],
    dom: &crate::analysis::dominators::DominatorTree,
    defined_in_block: &HashMap<LocalId, usize>,
) {
    // A genuine loop back-edge is one whose target dominates its source —
    // every path to `source` passes through `target` first, i.e. `target`
    // is the loop header. Block *index* order isn't a safe proxy for this:
    // `assert`'s desugared success/failure blocks get allocated (and so
    // numbered) before the main computation that jumps to them, which
    // looked exactly like a back-edge under an index check and produced a
    // double-drop (drop at the "back-edge", drop again at the real return).
    if !dom.dominates(target, source) {
        return;
    }
    // Drop trait objects whose definition is inside the loop. Two conditions,
    // and the second was missing:
    //
    //   The header dominates the definition — so it isn't something created
    //   before the loop, which is still live after it.
    //
    //   The definition dominates the block that jumps back — so the value really
    //   is written on every iteration. The loop's *exit* block is dominated by
    //   the header too, so the first check alone claimed anything defined after
    //   the loop. `let c: any Shape = …` written after a `while` was dropped on
    //   every back-edge, freeing whatever the uninitialised slot held; the second
    //   iteration then double-freed and the process segfaulted at the `i = i + 1`
    //   line (#764's neighbour).
    //
    // A trait object created in only one arm of a branch inside the loop doesn't
    // dominate the back-edge and so leaks rather than being dropped on a path
    // that never wrote it — the same trade the return path takes.
    let to_drop: Vec<LocalId> = defined_in_block.iter()
        .filter(|(_, &def_idx)| {
            let def = blocks[def_idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
        .map(|(&id, _)| id)
        .collect();
    if !to_drop.is_empty() {
        out.push((block_idx, to_drop));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MirLocal, MirTerminator};

    fn local(id: u32) -> LocalId { LocalId(id) }
    fn block_id(id: u32) -> BlockId { BlockId(id) }

    fn trait_local(id: u32) -> MirLocal { MirLocal { id: local(id), name: None, ty: MirType::TraitObject { trait_name: "Speaker".into() }, is_param: false, container: None } }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: block_id(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn has_trait_drop(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::TraitDrop { trait_object } if *trait_object == target))
    }

    fn trait_box(dst: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::TraitBox {
            dst,
            value: MirOperand::Local(local(99)),
            concrete_type: "Loud".into(),
            trait_name: "Speaker".into(),
            concrete_size: 32,
            vtable_name: ".vtable.Loud__Speaker".into(),
        })
    }

    fn trait_call(receiver: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::TraitCall {
            dst: None,
            trait_object: receiver,
            method_name: "speak".into(),
            vtable_offset: 24,
            args: vec![],
        })
    }

    #[test]
    fn non_escaping_dropped_before_return() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![trait_box(local(0)), trait_call(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    /// Reproduces the shape real lowering emits for `let s: any Speaker = ...`:
    /// the `TraitBox` result gets copied into a second local before use.
    #[test]
    fn moved_through_copy_drops_the_final_name_not_the_original() {
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    trait_call(local(1)),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)), "moved-from name should not be dropped");
        assert!(has_trait_drop(&f.blocks[0].statements, local(1)), "the name actually holding the value should be dropped");
    }

    #[test]
    fn returned_trait_object_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![trait_box(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn stored_trait_object_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Store {
                        addr: local(1),
                        offset: 0,
                        value: MirOperand::Local(local(0)),
                        store_size: None,
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn call_argument_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal("consume".into()),
                        args: vec![MirOperand::Local(local(0))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn loop_body_dropped_at_back_edge() {
        // block 0: entry -> goto 1
        // block 1: loop header -> branch 2/3
        // block 2: body — TraitBox local 0 (moved into local 1), TraitCall, goto 1 (back-edge)
        // block 3: exit — return
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![
                MirBlock {
                    id: block_id(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(1) }),
                },
                MirBlock {
                    id: block_id(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(50)),
                        then_block: block_id(2),
                        else_block: block_id(3),
                    }),
                },
                MirBlock {
                    id: block_id(2),
                    statements: vec![
                        trait_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        trait_call(local(1)),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(1) }),
                },
                MirBlock {
                    id: block_id(3),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[2].statements, local(1)), "back-edge block should drop the loop-local trait object");
        assert!(!has_trait_drop(&f.blocks[2].statements, local(0)), "moved-from name should not be dropped");
    }

    /// Reproduces the shape `assert`'s desugaring produces: the success and
    /// failure blocks are allocated (and so numbered) before the block that
    /// computes the condition and branches to them. A block-index check for
    /// "is this a back-edge" sees block 1 as jumped-to from a higher-numbered
    /// block 2 and mistakes it for a loop, inserting a second `TraitDrop` at
    /// block 2 on top of the one already correctly placed at block 1's
    /// return — a double free (#366 follow-up: this exact shape crashed
    /// `tests/suite/t11_traits.rk`'s "trait object dispatch" test in CI).
    #[test]
    fn assert_style_branch_to_lower_numbered_blocks_is_not_a_back_edge() {
        // block 0: entry — TraitBox local 0 (moved to local 1), goto 2
        // block 1: success — TraitDrop already placed here, return
        // (no block 1 predecessor other than block 2 — not a loop header)
        // block 2: TraitCall, branch to 1 (success) or 1 (success, for simplicity)
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![
                MirBlock {
                    id: block_id(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(2) }),
                },
                MirBlock {
                    id: block_id(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
                MirBlock {
                    id: block_id(2),
                    statements: vec![
                        trait_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        trait_call(local(1)),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(50)),
                        then_block: block_id(1),
                        else_block: block_id(1),
                    }),
                },
            ],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[1].statements, local(1)), "the return block should get the drop");
        assert!(!has_trait_drop(&f.blocks[2].statements, local(1)), "the branch is not a back-edge — no second drop here");
    }

    /// Reproduces `tests/suite/t62_trait_object_positions.rk`'s struct-field
    /// test: reading a trait object back out of a container (here, a struct
    /// field) twice produces two locals of the same type aliasing one heap
    /// box. Treating either as a fresh, droppable allocation — as a plain
    /// "is this local typed as a trait object" check would — drops the same
    /// pointer twice.
    #[test]
    fn field_read_trait_object_is_not_tracked_as_fresh() {
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(local(2)),
                            field_index: 0,
                            byte_offset: Some(0),
                            access: crate::FieldAccess::Sized(16),
                        },
                    }),
                    trait_call(local(0)),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(local(2)),
                            field_index: 0,
                            byte_offset: Some(0),
                            access: crate::FieldAccess::Sized(16),
                        },
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal("run".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)), "a field read is a borrow, not a fresh allocation");
        assert!(!has_trait_drop(&f.blocks[0].statements, local(1)), "same here — this pass must not touch the struct's own field");
    }
}
