// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The step counter of a claim build (type.claims/D2).
//!
//! A claim's input gets a budget counted in steps, never in time, so one that
//! runs out does so at the same place on every build. A step is a function
//! entered or a loop header reached: between them they bound everything a
//! program can do without stopping, recursion included. Only claim builds
//! carry the calls.

use crate::analysis::dominators::DominatorTree;
use crate::analysis::loops::detect_loops;
use crate::{FunctionRef, MirFunction, MirStmt, MirStmtKind};

/// What a step calls; codegen maps it to `rask_claim_step`.
pub const CLAIM_STEP: &str = "claim_step";

/// Add a step at the start of every function and every loop header. Runs
/// after SSA is taken down, so no block starts with a phi.
pub fn instrument(functions: &mut [MirFunction]) {
    for func in functions {
        if func.blocks.is_empty() {
            continue;
        }
        let dom = DominatorTree::build(func);
        let mut heads: Vec<_> = detect_loops(func, &dom).into_iter().map(|l| l.header).collect();
        heads.push(func.entry_block);
        heads.sort();
        heads.dedup();
        for block in &mut func.blocks {
            if heads.binary_search(&block.id).is_ok() {
                block.statements.insert(0, step());
            }
        }
    }
}

fn step() -> MirStmt {
    MirStmt::dummy(MirStmtKind::Call {
        dst: None,
        func: FunctionRef::internal(CLAIM_STEP.to_string()),
        args: Vec::new(),
    })
}
