// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Use/def analysis for MIR locals — which locals are read or written by
//! statements and terminators.
//!
//! This is the one place that knows how each `MirStmtKind`/`MirRValue` touches
//! locals. Predicates (`stmt_reads`) and collectors (`stmt_uses`) both derive
//! from the same visitors, so a new statement kind needs one edit here, not one
//! per pass.

use crate::{LocalId, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind};

/// The local an operand refers to, if it's a local (not a constant).
pub fn operand_local(op: &MirOperand) -> Option<LocalId> {
    match op {
        MirOperand::Local(id) => Some(*id),
        MirOperand::Constant(_) => None,
    }
}

/// True if `op` references the given local.
pub fn operand_reads(op: &MirOperand, local: LocalId) -> bool {
    operand_local(op) == Some(local)
}

/// Feed the operand's local (if any) to the visitor.
fn visit_operand_uses(op: &MirOperand, f: &mut impl FnMut(LocalId)) {
    if let Some(id) = operand_local(op) {
        f(id);
    }
}

/// Visit every local read by an rvalue.
fn visit_rvalue_uses(rv: &MirRValue, f: &mut impl FnMut(LocalId)) {
    match rv {
        MirRValue::Use(o) | MirRValue::Deref(o) => visit_operand_uses(o, f),
        MirRValue::Ref(id) => f(*id),
        MirRValue::BinaryOp { left, right, .. } => {
            visit_operand_uses(left, f);
            visit_operand_uses(right, f);
        }
        MirRValue::UnaryOp { operand, .. } => visit_operand_uses(operand, f),
        MirRValue::Cast { value, .. } | MirRValue::Convert { value, .. } => {
            visit_operand_uses(value, f)
        }
        MirRValue::Field { base, .. } => visit_operand_uses(base, f),
        MirRValue::EnumTag { value } => visit_operand_uses(value, f),
        MirRValue::ArrayIndex { base, index, .. } => {
            visit_operand_uses(base, f);
            visit_operand_uses(index, f);
        }
    }
}

/// Visit every local read by a statement (as an operand, not as a write dst).
fn visit_stmt_uses(stmt: &MirStmt, f: &mut impl FnMut(LocalId)) {
    match &stmt.kind {
        MirStmtKind::Assign { rvalue, .. } => visit_rvalue_uses(rvalue, f),
        MirStmtKind::Store { addr, value, .. } => {
            f(*addr);
            visit_operand_uses(value, f);
        }
        MirStmtKind::Call { args, .. } => {
            args.iter().for_each(|a| visit_operand_uses(a, f))
        }
        MirStmtKind::ClosureCall { closure, args, .. } => {
            f(*closure);
            args.iter().for_each(|a| visit_operand_uses(a, f));
        }
        MirStmtKind::PoolCheckedAccess { pool, handle, .. } => {
            f(*pool);
            f(*handle);
        }
        MirStmtKind::ClosureCreate { captures, .. }
        | MirStmtKind::EnsureHookRegister { captures, .. } => {
            captures.iter().for_each(|c| f(c.local_id));
        }
        MirStmtKind::LoadCapture { env_ptr, .. } => f(*env_ptr),
        MirStmtKind::ClosureDrop { closure } => f(*closure),
        MirStmtKind::ResourceConsume { resource_id } => f(*resource_id),
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            f(*base);
            visit_operand_uses(index, f);
            visit_operand_uses(value, f);
        }
        MirStmtKind::TraitBox { value, .. } => visit_operand_uses(value, f),
        MirStmtKind::TraitCall { trait_object, args, .. } => {
            f(*trait_object);
            args.iter().for_each(|a| visit_operand_uses(a, f));
        }
        MirStmtKind::TraitDrop { trait_object } => f(*trait_object),
        MirStmtKind::Phi { args, .. } => {
            args.iter().for_each(|(_, o)| visit_operand_uses(o, f))
        }
        MirStmtKind::RcInc { local }
        | MirStmtKind::RcDec { local }
        | MirStmtKind::RcDecContents { local } => f(*local),
        MirStmtKind::ResourceRegister { .. }
        | MirStmtKind::GlobalRef { .. }
        | MirStmtKind::EnsurePush { .. }
        | MirStmtKind::EnsurePop
        | MirStmtKind::EnsureHookPop
        | MirStmtKind::ResourceScopeCheck { .. } => {}
    }
}

/// True if the rvalue reads the given local.
pub fn rvalue_reads(rv: &MirRValue, local: LocalId) -> bool {
    let mut hit = false;
    visit_rvalue_uses(rv, &mut |id| hit |= id == local);
    hit
}

/// True if the statement reads the given local as an operand.
pub fn stmt_reads(stmt: &MirStmt, local: LocalId) -> bool {
    let mut hit = false;
    visit_stmt_uses(stmt, &mut |id| hit |= id == local);
    hit
}

/// All locals read by a statement (with duplicates — callers dedup if needed).
pub fn stmt_uses(stmt: &MirStmt) -> Vec<LocalId> {
    let mut uses = Vec::new();
    visit_stmt_uses(stmt, &mut |id| uses.push(id));
    uses
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

/// All locals read by a terminator.
pub fn terminator_uses(term: &MirTerminator) -> Vec<LocalId> {
    let op = match &term.kind {
        MirTerminatorKind::Return { value: Some(op) }
        | MirTerminatorKind::Branch { cond: op, .. }
        | MirTerminatorKind::Switch { value: op, .. }
        | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => op,
        _ => return Vec::new(),
    };
    operand_local(op).into_iter().collect()
}

/// Return the local defined (written) by this statement, if any.
pub fn stmt_def(stmt: &MirStmt) -> Option<LocalId> {
    match &stmt.kind {
        MirStmtKind::Assign { dst, .. }
        | MirStmtKind::Phi { dst, .. }
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
