// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Pass manager — runs MIR optimization passes in sequence.
//!
//! Each pass implements `MirPass`. The `PassManager` runs them in order,
//! threading a `PassContext` for metadata collection and diagnostic accumulation.

use std::collections::HashMap;
use rask_diagnostics::Diagnostic;
use crate::MirFunction;
use crate::transform::bounds_elim::BoundsCheckElimPass;
use crate::transform::typestate::TypestatePass;
use crate::transform::inline::InlineRegion;

/// Shared context threaded through the pass pipeline.
/// Passes write metadata and diagnostics here; downstream consumers read them.
#[derive(Debug, Default)]
pub struct PassContext {
    /// DI5: inline region metadata per caller function name.
    pub inline_regions: HashMap<String, Vec<InlineRegion>>,
    /// Accumulated diagnostics from analysis passes (typestate errors, etc.).
    pub diagnostics: Vec<Diagnostic>,
    /// BE1: Number of bounds checks proven unnecessary by interval analysis.
    pub bounds_checks_eliminated: u32,
    /// BE2: Number of bounds checks retained (couldn't prove in-bounds).
    pub bounds_checks_retained: u32,
}

/// Convenience alias.
pub type PipelineResult = PassContext;

/// A MIR-to-MIR transformation pass.
pub trait MirPass {
    /// Short name for logging/debugging.
    fn name(&self) -> &str;

    /// Run on the full set of functions with shared context.
    /// Default iterates per-function.
    fn run(&self, fns: &mut Vec<MirFunction>, ctx: &mut PassContext) {
        for func in fns.iter_mut() {
            self.run_function(func, ctx);
        }
    }

    /// Run on a single function. Default is no-op.
    fn run_function(&self, _func: &mut MirFunction, _ctx: &mut PassContext) {}
}

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
    ///
    /// `RASK_DUMP_PASS=<name>` prints the MIR after that pass — `--dump-mir`
    /// shows the lowering's output, which is before any of this ran, so a pass
    /// that breaks the CFG leaves no trace anywhere. `RASK_DUMP_PASS=all`
    /// prints after every pass. Restrict to one function with
    /// `RASK_DUMP_FN=<function name>`.
    pub fn run(&self, fns: &mut Vec<MirFunction>) -> PipelineResult {
        let mut ctx = PassContext::default();
        let dump_after = std::env::var("RASK_DUMP_PASS").ok();
        let dump_fn = std::env::var("RASK_DUMP_FN").ok();
        for pass in &self.passes {
            pass.run(fns, &mut ctx);
            if dump_after.as_deref().is_some_and(|w| w == "all" || w == pass.name()) {
                eprintln!("──── after {} ────", pass.name());
                for func in fns.iter() {
                    if dump_fn.as_deref().is_none_or(|want| func.name == want) {
                        eprintln!("{}", func);
                    }
                }
            }
        }
        ctx
    }

    /// Build the default optimization pipeline.
    pub fn default_pipeline() -> Self {
        let mut pm = Self::new();
        // Cross-function passes (sequential) — PC2
        pm.add(ClosureOptimizationPass);
        pm.add(InliningPass);
        pm.add(TraitDropInsertionPass);
        pm.add(ContainerDropInsertionPass);
        // Per-function passes — run after inlining for wider optimization window (IN5)
        pm.add(StringConcatPass);
        pm.add(CloneElisionPass);
        pm.add(StringRcInsertionPass);
        pm.add(StringRcElisionPass);
        // Phase G: Advanced analyses before gen coalescing (needs PoolCheckedAccess intact)
        pm.add(TypestatePass);
        pm.add(BoundsCheckElimPass);
        pm.add(GenerationCoalescingPass);
        pm.add(DeadCodeEliminationPass);
        pm
    }
}

// Wrapper structs for existing passes

/// Free a container this function built and never handed on (#1027).
pub struct ContainerDropInsertionPass;

impl MirPass for ContainerDropInsertionPass {
    fn name(&self) -> &str { "container_drop_insertion" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::insert_container_drops(fns);
    }
}

/// Cross-function closure escape analysis and stack/heap allocation decisions.
pub struct ClosureOptimizationPass;

impl MirPass for ClosureOptimizationPass {
    fn name(&self) -> &str { "closure_optimization" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::optimize_all_closures(fns);
    }
}

/// Insert `TraitDrop` for trait objects that don't escape their function (#366).
pub struct TraitDropInsertionPass;

impl MirPass for TraitDropInsertionPass {
    fn name(&self) -> &str { "trait_drop_insertion" }
    fn run(&self, fns: &mut Vec<MirFunction>, _ctx: &mut PassContext) {
        crate::insert_trait_drops(fns);
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
    fn run_function(&self, func: &mut MirFunction, _ctx: &mut PassContext) {
        crate::transform::dce::eliminate_dead_code(func);
    }
}

/// Insert explicit RcInc/RcDec for string-typed locals (RC1, RC2).
pub struct StringRcInsertionPass;

impl MirPass for StringRcInsertionPass {
    fn name(&self) -> &str { "string_rc_insert" }
    fn run_function(&self, func: &mut MirFunction, _ctx: &mut PassContext) {
        crate::transform::rc_insert::insert_rc_ops(func);
    }
}

/// Elide unnecessary RcInc/RcDec via escape analysis and literal propagation (RE1-RE6).
pub struct StringRcElisionPass;

impl MirPass for StringRcElisionPass {
    fn name(&self) -> &str { "string_rc_elide" }
    fn run_function(&self, func: &mut MirFunction, _ctx: &mut PassContext) {
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
