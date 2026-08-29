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

use crate::analysis::dominators::DominatorTree;
use crate::analysis::liveness;
use crate::analysis::uses;
use crate::{
    BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminatorKind, MirType,
};

/// Insert explicit RcInc/RcDec for all string-typed locals in a function.
pub fn insert_rc_ops(func: &mut MirFunction) {
    let string_locals: Vec<LocalId> = func.locals_of_type(&MirType::String);

    if string_locals.is_empty() {
        return;
    }

    // Insert RcInc after string copies
    insert_rc_inc(func, &string_locals);

    // Insert RcDec at last-use points
    insert_rc_dec(func, &string_locals);

    // A returned parameter is handed out, not owned — take a reference for it.
    retain_returned_params(func, &string_locals);

    // And the aggregates: a struct field or a wrapper's payload owns a string
    // just as much as a local does.
    insert_aggregate_release(func);
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
fn insert_aggregate_release(func: &mut MirFunction) {
    let ty_of: HashMap<LocalId, MirType> =
        func.locals.iter().map(|l| (l.id, l.ty.clone())).collect();
    let aggregates: HashSet<LocalId> = func
        .locals
        .iter()
        .filter(|l| aggregate_may_hold_string(&l.ty))
        .map(|l| l.id)
        .collect();
    if aggregates.is_empty() {
        return;
    }

    // One group per value. SSA renames an aggregate at every copy, and a
    // payload read out of a wrapper names the same bytes rather than copying
    // them — so `r`, `r.0`, and every SSA name of either are one thing that
    // dies once. Splitting them was how the wrapper's release ended up running
    // while a view into its payload was still live.
    let groups = aggregate_value_groups(func, &aggregates, &ty_of);

    // Anything that might keep the value alive elsewhere disqualifies its whole
    // group. Releasing there is a use-after-free rather than a leak, and this
    // pass can't see far enough to tell.
    let mut blocked: HashSet<usize> = HashSet::new();
    let group_of: HashMap<LocalId, usize> = groups
        .iter()
        .enumerate()
        .flat_map(|(gi, g)| g.iter().map(move |l| (*l, gi)))
        .collect();
    let mut block_local = |blocked: &mut HashSet<usize>, id: &LocalId| {
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
                // A call gives up what it returns — unless it's a container
                // accessor, which hands back a pointer into storage it keeps.
                // A `Vec` or a `Map` is a byte store that doesn't touch
                // refcounts, so who owns an element that came out of one is the
                // question this pass can't answer (#1027).
                MirStmtKind::Call { func: fref, .. } => !is_container_boundary(&fref.name),
                // A pool element, a capture, a global, a dynamic call: all
                // views into storage somebody else keeps.
                _ => false,
            };
            if !owns {
                block_local(&mut blocked, &dst);
            }
        }
    }

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                // Handed to something else, which may keep it.
                MirStmtKind::Call { args, .. } => {
                    for arg in args {
                        if let Some(id) = uses::operand_local(arg) {
                            block_local(&mut blocked, &id);
                        }
                    }
                }
                // Copied whole into memory — the destination owns it now.
                // Storing *into* an aggregate is the opposite: that's how one is
                // built, and the retain on the value is already there.
                MirStmtKind::Store { value, .. }
                | MirStmtKind::ArrayStore { value, .. }
                | MirStmtKind::TraitBox { value, .. } => {
                    if let Some(id) = uses::operand_local(value) {
                        block_local(&mut blocked, &id);
                    }
                }
                MirStmtKind::ClosureCreate { captures, .. } => {
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

    let groups: Vec<HashSet<LocalId>> = groups
        .into_iter()
        .enumerate()
        .filter(|(gi, _)| !blocked.contains(gi))
        .map(|(_, g)| g)
        .collect();
    if groups.is_empty() {
        return;
    }

    let live_out = aggregate_live_out(func, &groups);

    for block_idx in 0..func.blocks.len() {
        let stmts_len = func.blocks[block_idx].statements.len();
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        for (gi, group) in groups.iter().enumerate() {
            if live_out[block_idx][gi] {
                continue;
            }
            let mut last = None;
            let mut local = None;
            for si in 0..stmts_len {
                let stmt = &func.blocks[block_idx].statements[si];
                if matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                    continue;
                }
                for id in group {
                    if uses::stmt_reads(stmt, *id) || uses::stmt_def(stmt) == Some(*id) {
                        last = Some(si);
                        local = Some(*id);
                    }
                }
            }
            let (Some(si), Some(local)) = (last, local) else { continue };
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
fn aggregate_live_out(func: &MirFunction, groups: &[HashSet<LocalId>]) -> Vec<Vec<bool>> {
    let n_blocks = func.blocks.len();
    let n_groups = groups.len();
    let index_of: HashMap<BlockId, usize> =
        func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();

    // Upward-exposed use, and whether the block writes the group at all.
    let mut gen = vec![vec![false; n_groups]; n_blocks];
    let mut kill = vec![vec![false; n_groups]; n_blocks];

    for (bi, block) in func.blocks.iter().enumerate() {
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
                };
                if reads && !written {
                    gen[bi][gi] = true;
                }
                let writes =
                    stores_into || uses::stmt_def(stmt).is_some_and(|d| group.contains(&d));
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
    live_out
}

/// Type prefixes whose functions store values, or hand them back out of
/// storage they keep owning. Mirrors the list in `rc_elide`.
const CONTAINER_PREFIXES: &[&str] = &[
    "Vec_", "Map_", "Set_", "Deque_", "Rack_", "Pool_", "Link_",
    "Channel_", "Sender_", "Receiver_",
    "Shared_", "Mutex_", "Cell_", "Atomic",
];

fn is_container_boundary(name: &str) -> bool {
    let head = name.rsplit("::").next().unwrap_or(name);
    CONTAINER_PREFIXES.iter().any(|p| head.starts_with(p))
}

/// Shapes that can hold a string somewhere inside them. The layouts that would
/// settle it live in codegen, so this only rules out what it can.
fn aggregate_may_hold_string(ty: &MirType) -> bool {
    match ty {
        MirType::Struct(_) | MirType::Enum(_) => true,
        MirType::Tuple(elems) => elems.iter().any(|e| {
            *e == MirType::String || aggregate_may_hold_string(e)
        }),
        MirType::Array { elem, .. } => {
            **elem == MirType::String || aggregate_may_hold_string(elem)
        }
        MirType::Option(inner) => aggregate_may_hold_string(inner) || **inner == MirType::String,
        MirType::Result { ok, err } => {
            aggregate_may_hold_string(ok)
                || aggregate_may_hold_string(err)
                || **ok == MirType::String
                || **err == MirType::String
        }
        _ => false,
    }
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

/// Insert `RcDec` at last-use points for string locals.
///
/// Uses liveness analysis: when a string local is live at a statement but dead
/// after it (no further uses on any path), insert `RcDec` after that statement.
fn insert_rc_dec(func: &mut MirFunction, string_locals: &[LocalId]) {
    let dom = DominatorTree::build(func);
    let live = liveness::analyze(func, &dom);
    // A string parameter is borrowed from the caller, which keeps its own
    // reference and releases it at its own last use. Releasing here as well is
    // two releases for one reference:
    //
    //     open_result("/tmp/missing")   // main holds the only reference
    //       -> open_result decs `path` after fs.open(path)
    //       -> fs.open decs `path` too
    //
    // Nothing noticed because the elision pass deleted both. A callee that
    // needs to outlive the call takes its own reference: storing incs, and
    // returning a parameter incs just below.
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();

    for block_idx in 0..func.blocks.len() {
        let block_id = func.blocks[block_idx].id;
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        let stmts_len = func.blocks[block_idx].statements.len();

        for local in string_locals {
            if params.contains(local) {
                continue;
            }
            // Find the last use of this local in the block
            let mut last_use_idx: Option<usize> = None;

            for si in 0..stmts_len {
                let stmt = &func.blocks[block_idx].statements[si];
                // A phi reads its argument on the incoming edge, not here. Counting
                // it as a use in the phi's own block put the drop at the top of a
                // loop header, where it runs on the first iteration — before the
                // value it releases has been written. That freed whatever the
                // uninitialized slot happened to point at.
                let phi = matches!(stmt.kind, MirStmtKind::Phi { .. });
                if !phi && uses::stmt_reads(stmt, *local) {
                    last_use_idx = Some(si);
                }
                // If this statement defines the local, earlier uses are irrelevant
                if uses::stmt_def(stmt) == Some(*local) {
                    last_use_idx = None;
                }
            }

            // Check terminator
            let term_reads = uses::terminator_reads(&func.blocks[block_idx].terminator, *local);

            // A returned string is handed to the caller, not dropped. Decrementing
            // it here freed the buffer while the caller still held the only
            // reference — `return json.encode(v)` from a `string or E` function
            // came back with its first eight bytes overwritten by whatever the
            // caller allocated next (#499).
            let returned = matches!(
                &func.blocks[block_idx].terminator.kind,
                MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
                | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
                    if *id == *local
            );
            if returned {
                continue;
            }

            // If the local is live at block exit, it's used downstream — no dec here
            if live.live_at_exit(block_id, *local) {
                continue;
            }

            // Local dies in this block. Place RcDec after the last use.
            if term_reads {
                // Used in terminator and dead after — dec at block end
                // (We can't insert after terminator, so append to statements.
                //  The dec runs before the terminator logically.)
                let span = func.blocks[block_idx].terminator.span;
                insertions.push((stmts_len, MirStmt::new(
                    MirStmtKind::RcDec { local: *local },
                    span,
                )));
            } else if let Some(si) = last_use_idx {
                let span = func.blocks[block_idx].statements[si].span;
                // Step over the increments the copy pass already put here. At
                // `dst = src`, `src`'s last use is the copy itself, so the naive
                // spot is directly between `dst = src` and `RcInc(dst)` — the
                // release runs first, the buffer hits zero, and the increment
                // that was meant to keep it alive touches freed memory.
                let mut at = si + 1;
                while at < func.blocks[block_idx].statements.len()
                    && matches!(
                        func.blocks[block_idx].statements[at].kind,
                        MirStmtKind::RcInc { .. }
                    )
                {
                    at += 1;
                }
                insertions.push((at, MirStmt::new(
                    MirStmtKind::RcDec { local: *local },
                    span,
                )));
            } else {
                // Not used in this block at all but enters live — check entry
                if live.live_at_entry(block_id, *local) {
                    // Was live at entry, dead at exit, no uses: killed by redefinition.
                    // The old value needs an RcDec before the redefinition.
                    for si in 0..stmts_len {
                        if uses::stmt_def(&func.blocks[block_idx].statements[si]) == Some(*local) {
                            let span = func.blocks[block_idx].statements[si].span;
                            insertions.push((si, MirStmt::new(
                                MirStmtKind::RcDec { local: *local },
                                span,
                            )));
                            break;
                        }
                    }
                }
            }
        }

        // Sort by position descending so insertions don't shift indices
        insertions.sort_by(|a, b| b.0.cmp(&a.0));
        for (idx, stmt) in insertions {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
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
        MirLocal { id: local(id), name: Some(name.into()), ty: MirType::String, is_param: false }
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
        };
        let mut f = MirFunction {
            name: "put".to_string(),
            params: vec![param.clone()],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("self".into()), ty: MirType::Ptr, is_param: true },
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
        insert_rc_ops(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(count_rc_inc(stmts), 1, "one inc for the store: {stmts:?}");
        assert_eq!(count_rc_dec(stmts), 0, "a parameter is borrowed: {stmts:?}");
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
        insert_rc_ops(&mut f);
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
        insert_rc_ops(&mut f);
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
        insert_rc_ops(&mut f);
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
        insert_rc_ops(&mut f);
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
        insert_rc_ops(&mut f);
        let header = f.blocks.iter().find(|b| b.id == BlockId(1)).unwrap();
        assert!(
            !has_rc_dec(&header.statements, local(1)),
            "no release for a phi argument in the header: {:?}", header.statements
        );
    }

    #[test]
    fn no_ops_for_non_string_locals() {
        let mut f = make_fn(
            vec![MirLocal {
                id: local(0), name: Some("x".into()), ty: MirType::I64, is_param: false,
            }],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: local(0),
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(42))),
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f);
        assert_eq!(count_rc_inc(&f.blocks[0].statements), 0);
        assert_eq!(count_rc_dec(&f.blocks[0].statements), 0);
    }
}
