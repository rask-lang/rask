// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Typestate checking pass — MirPass wrapper for handle typestate analysis.
//!
//! Runs the analysis from `analysis::typestate` and converts detected
//! violations into diagnostics. Self-contained: the core compiler has
//! no knowledge of this pass beyond the pipeline entry in `pass.rs`.

use rask_diagnostics::Diagnostic;

use crate::analysis::typestate;
use crate::transform::pass::{MirPass, PassContext};
use crate::MirFunction;

/// Handle typestate checking pass (comp.advanced TS1-TS8).
///
/// Catches stale handle access at compile time by tracking handle validity
/// states through control flow. Emits errors for provably invalid accesses.
///
/// Uses interprocedural summaries: scans all functions first to determine
/// which callees invalidate handle parameters, then uses that during analysis.
pub struct TypestatePass;

impl MirPass for TypestatePass {
    fn name(&self) -> &str {
        "typestate"
    }

    fn run(&self, fns: &mut Vec<MirFunction>, ctx: &mut PassContext) {
        // Phase 1: compute interprocedural summaries from all functions
        let summaries = typestate::compute_summaries(fns);

        // Phase 2: analyze each function with summaries
        for func in fns.iter() {
            let Some((analysis, results)) = typestate::analyze_with_summaries(func, &summaries) else {
                continue;
            };

            let errors = typestate::check_errors(func, &analysis, &results);
            for e in &errors {
                ctx.diagnostics.push(error_to_diagnostic(e));
            }
        }
    }
}

/// Convert a typestate error into a rich diagnostic.
fn error_to_diagnostic(error: &typestate::TypestateError) -> Diagnostic {
    let handle_desc = match &error.handle_name {
        Some(name) => format!("`{}`", name),
        None => "handle".to_string(),
    };

    let mut diag = Diagnostic::error("stale handle access")
        .with_code("comp.advanced/TS8");

    // Only add span labels if we have real spans (not dummy 0..0)
    if error.invalidation_span.start != 0 || error.invalidation_span.end != 0 {
        diag = diag.with_secondary(error.invalidation_span, "handle invalidated here");
    }

    if error.access_span.start != 0 || error.access_span.end != 0 {
        diag = diag.with_primary(
            error.access_span,
            format!("{} is Invalid (provably stale)", handle_desc),
        );
    }

    diag = diag
        .with_why(
            "Handle typestate analysis proves this handle was removed and is no longer valid.",
        )
        .with_fix(format!(
            "Check validity before access:\n\n  if pool.get({}) is Some {{\n      pool[{}].field\n  }}",
            error.handle_name.as_deref().unwrap_or("h"),
            error.handle_name.as_deref().unwrap_or("h"),
        ));

    diag
}
