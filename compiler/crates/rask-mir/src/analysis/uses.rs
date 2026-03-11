// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Use/def analysis for MIR locals — which locals are read or written by
//! statements and terminators.

use crate::{LocalId, MirOperand, MirRValue, MirStmt, MirTerminator};

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
    match stmt {
        MirStmt::Assign { rvalue, .. } => rvalue_reads(rvalue, local),
        MirStmt::Store { addr, value, .. } => {
            *addr == local || operand_reads(value, local)
        }
        MirStmt::Call { args, .. } => {
            args.iter().any(|a| operand_reads(a, local))
        }
        MirStmt::ClosureCall { closure, args, .. } => {
            *closure == local || args.iter().any(|a| operand_reads(a, local))
        }
        MirStmt::PoolCheckedAccess { pool, handle, .. } => {
            *pool == local || *handle == local
        }
        MirStmt::ClosureCreate { captures, .. } => {
            captures.iter().any(|c| c.local_id == local)
        }
        MirStmt::LoadCapture { env_ptr, .. } => *env_ptr == local,
        MirStmt::ClosureDrop { closure } => *closure == local,
        MirStmt::ResourceConsume { resource_id } => *resource_id == local,
        MirStmt::ArrayStore { base, index, value, .. } => {
            *base == local || operand_reads(index, local) || operand_reads(value, local)
        }
        MirStmt::TraitBox { value, .. } => operand_reads(value, local),
        MirStmt::TraitCall { trait_object, args, .. } => {
            *trait_object == local || args.iter().any(|a| operand_reads(a, local))
        }
        MirStmt::TraitDrop { trait_object } => *trait_object == local,
        MirStmt::ResourceRegister { .. }
        | MirStmt::GlobalRef { .. }
        | MirStmt::SourceLocation { .. }
        | MirStmt::EnsurePush { .. }
        | MirStmt::EnsurePop
        | MirStmt::ResourceScopeCheck { .. } => false,
    }
}

/// True if the terminator reads a given local.
pub fn terminator_reads(term: &MirTerminator, local: LocalId) -> bool {
    match term {
        MirTerminator::Return { value: Some(op) } => operand_reads(op, local),
        MirTerminator::Branch { cond, .. } => operand_reads(cond, local),
        MirTerminator::Switch { value, .. } => operand_reads(value, local),
        MirTerminator::CleanupReturn { value: Some(op), .. } => operand_reads(op, local),
        _ => false,
    }
}

/// Return the local defined (written) by this statement, if any.
pub fn stmt_def(stmt: &MirStmt) -> Option<LocalId> {
    match stmt {
        MirStmt::Assign { dst, .. }
        | MirStmt::PoolCheckedAccess { dst, .. }
        | MirStmt::ClosureCreate { dst, .. }
        | MirStmt::LoadCapture { dst, .. }
        | MirStmt::ResourceRegister { dst, .. }
        | MirStmt::GlobalRef { dst, .. }
        | MirStmt::TraitBox { dst, .. } => Some(*dst),
        MirStmt::Call { dst: Some(d), .. }
        | MirStmt::ClosureCall { dst: Some(d), .. }
        | MirStmt::TraitCall { dst: Some(d), .. } => Some(*d),
        _ => None,
    }
}
