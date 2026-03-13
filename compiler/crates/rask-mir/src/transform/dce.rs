// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Dead code elimination — removes unreachable blocks and dead assignments.
//!
//! Phase 1: Remove blocks not reachable from the entry block.
//! Phase 2: Remove pure assignments to locals that are never read.
//!
//! Calls, stores, and resource operations are always kept (side effects).

use std::collections::HashSet;

use crate::analysis::cfg;
use crate::{LocalId, MirFunction, MirStmt, MirStmtKind};

/// Run dead code elimination on a single function.
/// Returns the number of items removed (blocks + statements).
pub fn eliminate_dead_code(func: &mut MirFunction) -> usize {
    let mut removed = 0;
    removed += remove_unreachable_blocks(func);
    removed += remove_dead_assignments(func);
    removed
}

/// Remove blocks not reachable from the entry block.
fn remove_unreachable_blocks(func: &mut MirFunction) -> usize {
    let reachable = cfg::reachable_blocks(func);
    let before = func.blocks.len();
    func.blocks.retain(|b| reachable.contains(&b.id));
    before - func.blocks.len()
}

/// Remove assignments to locals that are never read by any statement or terminator.
///
/// Only removes pure assignments (Assign with no side effects in rvalue).
/// Calls, stores, closures, resource ops, etc. are always kept.
fn remove_dead_assignments(func: &mut MirFunction) -> usize {
    // Collect all locals that are read anywhere
    let mut read_locals: HashSet<LocalId> = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            collect_reads(stmt, &mut read_locals);
        }
        collect_terminator_reads(&block.terminator, &mut read_locals);
    }

    // Remove Assign statements where dst is never read and rvalue is pure
    let mut removed = 0;
    for block in &mut func.blocks {
        let before = block.statements.len();
        block.statements.retain(|stmt| {
            if let MirStmtKind::Assign { dst, rvalue } = &stmt.kind {
                if !read_locals.contains(dst) && is_pure_rvalue(rvalue) {
                    return false;
                }
            }
            true
        });
        removed += before - block.statements.len();
    }
    removed
}

/// Collect all locals read by a statement.
fn collect_reads(stmt: &MirStmt, reads: &mut HashSet<LocalId>) {
    match &stmt.kind {
        MirStmtKind::Assign { rvalue, .. } => collect_rvalue_reads(rvalue, reads),
        MirStmtKind::Store { addr, value, .. } => {
            reads.insert(*addr);
            collect_operand_reads(value, reads);
        }
        MirStmtKind::Call { args, .. } => {
            for a in args { collect_operand_reads(a, reads); }
        }
        MirStmtKind::ClosureCall { closure, args, .. } => {
            reads.insert(*closure);
            for a in args { collect_operand_reads(a, reads); }
        }
        MirStmtKind::PoolCheckedAccess { pool, handle, .. } => {
            reads.insert(*pool);
            reads.insert(*handle);
        }
        MirStmtKind::ClosureCreate { captures, .. } => {
            for c in captures { reads.insert(c.local_id); }
        }
        MirStmtKind::LoadCapture { env_ptr, .. } => { reads.insert(*env_ptr); }
        MirStmtKind::ClosureDrop { closure } => { reads.insert(*closure); }
        MirStmtKind::ResourceConsume { resource_id } => { reads.insert(*resource_id); }
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            reads.insert(*base);
            collect_operand_reads(index, reads);
            collect_operand_reads(value, reads);
        }
        MirStmtKind::TraitBox { value, .. } => { collect_operand_reads(value, reads); }
        MirStmtKind::TraitCall { trait_object, args, .. } => {
            reads.insert(*trait_object);
            for a in args { collect_operand_reads(a, reads); }
        }
        MirStmtKind::TraitDrop { trait_object } => { reads.insert(*trait_object); }
        MirStmtKind::GlobalRef { .. }
        | MirStmtKind::ResourceRegister { .. }
        | MirStmtKind::EnsurePush { .. }
        | MirStmtKind::EnsurePop
        | MirStmtKind::ResourceScopeCheck { .. } => {}
    }
}

fn collect_operand_reads(op: &crate::MirOperand, reads: &mut HashSet<LocalId>) {
    if let crate::MirOperand::Local(id) = op {
        reads.insert(*id);
    }
}

fn collect_rvalue_reads(rv: &crate::MirRValue, reads: &mut HashSet<LocalId>) {
    match rv {
        crate::MirRValue::Use(op) => collect_operand_reads(op, reads),
        crate::MirRValue::Ref(id) => { reads.insert(*id); }
        crate::MirRValue::Deref(op) => collect_operand_reads(op, reads),
        crate::MirRValue::BinaryOp { left, right, .. } => {
            collect_operand_reads(left, reads);
            collect_operand_reads(right, reads);
        }
        crate::MirRValue::UnaryOp { operand, .. } => collect_operand_reads(operand, reads),
        crate::MirRValue::Cast { value, .. } => collect_operand_reads(value, reads),
        crate::MirRValue::Field { base, .. } => collect_operand_reads(base, reads),
        crate::MirRValue::EnumTag { value } => collect_operand_reads(value, reads),
        crate::MirRValue::ArrayIndex { base, index, .. } => {
            collect_operand_reads(base, reads);
            collect_operand_reads(index, reads);
        }
    }
}

fn collect_terminator_reads(term: &crate::MirTerminator, reads: &mut HashSet<LocalId>) {
    match &term.kind {
        crate::MirTerminatorKind::Return { value: Some(op) } => collect_operand_reads(op, reads),
        crate::MirTerminatorKind::Branch { cond, .. } => collect_operand_reads(cond, reads),
        crate::MirTerminatorKind::Switch { value, .. } => collect_operand_reads(value, reads),
        crate::MirTerminatorKind::CleanupReturn { value: Some(op), .. } => collect_operand_reads(op, reads),
        _ => {}
    }
}

/// An rvalue is pure if evaluating it has no side effects.
/// All current MirRValue variants are pure (no calls, no stores).
fn is_pure_rvalue(_rv: &crate::MirRValue) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, MirBlock, MirOperand, MirRValue, MirTerminator, MirTerminatorKind, MirType, MirLocal, BinOp};

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
