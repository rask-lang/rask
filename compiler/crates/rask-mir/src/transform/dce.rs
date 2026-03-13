// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Dead code elimination — removes unreachable blocks and dead assignments.
//!
//! Phase 1: Remove blocks not reachable from the entry block.
//! Phase 2: Remove pure assignments to locals that are never read.
//!
//! Calls, stores, and resource operations are always kept (side effects).

use crate::analysis::cfg;
use crate::analysis::dominators::DominatorTree;
use crate::analysis::liveness;
use crate::{MirFunction, MirStmtKind};

/// Run dead code elimination on a single function.
/// Returns the number of items removed (blocks + statements).
pub fn eliminate_dead_code(func: &mut MirFunction) -> usize {
    let mut removed = 0;
    removed += remove_unreachable_blocks(func);
    removed += remove_dead_assignments_with_liveness(func);
    removed
}

/// Remove blocks not reachable from the entry block.
fn remove_unreachable_blocks(func: &mut MirFunction) -> usize {
    let reachable = cfg::reachable_blocks(func);
    let before = func.blocks.len();
    func.blocks.retain(|b| reachable.contains(&b.id));
    before - func.blocks.len()
}

/// Remove dead assignments using liveness analysis.
///
/// More precise than global read-set: catches assignments that are always
/// overwritten before being read, not just locals that are never read anywhere.
/// Only removes pure assignments (Assign with no side effects in rvalue).
fn remove_dead_assignments_with_liveness(func: &mut MirFunction) -> usize {
    let dom = DominatorTree::build(func);
    let live = liveness::analyze(func, &dom);

    let mut removed = 0;
    for block_idx in 0..func.blocks.len() {
        let block_id = func.blocks[block_idx].id;
        let block = &func.blocks[block_idx];
        let stmts_len = block.statements.len();
        let mut dead_indices = Vec::new();

        for si in 0..stmts_len {
            if let MirStmtKind::Assign { dst, rvalue } = &block.statements[si].kind {
                if !is_pure_rvalue(rvalue) {
                    continue;
                }
                // Check if dst is used in remaining stmts or terminator
                let mut used_after = false;
                for later in (si + 1)..stmts_len {
                    if crate::analysis::uses::stmt_reads(&block.statements[later], *dst) {
                        used_after = true;
                        break;
                    }
                    // If later stmt redefines dst, stop looking
                    if let Some(def) = crate::analysis::uses::stmt_def(&block.statements[later]) {
                        if def == *dst {
                            break;
                        }
                    }
                }
                if !used_after {
                    if crate::analysis::uses::terminator_reads(&block.terminator, *dst) {
                        used_after = true;
                    }
                }
                if !used_after && !live.live_at_exit(block_id, *dst) {
                    dead_indices.push(si);
                }
            }
        }

        // Remove dead statements in reverse order to preserve indices
        for &si in dead_indices.iter().rev() {
            func.blocks[block_idx].statements.remove(si);
            removed += 1;
        }
    }
    removed
}


/// An rvalue is pure if evaluating it has no side effects.
/// All current MirRValue variants are pure (no calls, no stores).
fn is_pure_rvalue(_rv: &crate::MirRValue) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, LocalId, MirBlock, MirOperand, MirRValue, MirStmt, MirTerminator, MirTerminatorKind, MirType, MirLocal, BinOp};

    fn local(n: u32) -> LocalId { LocalId(n) }
    fn block(n: u32) -> BlockId { BlockId(n) }

    fn make_local(id: u32) -> MirLocal {
        MirLocal { id: local(id), name: Some(format!("_{}", id)), ty: MirType::I64, is_param: false }
    }

    #[test]
    fn removes_unreachable_block() {
        let mut func = MirFunction {
            name: "test".to_string(),
            params: vec![],
            locals: vec![make_local(0)],
            blocks: vec![
                MirBlock {
                    id: block(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(0),
                            rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(42))),
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
            entry_block: block(0),
            ret_ty: MirType::Void,
            source_file: None,
            is_extern_c: false,
        };

        let removed = eliminate_dead_code(&mut func);
        assert_eq!(func.blocks.len(), 1);
        assert!(removed >= 1);
    }

    #[test]
    fn removes_dead_assignment() {
        let mut func = MirFunction {
            name: "test".to_string(),
            params: vec![],
            locals: vec![make_local(0), make_local(1)],
            blocks: vec![
                MirBlock {
                    id: block(0),
                    statements: vec![
                        // _0 = 42 (dead — never read)
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(0),
                            rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(42))),
                        }),
                        // _1 = 10 (live — returned)
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(10))),
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(local(1))),
                    }),
                },
            ],
            entry_block: block(0),
            ret_ty: MirType::I64,
            source_file: None,
            is_extern_c: false,
        };

        let removed = eliminate_dead_code(&mut func);
        assert_eq!(removed, 1);
        assert_eq!(func.blocks[0].statements.len(), 1);
    }

    #[test]
    fn keeps_live_assignment() {
        let mut func = MirFunction {
            name: "test".to_string(),
            params: vec![],
            locals: vec![make_local(0), make_local(1)],
            blocks: vec![
                MirBlock {
                    id: block(0),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(0),
                            rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(1))),
                        }),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::BinaryOp {
                                op: BinOp::Add,
                                left: MirOperand::Local(local(0)),
                                right: MirOperand::Constant(crate::MirConst::Int(2)),
                            },
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(local(1))),
                    }),
                },
            ],
            entry_block: block(0),
            ret_ty: MirType::I64,
            source_file: None,
            is_extern_c: false,
        };

        let removed = eliminate_dead_code(&mut func);
        assert_eq!(removed, 0);
        assert_eq!(func.blocks[0].statements.len(), 2);
    }
}
