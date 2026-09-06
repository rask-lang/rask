// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! CT60: a `comptime func` is checked where it is written, not where it is called.
//!
//! CT7 rules out I/O, spawning and pool structural changes at compile time, and
//! CT6 says a call is legal iff the callee stays inside that subset
//! *transitively*. `comptime func` is the assertion of that property at the
//! definition — "the guarantee moves to the definition, where the author is,
//! instead of erupting at a call site three packages away".
//!
//! It bought nothing: a `comptime func` that reads a line compiled, and the
//! failure came later and elsewhere, as the evaluator not finding a function it
//! had never registered. Effect inference already computes the transitive
//! answer for every function, so the rule is one lookup — and the call site the
//! effect came in through is what the error points at.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::visit::walk_body;
use rask_ast::Span;

use crate::{EffectMap, Effects};

/// A `comptime func` that reaches outside the comptime subset.
#[derive(Debug, Clone)]
pub struct ComptimePurityError {
    /// The function that promised, e.g. `read_config`.
    pub func: String,
    /// What it reaches — "I/O", "concurrency", "a pool insert".
    pub effect: &'static str,
    /// The call that brings the effect in, when one can be named.
    pub via: Option<String>,
    /// Where to point: the offending call, or the declaration when the effect
    /// arrives through something with no call to blame.
    pub span: Span,
    /// The declaration, always, for the secondary label.
    pub decl_span: Span,
}

/// Check every `comptime func` against CT7, transitively (CT6, CT60).
pub fn check(decls: &[Decl], effects: &EffectMap) -> Vec<ComptimePurityError> {
    let mut out = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn(f, &f.name, effects, &mut out),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    check_fn(m, &format!("{}.{}", s.name, m.name), effects, &mut out);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    check_fn(m, &format!("{}.{}", e.name, m.name), effects, &mut out);
                }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    check_fn(m, &format!("{}.{}", i.target_ty, m.name), effects, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

fn check_fn(f: &FnDecl, qname: &str, effects: &EffectMap, out: &mut Vec<ComptimePurityError>) {
    if !f.is_comptime {
        return;
    }
    let own = effects.get(qname).copied().unwrap_or_default();
    let Some(effect) = names_the_effect(&own) else {
        return;
    };
    // Point at the call the effect came in through. Inference is transitive, so
    // the offender may be several hops down — the nearest call whose own
    // effects are impure is the one the reader can act on.
    let culprit = first_impure_call(&f.body, effects);
    let (via, span) = match culprit {
        Some((name, span)) => (Some(name), span),
        None => (None, f.span),
    };
    out.push(ComptimePurityError {
        func: f.name.clone(),
        effect,
        via,
        span,
        decl_span: f.span,
    });
}

/// What CT7 rules out, in the order a reader would want to hear it. `None` when
/// the function stays inside the subset.
fn names_the_effect(e: &Effects) -> Option<&'static str> {
    if e.async_ {
        // AS3 makes async imply io, so this has to be asked first or every
        // spawn reads as I/O.
        Some("concurrency")
    } else if e.io {
        Some("I/O")
    } else if e.grow {
        Some("a pool insert")
    } else if e.shrink {
        Some("a pool remove")
    } else {
        None
    }
}

/// The first call in the body whose callee isn't pure, with its span.
fn first_impure_call(body: &[rask_ast::stmt::Stmt], effects: &EffectMap) -> Option<(String, Span)> {
    let mut found: Option<(String, Span)> = None;
    walk_body(body, &mut |e: &Expr| {
        if found.is_some() {
            return;
        }
        let (name, span) = match &e.kind {
            ExprKind::Call { func, .. } => match callee_name(func) {
                Some(n) => (n, e.span),
                None => return,
            },
            ExprKind::MethodCall { object, method, .. } => {
                let recv = callee_name(object).unwrap_or_default();
                (format!("{}.{}", recv, method), e.span)
            }
            ExprKind::Spawn { .. } => ("spawn".to_string(), e.span),
            _ => return,
        };
        // Two sources: the ground-truth table for a stdlib call, and the
        // inferred map for a function in this program.
        let direct = crate::sources::classify_call(&name);
        let inferred = effects.get(&name).copied().unwrap_or_default();
        if !direct.is_pure() || !inferred.is_pure() {
            found = Some((name, span));
        }
    });
    found
}

/// A callee written as a name or a dotted path, rendered back the way the
/// effect tables spell it (`io.read_line`).
fn callee_name(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { object, field } => Some(format!("{}.{}", callee_name(object)?, field)),
        _ => None,
    }
}

