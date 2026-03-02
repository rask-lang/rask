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

pub mod infer;
pub mod sources;
pub mod warnings;

use std::collections::HashMap;

use rask_ast::Span;
use rask_ast::decl::Decl;

/// 3-bit effect mask per function (FX1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    pub io: bool,
    pub async_: bool,
    pub mutation: bool,
}

impl Effects {
    /// PU1: A function is pure if it has no effects.
    pub fn is_pure(&self) -> bool {
        !self.io && !self.async_ && !self.mutation
    }

    /// Merge another effect set into this one.
    pub fn union(&mut self, other: Effects) {
        self.io |= other.io;
        self.async_ |= other.async_;
        self.mutation |= other.mutation;
    }

    /// Ghost text label for IDE display (IDE1).
    pub fn label(&self) -> &'static str {
        match (self.io, self.async_, self.mutation) {
            (false, false, false) => "[pure]",
            (true, false, false) => "[io]",
            (true, true, false) => "[io, async]",
            (false, false, true) => "[mutation]",
            (true, false, true) => "[io, mutation]",
            (true, true, true) => "[io, async, mutation]",
            // AS3: Async implies IO, so async without io shouldn't happen.
            // Handle defensively anyway.
            (false, true, false) => "[async]",
            (false, true, true) => "[async, mutation]",
        }
    }
}

/// Per-function effect results keyed by qualified function name.
pub type EffectMap = HashMap<String, Effects>;

/// A warning from effect analysis (CW1, CW2).
#[derive(Debug, Clone)]
pub struct EffectWarning {
    /// Spec rule: "comp.effects/CW1" or "comp.effects/CW2".
    pub code: &'static str,
    pub message: String,
    /// Location of the problematic call site.
    pub span: Span,
    /// Name of the function that introduces the effect.
    pub callee_name: String,
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
        let mut a = Effects { io: true, async_: false, mutation: false };
        let b = Effects { io: false, async_: true, mutation: true };
        a.union(b);
        assert!(a.io);
        assert!(a.async_);
        assert!(a.mutation);
    }

    #[test]
    fn pure_detection() {
        assert!(Effects::default().is_pure());
        assert!(!Effects { io: true, ..Default::default() }.is_pure());
    }

    #[test]
    fn label_format() {
        assert_eq!(Effects::default().label(), "[pure]");
        assert_eq!(Effects { io: true, async_: false, mutation: false }.label(), "[io]");
        assert_eq!(Effects { io: true, async_: true, mutation: false }.label(), "[io, async]");
        assert_eq!(Effects { io: false, async_: false, mutation: true }.label(), "[mutation]");
        assert_eq!(Effects { io: true, async_: false, mutation: true }.label(), "[io, mutation]");
    }
}
