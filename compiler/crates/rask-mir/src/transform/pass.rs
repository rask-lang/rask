// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Pass manager — runs MIR optimization passes in sequence.
//!
//! Each pass implements `MirPass`. The `PassManager` runs them in order,
//! collecting diagnostics from analysis passes (e.g., typestate checking).

use rask_diagnostics::Diagnostic;

use crate::MirFunction;
use crate::transform::typestate::TypestatePass;

/// Result of running a pass — may contain diagnostics (errors, warnings, notes).
pub struct PassResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl PassResult {
    /// No diagnostics produced.
    pub fn ok() -> Self {
        Self { diagnostics: vec![] }
    }
}

/// A MIR-to-MIR transformation pass.
pub trait MirPass {
    /// Short name for logging/debugging.
    fn name(&self) -> &str;

    /// Run on the full set of functions. Default iterates per-function.
    /// Override for cross-function passes (e.g., closure escape analysis).
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        let mut result = PassResult::ok();
        for func in fns.iter_mut() {
            let r = self.run_function(func);
            result.diagnostics.extend(r.diagnostics);
        }
        result
    }

    /// Run on a single function. Default is no-op.
    /// Most passes override this.
    fn run_function(&self, _func: &mut MirFunction) -> PassResult {
        PassResult::ok()
    }
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

    /// Run all passes in order, collecting diagnostics.
    pub fn run(&self, fns: &mut Vec<MirFunction>) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        for pass in &self.passes {
            let result = pass.run(fns);
            diagnostics.extend(result.diagnostics);
        }
        diagnostics
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
        // Phase G: Typestate checking before gen coalescing (needs PoolCheckedAccess intact)
        pm.add(TypestatePass);
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
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        crate::optimize_all_closures(fns);
        PassResult::ok()
    }
}

/// Cross-function inliner — splices small/once-called function bodies into callers (IN1-IN5).
pub struct InliningPass;

impl MirPass for InliningPass {
    fn name(&self) -> &str { "inlining" }
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        crate::transform::inline::inline_functions(fns);
        PassResult::ok()
    }
}

/// Self-concat → in-place append (eliminates O(n² string building).
pub struct StringConcatPass;

impl MirPass for StringConcatPass {
    fn name(&self) -> &str { "string_concat" }
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        crate::optimize_string_concat(fns);
        PassResult::ok()
    }
}

/// Last-use clone → move when source is dead after clone.
pub struct CloneElisionPass;

impl MirPass for CloneElisionPass {
    fn name(&self) -> &str { "clone_elision" }
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        crate::elide_clones(fns);
        PassResult::ok()
    }
}

/// Remove unreachable blocks and dead assignments.
pub struct DeadCodeEliminationPass;

impl MirPass for DeadCodeEliminationPass {
    fn name(&self) -> &str { "dce" }
    fn run_function(&self, func: &mut MirFunction) -> PassResult {
        crate::transform::dce::eliminate_dead_code(func);
        PassResult::ok()
    }
}

/// Insert explicit RcInc/RcDec for string-typed locals (RC1, RC2).
pub struct StringRcInsertionPass;

impl MirPass for StringRcInsertionPass {
    fn name(&self) -> &str { "string_rc_insert" }
    fn run_function(&self, func: &mut MirFunction) -> PassResult {
        crate::transform::rc_insert::insert_rc_ops(func);
        PassResult::ok()
    }
}

/// Elide unnecessary RcInc/RcDec via escape analysis and literal propagation (RE1-RE6).
pub struct StringRcElisionPass;

impl MirPass for StringRcElisionPass {
    fn name(&self) -> &str { "string_rc_elide" }
    fn run_function(&self, func: &mut MirFunction) -> PassResult {
        crate::transform::rc_elide::elide_rc_ops(func);
        PassResult::ok()
    }
}

/// Merge redundant PoolCheckedAccess on same (pool, handle).
pub struct GenerationCoalescingPass;

impl MirPass for GenerationCoalescingPass {
    fn name(&self) -> &str { "generation_coalescing" }
    fn run(&self, fns: &mut Vec<MirFunction>) -> PassResult {
        crate::coalesce_generation_checks(fns);
        PassResult::ok()
    }
}
