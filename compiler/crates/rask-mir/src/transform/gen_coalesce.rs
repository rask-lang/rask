// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Generation check coalescing — eliminate redundant pool access checks.
//!
//! Each `pool[h]` access emits a `PoolCheckedAccess` that validates the
//! handle's generation at runtime. When multiple accesses to the same
//! (pool, handle) pair occur within a basic block with no intervening
//! pool mutations, redundant checks are replaced with simple copies.
//!
//! See `comp.gen-coalesce` spec for the full algorithm.

use std::collections::{HashMap, HashSet};

use crate::{LocalId, MirFunction, MirOperand, MirRValue, MirStmt};

/// Key for tracking validated (pool, handle) pairs.
type CheckKey = (LocalId, LocalId);

/// Pool-mutating function names that invalidate coalesced checks (MT1).
const POOL_MUTATORS: &[&str] = &[
    "Pool_insert",
    "Pool_remove",
    "Pool_clear",
    "Pool_drain",
    "Pool_alloc",
];

/// Known-safe pool reads that don't invalidate coalescing (MT4).
const SAFE_POOL_CALLS: &[&str] = &[
    "Pool_get",
    "Pool_index",
    "Pool_checked_access",
    "Pool_len",
    "Pool_handles",
    "Pool_values",
    "Pool_is_empty",
    "Pool_modify",
];

/// Coalesce redundant generation checks across all functions.
pub fn coalesce_generation_checks(fns: &mut [MirFunction]) {
    for func in fns.iter_mut() {
        coalesce_function(func);
    }
}

fn coalesce_function(func: &mut MirFunction) {
    // Collect all pool locals referenced by PoolCheckedAccess statements
    let pool_locals: HashSet<LocalId> = func.blocks.iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| {
            if let MirStmt::PoolCheckedAccess { pool, .. } = stmt {
                Some(*pool)
            } else {
                None
            }
        })
        .collect();

    if pool_locals.is_empty() {
        return;
    }

    for block in &mut func.blocks {
        coalesce_block(&mut block.statements, &pool_locals);
    }
}

fn coalesce_block(stmts: &mut [MirStmt], pool_locals: &HashSet<LocalId>) {
    // Map (pool, handle) → dst local from the first PoolCheckedAccess
    let mut checked: HashMap<CheckKey, LocalId> = HashMap::new();

    for stmt in stmts.iter_mut() {
        // Check for pool mutations before processing this statement
        if let Some(mutated_pool) = pool_mutation(stmt) {
            checked.retain(|&(pool, _), _| pool != mutated_pool);
        }

        // Unknown calls with a pool arg invalidate that pool's entries (MT3, CF4)
        if let MirStmt::Call { func, args, .. } = stmt {
            if !is_pool_mutator(&func.name) && !is_safe_pool_call(&func.name) {
                for arg in args.iter() {
                    if let MirOperand::Local(id) = arg {
                        if pool_locals.contains(id) {
                            let id = *id;
                            checked.retain(|&(pool, _), _| pool != id);
                        }
                    }
                }
            }
        }

        // Closure calls could capture pool references (conservative)
        if matches!(stmt, MirStmt::ClosureCall { .. }) {
            checked.clear();
        }

        // Handle reassignment invalidates entries referencing that local (GC3)
        if let Some(assigned) = stmt_def_local(stmt) {
            if !matches!(stmt, MirStmt::PoolCheckedAccess { .. }) {
                checked.retain(|&(pool, handle), &mut dst| {
                    pool != assigned && handle != assigned && dst != assigned
                });
            }
        }

        // Coalesce PoolCheckedAccess
        if let MirStmt::PoolCheckedAccess { dst, pool, handle } = stmt {
            let key = (*pool, *handle);
            if let Some(&prev_dst) = checked.get(&key) {
                // Redundant check — reuse previous result
                *stmt = MirStmt::Assign {
                    dst: *dst,
                    rvalue: MirRValue::Use(MirOperand::Local(prev_dst)),
                };
            } else {
                checked.insert(key, *dst);
            }
        }
    }
}

/// If this statement is a pool mutation, return the pool local being mutated.
fn pool_mutation(stmt: &MirStmt) -> Option<LocalId> {
    if let MirStmt::Call { func, args, .. } = stmt {
        if is_pool_mutator(&func.name) {
            if let Some(MirOperand::Local(pool_id)) = args.first() {
                return Some(*pool_id);
            }
        }
    }
    None
}

fn is_pool_mutator(name: &str) -> bool {
    POOL_MUTATORS.iter().any(|m| *m == name)
}

fn is_safe_pool_call(name: &str) -> bool {
    SAFE_POOL_CALLS.iter().any(|s| *s == name)
}

/// Return the local defined by this statement, if any.
fn stmt_def_local(stmt: &MirStmt) -> Option<LocalId> {
    match stmt {
        MirStmt::Assign { dst, .. }
        | MirStmt::PoolCheckedAccess { dst, .. }
        | MirStmt::ClosureCreate { dst, .. }
        | MirStmt::LoadCapture { dst, .. }
        | MirStmt::ResourceRegister { dst, .. }
        | MirStmt::GlobalRef { dst, .. } => Some(*dst),
        MirStmt::Call { dst: Some(d), .. }
        | MirStmt::ClosureCall { dst: Some(d), .. } => Some(*d),
        _ => None,
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
                MirLocal { id: local(0), name: Some("pool".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(1), name: Some("h".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(2), name: Some("t0".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(3), name: Some("t1".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(4), name: Some("pool2".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(5), name: Some("h2".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(6), name: Some("t2".into()), ty: MirType::I64, is_param: false },
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

    fn pool_access(dst: u32, pool: u32, handle: u32) -> MirStmt {
        MirStmt::PoolCheckedAccess {
            dst: local(dst),
            pool: local(pool),
            handle: local(handle),
        }
    }

    fn pool_call(name: &str, pool: u32) -> MirStmt {
        MirStmt::Call {
            dst: None,
            func: FunctionRef::internal(name.to_string()),
            args: vec![MirOperand::Local(local(pool))],
        }
    }

    fn is_coalesced(stmt: &MirStmt) -> bool {
        matches!(stmt, MirStmt::Assign { rvalue: MirRValue::Use(MirOperand::Local(_)), .. })
    }

    fn is_pool_checked(stmt: &MirStmt) -> bool {
        matches!(stmt, MirStmt::PoolCheckedAccess { .. })
    }

    #[test]
    fn basic_coalescing() {
        // pool[h] twice in same block → second becomes Assign
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_coalesced(&stmts[1]));
    }

    #[test]
    fn three_accesses_one_check() {
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_access(3, 0, 1),
            pool_access(6, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_coalesced(&stmts[1]));
        assert!(is_coalesced(&stmts[2]));
    }

    #[test]
    fn different_handles_no_coalescing() {
        // pool[h1] and pool[h2] → both keep checks
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_access(3, 0, 5),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[1]));
    }

    #[test]
    fn invalidation_by_pool_insert() {
        // pool[h], pool.insert(v), pool[h] → no coalescing across insert
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_call("Pool_insert", 0),
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[2]));
    }

    #[test]
    fn invalidation_by_pool_remove() {
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_call("Pool_remove", 0),
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[2]));
    }

    #[test]
    fn different_pool_mutation_no_invalidation() {
        // pool_a[h], pool_b.insert(v), pool_a[h] → coalesces
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            pool_call("Pool_insert", 4), // pool2
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_coalesced(&stmts[2]));
    }

    #[test]
    fn handle_reassignment_invalidates() {
        // pool[h], h = new_val, pool[h] → no coalescing
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            MirStmt::Assign {
                dst: local(1), // reassign handle
                rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(42))),
            },
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[2]));
    }

    #[test]
    fn unrelated_call_no_invalidation() {
        // pool[h], print(42), pool[h] → coalesces
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            MirStmt::Call {
                dst: None,
                func: FunctionRef::internal("print_i64".to_string()),
                args: vec![MirOperand::Constant(crate::MirConst::Int(42))],
            },
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_coalesced(&stmts[2]));
    }

    #[test]
    fn closure_call_invalidates_all() {
        let mut f = make_fn(vec![
            pool_access(2, 0, 1),
            MirStmt::ClosureCall {
                dst: None,
                closure: local(6),
                args: vec![],
            },
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[2]));
    }

    #[test]
    fn cross_block_isolation() {
        // Same (pool, handle) in different blocks → each gets its own check
        let mut f = MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("pool".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(1), name: Some("h".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(2), name: Some("t0".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(3), name: Some("t1".into()), ty: MirType::I64, is_param: false },
            ],
            blocks: vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![pool_access(2, 0, 1)],
                    terminator: MirTerminator::Goto { target: BlockId(1) },
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![pool_access(3, 0, 1)],
                    terminator: MirTerminator::Return { value: None },
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        assert!(is_pool_checked(&f.blocks[0].statements[0]));
        assert!(is_pool_checked(&f.blocks[1].statements[0]));
    }

    #[test]
    fn no_pool_accesses_is_noop() {
        let mut f = make_fn(vec![
            MirStmt::Call {
                dst: None,
                func: FunctionRef::internal("print_i64".to_string()),
                args: vec![MirOperand::Constant(crate::MirConst::Int(1))],
            },
        ]);
        coalesce_function(&mut f);
        // Should not crash or modify anything
        assert!(matches!(&f.blocks[0].statements[0], MirStmt::Call { .. }));
    }
}
