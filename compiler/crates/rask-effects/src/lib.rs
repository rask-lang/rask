// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Effect tracking for Rask (comp.effects).
//!
//! Infers IO, Async, and Mutation effects per function via transitive
//! call graph analysis. Effects are metadata — visible through IDE ghost
//! text and linter warnings, not enforced in the type system.
//!
//! Three categories (FX1):
//! - IO: syscalls, file/network/stdio operations
//! - Async: spawn, sleep, channel ops
//! - Mutation: pool structural changes (Grow/Shrink)
//!
//! Run after type checking. No AST modifications — annotation only.

pub mod frozen;
pub mod infer;
pub mod sources;
pub mod warnings;

use std::collections::HashMap;

use rask_ast::Span;
use rask_ast::decl::Decl;

/// Effect mask per function (FX1, EF1).
///
/// `grow` and `shrink` replace the old single `mutation` flag (EF1 split).
/// `mutation()` is a convenience that returns true if either is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    pub io: bool,
    pub async_: bool,
    /// Grow effect: pool.insert, pool.alloc (EF1).
    pub grow: bool,
    /// Shrink effect: pool.remove, pool.clear, pool.drain (EF1).
    pub shrink: bool,
    /// CC2: this function needs an active `using Multitasking` runtime at the call site.
    /// Set when the function (or something it calls without an internal block) reaches `spawn`.
    /// NOT propagated via the regular call graph — uses unguarded-call edges only.
    pub needs_runtime: bool,
}

impl Effects {
    /// PU1: A function is pure if it has no effects.
    pub fn is_pure(&self) -> bool {
        !self.io && !self.async_ && !self.grow && !self.shrink
    }

    /// Whether this function performs structural pool mutations.
    pub fn mutation(&self) -> bool {
        self.grow || self.shrink
    }

    /// Merge another effect set into this one.
    pub fn union(&mut self, other: Effects) {
        self.io |= other.io;
        self.async_ |= other.async_;
        self.grow |= other.grow;
        self.shrink |= other.shrink;
    }

    /// Ghost text label for IDE display (IDE1).
    pub fn label(&self) -> &'static str {
        let mutation = self.mutation();
        match (self.io, self.async_, mutation) {
            (false, false, false) => "[pure]",
            (true, false, false) => "[io]",
            (true, true, false) => "[io, async]",
            (false, false, true) => "[mutation]",
            (true, false, true) => "[io, mutation]",
            (true, true, true) => "[io, async, mutation]",
            // AS3: Async implies IO, so async without io shouldn't happen.
            (false, true, false) => "[async]",
            (false, true, true) => "[async, mutation]",
        }
    }
}

/// Per-function effect results keyed by qualified function name.
pub type EffectMap = HashMap<String, Effects>;

/// A warning or error from effect analysis (CW1, CW2, CC2).
#[derive(Debug, Clone)]
pub struct EffectWarning {
    /// Spec rule: "comp.effects/CW1", "comp.effects/CW2", "conc.async/CC2".
    pub code: &'static str,
    pub message: String,
    /// Location of the problematic call site.
    pub span: Span,
    /// Name of the function that introduces the effect.
    pub callee_name: String,
    /// True if this should be reported as an error (CC2), false for warnings (CW1, CW2).
    pub is_error: bool,
}

/// Run effect inference on a set of declarations.
///
/// Returns the per-function effect map and any warnings (CW1/CW2).
/// Call after type checking — no AST modifications.
pub fn infer_effects(decls: &[Decl]) -> (EffectMap, Vec<EffectWarning>) {
    let effects = infer::infer(decls);
    let warnings = warnings::detect(decls, &effects);
    (effects, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_union() {
        let mut a = Effects { io: true, async_: false, grow: false, shrink: false, needs_runtime: false };
        let b = Effects { io: false, async_: true, grow: true, shrink: true, needs_runtime: false };
        a.union(b);
        assert!(a.io);
        assert!(a.async_);
        assert!(a.grow);
        assert!(a.shrink);
    }

    #[test]
    fn pure_detection() {
        assert!(Effects::default().is_pure());
        assert!(!Effects { io: true, ..Default::default() }.is_pure());
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
        assert_eq!(Effects { io: true, ..Default::default() }.label(), "[io]");
        assert_eq!(Effects { io: true, async_: true, ..Default::default() }.label(), "[io, async]");
        assert_eq!(Effects { grow: true, ..Default::default() }.label(), "[mutation]");
        assert_eq!(Effects { io: true, shrink: true, ..Default::default() }.label(), "[io, mutation]");
    }
}
