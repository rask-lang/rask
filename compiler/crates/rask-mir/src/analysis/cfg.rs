// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! CFG analysis — successors, predecessors, reachability.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{BlockId, MirFunction, MirTerminator, MirTerminatorKind};

/// Successor block IDs from a terminator.
pub fn successors(term: &MirTerminator) -> Vec<BlockId> {
    match &term.kind {
        MirTerminatorKind::Return { .. } | MirTerminatorKind::Unreachable => vec![],
        MirTerminatorKind::Goto { target } => vec![*target],
        MirTerminatorKind::Branch { then_block, else_block, .. } => {
            vec![*then_block, *else_block]
        }
        MirTerminatorKind::Switch { cases, default, .. } => {
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, b)| *b).collect();
            targets.push(*default);
            targets
        }
        MirTerminatorKind::CleanupReturn { cleanup_chain, .. } => {
            cleanup_chain.clone()
        }
    }
}

/// Build a predecessor map: block → list of blocks that branch to it.
pub fn predecessors(func: &MirFunction) -> HashMap<BlockId, Vec<BlockId>> {
    let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for block in &func.blocks {
        for target in successors(&block.terminator) {
            preds.entry(target).or_default().push(block.id);
        }
    }
    preds
}

/// Set of all block IDs reachable from `start` via BFS.
pub fn reachable_from(func: &MirFunction, start: BlockId) -> HashSet<BlockId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(start);
    queue.push_back(start);

    while let Some(bid) = queue.pop_front() {
        let Some(block) = func.blocks.iter().find(|b| b.id == bid) else {
            continue;
        };
        for succ in successors(&block.terminator) {
            if visited.insert(succ) {
                queue.push_back(succ);
            }
        }
    }
    visited
}

/// Set of all block IDs reachable from the entry block.
pub fn reachable_blocks(func: &MirFunction) -> HashSet<BlockId> {
    reachable_from(func, func.entry_block)
}
