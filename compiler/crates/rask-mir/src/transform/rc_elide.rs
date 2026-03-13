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
use std::collections::HashSet;

/// Elide unnecessary RC operations on string locals.
pub fn elide_rc_ops(func: &mut MirFunction) {
    let removed_re2 = elide_local_only(func);
    let removed_re3 = elide_literals(func);
    let removed_re1 = cancel_inc_dec_pairs(func);
    let _total = removed_re1 + removed_re2 + removed_re3;
}

/// RE2: Remove all RcInc/RcDec for string locals that never escape the function.
fn elide_local_only(func: &mut MirFunction) -> usize {
    let escaped = escape::escaping_strings(func);
    let mut removed = 0;

    for block in &mut func.blocks {
        let before = block.statements.len();
        block.statements.retain(|stmt| {
            match &stmt.kind {
                MirStmtKind::RcInc { local } | MirStmtKind::RcDec { local } => {
                    // Keep only if the local escapes
                    escaped.contains(local)
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
/// Tracks which locals are "provably literal" — assigned from a string constant
/// or copied from another provably-literal local. Literals use sentinel refcount
/// and SSO strings (≤15 bytes) have no refcount at all.
fn elide_literals(func: &mut MirFunction) -> usize {
    let mut literal_locals: HashSet<LocalId> = HashSet::new();

    // Forward pass: identify locals assigned from string constants
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::Assign { dst, rvalue } = &stmt.kind {
                match rvalue {
                    // Direct string literal assignment
                    MirRValue::Use(MirOperand::Constant(MirConst::String(_))) => {
                        literal_locals.insert(*dst);
                    }
                    // Copy from another literal
                    MirRValue::Use(MirOperand::Local(src)) if literal_locals.contains(src) => {
                        literal_locals.insert(*dst);
                    }
                    // Any other assignment breaks the literal chain
                    _ => {
                        literal_locals.remove(dst);
                    }
                }
            } else if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                // Non-assignment defs (calls, etc.) break literal status
                literal_locals.remove(&dst);
            }
        }
    }

    if literal_locals.is_empty() {
        return 0;
    }

    // Remove RC ops on literal locals
    let mut removed = 0;
    for block in &mut func.blocks {
        let before = block.statements.len();
        block.statements.retain(|stmt| {
            match &stmt.kind {
                MirStmtKind::RcInc { local } | MirStmtKind::RcDec { local } => {
                    !literal_locals.contains(local)
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
