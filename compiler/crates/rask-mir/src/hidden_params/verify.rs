// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 3c: a context requirement that reached a function which can't supply it.
//!
//! A `using` clause is a hidden parameter the caller fills in. Propagation walks
//! that requirement up the call graph until it finds someone holding the pool.
//! Two places stop it, and until this pass ran neither said so:
//!
//!   * a public function, which CC6 says must declare its contexts — the
//!     signature is the contract, so nothing is inferred across it;
//!   * the entry point, which CC11 says can't declare one, because there is no
//!     caller left to fill it in.
//!
//! Either way the rewrite went ahead and emitted a hidden argument that names
//! nothing: MIR reported `unresolved variable __ctx_pool_Player` when the call
//! was reachable, and when it wasn't, native read the uninitialised parameter as
//! a pool pointer and segfaulted on the first index through it. #732 is the same
//! bug for a clause written by hand; this is the one propagation introduces.

use std::collections::HashSet;

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::visit::walk_body;
use rask_ast::Span;
use rask_diagnostics::Diagnostic;

use super::callgraph::can_resolve_locally;
use super::resolve::inferred_context;
use super::{extract_callee_name, FuncName, HiddenParamPass};

/// Report every requirement that has nowhere left to come from. Runs after
/// propagation and inference, before the rewrite that would otherwise emit a
/// hidden argument naming a variable that doesn't exist.
pub fn verify_contexts(pass: &mut HiddenParamPass, decls: &[Decl]) {
    if pass.typed.is_none() {
        // Name-based call-graph keying can confuse two methods that share a
        // name, and a wrong rejection is worse than a missing one.
        return;
    }

    let mut diagnostics = Vec::new();
    for_each_fn(decls, &mut |qname, f| {
        check_public_declares_its_contexts(pass, &qname, f, &mut diagnostics);
        check_calls_are_satisfiable(pass, &qname, f, &mut diagnostics);
    });
    for decl in decls {
        let (kind, name, body) = match &decl.kind {
            DeclKind::Test(t) => ("test", &t.name, &t.body),
            DeclKind::Benchmark(b) => ("benchmark", &b.name, &b.body),
            _ => continue,
        };
        // A block has no signature, so it can't declare a clause and can't be
        // given a hidden parameter: whatever it calls has to resolve from the
        // pools the block itself owns, exactly like `main`.
        let qname = super::block_scope_name(kind, name);
        check_block_calls(pass, &qname, kind, name, body, &mut diagnostics);
    }
    pass.diagnostics.extend(diagnostics);
}

/// The `test`/`benchmark` half of the entry-point rule: the block owns its
/// pools or the call has nowhere to get one.
fn check_block_calls(
    pass: &HiddenParamPass,
    qname: &str,
    kind: &str,
    name: &str,
    body: &[rask_ast::stmt::Stmt],
    out: &mut Vec<Diagnostic>,
) {
    let mut reported: HashSet<String> = HashSet::new();
    for (callee, span) in calls_in(pass, body) {
        let Some(reqs) = pass.func_contexts.get(&callee) else {
            continue;
        };
        for req in reqs.clone() {
            let pool = pass.type_to_source(&req.clause_type);
            if !reported.insert(pool.clone()) {
                continue;
            }
            if can_resolve_locally(pass, qname, &req.clause_type) {
                continue;
            }
            out.push(entry_point_has_no_pool(
                &format!("{kind} {name:?}"),
                &callee,
                &pool,
                span,
            ));
        }
    }
}

/// CC6: a public function that reaches handle fields needs the clause written
/// out. The inference that covers private functions deliberately stops at the
/// signature — a caller in another module reads the signature, not the body.
fn check_public_declares_its_contexts(
    pass: &HiddenParamPass,
    qname: &str,
    f: &FnDecl,
    out: &mut Vec<Diagnostic>,
) {
    if !f.is_pub || !f.context_clauses.is_empty() {
        return;
    }
    let Some(req) = inferred_context(pass, qname, f) else {
        return;
    };
    let pool = pass.type_to_source(&req.clause_type);
    out.push(
        Diagnostic::error(format!(
            "public function `{}` uses a handle without declaring its pool",
            f.name
        ))
        .with_code("E0867")
        .with_primary(f.span, format!("reads through a handle, so it needs {pool}"))
        .with_fix(format!(
            "add the clause: `func {}(…) using {pool}` — or `using pool: {pool}` \
             if the body also inserts or removes",
            f.name
        ))
        .with_why(
            "a `using` clause is part of a public function's signature, so the \
             compiler won't infer one across it — callers have to be able to see \
             what they must supply [mem.context/CC6]"
                .to_string(),
        ),
    );
}

/// CC5/CC11: a call whose context requirement the caller can neither hold nor
/// pass on.
fn check_calls_are_satisfiable(
    pass: &HiddenParamPass,
    qname: &str,
    f: &FnDecl,
    out: &mut Vec<Diagnostic>,
) {
    let entry = is_entry_point(f);
    let mut reported: HashSet<String> = HashSet::new();

    for (callee, span) in calls_in(pass, &f.body) {
        let Some(reqs) = pass.func_contexts.get(&callee) else {
            continue;
        };
        for req in reqs.clone() {
            let pool = pass.type_to_source(&req.clause_type);
            if !reported.insert(pool.clone()) {
                continue;
            }
            if can_resolve_locally(pass, qname, &req.clause_type) {
                continue;
            }
            let declared = f
                .context_clauses
                .iter()
                .filter_map(|cc| pass.parse_ty(&cc.ty))
                .any(|ty| ty == req.clause_type);
            if declared {
                continue;
            }
            if entry {
                out.push(entry_point_has_no_pool(&f.name, &callee, &pool, span));
            } else if f.is_pub {
                out.push(public_caller_must_declare(&f.name, &callee, &pool, span));
            }
            // A private non-entry caller got the requirement by propagation and
            // passes it on, so there is nothing to report here.
        }
    }
}

fn entry_point_has_no_pool(
    entry: &str,
    callee: &str,
    pool: &str,
    span: Span,
) -> Diagnostic {
    let head = pool.split('<').next().unwrap_or(pool).trim();
    Diagnostic::error(format!("no {pool} for `{callee}` to use"))
        .with_code("E0868")
        .with_primary(span, format!("this call needs {pool}"))
        .with_fix(format!(
            "own the pool here — `mut pool: {pool} = {head}.new()` — and the call \
             resolves it out of `{entry}`'s scope"
        ))
        .with_why(format!(
            "`{callee}` takes {pool} as a hidden parameter its caller fills in, and \
             the chain of callers ends at `{entry}`, which has no caller of its own \
             to supply one [mem.context/CC11]"
        ))
}

fn public_caller_must_declare(
    caller: &str,
    callee: &str,
    pool: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(format!("no {pool} for `{callee}` to use"))
        .with_code("E0868")
        .with_primary(span, format!("this call needs {pool}"))
        .with_fix(format!(
            "declare it: `func {caller}(…) using {pool}` — or take the pool as an \
             ordinary parameter and index it directly"
        ))
        .with_why(format!(
            "`{callee}` takes {pool} as a hidden parameter its caller fills in. \
             `{caller}` is public, so the compiler won't add one behind its \
             signature — callers have to be able to see what they supply \
             [mem.context/CC6]"
        ))
}

/// The process entry point: a free `main`, or a function marked `@entry`.
/// Matches the checker's own rule for CC11 (`check_fn::is_entry_point`).
fn is_entry_point(f: &FnDecl) -> bool {
    f.attrs.iter().any(|a| a == "entry") || f.name == "main"
}

/// Every call in a body, keyed the way the call graph keys it, with the span to
/// point at.
fn calls_in(pass: &HiddenParamPass, body: &[rask_ast::stmt::Stmt]) -> Vec<(FuncName, Span)> {
    let mut calls = Vec::new();
    walk_body(body, &mut |e: &Expr| match &e.kind {
        ExprKind::Call { func, .. } => {
            if let Some(key) = pass.callee_key(e.id).or_else(|| extract_callee_name(func)) {
                calls.push((key, e.span));
            }
        }
        ExprKind::MethodCall { method, .. } => {
            let key = pass.callee_key(e.id).unwrap_or_else(|| method.clone());
            calls.push((key, e.span));
        }
        _ => {}
    });
    calls
}

/// Every function declaration with the name the call graph knows it by, so a
/// method keys as `Type.method` exactly as `collect_contexts` recorded it.
fn for_each_fn(decls: &[Decl], f: &mut impl FnMut(String, &FnDecl)) {
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(fd) => f(fd.name.clone(), fd),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    f(format!("{}.{}", s.name, m.name), m);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    f(format!("{}.{}", e.name, m.name), m);
                }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    f(format!("{}.{}", i.target_ty, m.name), m);
                }
            }
            _ => {}
        }
    }
}
