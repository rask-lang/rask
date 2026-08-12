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

    // Aggregate-element pool accesses must NOT coalesce. A checked access returns
    // a pointer into the pool slot; reuse is expressed as `Assign Use(prev)`,
    // which codegen value-copies for struct/enum/tuple locals (the same rule that
    // makes `p = q` copy bytes). That copy detaches the reused local from the
    // arena, so a later `pool[h].f = v` writes into the copy and is lost (#402).
    // Scalar-element accesses copy the pointer fine, so they still coalesce.
    // Keeping the real check for aggregates is a small cost; a proper
    // pointer-alias reuse can re-enable it later.
    let aggregate_results: HashSet<LocalId> = func.locals.iter()
        .filter(|l| matches!(l.ty,
            crate::MirType::Struct(_) | crate::MirType::Enum(_) | crate::MirType::Tuple(_)))
        .map(|l| l.id)
        .collect();

    // Phase 1: Per-block coalescing (original algorithm)
    for block in &mut func.blocks {
        coalesce_block(&mut block.statements, &pool_locals, &aggregate_results);
    }

    // Phase 2: Cross-block propagation (CF2 expansion).
    // Propagate validated (pool, handle) pairs from dominating blocks
    // into successors along Goto edges (linear chains and if-else merges).
    cross_block_coalesce(func, &pool_locals, &aggregate_results);
}

/// Propagate validated checks across block boundaries.
///
/// Single incremental pass in entry-first block order (each forward predecessor
/// has a lower index than its successor, so by the time we reach a block every
/// forward predecessor's real exit state — reflecting any reuse it already did —
/// is on hand):
/// - Single predecessor via Goto: inherit its exit state
/// - Multiple predecessors: keep a (pool, handle) pair only where every
///   predecessor landed on the *same* result local (#368) — two predecessors
///   that each ran their own independent check store the pair in different
///   locals, and only one of those locals is actually defined on any given
///   incoming path, so reusing either at the merge reads an undefined value.
///   Requiring the identical local is what still lets the common
///   check-before-branch pattern coalesce: both arms just alias the
///   dominating check's local, so they agree.
/// - Loop back-edges (target ≤ source in block order): ignored (CF3: fresh
///   check per iteration). Keeping block order (rather than the dominator
///   tree) for this is deliberate — the incremental pass below depends on it.
fn cross_block_coalesce(
    func: &mut MirFunction,
    pool_locals: &HashSet<LocalId>,
    aggregate_results: &HashSet<LocalId>,
) {
    if func.blocks.len() <= 1 {
        return;
    }

    let predecessors = cfg::forward_predecessors(func);
    let mut exit_states: HashMap<BlockId, CheckedMap> = HashMap::new();

    for block_idx in 0..func.blocks.len() {
        let bid = func.blocks[block_idx].id;
        let incoming = match predecessors.get(&bid) {
            Some(preds) => intersect_predecessor_states(preds, &exit_states),
            None => CheckedMap::new(), // Entry block or unreachable — no propagation
        };

        let stmts = &mut func.blocks[block_idx].statements;
        let live = apply_incoming_checks(stmts, &incoming, pool_locals, aggregate_results);
        exit_states.insert(bid, live);
    }
}

/// Intersect exit states from all predecessors. Only keep (pool, handle) pairs
/// that every predecessor validated into the *same* result local — a pair
/// resolving to different locals means at least one predecessor's local is
/// undefined on the other's path, so it can't be reused at the merge (#368).
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
        result.retain(|key, dst| state.get(key) == Some(dst));
    }
    result
}

/// Apply incoming checked pairs to a block's statements: if a PoolCheckedAccess
/// at the start of the block checks a pair already validated by incoming state,
/// replace it with an Assign (reuse the predecessor's result local). Returns
/// the resulting live map — the block's real exit state — for successors to
/// use. A reused pair keeps mapping to the predecessor's local (not the
/// Assign's own `dst`), since that's what still holds the value on every path
/// that reaches this block.
fn apply_incoming_checks(
    stmts: &mut [MirStmt],
    incoming: &CheckedMap,
    pool_locals: &HashSet<LocalId>,
    aggregate_results: &HashSet<LocalId>,
) -> CheckedMap {
    let mut live = incoming.clone();

    for stmt in stmts.iter_mut() {
        // Invalidations kill entries
        process_invalidations(stmt, &mut live, pool_locals);

        if let MirStmtKind::PoolCheckedAccess { dst, pool, handle } = &stmt.kind {
            let key = (*pool, *handle);
            let dst = *dst;
            // Aggregate results can't be reused via a value-copying Assign (#402).
            if aggregate_results.contains(&dst) {
                continue;
            }
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

    live
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


fn coalesce_block(
    stmts: &mut [MirStmt],
    pool_locals: &HashSet<LocalId>,
    aggregate_results: &HashSet<LocalId>,
) {
    // Map (pool, handle) → dst local from the first PoolCheckedAccess
    let mut checked: HashMap<CheckKey, LocalId> = HashMap::new();

    for stmt in stmts.iter_mut() {
        // Same invalidation rules as the cross-block pass (mutations, unknown
        // pool-arg calls, closure calls, reassigned locals).
        process_invalidations(stmt, &mut checked, pool_locals);

        // Coalesce PoolCheckedAccess
        if let MirStmtKind::PoolCheckedAccess { dst, pool, handle } = &stmt.kind {
            let key = (*pool, *handle);
            let dst = *dst;
            // Aggregate results can't be reused via a value-copying Assign (#402).
            if aggregate_results.contains(&dst) {
                continue;
            }
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

    /// The local a coalesced `Assign` reuses.
    fn coalesced_source(stmt: &MirStmt) -> LocalId {
        match &stmt.kind {
            MirStmtKind::Assign { rvalue: MirRValue::Use(MirOperand::Local(id)), .. } => *id,
            other => panic!("expected a coalesced Assign, got {other:?}"),
        }
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
        // Block 3: both predecessors (1, 2) have validated — coalesced, and
        // reusing Block 0's local (2) specifically, not either branch-local
        // copy (3, 4) — only Block 0's local is defined on every incoming path.
        assert!(is_coalesced(&f.blocks[3].statements[0]));
        assert_eq!(coalesced_source(&f.blocks[3].statements[0]), local(2));
    }

    #[test]
    fn cross_block_divergent_predecessor_checks_not_coalesced() {
        // Block 0: no pool access; branch to 1/2
        // Block 1: pool[h] — own check, result in t0
        // Block 2: pool[h] — own check, result in t1
        // Block 3: pool[h] — predecessors validated the pair into DIFFERENT
        // locals (t0 vs t1). Reusing either would read an undefined value on
        // the path through the other predecessor, so Block 3 must keep its
        // own check (#368 — this used to reuse Block 1's local unconditionally
        // and crash codegen on the Block 2 path).
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
            ],
            blocks: vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Constant(crate::MirConst::Bool(true)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![pool_access(2, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) }),
                },
                MirBlock {
                    id: BlockId(2),
                    statements: vec![pool_access(3, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(3) }),
                },
                MirBlock {
                    id: BlockId(3),
                    statements: vec![pool_access(4, 0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        };
        coalesce_function(&mut f);
        // Block 1, 2: each has a single predecessor (Block 0) with no prior
        // check — both keep their own real check.
        assert!(is_pool_checked(&f.blocks[1].statements[0]));
        assert!(is_pool_checked(&f.blocks[2].statements[0]));
        // Block 3: predecessors disagree on the result local — must NOT coalesce.
        assert!(is_pool_checked(&f.blocks[3].statements[0]));
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
