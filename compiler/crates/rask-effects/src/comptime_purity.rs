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

use crate::sources::ImportedNames;
use crate::{EffectMap, Effects, MethodTargets};

/// What a lookup reads: the inferred effects, and how a call names its callee.
struct Lookup<'a> {
    effects: &'a EffectMap,
    imports: ImportedNames,
    targets: &'a MethodTargets,
}

impl Lookup<'_> {
    /// The callee a call reaches, and the effects it has.
    fn callee(&self, e: &Expr) -> Option<(String, Effects)> {
        let name = match &e.kind {
            ExprKind::Call { func, .. } => self.imports.qualify(&callee_name(func)?),
            ExprKind::MethodCall { object, method, .. } => {
                crate::method_callees(e.id, object, method, self.targets, &self.imports)
                    .into_iter()
                    .next()?
            }
            _ => return None,
        };
        let effects = self.effects.get(&name).copied().unwrap_or_default();
        Some((name, effects))
    }
}

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
pub fn check(decls: &[Decl], effects: &EffectMap, targets: &MethodTargets) -> Vec<ComptimePurityError> {
    let lookup = Lookup { effects, imports: ImportedNames::from_decls(decls), targets };
    let mut out = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn(f, &f.name, &lookup, &mut out),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    check_fn(m, &format!("{}.{}", s.name, m.name), &lookup, &mut out);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    check_fn(m, &format!("{}.{}", e.name, m.name), &lookup, &mut out);
                }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    check_fn(m, &crate::method_key(&i.target_ty, &m.name), &lookup, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

fn check_fn(f: &FnDecl, qname: &str, lookup: &Lookup, out: &mut Vec<ComptimePurityError>) {
    if !f.is_comptime {
        return;
    }
    let own = lookup.effects.get(qname).copied().unwrap_or_default();
    let Some(effect) = names_the_effect(&own) else {
        return;
    };
    // Point at the call the effect came in through. Inference is transitive, so
    // the offender may be several hops down — the nearest call whose own
    // effects CT7 rules out is the one the reader can act on.
    let culprit = first_impure_call(&f.body, lookup).map(|(name, span, _)| (name, span));
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

/// Which compile-time value an error is about, in the reader's words.
#[derive(Debug, Clone, PartialEq)]
pub enum CompileTimeValue {
    /// The default of parameter `param` of `func`.
    Param { func: String, param: String },
    /// A field's default: the declared one, or `@default(…)`.
    Field { field: String },
    /// A `comptime { … }` someone wrote.
    Block,
}

/// A compile-time value that reaches outside the compile-time subset.
#[derive(Debug, Clone)]
pub struct CompileTimeValueError {
    pub what: CompileTimeValue,
    /// What it reaches — "I/O", "concurrency", "a pool insert".
    pub effect: &'static str,
    /// The call that brings the effect in.
    pub via: String,
    pub span: Span,
}

/// The functions and methods a declaration carries.
fn fns(decl: &Decl) -> Vec<&FnDecl> {
    match &decl.kind {
        DeclKind::Fn(f) => vec![f],
        DeclKind::Struct(s) => s.methods.iter().collect(),
        DeclKind::Enum(e) => e.methods.iter().collect(),
        DeclKind::Impl(i) => i.methods.iter().collect(),
        _ => Vec::new(),
    }
}

/// Every value the program computes while compiling — a default argument, a
/// field's default, a `comptime` block — checked against CT7 transitively,
/// the way a `comptime func` is (CT60), and pointed at the call that leaves
/// the subset.
///
/// A default is filled in as a `comptime` block at each use, so its copies
/// would report once per use, at the default. Each place reports once, under
/// the name the reader knows it by: the declaration's own defaults are seen
/// first.
pub fn check_compile_time_values(
    decls: &[Decl],
    effects: &EffectMap,
    targets: &MethodTargets,
) -> Vec<CompileTimeValueError> {
    let lookup = Lookup { effects, imports: ImportedNames::from_decls(decls), targets };
    let mut out: Vec<CompileTimeValueError> = Vec::new();
    let push = |what: CompileTimeValue, expr: &Expr, out: &mut Vec<CompileTimeValueError>| {
        let body = [rask_ast::stmt::Stmt {
            id: rask_ast::NodeId(0),
            kind: rask_ast::stmt::StmtKind::Expr(expr.clone()),
            span: expr.span,
        }];
        if let Some((via, span, effect)) = first_impure_call(&body, &lookup) {
            if !out.iter().any(|e| e.span == span) {
                out.push(CompileTimeValueError { what, effect, via, span });
            }
        }
    };
    for decl in decls {
        for f in fns(decl) {
            for param in &f.params {
                if let Some(default) = &param.default {
                    let what = CompileTimeValue::Param { func: f.name.clone(), param: param.name.clone() };
                    push(what, default, &mut out);
                }
            }
        }
        if let DeclKind::Struct(s) = &decl.kind {
            for field in &s.fields {
                let declared = field.default.as_ref();
                let decode = rask_ast::decl::field_attrs::decode_default(field);
                for default in declared.into_iter().chain(decode) {
                    push(CompileTimeValue::Field { field: field.name.clone() }, default, &mut out);
                }
            }
        }
    }
    for decl in decls {
        for f in fns(decl) {
            if f.is_comptime {
                continue;
            }
            rask_ast::visit::walk_body(&f.body, &mut |e: &Expr| {
                if matches!(e.kind, ExprKind::Comptime { .. }) {
                    push(CompileTimeValue::Block, e, &mut out);
                }
            });
        }
        if let DeclKind::Test(t) = &decl.kind {
            rask_ast::visit::walk_body(&t.body, &mut |e: &Expr| {
                if matches!(e.kind, ExprKind::Comptime { .. }) {
                    push(CompileTimeValue::Block, e, &mut out);
                }
            });
        }
    }
    out
}

/// What CT7 rules out, in the order a reader would want to hear it. `None` when
/// the function stays inside the subset.
///
/// A size change isn't on the list. `Vec`, `Map` and `string` work at compile
/// time (CT26), and building a table is what comptime is for; it was on the
/// list while `@grows` on `Vec.push` read as "a pool insert". Pools themselves
/// (CT31) are the evaluator's to refuse.
fn names_the_effect(e: &Effects) -> Option<&'static str> {
    if e.async_ {
        Some("concurrency")
    } else if e.blocks {
        Some("a call that blocks")
    } else if e.external {
        Some("state outside the program")
    } else {
        None
    }
}

/// The first call in the body that brings in something CT7 rules out: its
/// callee, its span, and the name of what it reaches.
fn first_impure_call(
    body: &[rask_ast::stmt::Stmt],
    lookup: &Lookup,
) -> Option<(String, Span, &'static str)> {
    let mut found = None;
    walk_body(body, &mut |e: &Expr| {
        if found.is_some() {
            return;
        }
        if let Some((name, effects)) = lookup.callee(e) {
            if let Some(effect) = names_the_effect(&effects) {
                found = Some((name, e.span, effect));
            }
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

