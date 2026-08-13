// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Generic dataflow framework — block-level transfer with RPO worklist.
//!
//! Provides forward and backward dataflow analysis over MIR CFGs.
//! Uses the dominator tree's RPO for worklist ordering (forward) or
//! reverse RPO (backward) for fast convergence.

use std::collections::{HashMap, HashSet};

use crate::analysis::cfg;
use crate::analysis::dominators::DominatorTree;
use crate::{BlockId, MirBlock, MirFunction};

/// Dataflow direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
}

/// Trait for defining a dataflow analysis.
///
/// Implement this trait to define what information flows through the CFG.
/// The framework handles worklist iteration and convergence.
pub trait DataflowAnalysis {
    /// The lattice domain — must support equality checks for convergence.
    type Domain: Clone + PartialEq;

    /// Forward or backward analysis.
    fn direction(&self) -> Direction;

    /// Bottom element of the lattice (initial state for all blocks except entry/exit).
    fn bottom(&self) -> Self::Domain;

    /// Join (meet) operator — combines states from multiple predecessors/successors.
    /// Must be monotone: join(a, b) >= a and join(a, b) >= b.
    fn join(&self, a: &Self::Domain, b: &Self::Domain) -> Self::Domain;

    /// Transfer function for a basic block.
    /// Forward: maps entry state → exit state.
    /// Backward: maps exit state → entry state.
    fn transfer_block(&self, block: &MirBlock, in_state: &Self::Domain) -> Self::Domain;

    /// Optional widening operator for non-finite lattices.
    /// Default is identity (no widening) — fine for finite lattices like liveness.
    ///
    /// `is_widening_point` is true only for blocks that need widening to
    /// guarantee convergence (see `widening_points`) — an analysis that only
    /// needs to widen at loop headers can gate on it instead of widening on
    /// every call.
    fn widen(&self, _is_widening_point: bool, _old: &Self::Domain, new: &Self::Domain) -> Self::Domain {
        new.clone()
    }

    /// Optional per-edge transfer for conditional narrowing.
    ///
    /// Applied after exit state is computed, before joining into the successor.
    /// Override to produce different states for different branch targets (e.g.,
    /// narrowing a handle to Valid in the true branch of `pool.get(h) is Some`).
    ///
    /// Default: pass exit state through unchanged.
    fn transfer_edge(
        &self,
        _from: BlockId,
        _to: BlockId,
        _terminator: &crate::MirTerminator,
        exit_state: &Self::Domain,
    ) -> Self::Domain {
        exit_state.clone()
    }
}

/// Results of a dataflow analysis — entry and exit states per block.
pub struct DataflowResults<D: Clone> {
    pub entry: HashMap<BlockId, D>,
    pub exit: HashMap<BlockId, D>,
}

impl<D: Clone> DataflowResults<D> {
    /// Re-run the transfer function within a block to get the state at a
    /// specific statement index. Useful for precise per-statement queries.
    ///
    /// For forward analysis: returns state *after* stmt_idx.
    /// For backward analysis: returns state *before* stmt_idx.
    pub fn state_at_statement<A: DataflowAnalysis<Domain = D>>(
        &self,
        analysis: &A,
        block: &MirBlock,
        stmt_idx: usize,
    ) -> D {
        match analysis.direction() {
            Direction::Forward => {
                // Start from block entry, apply transfer for stmts 0..=stmt_idx
                let mut state = self.entry[&block.id].clone();
                let partial_block = MirBlock {
                    id: block.id,
                    statements: block.statements[..=stmt_idx].to_vec(),
                    terminator: block.terminator.clone(),
                };
                state = analysis.transfer_block(&partial_block, &state);
                state
            }
            Direction::Backward => {
                // Start from block exit, apply transfer for stmts stmt_idx..
                let mut state = self.exit[&block.id].clone();
                let partial_block = MirBlock {
                    id: block.id,
                    statements: block.statements[stmt_idx..].to_vec(),
                    terminator: block.terminator.clone(),
                };
                state = analysis.transfer_block(&partial_block, &state);
                state
            }
        }
    }
}

/// Blocks that need widening to guarantee convergence on infinite lattices —
/// any block reached by a "retreating" edge, one whose target's reverse-
/// postorder position is at or before its source's.
///
/// Every cycle in a CFG contains at least one retreating edge relative to
/// any DFS-derived RPO — a standard fact about DFS spanning trees that holds
/// regardless of whether the CFG is reducible. Dominator-based natural loops
/// (`analysis::loops::detect_loops`) miss cycles in irreducible CFGs (a loop
/// entered from two different places at once, so no single block dominates
/// the whole cycle); this catches those too, so gating widening to these
/// blocks is always sound, never just at "real" loop headers.
pub fn widening_points(func: &MirFunction, dom_tree: &DominatorTree) -> HashSet<BlockId> {
    let rpo_index: HashMap<BlockId, usize> = dom_tree
        .rpo_order()
        .iter()
        .enumerate()
        .map(|(i, &b)| (b, i))
        .collect();

    let mut points = HashSet::new();
    for block in &func.blocks {
        let Some(&from) = rpo_index.get(&block.id) else { continue };
        for succ in cfg::successors(&block.terminator) {
            if let Some(&to) = rpo_index.get(&succ) {
                if to <= from {
                    points.insert(succ);
                }
            }
        }
    }
    points
}

/// Solve a dataflow analysis to a fixed point.
///
/// Uses RPO worklist for forward, reverse RPO for backward.
pub fn solve<A: DataflowAnalysis>(
    func: &MirFunction,
    analysis: &A,
    dom_tree: &DominatorTree,
) -> DataflowResults<A::Domain> {
    let rpo = dom_tree.rpo_order();
    let bottom = analysis.bottom();
    let widen_points = widening_points(func, dom_tree);

    let mut entry: HashMap<BlockId, A::Domain> = HashMap::new();
    let mut exit: HashMap<BlockId, A::Domain> = HashMap::new();

    // Initialize all blocks to bottom
    for &block_id in rpo {
        entry.insert(block_id, bottom.clone());
        exit.insert(block_id, bottom.clone());
    }

    let preds = cfg::predecessors(func);

    // Build successor map for backward analysis
    let succs: HashMap<BlockId, Vec<BlockId>> = func
        .blocks
        .iter()
        .map(|b| (b.id, cfg::successors(&b.terminator)))
        .collect();

    // Block lookup
    let block_map: HashMap<BlockId, &MirBlock> = func
        .blocks
        .iter()
        .map(|b| (b.id, b))
        .collect();

    // Worklist with bitset membership
    let rpo_index: HashMap<BlockId, usize> = rpo
        .iter()
        .enumerate()
        .map(|(i, &b)| (b, i))
        .collect();

    let mut in_worklist: HashSet<BlockId> = rpo.iter().copied().collect();
    let mut worklist: Vec<BlockId> = match analysis.direction() {
        Direction::Forward => rpo.to_vec(),
        Direction::Backward => {
            let mut rev = rpo.to_vec();
            rev.reverse();
            rev
        }
    };

    while let Some(block_id) = worklist.pop() {
        in_worklist.remove(&block_id);

        let Some(block) = block_map.get(&block_id) else {
            continue;
        };

        match analysis.direction() {
            Direction::Forward => {
                // Join predecessor exits, applying per-edge transfer
                let block_preds: Vec<BlockId> = preds.get(&block_id).cloned().unwrap_or_default()
                    .into_iter().filter(|p| rpo_index.contains_key(p)).collect();
                let new_entry = if block_id == func.entry_block {
                    entry[&block_id].clone()
                } else if block_preds.is_empty() {
                    bottom.clone()
                } else {
                    let edge_state = |pred: &BlockId| {
                        let pred_term = &block_map[pred].terminator;
                        analysis.transfer_edge(*pred, block_id, pred_term, &exit[pred])
                    };
                    let mut joined = edge_state(&block_preds[0]);
                    for pred in &block_preds[1..] {
                        joined = analysis.join(&joined, &edge_state(pred));
                    }
                    joined
                };

                entry.insert(block_id, new_entry.clone());
                let new_exit = analysis.transfer_block(block, &new_entry);
                let new_exit = analysis.widen(widen_points.contains(&block_id), &exit[&block_id], &new_exit);

                if new_exit != exit[&block_id] {
                    exit.insert(block_id, new_exit);
                    // Add successors to worklist
                    if let Some(block_succs) = succs.get(&block_id) {
                        for &succ in block_succs {
                            if !in_worklist.contains(&succ) {
                                if rpo_index.contains_key(&succ) {
                                    in_worklist.insert(succ);
                                    worklist.push(succ);
                                }
                            }
                        }
                    }
                }
            }
            Direction::Backward => {
                // Join successor entries
                let block_succs: Vec<BlockId> = succs.get(&block_id).cloned().unwrap_or_default()
                    .into_iter().filter(|s| rpo_index.contains_key(s)).collect();
                let new_exit = if block_succs.is_empty() {
                    exit[&block_id].clone()
                } else {
                    let mut joined = entry[&block_succs[0]].clone();
                    for succ in &block_succs[1..] {
                        joined = analysis.join(&joined, &entry[succ]);
                    }
                    joined
                };

                exit.insert(block_id, new_exit.clone());
                let new_entry = analysis.transfer_block(block, &new_exit);
                let new_entry = analysis.widen(widen_points.contains(&block_id), &entry[&block_id], &new_entry);

                if new_entry != entry[&block_id] {
                    entry.insert(block_id, new_entry);
                    // Add predecessors to worklist
                    let block_preds = preds.get(&block_id).cloned().unwrap_or_default();
                    for pred in block_preds {
                        if !in_worklist.contains(&pred) {
                            if rpo_index.contains_key(&pred) {
                                in_worklist.insert(pred);
                                worklist.push(pred);
                            }
                        }
                    }
                }
            }
        }
    }

    DataflowResults { entry, exit }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::dominators::DominatorTree;
    use crate::function::MirBlock;
    use crate::{MirTerminator, MirTerminatorKind, MirType, MirOperand, MirConst};

    fn block(n: u32) -> BlockId {
        BlockId(n)
    }

    fn make_fn(blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks,
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn term_goto(target: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Goto { target: block(target) })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    fn term_branch(then_b: u32, else_b: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Constant(MirConst::Bool(true)),
            then_block: block(then_b),
            else_block: block(else_b),
        })
    }

    /// Trivial forward analysis: count blocks visited
    struct BlockCountAnalysis;

    impl DataflowAnalysis for BlockCountAnalysis {
        type Domain = u32;
        fn direction(&self) -> Direction { Direction::Forward }
        fn bottom(&self) -> u32 { 0 }
        fn join(&self, a: &u32, b: &u32) -> u32 { (*a).max(*b) }
        fn transfer_block(&self, _block: &MirBlock, in_state: &u32) -> u32 {
            in_state + 1
        }
    }

    #[test]
    fn forward_linear() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let results = solve(&func, &BlockCountAnalysis, &dom);
        assert_eq!(results.exit[&block(0)], 1);
        assert_eq!(results.exit[&block(1)], 2);
        assert_eq!(results.exit[&block(2)], 3);
    }

    #[test]
    fn framework_converges_on_diamond() {
        let func = make_fn(vec![
            MirBlock {
                id: block(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Constant(MirConst::Bool(true)),
                    then_block: block(1),
                    else_block: block(2),
                }),
            },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(2), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let results = solve(&func, &BlockCountAnalysis, &dom);
        // Block 3 joins max(2, 2) = 2, then +1 = 3
        assert_eq!(results.exit[&block(3)], 3);
    }

    #[test]
    fn widening_points_empty_without_a_cycle() {
        // Linear chain and a diamond both have no back edges.
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(2), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert!(widening_points(&func, &dom).is_empty());
    }

    #[test]
    fn widening_points_finds_reducible_loop_header() {
        // 0 -> 1 -> {1, 2}: block 1 is the loop header, reached by the 1->1 back edge.
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_branch(1, 2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let points = widening_points(&func, &dom);
        assert_eq!(points, HashSet::from([block(1)]));
    }

    #[test]
    fn widening_points_finds_nested_loop_headers() {
        // 0 -> 1 -> 2 -> {2, 1, 3}: inner header 2, outer header 1.
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(2) },
            MirBlock {
                id: block(2),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Switch {
                    value: MirOperand::Constant(MirConst::Int(0)),
                    cases: vec![(0, block(2)), (1, block(1))],
                    default: block(3),
                }),
            },
            MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let points = widening_points(&func, &dom);
        assert_eq!(points, HashSet::from([block(1), block(2)]));
    }

    #[test]
    fn widening_points_catches_irreducible_cycle() {
        // entry branches straight into BOTH sides of a 1<->2 cycle, so neither
        // block dominates the other — a dominator-based natural-loop header
        // (analysis::loops::detect_loops) finds no loop here at all. This is
        // exactly the case the RPO-retreating-edge check exists to catch: miss
        // it and the interval solver's widen() never fires on this cycle, so a
        // moving bound keeps changing forever and the fixpoint never converges.
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_goto(1) },
        ]);
        let dom = DominatorTree::build(&func);
        assert!(
            !widening_points(&func, &dom).is_empty(),
            "must widen somewhere on the 1<->2 cycle or the solver can't converge"
        );
    }
}
