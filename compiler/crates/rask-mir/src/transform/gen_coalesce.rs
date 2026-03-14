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

use crate::{BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind};
use crate::analysis::{cfg, pool_ops, uses};

/// Key for tracking validated (pool, handle) pairs.
type CheckKey = (LocalId, LocalId);

/// Checked state at the exit of a block: maps (pool, handle) → result local.
type CheckedMap = HashMap<CheckKey, LocalId>;

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
            if let MirStmtKind::PoolCheckedAccess { pool, .. } = &stmt.kind {
                Some(*pool)
            } else {
                None
            }
        })
        .collect();

    if pool_locals.is_empty() {
        return;
    }

    // Phase 1: Per-block coalescing (original algorithm)
    for block in &mut func.blocks {
        coalesce_block(&mut block.statements, &pool_locals);
    }

    // Phase 2: Cross-block propagation (CF2 expansion).
    // Propagate validated (pool, handle) pairs from dominating blocks
    // into successors along Goto edges (linear chains and if-else merges).
    cross_block_coalesce(func, &pool_locals);
}

/// Propagate validated checks across block boundaries.
///
/// Process blocks in entry-first order. For each block, compute the incoming
/// checked set from predecessors:
/// - Single predecessor via Goto: inherit exit state
/// - Multiple predecessors: intersect exit states (only pairs valid in ALL predecessors)
/// - Loop back-edges (target ≤ source in RPO): ignored (CF3: fresh check per iteration)
fn cross_block_coalesce(func: &mut MirFunction, pool_locals: &HashSet<LocalId>) {
    if func.blocks.len() <= 1 {
        return;
    }

    // Build block index for fast lookup
    let block_ids: Vec<BlockId> = func.blocks.iter().map(|b| b.id).collect();
    let block_index: HashMap<BlockId, usize> = block_ids.iter()
        .enumerate()
        .map(|(i, &id)| (id, i))
        .collect();

    // Compute predecessor map (only forward edges — target index > source index)
    let mut predecessors: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for (src_idx, block) in func.blocks.iter().enumerate() {
        for target in cfg::successors(&block.terminator) {
            if let Some(&tgt_idx) = block_index.get(&target) {
                // CF3: Skip back-edges (loop boundaries)
                if tgt_idx > src_idx {
                    predecessors.entry(target).or_default().push(block.id);
                }
            }
        }
    }

    // Compute exit state for each block (simulate without modifying)
    let mut exit_states: HashMap<BlockId, CheckedMap> = HashMap::new();
    for block in func.blocks.iter() {
        let exit = compute_block_exit_state(&block.statements, pool_locals);
        exit_states.insert(block.id, exit);
    }

    // Process blocks in order (entry-first), applying incoming state
    for block_idx in 0..func.blocks.len() {
        let bid = func.blocks[block_idx].id;
        let preds = match predecessors.get(&bid) {
            Some(p) => p.clone(),
            None => continue, // Entry block or unreachable — no propagation
        };

        // Compute incoming checked set from predecessors
        let incoming = intersect_predecessor_states(&preds, &exit_states);
        if incoming.is_empty() {
            continue;
        }

        // Apply incoming state to this block's statements
        let stmts = &mut func.blocks[block_idx].statements;
        apply_incoming_checks(stmts, &incoming, pool_locals);
    }
}

/// Compute the set of validated (pool, handle) pairs at the exit of a block.
fn compute_block_exit_state(
    stmts: &[MirStmt],
    pool_locals: &HashSet<LocalId>,
) -> CheckedMap {
    let mut checked: CheckedMap = HashMap::new();

    for stmt in stmts {
        process_invalidations(stmt, &mut checked, pool_locals);

        if let MirStmtKind::PoolCheckedAccess { dst, pool, handle } = &stmt.kind {
            checked.entry((*pool, *handle)).or_insert(*dst);
        }
        // Coalesced accesses (Assign from a checked local) also carry forward
    }

    checked
}

/// Intersect exit states from all predecessors. Only keep (pool, handle) pairs
/// that are validated in ALL predecessors.
fn intersect_predecessor_states(
    preds: &[BlockId],
    exit_states: &HashMap<BlockId, CheckedMap>,
) -> CheckedMap {
    let mut iter = preds.iter().filter_map(|p| exit_states.get(p));
    let first = match iter.next() {
        Some(s) => s.clone(),
        None => return CheckedMap::new(),
    };

    let mut result = first;
    for state in iter {
        result.retain(|key, _| state.contains_key(key));
    }
    result
}

/// Apply incoming checked pairs to a block's statements: if a PoolCheckedAccess
/// at the start of the block checks a pair already validated by incoming state,
/// replace it with an Assign (reuse the predecessor's result local).
fn apply_incoming_checks(
    stmts: &mut [MirStmt],
    incoming: &CheckedMap,
    pool_locals: &HashSet<LocalId>,
) {
    let mut live = incoming.clone();

    for stmt in stmts.iter_mut() {
        // Invalidations kill entries
        process_invalidations(stmt, &mut live, pool_locals);

        if let MirStmtKind::PoolCheckedAccess { dst, pool, handle } = &stmt.kind {
            let key = (*pool, *handle);
            let dst = *dst;
            if let Some(&prev_dst) = live.get(&key) {
                // Already validated by predecessor — reuse
                let span = stmt.span;
                *stmt = MirStmt::new(MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Use(MirOperand::Local(prev_dst)),
                }, span);
            } else {
                live.insert(key, dst);
            }
        }
    }
}

/// Process invalidations from a statement, updating the checked map.
fn process_invalidations(
    stmt: &MirStmt,
    checked: &mut CheckedMap,
    pool_locals: &HashSet<LocalId>,
) {
    if let Some(mutated_pool) = pool_ops::pool_mutation(stmt) {
        checked.retain(|&(pool, _), _| pool != mutated_pool);
    }

    if let MirStmtKind::Call { func, args, .. } = &stmt.kind {
        if !pool_ops::is_pool_mutator(&func.name) && !pool_ops::is_safe_pool_call(&func.name) {
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

    if matches!(&stmt.kind, MirStmtKind::ClosureCall { .. }) {
        checked.clear();
    }

    if let Some(assigned) = uses::stmt_def(stmt) {
        if !matches!(&stmt.kind, MirStmtKind::PoolCheckedAccess { .. }) {
            checked.retain(|&(pool, handle), &mut dst| {
                pool != assigned && handle != assigned && dst != assigned
            });
        }
    }
}


fn coalesce_block(stmts: &mut [MirStmt], pool_locals: &HashSet<LocalId>) {
    // Map (pool, handle) → dst local from the first PoolCheckedAccess
    let mut checked: HashMap<CheckKey, LocalId> = HashMap::new();

    for stmt in stmts.iter_mut() {
        // Check for pool mutations before processing this statement
        if let Some(mutated_pool) = pool_ops::pool_mutation(stmt) {
            checked.retain(|&(pool, _), _| pool != mutated_pool);
        }

        // Unknown calls with a pool arg invalidate that pool's entries (MT3, CF4)
        if let MirStmtKind::Call { func, args, .. } = &stmt.kind {
            if !pool_ops::is_pool_mutator(&func.name) && !pool_ops::is_safe_pool_call(&func.name) {
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
        if matches!(&stmt.kind, MirStmtKind::ClosureCall { .. }) {
            checked.clear();
        }

        // Handle reassignment invalidates entries referencing that local (GC3)
        if let Some(assigned) = uses::stmt_def(stmt) {
            if !matches!(&stmt.kind, MirStmtKind::PoolCheckedAccess { .. }) {
                checked.retain(|&(pool, handle), &mut dst| {
                    pool != assigned && handle != assigned && dst != assigned
                });
            }
        }

        // Coalesce PoolCheckedAccess
        if let MirStmtKind::PoolCheckedAccess { dst, pool, handle } = &stmt.kind {
            let key = (*pool, *handle);
            let dst = *dst;
            if let Some(&prev_dst) = checked.get(&key) {
                // Redundant check — reuse previous result
                let span = stmt.span;
                *stmt = MirStmt::new(MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Use(MirOperand::Local(prev_dst)),
                }, span);
            } else {
                checked.insert(key, dst);
            }
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, FunctionRef, MirTerminator, MirTerminatorKind, MirType};
    use crate::function::{MirBlock, MirLocal};

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
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn pool_access(dst: u32, pool: u32, handle: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
            dst: local(dst),
            pool: local(pool),
            handle: local(handle),
        })
    }

    fn pool_call(name: &str, pool: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(name.to_string()),
            args: vec![MirOperand::Local(local(pool))],
        })
    }

    fn is_coalesced(stmt: &MirStmt) -> bool {
        matches!(&stmt.kind, MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(_)), .. })
    }

    fn is_pool_checked(stmt: &MirStmt) -> bool {
        matches!(&stmt.kind, MirStmtKind::PoolCheckedAccess { .. })
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
            MirStmt::dummy(MirStmtKind::Assign {
                dst: local(1), // reassign handle
                rvalue: MirRValue::Use(MirOperand::Constant(crate::MirConst::Int(42))),
            }),
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
            MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("print_i64".to_string()),
                args: vec![MirOperand::Constant(crate::MirConst::Int(42))],
            }),
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
            MirStmt::dummy(MirStmtKind::ClosureCall {
                dst: None,
                closure: local(6),
                args: vec![],
            }),
            pool_access(3, 0, 1),
        ]);
        coalesce_function(&mut f);
        let stmts = &f.blocks[0].statements;
        assert!(is_pool_checked(&stmts[0]));
        assert!(is_pool_checked(&stmts[2]));
    }

    #[test]
    fn cross_block_goto_coalesces() {
        // Block 0: pool[h] → Goto Block 1
        // Block 1: pool[h] → Return
        // With cross-block propagation, Block 1's check should be coalesced
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
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![pool_access(3, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        assert!(is_pool_checked(&f.blocks[0].statements[0]));
        // Cross-block: Block 1 inherits Block 0's validated pair
        assert!(is_coalesced(&f.blocks[1].statements[0]));
    }

    #[test]
    fn cross_block_if_else_merge_coalesces() {
        // Block 0: pool[h]; branch to 1/2
        // Block 1: pool[h]; goto 3
        // Block 2: pool[h]; goto 3
        // Block 3: pool[h] — should be coalesced (both predecessors validated it)
        let mut f = MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("pool".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(1), name: Some("h".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(2), name: Some("t0".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(3), name: Some("t1".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(4), name: Some("t2".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(5), name: Some("t3".into()), ty: MirType::I64, is_param: false },
            ],
            blocks: vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![pool_access(2, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Constant(crate::MirConst::Bool(true)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![pool_access(3, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) }),
                },
                MirBlock {
                    id: BlockId(2),
                    statements: vec![pool_access(4, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) }),
                },
                MirBlock {
                    id: BlockId(3),
                    statements: vec![pool_access(5, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        // Block 0: original check
        assert!(is_pool_checked(&f.blocks[0].statements[0]));
        // Block 1, 2: coalesced from Block 0's propagation
        assert!(is_coalesced(&f.blocks[1].statements[0]));
        assert!(is_coalesced(&f.blocks[2].statements[0]));
        // Block 3: both predecessors (1, 2) have validated — coalesced
        assert!(is_coalesced(&f.blocks[3].statements[0]));
    }

    #[test]
    fn cross_block_mutation_breaks_propagation() {
        // Block 0: pool[h]; goto 1
        // Block 1: pool.insert(); pool[h] — mutation invalidates → fresh check
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
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![
                        pool_call("Pool_insert", 0),
                        pool_access(3, 0, 1),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        assert!(is_pool_checked(&f.blocks[0].statements[0]));
        // Mutation in Block 1 invalidates the incoming state → fresh check needed
        assert!(is_pool_checked(&f.blocks[1].statements[1]));
    }

    #[test]
    fn cross_block_loop_back_edge_fresh_check() {
        // Block 0: pool[h]; goto 1
        // Block 1: pool[h]; goto 0 (loop back-edge)
        // CF3: Back-edges are ignored, so Block 0 always gets a fresh check
        // (on first pass, Block 1 still gets coalesced from Block 0's forward edge)
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
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![pool_access(3, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(0) }),
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        // Block 0: always checked (entry block, no forward predecessors)
        assert!(is_pool_checked(&f.blocks[0].statements[0]));
        // Block 1: coalesced from Block 0's forward edge
        assert!(is_coalesced(&f.blocks[1].statements[0]));
    }

    #[test]
    fn no_pool_accesses_is_noop() {
        let mut f = make_fn(vec![
            MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("print_i64".to_string()),
                args: vec![MirOperand::Constant(crate::MirConst::Int(1))],
            }),
        ]);
        coalesce_function(&mut f);
        // Should not crash or modify anything
        assert!(matches!(&f.blocks[0].statements[0].kind, MirStmtKind::Call { .. }));
    }
}
