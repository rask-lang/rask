// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Dominator tree — iterative algorithm (Cooper, Harvey, Kennedy 2001).
//!
//! Computes immediate dominators, dominance queries, and dominance frontiers
//! from a MIR function's CFG. Uses reverse postorder numbering for fast
//! convergence.

use std::collections::{HashMap, HashSet};

use crate::analysis::cfg;
use crate::{BlockId, MirFunction};

/// Dominator tree for a single function's CFG.
pub struct DominatorTree {
    /// Immediate dominator per block (None for the entry block).
    idom: Vec<Option<BlockId>>,
    /// Blocks in reverse postorder.
    block_order: Vec<BlockId>,
    /// Block → RPO index for O(1) lookup.
    block_to_rpo: HashMap<BlockId, usize>,
}

impl DominatorTree {
    /// Build the dominator tree for a function.
    pub fn build(func: &MirFunction) -> Self {
        let (block_order, block_to_rpo) = reverse_postorder(func);
        let n = block_order.len();

        // idom[i] stores the RPO index of the immediate dominator of block_order[i].
        // Use usize::MAX as sentinel for "undefined".
        let undefined = usize::MAX;
        let mut idom_idx: Vec<usize> = vec![undefined; n];

        // Entry block dominates itself.
        let entry_rpo = block_to_rpo[&func.entry_block];
        idom_idx[entry_rpo] = entry_rpo;

        let preds = cfg::predecessors(func);

        let mut changed = true;
        while changed {
            changed = false;
            for &block in &block_order {
                let b = block_to_rpo[&block];
                if b == entry_rpo {
                    continue;
                }

                // Find first processed predecessor
                let block_preds = preds.get(&block).cloned().unwrap_or_default();
                let mut new_idom = undefined;
                for &pred in &block_preds {
                    if let Some(&p) = block_to_rpo.get(&pred) {
                        if idom_idx[p] != undefined {
                            new_idom = p;
                            break;
                        }
                    }
                }

                if new_idom == undefined {
                    continue;
                }

                // Intersect with remaining processed predecessors
                for &pred in &block_preds {
                    if let Some(&p) = block_to_rpo.get(&pred) {
                        if idom_idx[p] != undefined {
                            new_idom = intersect(&idom_idx, p, new_idom);
                        }
                    }
                }

                if idom_idx[b] != new_idom {
                    idom_idx[b] = new_idom;
                    changed = true;
                }
            }
        }

        // Convert RPO indices back to BlockIds
        let idom: Vec<Option<BlockId>> = (0..n)
            .map(|i| {
                if i == entry_rpo || idom_idx[i] == undefined {
                    None
                } else {
                    Some(block_order[idom_idx[i]])
                }
            })
            .collect();

        DominatorTree {
            idom,
            block_order,
            block_to_rpo,
        }
    }

    /// Immediate dominator of a block. None for the entry block.
    pub fn idom(&self, block: BlockId) -> Option<BlockId> {
        let idx = *self.block_to_rpo.get(&block)?;
        self.idom[idx]
    }

    /// True if `a` dominates `b` (a == b counts as domination).
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        let Some(&a_rpo) = self.block_to_rpo.get(&a) else {
            return false;
        };
        let Some(&mut_b_rpo) = self.block_to_rpo.get(&b) else {
            return false;
        };

        let mut current = mut_b_rpo;
        loop {
            if current == a_rpo {
                return true;
            }
            match &self.idom[current] {
                Some(idom_block) => {
                    let idom_rpo = self.block_to_rpo[idom_block];
                    if idom_rpo == current {
                        // Entry block — reached the root
                        return current == a_rpo;
                    }
                    current = idom_rpo;
                }
                None => {
                    // Entry block
                    return current == a_rpo;
                }
            }
        }
    }

    /// Compute dominance frontiers for all blocks.
    ///
    /// DF(n) = set of blocks where n's dominance ends — join points where
    /// at least one predecessor is dominated by n but the block itself isn't
    /// strictly dominated by n.
    pub fn dominance_frontier(&self, func: &MirFunction) -> HashMap<BlockId, HashSet<BlockId>> {
        let preds = cfg::predecessors(func);
        let mut df: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();

        for &block in &self.block_order {
            let block_preds = preds.get(&block).cloned().unwrap_or_default();
            if block_preds.len() < 2 {
                continue;
            }

            // Join point: walk each predecessor's idom chain up to this block's idom
            let block_idom = self.idom(block);
            for pred in &block_preds {
                let mut runner = *pred;
                // Walk up the dominator tree from pred until we reach block's idom
                while Some(runner) != block_idom {
                    df.entry(runner).or_default().insert(block);
                    match self.idom(runner) {
                        Some(next) => runner = next,
                        None => break, // reached entry
                    }
                }
            }
        }

        df
    }

    /// Blocks in reverse postorder (entry first).
    pub fn rpo_order(&self) -> &[BlockId] {
        &self.block_order
    }
}

/// Intersect two dominator paths using RPO numbering.
/// Walks both paths toward the root until they meet.
fn intersect(idom: &[usize], mut a: usize, mut b: usize) -> usize {
    while a != b {
        while a > b {
            a = idom[a];
        }
        while b > a {
            b = idom[b];
        }
    }
    a
}

/// Compute reverse postorder via DFS from the entry block.
fn reverse_postorder(func: &MirFunction) -> (Vec<BlockId>, HashMap<BlockId, usize>) {
    let mut visited = HashSet::new();
    let mut postorder = Vec::new();

    // Build successor map for DFS
    fn dfs(
        block: BlockId,
        func: &MirFunction,
        visited: &mut HashSet<BlockId>,
        postorder: &mut Vec<BlockId>,
    ) {
        if !visited.insert(block) {
            return;
        }
        let Some(b) = func.blocks.iter().find(|b| b.id == block) else {
            return;
        };
        for succ in cfg::successors(&b.terminator) {
            dfs(succ, func, visited, postorder);
        }
        postorder.push(block);
    }

    dfs(func.entry_block, func, &mut visited, &mut postorder);
    postorder.reverse();

    let block_to_rpo: HashMap<BlockId, usize> = postorder
        .iter()
        .enumerate()
        .map(|(i, &b)| (b, i))
        .collect();

    (postorder, block_to_rpo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function::{MirBlock, MirLocal};
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

    fn term_branch(then_b: u32, else_b: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Constant(MirConst::Bool(true)),
            then_block: block(then_b),
            else_block: block(else_b),
        })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    // Linear: 0 → 1 → 2
    #[test]
    fn linear_chain() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert!(dom.idom(block(0)).is_none());
        assert_eq!(dom.idom(block(1)), Some(block(0)));
        assert_eq!(dom.idom(block(2)), Some(block(1)));
        assert!(dom.dominates(block(0), block(2)));
        assert!(!dom.dominates(block(2), block(0)));
    }

    // Diamond: 0 → {1, 2} → 3
    #[test]
    fn diamond() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(2), statements: vec![], terminator: term_goto(3) },
            MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert_eq!(dom.idom(block(1)), Some(block(0)));
        assert_eq!(dom.idom(block(2)), Some(block(0)));
        assert_eq!(dom.idom(block(3)), Some(block(0)));
        assert!(dom.dominates(block(0), block(3)));
        assert!(!dom.dominates(block(1), block(3)));

        let df = dom.dominance_frontier(&func);
        // Block 1 and 2 have block 3 in their DF
        assert!(df.get(&block(1)).map_or(false, |s| s.contains(&block(3))));
        assert!(df.get(&block(2)).map_or(false, |s| s.contains(&block(3))));
    }

    // Loop: 0 → 1 → {1, 2}
    #[test]
    fn simple_loop() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock {
                id: block(1),
                statements: vec![],
                terminator: term_branch(1, 2), // loop back to 1, exit to 2
            },
            MirBlock { id: block(2), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert_eq!(dom.idom(block(1)), Some(block(0)));
        assert_eq!(dom.idom(block(2)), Some(block(1)));
        assert!(dom.dominates(block(1), block(2)));
        // Block 1 has itself in its DF (loop header)
        let df = dom.dominance_frontier(&func);
        assert!(df.get(&block(1)).map_or(false, |s| s.contains(&block(1))));
    }

    // Unreachable block not in RPO
    #[test]
    fn unreachable_block() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_ret() },
            MirBlock { id: block(1), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert!(dom.idom(block(0)).is_none());
        // Block 1 unreachable — not in RPO
        assert_eq!(dom.block_to_rpo.get(&block(1)), None);
        assert!(!dom.dominates(block(0), block(1)));
    }

    #[test]
    fn self_dominates() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        assert!(dom.dominates(block(0), block(0)));
    }
}
