// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Use/def analysis for MIR locals — which locals are read or written by
//! statements and terminators.

use crate::{LocalId, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind};

/// True if `op` references the given local.
pub fn operand_reads(op: &MirOperand, local: LocalId) -> bool {
    matches!(op, MirOperand::Local(id) if *id == local)
}

/// True if the rvalue reads the given local.
pub fn rvalue_reads(rv: &MirRValue, local: LocalId) -> bool {
    match rv {
        MirRValue::Use(op) => operand_reads(op, local),
        MirRValue::Ref(id) => *id == local,
        MirRValue::Deref(op) => operand_reads(op, local),
        MirRValue::BinaryOp { left, right, .. } => {
            operand_reads(left, local) || operand_reads(right, local)
        }
        MirRValue::UnaryOp { operand, .. } => operand_reads(operand, local),
        MirRValue::Cast { value, .. } => operand_reads(value, local),
        MirRValue::Field { base, .. } => operand_reads(base, local),
        MirRValue::EnumTag { value } => operand_reads(value, local),
        MirRValue::ArrayIndex { base, index, .. } => {
            operand_reads(base, local) || operand_reads(index, local)
        }
    }
}

/// True if the statement reads the given local as an operand.
pub fn stmt_reads(stmt: &MirStmt, local: LocalId) -> bool {
    match &stmt.kind {
        MirStmtKind::Assign { rvalue, .. } => rvalue_reads(rvalue, local),
        MirStmtKind::Store { addr, value, .. } => {
            *addr == local || operand_reads(value, local)
        }
        MirStmtKind::Call { args, .. } => {
            args.iter().any(|a| operand_reads(a, local))
        }
        MirStmtKind::ClosureCall { closure, args, .. } => {
            *closure == local || args.iter().any(|a| operand_reads(a, local))
        }
        MirStmtKind::PoolCheckedAccess { pool, handle, .. } => {
            *pool == local || *handle == local
        }
        MirStmtKind::ClosureCreate { captures, .. } => {
            captures.iter().any(|c| c.local_id == local)
        }
        MirStmtKind::LoadCapture { env_ptr, .. } => *env_ptr == local,
        MirStmtKind::ClosureDrop { closure } => *closure == local,
        MirStmtKind::ResourceConsume { resource_id } => *resource_id == local,
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            *base == local || operand_reads(index, local) || operand_reads(value, local)
        }
        MirStmtKind::TraitBox { value, .. } => operand_reads(value, local),
        MirStmtKind::TraitCall { trait_object, args, .. } => {
            *trait_object == local || args.iter().any(|a| operand_reads(a, local))
        }
        MirStmtKind::TraitDrop { trait_object } => *trait_object == local,
        MirStmtKind::ResourceRegister { .. }
        | MirStmtKind::GlobalRef { .. }
        | MirStmtKind::EnsurePush { .. }
        | MirStmtKind::EnsurePop
        | MirStmtKind::ResourceScopeCheck { .. } => false,
    }
}

/// True if the terminator reads a given local.
pub fn terminator_reads(term: &MirTerminator, local: LocalId) -> bool {
    match &term.kind {
        MirTerminatorKind::Return { value: Some(op) } => operand_reads(op, local),
        MirTerminatorKind::Branch { cond, .. } => operand_reads(cond, local),
        MirTerminatorKind::Switch { value, .. } => operand_reads(value, local),
        MirTerminatorKind::CleanupReturn { value: Some(op), .. } => operand_reads(op, local),
        _ => false,
    }
}

/// Return the local defined (written) by this statement, if any.
pub fn stmt_def(stmt: &MirStmt) -> Option<LocalId> {
    match &stmt.kind {
        MirStmtKind::Assign { dst, .. }
        | MirStmtKind::PoolCheckedAccess { dst, .. }
        | MirStmtKind::ClosureCreate { dst, .. }
        | MirStmtKind::LoadCapture { dst, .. }
        | MirStmtKind::ResourceRegister { dst, .. }
        | MirStmtKind::GlobalRef { dst, .. }
        | MirStmtKind::TraitBox { dst, .. } => Some(*dst),
        MirStmtKind::Call { dst: Some(d), .. }
        | MirStmtKind::ClosureCall { dst: Some(d), .. }
        | MirStmtKind::TraitCall { dst: Some(d), .. } => Some(*d),
        _ => None,
    }
}
