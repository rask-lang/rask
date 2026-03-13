// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Natural loop detection using dominator tree back-edge analysis.
//!
//! A back-edge B → H exists when H dominates B. The natural loop for
//! that back-edge is all blocks reachable backwards from B without
//! passing through H, plus H itself.

use std::collections::HashSet;

use crate::analysis::cfg;
use crate::analysis::dominators::DominatorTree;
use crate::{BlockId, MirFunction};

/// A natural loop in the CFG.
#[derive(Debug)]
pub struct NaturalLoop {
    /// Loop header — the target of the back-edge, dominates all loop blocks.
    pub header: BlockId,
    /// All blocks in the loop body (includes header).
    pub blocks: HashSet<BlockId>,
    /// Back-edges that form this loop: (tail → header).
    pub back_edges: Vec<(BlockId, BlockId)>,
}

/// Detect all natural loops in a function.
///
/// Finds back-edges (B → H where H dominates B), then computes the
/// natural loop body for each header by walking predecessors backwards.
pub fn detect_loops(func: &MirFunction, dom_tree: &DominatorTree) -> Vec<NaturalLoop> {
    let preds = cfg::predecessors(func);

    // Find back-edges: for each edge B → H, if H dominates B, it's a back-edge.
    let mut back_edges_by_header: std::collections::HashMap<BlockId, Vec<BlockId>> =
        std::collections::HashMap::new();

    for block in &func.blocks {
        for succ in cfg::successors(&block.terminator) {
            if dom_tree.dominates(succ, block.id) {
                back_edges_by_header
                    .entry(succ)
                    .or_default()
                    .push(block.id);
            }
        }
    }

    // Build natural loop for each header
    let mut loops = Vec::new();
    for (header, tails) in back_edges_by_header {
        let mut body = HashSet::new();
        body.insert(header);

        // Walk backwards from each tail, collecting loop body blocks
        let mut worklist: Vec<BlockId> = tails
            .iter()
            .filter(|&&t| t != header)
            .copied()
            .collect();

        while let Some(block) = worklist.pop() {
            if body.insert(block) {
                // Add predecessors to worklist
                if let Some(block_preds) = preds.get(&block) {
                    for &pred in block_preds {
                        if !body.contains(&pred) {
                            worklist.push(pred);
                        }
                    }
                }
            }
        }

        let back_edges = tails.iter().map(|&tail| (tail, header)).collect();
        loops.push(NaturalLoop {
            header,
            blocks: body,
            back_edges,
        });
    }

    loops
}

#[cfg(test)]
mod tests {
    use super::*;
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

    // Simple loop: 0 → 1 → {1, 2}
    #[test]
    fn simple_while_loop() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_branch(1, 2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let loops = detect_loops(&func, &dom);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header, block(1));
        assert!(loops[0].blocks.contains(&block(1)));
        assert!(!loops[0].blocks.contains(&block(0)));
        assert!(!loops[0].blocks.contains(&block(2)));
    }

    // Loop with body: 0 → 1 → 2 → {1, 3}
    #[test]
    fn loop_with_body() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_goto(2) },
            MirBlock { id: block(2), statements: vec![], terminator: term_branch(1, 3) },
            MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let loops = detect_loops(&func, &dom);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header, block(1));
        assert!(loops[0].blocks.contains(&block(1)));
        assert!(loops[0].blocks.contains(&block(2)));
        assert!(!loops[0].blocks.contains(&block(0)));
        assert!(!loops[0].blocks.contains(&block(3)));
    }

    // No loops
    #[test]
    fn no_loops() {
        let func = make_fn(vec![
            MirBlock { id: block(0), statements: vec![], terminator: term_goto(1) },
            MirBlock { id: block(1), statements: vec![], terminator: term_ret() },
        ]);
        let dom = DominatorTree::build(&func);
        let loops = detect_loops(&func, &dom);
        assert!(loops.is_empty());
    }

    // Nested loops: 0 → 1 → 2 → {2, 1, 3}
    #[test]
    fn nested_loops() {
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
        let loops = detect_loops(&func, &dom);
        // Two loops: inner (header=2, body={2}) and outer (header=1, body={1,2})
        assert_eq!(loops.len(), 2);
        let headers: HashSet<BlockId> = loops.iter().map(|l| l.header).collect();
        assert!(headers.contains(&block(1)));
        assert!(headers.contains(&block(2)));
    }
}
