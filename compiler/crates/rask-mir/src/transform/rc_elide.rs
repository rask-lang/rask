// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! String RC elision — removes unnecessary RcInc/RcDec operations.
//!
//! Implements the optimizations from `comp.string-refcount-elision`:
//! - **RE1: Inc/dec cancellation** — Copy followed by drop of original → cancel both
//! - **RE2: Local-only strings** — Strings that don't escape skip all RC ops
//! - **RE3: Literal propagation** — String literals use sentinel refcount, no ops needed
//! - **RE6: SSO bypass** — Constants ≤15 bytes are SSO, no refcount exists
//!
//! Runs after rc_insert. Removes RcInc/RcDec statements that are provably unnecessary.

use crate::analysis::escape;
use crate::{LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmtKind};
use std::collections::{HashMap, HashSet};

/// Elide unnecessary RC operations on string locals.
pub fn elide_rc_ops(func: &mut MirFunction) {
    elide_local_only(func);
    elide_literals(func);
    cancel_inc_dec_pairs(func);
}

/// Type prefixes whose functions store a string, or hand one back out of
/// storage they keep owning.
///
/// A `Vec` or a `Map` is a byte store: `push` memcpys the sixteen bytes in and
/// `get` hands back a pointer to them, and neither touches the refcount. So a
/// string that crosses one of these has no owner the count knows about, and the
/// only rule that doesn't crash is the one in force today — the reference moves
/// in and is never released, and a read borrows without taking one. That leaks
/// the elements when the container dies, which is a real bug (#1027) and a
/// different one: fixing it means the container releasing what it holds, which
/// means it has to know its elements are strings.
///
/// Until then, a string that touches one of these keeps its ops elided, exactly
/// as before. Everything that doesn't gets its release back.
const CONTAINER_PREFIXES: &[&str] = &[
    "Vec_", "Map_", "Set_", "Deque_", "Rack_", "Pool_", "Link_",
    "Channel_", "Sender_", "Receiver_",
    "Shared_", "Mutex_", "Cell_", "Atomic",
];

fn is_container_boundary(name: &str) -> bool {
    // MIR names a stdlib method `<Type>_<method>`, and a monomorphized one
    // carries a `$` suffix. Match on the head so `Vec_push$string` counts.
    let head = name.rsplit("::").next().unwrap_or(name);
    CONTAINER_PREFIXES.iter().any(|p| head.starts_with(p))
}

/// String locals holding a reference this function didn't take itself.
///
/// RE2's premise is that a string that never escapes has balanced RC ops: the
/// copy that incremented it is what the drop releases, so removing both changes
/// nothing. That holds for a string this function *created and copied*. It does
/// not hold for one handed over with its reference already taken — a call's
/// return value, a payload read out of an aggregate, a parameter. There the
/// `RcDec` is the only release there will ever be.
///
/// The spec says as much: RE2 is "skip all *atomic* ops — refcount stays at 1,
/// free on drop". The implementation dropped the free along with the atomics,
/// so `let s = make_it(i)` in a loop leaked about 96 bytes a turn in ordinary
/// single-threaded code (#1024). It hid because the obvious probe uses a string
/// *literal*, and RE3 exempts those for an unrelated reason — they carry a
/// sentinel refcount — so the shape that leaks is the shape a quick test
/// doesn't reach for.
///
/// Owned-from-elsewhere: anything but a copy of another string local or a
/// string constant. Being wrong in that direction costs an RC pair; being wrong
/// the other way costs the buffer.
fn owned_from_elsewhere(func: &MirFunction, string_locals: &HashSet<LocalId>) -> HashSet<LocalId> {
    // Not parameters: those are borrowed from the caller, which keeps its own
    // reference. `rc_insert` gives them no release for the same reason, and the
    // one increment they do get — a parameter handed straight back out — is
    // covered because escape analysis counts a returned local as escaping.
    let mut owned: HashSet<LocalId> = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue } if string_locals.contains(dst) => {
                    let self_made = matches!(
                        rvalue,
                        MirRValue::Use(MirOperand::Local(_))
                            | MirRValue::Use(MirOperand::Constant(MirConst::String(_)))
                    );
                    if !self_made {
                        owned.insert(*dst);
                    }
                }
                // A call writing into a string local hands over its reference.
                MirStmtKind::Call { dst: Some(dst), .. } if string_locals.contains(dst) => {
                    owned.insert(*dst);
                }
                _ => {}
            }
        }
    }

    owned
}

/// String locals that cross a container boundary — see `CONTAINER_PREFIXES`.
fn container_touched(func: &MirFunction, string_locals: &HashSet<LocalId>) -> HashSet<LocalId> {
    let mut touched: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Call { dst, func: fref, args } = &stmt.kind else { continue };
            if !is_container_boundary(&fref.name) {
                continue;
            }
            if let Some(dst) = dst {
                if string_locals.contains(dst) {
                    touched.insert(*dst);
                }
            }
            for arg in args {
                if let Some(id) = crate::analysis::uses::operand_local(arg) {
                    if string_locals.contains(&id) {
                        touched.insert(id);
                    }
                }
            }
        }
    }
    touched
}

/// Group string locals that name the same buffer: `dst = src` and phis.
///
/// RC ops have to be kept or dropped for a whole group at once. Keeping one
/// local's `RcDec` while dropping the `RcInc` on the copy that outlives it
/// frees the buffer out from under the copy — `let taken = v.remove(0)` read
/// back as whatever the next allocation wrote there.
fn copy_groups(func: &MirFunction, string_locals: &HashSet<LocalId>) -> Vec<HashSet<LocalId>> {
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
                    if string_locals.contains(dst) && string_locals.contains(src) =>
                {
                    union(&mut parent, *dst, *src);
                }
                MirStmtKind::Phi { dst, args } if string_locals.contains(dst) => {
                    for (_, arg) in args {
                        if let MirOperand::Local(src) = arg {
                            if string_locals.contains(src) {
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
    for local in string_locals {
        let root = find(&mut parent, *local);
        groups.entry(root).or_default().insert(*local);
    }
    groups.into_values().collect()
}

/// RE2: Remove all RcInc/RcDec for string locals that never escape the function.
fn elide_local_only(func: &mut MirFunction) -> usize {
    let escaped = escape::escaping_strings(func);
    let string_locals: HashSet<LocalId> =
        func.locals_of_type(&crate::MirType::String).into_iter().collect();
    let owned = owned_from_elsewhere(func, &string_locals);
    let containers = container_touched(func, &string_locals);

    // Decide per group, not per local: keeping one local's release while
    // dropping the increment on the copy that outlives it frees the buffer out
    // from under the copy.
    let mut keep: HashSet<LocalId> = HashSet::new();
    for group in copy_groups(func, &string_locals) {
        let crosses_container = group.iter().any(|l| containers.contains(l));
        let borrowed_in = group.iter().any(|l| owned.contains(l));
        if borrowed_in && !crosses_container {
            keep.extend(group);
        }
    }

    let mut removed = 0;
    for block in &mut func.blocks {
        let before = block.statements.len();
        block.statements.retain(|stmt| {
            match &stmt.kind {
                MirStmtKind::RcInc { local } | MirStmtKind::RcDec { local } => {
                    // Keep if the local escapes, or if its reference came from
                    // somewhere this function has to release.
                    escaped.contains(local) || keep.contains(local)
                }
                _ => true,
            }
        });
        removed += before - block.statements.len();
    }

    removed
}

/// RE3 + RE6: Remove RC ops on locals that provably hold string literals or SSO strings.
///
/// A literal's buffer is static with a sentinel refcount, so retaining and
/// releasing it are both no-ops and the pair can go.
///
/// "Provably" has to mean *on every path*. The old version walked the blocks in
/// order and kept a running set, so the last write to a local won:
///
/// ```text
/// bb3:  _5 = concat("not found: ", p)   // removes _5 from the set
/// bb4:  _5 = "timed out"                // puts it back
/// ```
///
/// `_5` came out marked literal, its RC ops were dropped, and the concat
/// result on the other arm was released by nobody — or, once releases started
/// surviving, released without the matching retain, so `println` read a freed
/// buffer. Now a local is literal only when *every* definition of it is, which
/// takes a fixed point because a copy's answer depends on its source.
fn elide_literals(func: &mut MirFunction) -> usize {
    let string_locals: HashSet<LocalId> =
        func.locals_of_type(&crate::MirType::String).into_iter().collect();

    // Start optimistic and only ever remove: a local drops out the moment one
    // of its definitions can't be shown to be a literal.
    let mut literal: HashSet<LocalId> = string_locals.clone();

    // A parameter's value comes from the caller — nothing here can vouch for it.
    for param in &func.params {
        literal.remove(&param.id);
    }

    loop {
        let mut changed = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                let Some(dst) = crate::analysis::uses::stmt_def(stmt) else { continue };
                if !literal.contains(&dst) {
                    continue;
                }
                let is_literal_def = match &stmt.kind {
                    MirStmtKind::Assign { rvalue, .. } => match rvalue {
                        MirRValue::Use(MirOperand::Constant(MirConst::String(_))) => true,
                        MirRValue::Use(MirOperand::Local(src)) => literal.contains(src),
                        _ => false,
                    },
                    MirStmtKind::Phi { args, .. } => args.iter().all(|(_, arg)| match arg {
                        MirOperand::Constant(MirConst::String(_)) => true,
                        MirOperand::Local(src) => literal.contains(src),
                        _ => false,
                    }),
                    // RC ops don't define; anything else that does (a call, a
                    // field read) produces something this pass can't vouch for.
                    MirStmtKind::RcInc { .. } | MirStmtKind::RcDec { .. } => continue,
                    _ => false,
                };
                if !is_literal_def {
                    literal.remove(&dst);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // A local nothing ever defines holds nothing to elide.
    let mut defined: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                defined.insert(dst);
            }
        }
    }
    literal.retain(|l| defined.contains(l));

    if literal.is_empty() {
        return 0;
    }

    let mut removed = 0;
    for block in &mut func.blocks {
        let before = block.statements.len();
        block.statements.retain(|stmt| {
            match &stmt.kind {
                MirStmtKind::RcInc { local } | MirStmtKind::RcDec { local } => {
                    !literal.contains(local)
                }
                _ => true,
            }
        });
        removed += before - block.statements.len();
    }

    removed
}

/// RE1: Cancel adjacent or nearby RcInc/RcDec pairs on the same local.
///
/// Pattern: `RcInc(x)` followed by `RcDec(x)` with no intervening uses of x
/// that could observe the refcount. The inc and dec cancel out.
///
/// Also handles the reverse: `RcDec(x)` followed by `RcInc(x)` when x is
/// a copy of the original being dropped.
fn cancel_inc_dec_pairs(func: &mut MirFunction) -> usize {
    let mut total_removed = 0;

    for block in &mut func.blocks {
        let mut to_remove: HashSet<usize> = HashSet::new();
        let stmts = &block.statements;

        for i in 0..stmts.len() {
            if to_remove.contains(&i) {
                continue;
            }

            // Look for RcInc followed by RcDec on same local (or vice versa)
            let (is_inc, local_i) = match &stmts[i].kind {
                MirStmtKind::RcInc { local } => (true, *local),
                MirStmtKind::RcDec { local } => (false, *local),
                _ => continue,
            };

            // Scan forward for matching opposite op
            for j in (i + 1)..stmts.len() {
                if to_remove.contains(&j) {
                    continue;
                }

                let (is_inc_j, local_j) = match &stmts[j].kind {
                    MirStmtKind::RcInc { local } => (true, *local),
                    MirStmtKind::RcDec { local } => (false, *local),
                    _ => {
                        // If this statement uses the local, stop scanning —
                        // there's an observable use between the pair
                        if crate::analysis::uses::stmt_reads(&stmts[j], local_i) {
                            break;
                        }
                        continue;
                    }
                };

                if local_j == local_i && is_inc != is_inc_j {
                    // Found matching pair — cancel both
                    to_remove.insert(i);
                    to_remove.insert(j);
                    break;
                }

                // Same-direction RC op on same local — stop (can't cancel)
                if local_j == local_i {
                    break;
                }
            }
        }

        if !to_remove.is_empty() {
            total_removed += to_remove.len();
            let mut idx = 0;
            block.statements.retain(|_| {
                let keep = !to_remove.contains(&idx);
                idx += 1;
                keep
            });
        }
    }

    total_removed
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

    fn count_rc_ops(stmts: &[MirStmt]) -> usize {
        stmts.iter().filter(|s| matches!(
            &s.kind, MirStmtKind::RcInc { .. } | MirStmtKind::RcDec { .. }
        )).count()
    }

    // ── RE1: Inc/dec cancellation ────────────────────────────────

    #[test]
    fn adjacent_inc_dec_cancelled() {
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = cancel_inc_dec_pairs(&mut f);
        assert_eq!(removed, 2);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 0);
    }

    #[test]
    fn non_adjacent_pair_with_no_use_cancelled() {
        let mut f = make_fn(
            vec![string_local(0, "s"), string_local(1, "t")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                    // Unrelated statement between — doesn't use local(0)
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(1) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = cancel_inc_dec_pairs(&mut f);
        assert_eq!(removed, 2);
        // Only the RcInc on local(1) remains
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 1);
    }

    #[test]
    fn pair_with_intervening_use_not_cancelled() {
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                    // This reads local(0) — observable between inc and dec
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("print_string".into()),
                        args: vec![MirOperand::Local(local(0))],
                    }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = cancel_inc_dec_pairs(&mut f);
        assert_eq!(removed, 0);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 2);
    }

    // ── RE2: Local-only elision ──────────────────────────────────

    #[test]
    fn local_only_rc_ops_elided() {
        // String never escapes — all RC ops removed
        let mut f = make_fn(
            vec![string_local(0, "s"), string_local(1, "t")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(1) }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("print_string".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(1) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = elide_local_only(&mut f);
        assert!(removed > 0);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 0);
    }

    #[test]
    fn escaped_string_rc_ops_kept() {
        // String returned — escapes
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        let removed = elide_local_only(&mut f);
        assert_eq!(removed, 0);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 1);
    }

    // ── RE3: Literal propagation ─────────────────────────────────

    #[test]
    fn literal_rc_ops_elided() {
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(
                            MirConst::String("hello".into()),
                        )),
                    }),
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = elide_literals(&mut f);
        assert_eq!(removed, 2);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 0);
    }

    #[test]
    fn literal_copy_chain_elided() {
        // s = "hello"; t = s → both are literal, both RC ops elided
        let mut f = make_fn(
            vec![string_local(0, "s"), string_local(1, "t")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(
                            MirConst::String("hello".into()),
                        )),
                    }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(1) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(1) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = elide_literals(&mut f);
        assert_eq!(removed, 3);
    }

    #[test]
    fn non_literal_assignment_breaks_chain() {
        // s = "hello"; s = call() → s is no longer literal
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(
                            MirConst::String("hello".into()),
                        )),
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("string_concat".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let removed = elide_literals(&mut f);
        assert_eq!(removed, 0);
    }

    // ── Combined ─────────────────────────────────────────────────

    #[test]
    fn full_elision_pipeline() {
        // Local-only string copied from literal — all RC ops should be eliminated
        let mut f = make_fn(
            vec![string_local(0, "s"), string_local(1, "t")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(
                            MirConst::String("test".into()),
                        )),
                    }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    MirStmt::dummy(MirStmtKind::RcInc { local: local(1) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(0) }),
                    MirStmt::dummy(MirStmtKind::RcDec { local: local(1) }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        elide_rc_ops(&mut f);
        assert_eq!(count_rc_ops(&f.blocks[0].statements), 0);
    }
}
