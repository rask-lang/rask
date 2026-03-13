// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Liveness analysis — backward dataflow over MIR locals.
//!
//! A local is *live* at a program point if there exists a path from that point
//! to a use of the local that doesn't pass through a redefinition. Uses the
//! generic dataflow framework with gen/kill sets derived from `uses.rs`.

use crate::analysis::dataflow::{self, DataflowAnalysis, DataflowResults, Direction};
use crate::analysis::dominators::DominatorTree;
use crate::analysis::uses;
use crate::{BlockId, LocalId, MirBlock, MirFunction};

/// Dense bitvec indexed by LocalId.0 — one bit per local.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LiveSet {
    bits: Vec<bool>,
}

impl LiveSet {
    fn new(num_locals: usize) -> Self {
        Self {
            bits: vec![false; num_locals],
        }
    }

    pub fn is_live(&self, local: LocalId) -> bool {
        self.bits.get(local.0 as usize).copied().unwrap_or(false)
    }

    fn set(&mut self, local: LocalId) {
        if let Some(b) = self.bits.get_mut(local.0 as usize) {
            *b = true;
        }
    }

    fn clear(&mut self, local: LocalId) {
        if let Some(b) = self.bits.get_mut(local.0 as usize) {
            *b = false;
        }
    }

    fn union(&self, other: &Self) -> Self {
        let len = self.bits.len().max(other.bits.len());
        let mut result = vec![false; len];
        for i in 0..len {
            let a = self.bits.get(i).copied().unwrap_or(false);
            let b = other.bits.get(i).copied().unwrap_or(false);
            result[i] = a || b;
        }
        Self { bits: result }
    }
}

/// Liveness analysis configuration.
pub struct LivenessAnalysis {
    pub num_locals: usize,
}

impl DataflowAnalysis for LivenessAnalysis {
    type Domain = LiveSet;

    fn direction(&self) -> Direction {
        Direction::Backward
    }

    fn bottom(&self) -> LiveSet {
        LiveSet::new(self.num_locals)
    }

    fn join(&self, a: &LiveSet, b: &LiveSet) -> LiveSet {
        a.union(b)
    }

    fn transfer_block(&self, block: &MirBlock, in_state: &LiveSet) -> LiveSet {
        // Backward: in_state is the exit state (what's live after the block).
        // Walk statements in reverse, applying kill then gen.
        let mut state = in_state.clone();

        // Terminator reads are gen (live before terminator)
        for local_idx in 0..self.num_locals {
            let local = LocalId(local_idx as u32);
            if uses::terminator_reads(&block.terminator, local) {
                state.set(local);
            }
        }

        // Walk statements in reverse
        for stmt in block.statements.iter().rev() {
            // Kill: if this statement defines a local, it's no longer live above
            if let Some(def) = uses::stmt_def(stmt) {
                state.clear(def);
            }
            // Gen: if this statement reads a local, it becomes live above
            for local_idx in 0..self.num_locals {
                let local = LocalId(local_idx as u32);
                if uses::stmt_reads(stmt, local) {
                    state.set(local);
                }
            }
        }

        state
    }
}

/// Liveness results with convenience accessors.
pub struct LivenessResults {
    pub results: DataflowResults<LiveSet>,
    pub num_locals: usize,
}

impl LivenessResults {
    /// True if `local` is live at the entry of `block`.
    pub fn live_at_entry(&self, block: BlockId, local: LocalId) -> bool {
        self.results
            .entry
            .get(&block)
            .map_or(false, |s| s.is_live(local))
    }

    /// True if `local` is live at the exit of `block`.
    pub fn live_at_exit(&self, block: BlockId, local: LocalId) -> bool {
        self.results
            .exit
            .get(&block)
            .map_or(false, |s| s.is_live(local))
    }
}

/// Run liveness analysis on a function.
pub fn analyze(func: &MirFunction, dom_tree: &DominatorTree) -> LivenessResults {
    let num_locals = func.locals.len() + func.params.len();
    let analysis = LivenessAnalysis { num_locals };
    let results = dataflow::solve(func, &analysis, dom_tree);
    LivenessResults {
        results,
        num_locals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::dominators::DominatorTree;
    use crate::{
        MirBlock, MirConst, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt, MirStmtKind,
        MirTerminator, MirTerminatorKind, MirType,
    };

    fn block(n: u32) -> BlockId {
        BlockId(n)
    }
    fn local(n: u32) -> LocalId {
        LocalId(n)
    }

    fn make_fn(locals_count: usize, blocks: Vec<MirBlock>) -> MirFunction {
        let locals: Vec<MirLocal> = (0..locals_count)
            .map(|i| MirLocal {
                id: LocalId(i as u32),
                name: None,
                ty: MirType::I32,
                is_param: false,
            })
            .collect();
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn assign(dst: u32, src: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Local(local(src))),
        })
    }

    fn assign_const(dst: u32, val: i64) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(val))),
        })
    }

    fn term_goto(target: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Goto {
            target: block(target),
        })
    }

    fn term_ret_local(l: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Local(local(l))),
        })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    #[test]
    fn dead_local_not_live() {
        // x = 1; return
        // x is defined but never used — not live at exit
        let func = make_fn(
            1,
            vec![MirBlock {
                id: block(0),
                statements: vec![assign_const(0, 1)],
                terminator: term_ret(),
            }],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        assert!(!live.live_at_entry(block(0), local(0)));
        assert!(!live.live_at_exit(block(0), local(0)));
    }

    #[test]
    fn used_in_return_is_live() {
        // block 0: goto 1
        // block 1: x = 1; goto 2
        // block 2: return x
        // x is live at entry of block 2 and exit of block 1
        let func = make_fn(
            1,
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![assign_const(0, 1)],
                    terminator: term_goto(2),
                },
                MirBlock {
                    id: block(2),
                    statements: vec![],
                    terminator: term_ret_local(0),
                },
            ],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        // x is live at entry of block 2 (used in return terminator)
        assert!(live.live_at_entry(block(2), local(0)));
        // x is live at exit of block 1 (flows to block 2 where it's used)
        assert!(live.live_at_exit(block(1), local(0)));
        // x is NOT live at entry of block 1 (defined there before any use)
        assert!(!live.live_at_entry(block(1), local(0)));
    }

    #[test]
    fn liveness_propagates_across_blocks() {
        // block 0: x = 1; goto 1
        // block 1: return x
        let func = make_fn(
            1,
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 1)],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![],
                    terminator: term_ret_local(0),
                },
            ],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        // x live at entry of block 1 (used in return)
        assert!(live.live_at_entry(block(1), local(0)));
        // x live at exit of block 0 (flows to block 1)
        assert!(live.live_at_exit(block(0), local(0)));
        // x NOT live at entry of block 0 (defined there)
        assert!(!live.live_at_entry(block(0), local(0)));
    }

    #[test]
    fn redefinition_kills_liveness() {
        // block 0: x = 1; goto 1
        // block 1: x = 2; return x
        // x should NOT be live at exit of block 0 (killed in block 1 before use)
        let func = make_fn(
            1,
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 1)],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![assign_const(0, 2)],
                    terminator: term_ret_local(0),
                },
            ],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        // x NOT live at exit of block 0 — redefined before use in block 1
        assert!(!live.live_at_exit(block(0), local(0)));
    }

    #[test]
    fn multiple_locals() {
        // block 0: x = 1; y = x; goto 1
        // block 1: return y
        let func = make_fn(
            2,
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 1), assign(1, 0)],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![],
                    terminator: term_ret_local(1),
                },
            ],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        // y is live at exit of block 0 (used in block 1's return)
        assert!(live.live_at_exit(block(0), local(1)));
        // x is NOT live at exit of block 0 (not used after y = x)
        assert!(!live.live_at_exit(block(0), local(0)));
        // Neither live at entry of block 0 (both defined before use)
        assert!(!live.live_at_entry(block(0), local(0)));
        assert!(!live.live_at_entry(block(0), local(1)));
    }

    #[test]
    fn loop_liveness() {
        // block 0: x = 0; goto 1
        // block 1: y = x; x = y + 1 (simulated as x = y); branch to 1 or 2
        // block 2: return x
        let func = make_fn(
            2,
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 0)],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![assign(1, 0), assign(0, 1)],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Constant(MirConst::Bool(true)),
                        then_block: block(1),
                        else_block: block(2),
                    }),
                },
                MirBlock {
                    id: block(2),
                    statements: vec![],
                    terminator: term_ret_local(0),
                },
            ],
        );
        let dom = DominatorTree::build(&func);
        let live = analyze(&func, &dom);
        // x is live at entry of block 1 (used as y = x)
        assert!(live.live_at_entry(block(1), local(0)));
        // x is live at exit of block 1 (loop back to block 1 where it's used, or to block 2)
        assert!(live.live_at_exit(block(1), local(0)));
    }
}
