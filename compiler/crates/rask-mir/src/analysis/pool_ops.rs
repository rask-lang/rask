// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Pool operation classification — shared between generation coalescing,
//! typestate analysis, and future pool-related passes.

use crate::{LocalId, MirOperand, MirStmt, MirStmtKind};

/// Pool-mutating function names that add elements (Grow effect).
pub const POOL_GROWERS: &[&str] = &[
    "Pool_insert",
    "Pool_alloc",
];

/// Pool-mutating function names that remove elements (Shrink effect).
pub const POOL_SHRINKERS: &[&str] = &[
    "Pool_remove",
    "Pool_clear",
    "Pool_drain",
];

/// All structural pool mutators (union of growers and shrinkers).
pub const POOL_MUTATORS: &[&str] = &[
    "Pool_insert",
    "Pool_remove",
    "Pool_clear",
    "Pool_drain",
    "Pool_alloc",
];

/// Known-safe pool reads that don't invalidate anything.
pub const SAFE_POOL_CALLS: &[&str] = &[
    "Pool_get",
    "Pool_index",
    "Pool_checked_access",
    "Pool_len",
    "Pool_handles",
    "Pool_values",
    "Pool_is_empty",
    "Pool_modify",
    "Pool_contains",
    "Pool_cursor",
];

pub fn is_pool_mutator(name: &str) -> bool {
    POOL_MUTATORS.iter().any(|m| *m == name)
}

pub fn is_pool_grower(name: &str) -> bool {
    POOL_GROWERS.iter().any(|m| *m == name)
}

pub fn is_pool_shrinker(name: &str) -> bool {
    POOL_SHRINKERS.iter().any(|m| *m == name)
}

pub fn is_safe_pool_call(name: &str) -> bool {
    SAFE_POOL_CALLS.iter().any(|s| *s == name)
}

/// If this statement is a pool structural mutation, return the pool local being mutated.
pub fn pool_mutation(stmt: &MirStmt) -> Option<LocalId> {
    if let MirStmtKind::Call { func, args, .. } = &stmt.kind {
        if is_pool_mutator(&func.name) {
            if let Some(MirOperand::Local(pool_id)) = args.first() {
                return Some(*pool_id);
            }
        }
    }
    None
}

/// If this statement is `Pool_remove(pool, handle)`, return (pool, handle).
pub fn pool_remove_target(stmt: &MirStmt) -> Option<(LocalId, LocalId)> {
    if let MirStmtKind::Call { func, args, .. } = &stmt.kind {
        if func.name == "Pool_remove" {
            if let (Some(MirOperand::Local(pool)), Some(MirOperand::Local(handle))) =
                (args.get(0), args.get(1))
            {
                return Some((*pool, *handle));
            }
        }
    }
    None
}
