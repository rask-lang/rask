// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Bounds check elimination pass (comp.advanced BE1-BE4).
//!
//! Uses interval analysis to identify indexing operations where the index
//! is provably within bounds. Currently reports results as diagnostics;
//! actual MIR rewriting (Vec_get → Vec_get_unchecked) requires codegen
//! support and is a follow-up.

use crate::analysis::intervals;
use crate::transform::pass::{MirPass, PassContext};
use crate::MirFunction;

/// Bounds check elimination pass (comp.advanced BE1-BE4).
///
/// Runs interval analysis on functions with indexing operations.
/// Reports provably-in-bounds accesses (BE1) and retained checks (BE2).
pub struct BoundsCheckElimPass;

impl MirPass for BoundsCheckElimPass {
    fn name(&self) -> &str {
        "bounds_check_elim"
    }

    fn run_function(&self, func: &mut MirFunction, ctx: &mut PassContext) {
        let Some((analysis, index_ops, results)) = intervals::analyze(func) else {
            return;
        };

        for op in &index_ops {
            if intervals::is_in_bounds(func, &analysis, &results, op) {
                // BE1: Provably in-bounds — mark for elimination.
                // For now, just track the count. MIR rewrite is a follow-up.
                ctx.bounds_checks_eliminated += 1;
            } else {
                // BE2: Can't prove — check retained (no diagnostic for retained checks,
                // that would be too noisy for normal compilation).
                ctx.bounds_checks_retained += 1;
            }
        }
    }
}
