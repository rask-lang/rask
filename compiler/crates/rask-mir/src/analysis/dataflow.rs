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
    fn widen(&self, _old: &Self::Domain, new: &Self::Domain) -> Self::Domain {
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
                let new_exit = analysis.widen(&exit[&block_id], &new_exit);

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
                let new_entry = analysis.widen(&entry[&block_id], &new_entry);

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
}
