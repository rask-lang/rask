// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where control leaves the region a definition rules.
//!
//! Every drop pass places a release at a boundary: before each return, and
//! before each loop back edge. A value made on *one arm of a branch* reaches
//! neither. The arm doesn't dominate any return, and it doesn't dominate the
//! back edge either, so there was no point where freeing it was unconditionally
//! safe and nothing freed it at all:
//!
//! ```text
//! if cond { mut v = Vec.new()  … }          // container_drop
//! classify(-1) catch e => -1                // trait_drop, the boxed error
//! while … { if … { counter(r).count() } }   // closures, the environment
//! ```
//!
//! The answer is the same in all three: the last place the value is certainly
//! alive and certainly finished with is the edge out of the region its
//! definition dominates. Three passes needed it, all three got it wrong the
//! same two ways first, so it lives here once.

use crate::analysis::dominators::DominatorTree;
use crate::{BlockId, LocalId, MirFunction};

/// The blocks whose end is an edge out of the region `def` dominates.
///
/// Empty when a block outside that region still names `local`: a phi merging
/// this arm's value with another arm's is the shape that matters, and freeing
/// here would hand it a value that is gone.
///
/// Two guards, both found by a segfault rather than by reasoning:
///
///   - *every* successor has to be outside, not any. The release goes at the
///     end of the block, so a block that can also carry on inside the region
///     would run it and keep going — a loop header branches to its own body as
///     well as to the exit. Mixed blocks are left alone, which leaks where
///     splitting the edge would free, and leaking is the safe half.
///   - a successor that dominates this block is a back edge, whatever the
///     first guard says. The back-edge rule each pass already has frees there,
///     and freeing twice is a double free: `while i < 4 { mut v = Vec.new() … }`
///     segfaulted on the second turn.
pub fn where_control_leaves(
    func: &MirFunction,
    dom: &DominatorTree,
    def: BlockId,
    local: LocalId,
) -> Vec<usize> {
    let named_outside = func.blocks.iter().any(|b| {
        !dom.dominates(def, b.id)
            && (b.statements.iter().any(|st| crate::analysis::uses::stmt_reads(st, local))
                || crate::analysis::uses::terminator_reads(&b.terminator, local))
    });
    if named_outside {
        return Vec::new();
    }
    func.blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| dom.dominates(def, block.id))
        .filter(|(_, block)| {
            let succs = crate::analysis::cfg::successors(&block.terminator);
            !succs.is_empty()
                && succs
                    .iter()
                    .all(|s| !dom.dominates(def, *s) && !dom.dominates(*s, block.id))
        })
        .map(|(idx, _)| idx)
        .collect()
}
