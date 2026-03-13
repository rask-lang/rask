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

use crate::analysis::dominators::DominatorTree;
use crate::analysis::liveness;
use crate::analysis::uses;
use crate::{LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirType};

/// Insert explicit RcInc/RcDec for all string-typed locals in a function.
pub fn insert_rc_ops(func: &mut MirFunction) {
    let string_locals: Vec<LocalId> = func.locals.iter()
        .chain(func.params.iter())
        .filter(|l| l.ty == MirType::String)
        .map(|l| l.id)
        .collect();

    if string_locals.is_empty() {
        return;
    }

    // Insert RcInc after string copies
    insert_rc_inc(func, &string_locals);

    // Insert RcDec at last-use points
    insert_rc_dec(func, &string_locals);
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

                _ => {}
            }
        }

        // Apply insertions in reverse to preserve indices
        for (idx, stmt) in insertions.into_iter().rev() {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
    }
}

/// Insert `RcDec` at last-use points for string locals.
///
/// Uses liveness analysis: when a string local is live at a statement but dead
/// after it (no further uses on any path), insert `RcDec` after that statement.
fn insert_rc_dec(func: &mut MirFunction, string_locals: &[LocalId]) {
    let dom = DominatorTree::build(func);
    let live = liveness::analyze(func, &dom);

    for block_idx in 0..func.blocks.len() {
        let block_id = func.blocks[block_idx].id;
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        let stmts_len = func.blocks[block_idx].statements.len();

        for local in string_locals {
            // Find the last use of this local in the block
            let mut last_use_idx: Option<usize> = None;

            for si in 0..stmts_len {
                let stmt = &func.blocks[block_idx].statements[si];
                if uses::stmt_reads(stmt, *local) {
                    last_use_idx = Some(si);
                }
                // If this statement defines the local, earlier uses are irrelevant
                if uses::stmt_def(stmt) == Some(*local) {
                    last_use_idx = None;
                }
            }

            // Check terminator
            let term_reads = uses::terminator_reads(&func.blocks[block_idx].terminator, *local);

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
                insertions.push((si + 1, MirStmt::new(
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
