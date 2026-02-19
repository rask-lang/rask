// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Self-concat → in-place append optimization.
//!
//! Detects the pattern:
//!     _t = call concat(s, arg)
//!     s = Use(_t)
//!
//! And rewrites to:
//!     call string_append(s, arg)
//!
//! Safe because Rask has single-owner semantics — when `s` is immediately
//! overwritten, the old value is dead and mutating in place is equivalent.
//! Eliminates O(n²) copying and per-iteration allocation.

use crate::{LocalId, MirFunction, MirOperand, MirRValue, MirStmt};

/// Rewrite self-concat patterns to in-place append across all functions.
pub fn optimize_string_concat(fns: &mut [MirFunction]) {
    for func in fns.iter_mut() {
        optimize_function(func);
    }
}

fn optimize_function(func: &mut MirFunction) {
    for block in &mut func.blocks {
        optimize_block(&mut block.statements);
    }
}

fn optimize_block(stmts: &mut Vec<MirStmt>) {
    // Scan for the pattern: Call{dst=Some(t), func="concat", args=[Local(s), arg]}
    // followed by Assign{dst=s, rvalue=Use(Local(t))}
    let mut i = 0;
    while i + 1 < stmts.len() {
        if let Some((target_local, temp_local)) = match_self_concat(&stmts[i], &stmts[i + 1]) {
            // Verify the temp is only used in the assignment (not referenced elsewhere)
            if !temp_used_elsewhere(stmts, i, temp_local) {
                // Rewrite: call concat(s, arg) → call string_append(s, arg)
                rewrite_to_append(&mut stmts[i], target_local);
                // Remove the now-redundant assignment
                stmts.remove(i + 1);
                // Don't advance — check the new i+1 pair
                continue;
            }
        }
        i += 1;
    }
}

/// Check if stmts[i] and stmts[i+1] form the self-concat pattern.
/// Returns (target_local, temp_local) if matched.
fn match_self_concat(call_stmt: &MirStmt, assign_stmt: &MirStmt) -> Option<(LocalId, LocalId)> {
    // Match: _t = call concat(Local(s), arg)
    let (temp, target, _arg) = match call_stmt {
        MirStmt::Call {
            dst: Some(temp),
            func,
            args,
        } if func.name == "concat" && args.len() == 2 => {
            match &args[0] {
                MirOperand::Local(s) => (*temp, *s, &args[1]),
                _ => return None,
            }
        }
        _ => return None,
    };

    // Match: s = Use(Local(_t))
    match assign_stmt {
        MirStmt::Assign {
            dst,
            rvalue: MirRValue::Use(MirOperand::Local(src)),
        } if *dst == target && *src == temp => Some((target, temp)),
        _ => None,
    }
}

/// Check whether `temp` is *read* anywhere other than at the call (call_idx)
/// and the assignment (call_idx + 1). Stops forward scan at next redefine of
/// `temp` — reads after a redefine see the new value, not ours.
fn temp_used_elsewhere(stmts: &[MirStmt], call_idx: usize, temp: LocalId) -> bool {
    // Check before the pair
    for j in 0..call_idx {
        if stmt_reads_local(&stmts[j], temp) {
            return true;
        }
    }
    // Check after the pair, stopping at the next redefine of temp
    for j in (call_idx + 2)..stmts.len() {
        if stmt_reads_local(&stmts[j], temp) {
            return true;
        }
        if stmt_defines_local(&stmts[j], temp) {
            break;
        }
    }
    false
}

/// Check if a statement reads a given local (as an operand, not as a write destination).
fn stmt_reads_local(stmt: &MirStmt, local: LocalId) -> bool {
    match stmt {
        MirStmt::Assign { rvalue, .. } => rvalue_references(rvalue, local),
        MirStmt::Call { args, .. } => {
            args.iter().any(|a| operand_is(a, local))
        }
        MirStmt::ClosureCall { closure, args, .. } => {
            *closure == local || args.iter().any(|a| operand_is(a, local))
        }
        MirStmt::Store { addr, value, .. } => {
            *addr == local || operand_is(value, local)
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
            *base == local || operand_is(index, local) || operand_is(value, local)
        }
        MirStmt::ResourceRegister { .. }
        | MirStmt::GlobalRef { .. }
        | MirStmt::SourceLocation { .. }
        | MirStmt::EnsurePush { .. }
        | MirStmt::EnsurePop
        | MirStmt::ResourceScopeCheck { .. } => false,
    }
}

/// Return true if this statement writes (defines) the given local.
fn stmt_defines_local(stmt: &MirStmt, local: LocalId) -> bool {
    match stmt {
        MirStmt::Assign { dst, .. }
        | MirStmt::PoolCheckedAccess { dst, .. }
        | MirStmt::ClosureCreate { dst, .. }
        | MirStmt::LoadCapture { dst, .. }
        | MirStmt::ResourceRegister { dst, .. }
        | MirStmt::GlobalRef { dst, .. } => *dst == local,
        MirStmt::Call { dst: Some(d), .. }
        | MirStmt::ClosureCall { dst: Some(d), .. } => *d == local,
        _ => false,
    }
}

fn operand_is(op: &MirOperand, local: LocalId) -> bool {
    matches!(op, MirOperand::Local(id) if *id == local)
}

fn rvalue_references(rv: &MirRValue, local: LocalId) -> bool {
    match rv {
        MirRValue::Use(op) => operand_is(op, local),
        MirRValue::Ref(id) => *id == local,
        MirRValue::Deref(op) => operand_is(op, local),
        MirRValue::BinaryOp { left, right, .. } => {
            operand_is(left, local) || operand_is(right, local)
        }
        MirRValue::UnaryOp { operand, .. } => operand_is(operand, local),
        MirRValue::Cast { value, .. } => operand_is(value, local),
        MirRValue::Field { base, .. } => operand_is(base, local),
        MirRValue::EnumTag { value } => operand_is(value, local),
        MirRValue::ArrayIndex { base, index, .. } => {
            operand_is(base, local) || operand_is(index, local)
        }
    }
}

/// Rewrite a `concat` call to in-place append, dropping the return value.
/// Uses `string_append_cstr` when the second arg is a string constant
/// (avoids allocating a temporary RaskString for the literal).
fn rewrite_to_append(stmt: &mut MirStmt, _target: LocalId) {
    if let MirStmt::Call { dst, func, args } = stmt {
        let use_cstr = matches!(args.get(1), Some(MirOperand::Constant(crate::MirConst::String(_))));
        func.name = if use_cstr {
            "string_append_cstr".to_string()
        } else {
            "string_append".to_string()
        };
        *dst = None; // append mutates in place, return value unused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, FunctionRef, MirType};
    use crate::function::{MirBlock, MirLocal};
    use crate::MirTerminator;

    fn local(id: u32) -> LocalId {
        LocalId(id)
    }

    fn make_fn(stmts: Vec<MirStmt>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("s".into()), ty: MirType::String, is_param: false },
                MirLocal { id: local(1), name: Some("_t".into()), ty: MirType::String, is_param: false },
                MirLocal { id: local(2), name: Some("arg".into()), ty: MirType::String, is_param: false },
                MirLocal { id: local(3), name: Some("other".into()), ty: MirType::I64, is_param: false },
            ],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: stmts,
                terminator: MirTerminator::Return { value: None },
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn concat_call(dst: u32, src: u32, arg: MirOperand) -> MirStmt {
        MirStmt::Call {
            dst: Some(local(dst)),
            func: FunctionRef::internal("concat".to_string()),
            args: vec![MirOperand::Local(local(src)), arg],
        }
    }

    fn assign_use(dst: u32, src: u32) -> MirStmt {
        MirStmt::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Local(local(src))),
        }
    }

    #[test]
    fn basic_self_concat_rewrite() {
        // _t = concat(s, arg) → s = Use(_t)  ⟹  string_append(s, arg)
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Local(local(2))),
            assign_use(0, 1),
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            MirStmt::Call { dst, func, args } => {
                assert_eq!(func.name, "string_append");
                assert!(dst.is_none());
                assert_eq!(args.len(), 2);
                assert!(matches!(&args[0], MirOperand::Local(id) if *id == local(0)));
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn self_concat_with_string_literal_uses_cstr() {
        // _t = concat(s, "x") → s = Use(_t)  ⟹  string_append_cstr(s, "x")
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Constant(crate::MirConst::String("x".into()))),
            assign_use(0, 1),
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            MirStmt::Call { func, .. } => assert_eq!(func.name, "string_append_cstr"),
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn no_rewrite_when_different_target() {
        // _t = concat(s, arg) → other = Use(_t)  — different target, don't rewrite
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Local(local(2))),
            assign_use(3, 1), // assigns to `other`, not `s`
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 2); // no change
        match &stmts[0] {
            MirStmt::Call { func, .. } => assert_eq!(func.name, "concat"),
            other => panic!("expected concat Call, got {:?}", other),
        }
    }

    #[test]
    fn no_rewrite_when_temp_used_elsewhere() {
        // _t = concat(s, arg) → s = Use(_t), then _t used again
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Local(local(2))),
            assign_use(0, 1),
            // _t referenced in another statement
            MirStmt::Call {
                dst: None,
                func: FunctionRef::internal("print_string".to_string()),
                args: vec![MirOperand::Local(local(1))],
            },
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 3); // no change
        match &stmts[0] {
            MirStmt::Call { func, .. } => assert_eq!(func.name, "concat"),
            other => panic!("expected concat Call, got {:?}", other),
        }
    }

    #[test]
    fn no_rewrite_non_adjacent() {
        // _t = concat(s, arg), <other stmt>, s = Use(_t) — not adjacent
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Local(local(2))),
            MirStmt::Call {
                dst: None,
                func: FunctionRef::internal("print_i64".to_string()),
                args: vec![MirOperand::Constant(crate::MirConst::Int(42))],
            },
            assign_use(0, 1),
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 3); // no change
    }

    #[test]
    fn multiple_self_concats_in_sequence() {
        // Two self-concats: local arg → string_append, literal arg → string_append_cstr
        let mut f = make_fn(vec![
            concat_call(1, 0, MirOperand::Local(local(2))),
            assign_use(0, 1),
            concat_call(1, 0, MirOperand::Constant(crate::MirConst::String("y".into()))),
            assign_use(0, 1),
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 2);
        match &stmts[0] {
            MirStmt::Call { func, .. } => assert_eq!(func.name, "string_append"),
            other => panic!("expected string_append, got {:?}", other),
        }
        match &stmts[1] {
            MirStmt::Call { func, .. } => assert_eq!(func.name, "string_append_cstr"),
            other => panic!("expected string_append_cstr, got {:?}", other),
        }
    }

    #[test]
    fn constant_first_arg_not_rewritten() {
        // _t = concat("literal", arg) → not a self-concat (first arg isn't a local)
        let mut f = make_fn(vec![
            MirStmt::Call {
                dst: Some(local(1)),
                func: FunctionRef::internal("concat".to_string()),
                args: vec![
                    MirOperand::Constant(crate::MirConst::String("hello".into())),
                    MirOperand::Local(local(2)),
                ],
            },
            assign_use(0, 1),
        ]);
        optimize_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert_eq!(stmts.len(), 2); // no change — can't mutate a constant
    }
}
