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

/// What a statement or rvalue names: an operand, or a local named directly
/// (a store's address, a closure, a capture).
#[derive(Debug, Clone, Copy)]
pub enum Named<'a> {
    Operand(&'a MirOperand),
    Local(LocalId),
}

/// Visit everything an rvalue reads.
fn visit_rvalue_named<'a>(rv: &'a MirRValue, f: &mut impl FnMut(Named<'a>)) {
    match rv {
        MirRValue::Use(o) | MirRValue::Deref(o) => f(Named::Operand(o)),
        MirRValue::Ref(id) => f(Named::Local(*id)),
        MirRValue::FuncAddr(_) => {}
        MirRValue::BinaryOp { left, right, .. } => {
            f(Named::Operand(left));
            f(Named::Operand(right));
        }
        MirRValue::UnaryOp { operand, .. } => f(Named::Operand(operand)),
        MirRValue::Cast { value, .. } | MirRValue::Convert { value, .. } => {
            f(Named::Operand(value))
        }
        MirRValue::Field { base, .. } => f(Named::Operand(base)),
        MirRValue::EnumTag { value } => f(Named::Operand(value)),
        MirRValue::ArrayIndex { base, index, .. } => {
            f(Named::Operand(base));
            f(Named::Operand(index));
        }
    }
}

/// Visit every local read by an rvalue.
pub(crate) fn visit_rvalue_uses(rv: &MirRValue, f: &mut impl FnMut(LocalId)) {
    visit_rvalue_named(rv, &mut |n| match n {
        Named::Operand(o) => visit_operand_uses(o, f),
        Named::Local(id) => f(id),
    });
}

/// Visit everything a statement reads, as an operand or as a local it names
/// (not as a write destination).
///
/// Exhaustive on purpose: a new statement kind is a compile error here, not a
/// walk somewhere that quietly skips it. Codegen's string-literal scan was two
/// hand-written walks with a `_ => {}`, and a literal passed to an interface
/// method came out as `""` (#1558).
fn visit_stmt_named<'a>(stmt: &'a MirStmt, f: &mut impl FnMut(Named<'a>)) {
    match &stmt.kind {
        MirStmtKind::Assign { rvalue, .. } => visit_rvalue_named(rvalue, f),
        MirStmtKind::Store { addr, value, .. } => {
            f(Named::Local(*addr));
            f(Named::Operand(value));
        }
        MirStmtKind::Call { args, .. } => {
            args.iter().for_each(|a| f(Named::Operand(a)))
        }
        MirStmtKind::ClosureCall { closure, args, .. } => {
            f(Named::Local(*closure));
            args.iter().for_each(|a| f(Named::Operand(a)));
        }
        MirStmtKind::ClosureCreate { captures, .. }
        | MirStmtKind::EnsureHookRegister { captures, .. } => {
            captures.iter().for_each(|c| f(Named::Local(c.local_id)));
        }
        MirStmtKind::LoadCapture { env_ptr, .. } => f(Named::Local(*env_ptr)),
        MirStmtKind::ClosureDrop { closure, .. } | MirStmtKind::ClosureRetain { closure, .. } => f(Named::Local(*closure)),
        MirStmtKind::ResourceConsume { resource_id } => f(Named::Local(*resource_id)),
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            f(Named::Local(*base));
            f(Named::Operand(index));
            f(Named::Operand(value));
        }
        MirStmtKind::InterfaceBox { value, .. } => f(Named::Operand(value)),
        MirStmtKind::InterfaceCall { interface_object, args, .. } => {
            f(Named::Local(*interface_object));
            args.iter().for_each(|a| f(Named::Operand(a)));
        }
        MirStmtKind::InterfaceDrop { interface_object, .. } => f(Named::Local(*interface_object)),
        MirStmtKind::Phi { args, .. } => {
            args.iter().for_each(|(_, o)| f(Named::Operand(o)))
        }
        MirStmtKind::RcInc { local }
        | MirStmtKind::RcDec { local }
        | MirStmtKind::RcDecContents { local }
        | MirStmtKind::RcIncContents { local } => f(Named::Local(*local)),
        MirStmtKind::ReleaseSlot { addr, .. } => f(Named::Local(*addr)),
        // Bookkeeping: an arm copies the value into the frame's unwind record
        // and nothing reads it there but a panic. Counted as a read, every
        // analysis that treats an unknown reader as a keeper would stop
        // owning what it just armed. `dce` keeps an armed value's def itself.
        // Zeroing a literal's storage is the same: only the unwind glue reads
        // the zeroes, and counted as a read it made a struct built for a box
        // look read by something else, so the box stopped owning it.
        MirStmtKind::ResourceRegister { .. }
        | MirStmtKind::ZeroAggregate { .. }
        | MirStmtKind::GlobalRef { .. }
        | MirStmtKind::EnsurePush { .. }
        | MirStmtKind::EnsurePop
        | MirStmtKind::EnsureHookPop
        | MirStmtKind::UnwindArm { .. }
        | MirStmtKind::UnwindDisarm { .. } => {}
    }
}

/// Visit every local read by a statement (as an operand, not as a write dst).
fn visit_stmt_uses(stmt: &MirStmt, f: &mut impl FnMut(LocalId)) {
    visit_stmt_named(stmt, &mut |n| match n {
        Named::Operand(o) => visit_operand_uses(o, f),
        Named::Local(id) => f(id),
    });
}

/// Visit every operand a statement reads, constants included.
pub fn visit_stmt_operands<'a>(stmt: &'a MirStmt, f: &mut impl FnMut(&'a MirOperand)) {
    visit_stmt_named(stmt, &mut |n| {
        if let Named::Operand(o) = n {
            f(o);
        }
    });
}

/// Visit every operand a terminator reads, constants included.
pub fn visit_terminator_operands<'a>(term: &'a MirTerminator, f: &mut impl FnMut(&'a MirOperand)) {
    match &term.kind {
        MirTerminatorKind::Return { value: Some(op) }
        | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => f(op),
        MirTerminatorKind::Branch { cond, .. } => f(cond),
        MirTerminatorKind::Switch { value, .. } => f(value),
        MirTerminatorKind::Return { value: None }
        | MirTerminatorKind::CleanupReturn { value: None, .. }
        | MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::Unreachable => {}
    }
}

/// What a statement wants from a local it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    /// The local's value is read.
    Value,
    /// The local's *address* is what's wanted — a closure capture, or `Ref`.
    /// Substituting a different local here changes which variable is pointed
    /// at, so a rewrite that replaces reads must leave these alone.
    AddressOf,
}

fn visit_operand_local_mut(op: &mut MirOperand, f: &mut impl FnMut(&mut LocalId, UseKind)) {
    if let MirOperand::Local(id) = op {
        f(id, UseKind::Value);
    }
}

fn visit_rvalue_locals_mut(rv: &mut MirRValue, f: &mut impl FnMut(&mut LocalId, UseKind)) {
    match rv {
        MirRValue::Use(o) | MirRValue::Deref(o) => visit_operand_local_mut(o, f),
        MirRValue::Ref(id) => f(id, UseKind::AddressOf),
        MirRValue::FuncAddr(_) => {}
        MirRValue::BinaryOp { left, right, .. } => {
            visit_operand_local_mut(left, f);
            visit_operand_local_mut(right, f);
        }
        MirRValue::UnaryOp { operand, .. } => visit_operand_local_mut(operand, f),
        MirRValue::Cast { value, .. } | MirRValue::Convert { value, .. } => {
            visit_operand_local_mut(value, f)
        }
        MirRValue::Field { base, .. } => visit_operand_local_mut(base, f),
        MirRValue::EnumTag { value } => visit_operand_local_mut(value, f),
        MirRValue::ArrayIndex { base, index, .. } => {
            visit_operand_local_mut(base, f);
            visit_operand_local_mut(index, f);
        }
    }
}

/// Visit every local a statement reads, with a mutable handle so a pass can
/// substitute one local for another. Mirrors `visit_stmt_uses`; write
/// destinations are not visited (`stmt_def` has those).
pub fn visit_stmt_use_locals_mut(
    stmt: &mut MirStmt,
    f: &mut impl FnMut(&mut LocalId, UseKind),
) {
    match &mut stmt.kind {
        MirStmtKind::Assign { rvalue, .. } => visit_rvalue_locals_mut(rvalue, f),
        MirStmtKind::Store { addr, value, .. } => {
            f(addr, UseKind::Value);
            visit_operand_local_mut(value, f);
        }
        MirStmtKind::Call { args, .. } => {
            args.iter_mut().for_each(|a| visit_operand_local_mut(a, f))
        }
        MirStmtKind::ClosureCall { closure, args, .. } => {
            f(closure, UseKind::Value);
            args.iter_mut().for_each(|a| visit_operand_local_mut(a, f));
        }
        MirStmtKind::ClosureCreate { captures, .. }
        | MirStmtKind::EnsureHookRegister { captures, .. } => {
            for c in captures.iter_mut() {
                // A by-ref capture wants the variable itself; a by-value one
                // wants what it holds. Only the first is an address use.
                let kind = if c.by_ref { UseKind::AddressOf } else { UseKind::Value };
                f(&mut c.local_id, kind);
            }
        }
        MirStmtKind::LoadCapture { env_ptr, .. } => f(env_ptr, UseKind::Value),
        MirStmtKind::ClosureDrop { closure, .. } | MirStmtKind::ClosureRetain { closure, .. } => {
            f(closure, UseKind::Value)
        }
        MirStmtKind::ResourceConsume { resource_id } => f(resource_id, UseKind::Value),
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            f(base, UseKind::Value);
            visit_operand_local_mut(index, f);
            visit_operand_local_mut(value, f);
        }
        MirStmtKind::InterfaceBox { value, .. } => visit_operand_local_mut(value, f),
        MirStmtKind::InterfaceCall { interface_object, args, .. } => {
            f(interface_object, UseKind::Value);
            args.iter_mut().for_each(|a| visit_operand_local_mut(a, f));
        }
        MirStmtKind::InterfaceDrop { interface_object, .. } => f(interface_object, UseKind::Value),
        MirStmtKind::Phi { args, .. } => {
            args.iter_mut().for_each(|(_, o)| visit_operand_local_mut(o, f))
        }
        MirStmtKind::RcInc { local }
        | MirStmtKind::RcDec { local }
        | MirStmtKind::RcDecContents { local }
        | MirStmtKind::RcIncContents { local } => f(local, UseKind::Value),
        // The same shape as a `Store`'s destination: the local holds an address
        // and the release reads through it.
        MirStmtKind::ReleaseSlot { addr, .. } => f(addr, UseKind::Value),
        // Not a read (see `visit_stmt_uses`), but it names the storage, so a
        // pass that moves the aggregate to another local moves this too.
        MirStmtKind::ZeroAggregate { local } => f(local, UseKind::Value),
        MirStmtKind::ResourceRegister { .. }
        | MirStmtKind::GlobalRef { .. }
        | MirStmtKind::EnsurePush { .. }
        | MirStmtKind::EnsurePop
        | MirStmtKind::EnsureHookPop
        | MirStmtKind::UnwindArm { .. }
        | MirStmtKind::UnwindDisarm { .. } => {}
    }
}

/// As `visit_stmt_use_locals_mut`, for a terminator.
pub fn visit_terminator_use_locals_mut(
    term: &mut MirTerminator,
    f: &mut impl FnMut(&mut LocalId, UseKind),
) {
    match &mut term.kind {
        MirTerminatorKind::Return { value: Some(op) }
        | MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
            visit_operand_local_mut(op, f)
        }
        MirTerminatorKind::Branch { cond, .. } => visit_operand_local_mut(cond, f),
        MirTerminatorKind::Switch { value, .. } => visit_operand_local_mut(value, f),
        MirTerminatorKind::Return { value: None }
        | MirTerminatorKind::CleanupReturn { value: None, .. }
        | MirTerminatorKind::Goto { .. }
        | MirTerminatorKind::Unreachable => {}
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
        | MirStmtKind::ClosureCreate { dst, .. }
        | MirStmtKind::LoadCapture { dst, .. }
        | MirStmtKind::ResourceRegister { dst, .. }
        | MirStmtKind::GlobalRef { dst, .. }
        | MirStmtKind::InterfaceBox { dst, .. } => Some(*dst),
        MirStmtKind::Call { dst: Some(d), .. }
        | MirStmtKind::ClosureCall { dst: Some(d), .. }
        | MirStmtKind::InterfaceCall { dst: Some(d), .. } => Some(*d),
        _ => None,
    }
}
