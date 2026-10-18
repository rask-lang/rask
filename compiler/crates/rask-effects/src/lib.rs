// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Effect tracking for Rask (comp.effects).
//!
//! Infers what each function does beyond computing its result, via
//! transitive call graph analysis. Effects are metadata — visible through IDE
//! ghost text and linter warnings, not enforced in the type system.
//!
//! Categories (FX1):
//! - Blocks: the thread can park until something outside it happens — a
//!   syscall, `sleep`, a channel op, a `join`
//! - External: reads or changes state outside the program without blocking —
//!   the clock, the environment, the process
//! - Async: spawns a task
//! - Mutation: changes a collection's size (Grow/Shrink)
//!
//! The stdlib's bodies are inferred like the program's, so only the natives at
//! the bottom say anything themselves (`@blocks`, `@ambient`), and an
//! `extern` C function is assumed to do both unless it says `@ambient`
//! (outside state, never waits) or `@pure` (neither).
//!
//! Run after type checking. No AST modifications — annotation only.

pub mod comptime_purity;
pub mod infer;
pub mod sources;
pub mod warnings;

use std::collections::HashMap;

use rask_ast::{NodeId, Span};
use rask_ast::decl::Decl;
use rask_ast::expr::Expr;
use rask_ast::ty::TypeExpr;

/// Effect mask per function (FX1, EF1).
///
/// `grow` and `shrink` replace the old single `mutation` flag (EF1 split).
/// `mutation()` is a convenience that returns true if either is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    /// Can park the thread until something outside it happens.
    pub blocks: bool,
    /// Reads or changes state nobody passed in: the clock, the environment,
    /// the process. Doesn't wait.
    pub ambient: bool,
    /// Spawns a task.
    pub async_: bool,
    /// Grow effect: pool.insert, pool.alloc (EF1).
    pub grow: bool,
    /// Shrink effect: remove, clear, drain (EF1).
    pub shrink: bool,
    /// CC2: this function needs an active `using Multitasking` runtime at the call site.
    /// Set when the function (or something it calls without an internal block) reaches `spawn`.
    /// NOT propagated via the regular call graph — uses unguarded-call edges only.
    pub needs_runtime: bool,
}

impl Effects {
    /// PU1: A function is pure if it has no effects.
    pub fn is_pure(&self) -> bool {
        !self.blocks && !self.ambient && !self.async_ && !self.grow && !self.shrink
    }

    /// Whether this function performs structural pool mutations.
    pub fn mutation(&self) -> bool {
        self.grow || self.shrink
    }

    /// Merge another effect set into this one.
    pub fn union(&mut self, other: Effects) {
        self.blocks |= other.blocks;
        self.ambient |= other.ambient;
        self.async_ |= other.async_;
        self.grow |= other.grow;
        self.shrink |= other.shrink;
    }

    /// Ghost text label for IDE display (IDE1): `[pure]`, or the effects
    /// present, in a fixed order.
    pub fn label(&self) -> String {
        let parts: Vec<&str> = [
            (self.blocks, "blocks"),
            (self.ambient, "ambient"),
            (self.async_, "async"),
            (self.mutation(), "mutation"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if parts.is_empty() {
            "[pure]".to_string()
        } else {
            format!("[{}]", parts.join(", "))
        }
    }
}

/// Per-function effect results keyed by qualified function name.
pub type EffectMap = HashMap<String, Effects>;

/// A warning or error from effect analysis (CW1, CW2, CC1, CC2).
#[derive(Debug, Clone)]
pub struct EffectWarning {
    /// Spec rule: "comp.effects/CW1", "comp.effects/CW2", "conc.async/CC2".
    pub code: &'static str,
    pub message: String,
    /// Location of the problematic call site.
    pub span: Span,
    /// Name of the function that introduces the effect.
    pub callee_name: String,
    /// True if this should be reported as an error (CC1, CC2), false for
    /// warnings (CW1, CW2).
    pub is_error: bool,
    /// What the caret under `span` says.
    pub label: String,
    /// The fix, as code where there is one to show.
    pub fix: Option<String>,
    /// Why the rule exists — not a restatement of the message.
    pub why: Option<String>,
}

/// The checker's answer for each method call it resolved, keyed by the
/// call's node.
pub type MethodTargets = HashMap<NodeId, rask_ast::decl::MethodTarget>;

/// Run effect inference on a program and the stdlib it was checked with.
///
/// The stdlib's bodies are inferred too, so a call into it carries what the
/// body does; warnings (CW1/CW2) are only about the program's own code.
/// Returns the per-function effect map and those warnings. Call after type
/// checking, with what it resolved — no AST modifications.
pub fn infer_effects(
    program: &[Decl],
    stdlib: &[Decl],
    targets: &MethodTargets,
) -> (EffectMap, Vec<EffectWarning>) {
    let (effects, runtime_only) = infer::infer_with_reach(program, stdlib, targets);
    let warnings = warnings::detect(program, &effects, &runtime_only, targets);
    (effects, warnings)
}

/// The effect-map key of every function the declarations hold, the way
/// inference files them.
pub(crate) fn decl_keys(decls: &[Decl]) -> Vec<String> {
    use rask_ast::decl::DeclKind;
    let mut keys = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => keys.push(f.name.clone()),
            DeclKind::Struct(s) => keys.extend(s.methods.iter().map(|m| format!("{}.{}", s.name, m.name))),
            DeclKind::Enum(e) => keys.extend(e.methods.iter().map(|m| format!("{}.{}", e.name, m.name))),
            DeclKind::Impl(i) => keys.extend(i.methods.iter().map(|m| method_key(&i.target_ty, &m.name))),
            DeclKind::Interface(t) => keys.extend(t.methods.iter().map(|m| format!("{}.{}", t.name, m.name))),
            DeclKind::Test(t) => keys.push(format!("test {:?}", t.name)),
            DeclKind::Benchmark(b) => keys.push(format!("benchmark {:?}", b.name)),
            _ => {}
        }
    }
    keys
}

/// The effect-map key for a method declared in `extend Target`.
pub(crate) fn method_key(target: &TypeExpr, method: &str) -> String {
    let ty = target.name().unwrap_or_else(|| target.to_string());
    format!("{ty}.{method}")
}

/// The names a method call counts as calling.
///
/// The checker's `Type.method` when it resolved the call. A call it didn't
/// resolve falls back to how it's spelled: `recv.method` plus the bare
/// method name. Spelling alone classed `t.join()` by the variable `t`, so a
/// call through any variable missed every `Type.method` source (#1418). The
/// spelled form goes through the imports, so `f.exists` under `import fs as f`
/// is `fs.exists`.
pub(crate) fn method_callees(
    call: NodeId,
    object: &Expr,
    method: &str,
    targets: &MethodTargets,
    imports: &sources::ImportedNames,
) -> Vec<String> {
    if let Some(target) = targets.get(&call) {
        return vec![target.name.clone()];
    }
    let mut names = Vec::new();
    if let Some(recv) = object.name() {
        names.push(imports.qualify(&format!("{recv}.{method}")));
    }
    names.push(method.to_string());
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_union() {
        let mut a = Effects { blocks: true, ..Default::default() };
        let b = Effects { ambient: true, async_: true, grow: true, shrink: true, ..Default::default() };
        a.union(b);
        assert!(a.blocks);
        assert!(a.ambient);
        assert!(a.async_);
        assert!(a.grow);
        assert!(a.shrink);
    }

    #[test]
    fn pure_detection() {
        assert!(Effects::default().is_pure());
        assert!(!Effects { blocks: true, ..Default::default() }.is_pure());
        assert!(!Effects { ambient: true, ..Default::default() }.is_pure());
        assert!(!Effects { grow: true, ..Default::default() }.is_pure());
    }

    #[test]
    fn mutation_convenience() {
        assert!(!Effects::default().mutation());
        assert!(Effects { grow: true, ..Default::default() }.mutation());
        assert!(Effects { shrink: true, ..Default::default() }.mutation());
    }

    #[test]
    fn label_format() {
        assert_eq!(Effects::default().label(), "[pure]");
        assert_eq!(Effects { blocks: true, ..Default::default() }.label(), "[blocks]");
        assert_eq!(Effects { blocks: true, async_: true, ..Default::default() }.label(), "[blocks, async]");
        assert_eq!(Effects { grow: true, ..Default::default() }.label(), "[mutation]");
        assert_eq!(Effects { ambient: true, shrink: true, ..Default::default() }.label(), "[ambient, mutation]");
    }
}
