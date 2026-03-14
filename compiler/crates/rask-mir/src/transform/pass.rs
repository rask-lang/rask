// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Pass manager — runs MIR optimization passes in sequence.
//!
//! Each pass implements `MirPass`. The `PassManager` runs them in order,
//! threading a `PassContext` through for side-channel metadata collection.

use std::collections::HashMap;
use crate::MirFunction;
use crate::transform::inline::InlineRegion;

/// A MIR-to-MIR transformation pass.
pub trait MirPass {
    /// Short name for logging/debugging.
    fn name(&self) -> &str;

    /// Run on the full set of functions with shared context.
    /// Default iterates per-function (ignoring ctx).
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        for func in fns.iter_mut() {
            self.run_function(func);
        }
    }

    /// Run on a single function. Default is no-op.
    fn run_function(&self, _func: &mut MirFunction) {}
}

/// Shared context threaded through the pass pipeline.
/// Passes can write metadata here; downstream consumers read it.
#[derive(Debug, Default)]
pub struct PassContext {
    /// DI5: inline region metadata per caller function name.
    pub inline_regions: HashMap<String, Vec<InlineRegion>>,
}

/// Convenience alias — the result of running the pipeline is the context.
pub type PipelineResult = PassContext;

/// Runs a sequence of MIR passes.
pub struct PassManager {
    passes: Vec<Box<dyn MirPass>>,
}

impl PassManager {
    pub fn new() -> Self {
        Self { passes: Vec::new() }
    }

    /// Add a pass to the pipeline.
    pub fn add(&mut self, pass: impl MirPass + 'static) {
        self.passes.push(Box::new(pass));
    }

    /// Run all passes in order. Returns the accumulated context.
    pub fn run(&self, fns: &mut Vec<MirFunction>) -> PipelineResult {
        let mut ctx = PassContext::default();
        for pass in &self.passes {
            pass.run(fns, &mut ctx);
        }
        ctx
    }

    /// Build the default optimization pipeline.
    pub fn default_pipeline() -> Self {
        let mut pm = Self::new();
        // Cross-function passes (sequential) — PC2
        pm.add(ClosureOptimizationPass);
        pm.add(InliningPass);
        // Per-function passes — run after inlining for wider optimization window (IN5)
        pm.add(StringConcatPass);
        pm.add(CloneElisionPass);
        pm.add(StringRcInsertionPass);
        pm.add(StringRcElisionPass);
        pm.add(GenerationCoalescingPass);
        pm.add(DeadCodeEliminationPass);
        pm
    }
}

// Wrapper structs for existing passes

/// Cross-function closure escape analysis and stack/heap allocation decisions.
pub struct ClosureOptimizationPass;

impl MirPass for ClosureOptimizationPass {
    fn name(&self) -> &str { "closure_optimization" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::optimize_all_closures(fns);
    }
}

/// Cross-function inliner — splices small/once-called function bodies into callers (IN1-IN5).
pub struct InliningPass;

impl MirPass for InliningPass {
    fn name(&self) -> &str { "inlining" }
    fn run(&self, fns: &mut Vec<MirFunction>, ctx: &mut PassContext) {
        ctx.inline_regions = crate::transform::inline::inline_functions(fns);
    }
}

/// Self-concat → in-place append (eliminates O(n²) string building).
pub struct StringConcatPass;

impl MirPass for StringConcatPass {
    fn name(&self) -> &str { "string_concat" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::optimize_string_concat(fns);
    }
}

/// Last-use clone → move when source is dead after clone.
pub struct CloneElisionPass;

impl MirPass for CloneElisionPass {
    fn name(&self) -> &str { "clone_elision" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::elide_clones(fns);
    }
}

/// Remove unreachable blocks and dead assignments.
pub struct DeadCodeEliminationPass;

impl MirPass for DeadCodeEliminationPass {
    fn name(&self) -> &str { "dce" }
    fn run_function(&self, func: &mut MirFunction) {
        crate::transform::dce::eliminate_dead_code(func);
    }
}

/// Insert explicit RcInc/RcDec for string-typed locals (RC1, RC2).
pub struct StringRcInsertionPass;

impl MirPass for StringRcInsertionPass {
    fn name(&self) -> &str { "string_rc_insert" }
    fn run_function(&self, func: &mut MirFunction) {
        crate::transform::rc_insert::insert_rc_ops(func);
    }
}

/// Elide unnecessary RcInc/RcDec via escape analysis and literal propagation (RE1-RE6).
pub struct StringRcElisionPass;

impl MirPass for StringRcElisionPass {
    fn name(&self) -> &str { "string_rc_elide" }
    fn run_function(&self, func: &mut MirFunction) {
        crate::transform::rc_elide::elide_rc_ops(func);
    }
}

/// Merge redundant PoolCheckedAccess on same (pool, handle).
pub struct GenerationCoalescingPass;

impl MirPass for GenerationCoalescingPass {
    fn name(&self) -> &str { "generation_coalescing" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::coalesce_generation_checks(fns);
    }
}
