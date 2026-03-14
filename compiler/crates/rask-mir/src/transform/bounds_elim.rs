// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Bounds check elimination pass (comp.advanced BE1-BE4).
//!
//! Uses interval analysis to prove indices are in-bounds, then rewrites
//! Vec_get/Vec_index → Vec_get_unchecked to skip runtime bounds checks.

use crate::analysis::intervals;
use crate::transform::pass::{MirPass, PassContext};
use crate::{MirFunction, MirStmtKind};

/// Bounds check elimination pass (comp.advanced BE1-BE4).
///
/// Runs interval analysis on functions with indexing operations.
/// Rewrites provably-in-bounds accesses (BE1) to unchecked variants.
pub struct BoundsCheckElimPass;

impl MirPass for BoundsCheckElimPass {
    fn name(&self) -> &str {
        "bounds_check_elim"
    }

    fn run_function(&self, func: &mut MirFunction, ctx: &mut PassContext) {
        let Some((analysis, index_ops, results)) = intervals::analyze(func) else {
            return;
        };

        // Collect rewrites: (block_id, stmt_idx) for in-bounds Vec_get calls
        let mut rewrites = Vec::new();

        for op in &index_ops {
            if intervals::is_in_bounds(func, &analysis, &results, op) {
                // BE1: Provably in-bounds
                ctx.bounds_checks_eliminated += 1;
                if op.is_vec_get {
                    rewrites.push((op.block, op.stmt_idx));
                }
            } else {
                // BE2: Can't prove — check retained
                ctx.bounds_checks_retained += 1;
            }
        }

        // Apply rewrites: Vec_get/Vec_index → Vec_get_unchecked
        for (block_id, stmt_idx) in rewrites {
            let block = func.blocks.iter_mut().find(|b| b.id == block_id).unwrap();
            if let MirStmtKind::Call { func: ref mut fref, .. } = block.statements[stmt_idx].kind {
                if fref.name == "Vec_get" || fref.name == "Vec_index" {
                    fref.name = "Vec_get_unchecked".into();
                }
            }
        }
    }
}
