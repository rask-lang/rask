// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! CC4: Context resolution order (local > param > self.field > using clause).
//! CC7: Private function context inference from handle field access.
//! CC8: Ambiguity detection (multiple pools of same type in scope).
//!
//! CC9 (inline closures inherit the enclosing context) and CC10 (storable
//! closures resolve only from their own params, else error) live in `rewrite`.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_types::{GenericArg, Type};

use super::callgraph::can_resolve_locally;
use super::{ContextReq, HiddenParamPass, PoolSource, ScopePool};

// ── CC4: Scope Resolution ───────────────────────────────────────────────

/// Resolve a context requirement from the current function's scope.
/// Returns the variable name to use as the hidden argument, following CC4 order:
///   1. Local variables
///   2. Function parameters
///   3. Fields of `self`
///   4. Own `using` clause (hidden param)
///
/// Returns None if no resolution found, or Ambiguous if two pools of the same
/// type sit at the same priority (CC8).
pub(crate) fn resolve_context_in_scope(
    pass: &HiddenParamPass,
    caller_name: &str,
    clause_type: &Type,
) -> ResolveResult {
    let info = match pass.func_info.get(caller_name) {
        Some(i) => i,
        None => return ResolveResult::NotFound,
    };

    let mut candidates: Vec<ScopePool> = Vec::new();

    // CC4 priority 1: Local variables
    for (name, ty) in &info.locals {
        if ty == clause_type {
            candidates.push(ScopePool {
                var_name: name.clone(),
                source: PoolSource::Local,
            });
        }
    }

    // CC4 priority 2: Function parameters
    for (name, ty) in &info.params {
        if ty == clause_type {
            candidates.push(ScopePool {
                var_name: name.clone(),
                source: PoolSource::Parameter,
            });
        }
    }

    // CC4 priority 3: Fields of self
    for (field_name, ty) in &info.self_fields {
        if ty == clause_type {
            candidates.push(ScopePool {
                var_name: format!("self.{}", field_name),
                source: PoolSource::SelfField,
            });
        }
    }

    // CC4 priority 4: Own using clause (already a hidden param)
    for req in &info.reqs {
        if &req.clause_type == clause_type {
            candidates.push(ScopePool {
                var_name: req.param_name.clone(),
                source: PoolSource::UsingClause,
            });
        }
    }

    match candidates.len() {
        0 => ResolveResult::NotFound,
        1 => ResolveResult::Resolved(candidates.into_iter().next().unwrap()),
        _ => {
            // CC8: Check if all candidates are from the same priority level
            // If multiple pools of the same type exist at the same level, it's ambiguous
            let first_source = &candidates[0].source;
            let all_same_source = candidates.iter().all(|c| &c.source == first_source);

            if all_same_source && candidates.len() > 1 {
                // CC8: Ambiguous — multiple pools of same type at same priority
                ResolveResult::Ambiguous(candidates)
            } else {
                // Take the highest-priority candidate (first in the list)
                ResolveResult::Resolved(candidates.into_iter().next().unwrap())
            }
        }
    }
}

pub(crate) enum ResolveResult {
    Resolved(ScopePool),
    Ambiguous(Vec<ScopePool>),
    NotFound,
}

// ── CC7: Private Function Context Inference ─────────────────────────────

/// Scan private functions for handle field access without `using` clauses.
/// For each such function, infer an unnamed context requirement.
pub fn infer_private_contexts(pass: &mut HiddenParamPass, decls: &[Decl]) {
    let mut inferred: Vec<(String, ContextReq)> = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => {
                if let Some(req) = maybe_infer_context(&f.name, f, pass) {
                    inferred.push((f.name.clone(), req));
                }
            }
            DeclKind::Struct(s) => {
                for method in &s.methods {
                    let qname = format!("{}.{}", s.name, method.name);
                    if let Some(req) = maybe_infer_context(&qname, method, pass) {
                        inferred.push((qname, req));
                    }
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    let qname = format!("{}.{}", e.name, method.name);
                    if let Some(req) = maybe_infer_context(&qname, method, pass) {
                        inferred.push((qname, req));
                    }
                }
            }
            DeclKind::Impl(i) => {
                for method in &i.methods {
                    let qname = format!("{}.{}", i.target_ty, method.name);
                    if let Some(req) = maybe_infer_context(&qname, method, pass) {
                        inferred.push((qname, req));
                    }
                }
            }
            _ => {}
        }
    }

    // Add inferred contexts
    for (qname, req) in inferred {
        pass.func_contexts
            .entry(qname)
            .or_default()
            .push(req);
    }
}

/// Check if a private function should have its context inferred (CC7).
/// Returns Some(ContextReq) if the function:
/// - Is not public
/// - Has Handle<T> parameters
/// - Accesses handle fields in the body
/// - Doesn't already have a `using` clause for the relevant Pool<T>
fn maybe_infer_context(
    qname: &str,
    f: &FnDecl,
    pass: &HiddenParamPass,
) -> Option<ContextReq> {
    // Only infer for private functions (CC7 — public must declare explicitly)
    if f.is_pub {
        return None;
    }

    // Skip if already has context clauses
    if !f.context_clauses.is_empty() {
        return None;
    }

    // Skip if already has propagated contexts
    if pass.func_contexts.contains_key(qname) {
        return None;
    }

    // Handle<T> parameters, by element type.
    let handle_params: Vec<(&str, Type)> = f
        .params
        .iter()
        .filter_map(|p| handle_elem(pass, &p.ty).map(|elem| (p.name.as_str(), elem)))
        .collect();

    if handle_params.is_empty() {
        return None;
    }

    // Only infer when the body actually reaches handle fields (h.field).
    let handle_param_names: Vec<&str> = handle_params.iter().map(|(n, _)| *n).collect();
    if !body_accesses_handle_fields(&f.body, &handle_param_names) {
        return None;
    }

    // Infer an unnamed Pool<T> context from the first handle's element type.
    let elem = &handle_params[0].1;
    let pool_type = pass.canonical_type(&pool_of(elem));
    let elem_name = pass.type_head_name(elem)?;

    // CC4/CC7: if the pool is already reachable from the function's own scope
    // (a param, local, or `self` field), auto-deref resolves from there — no
    // hidden param needed. Inferring one would force callers to supply a context
    // they don't have (e.g. a method whose `self.players` backs the handle).
    if can_resolve_locally(pass, qname, &pool_type) {
        return None;
    }

    Some(ContextReq {
        param_name: format!("__ctx_pool_{}", elem_name),
        param_type: format!("&{}", pass.type_to_source(&pool_type)),
        clause_type: pool_type,
        alias: None,
    })
}

/// The element type of a `Handle<T>` parameter (given its source string), or
/// `None` if the parameter isn't a handle. Parses through the type table, so it
/// matches the canonical `Generic` form as well as the unresolved one.
fn handle_elem(pass: &HiddenParamPass, ty_str: &str) -> Option<Type> {
    pass.handle_elem_of_type(&pass.parse_ty(ty_str)?)
}

/// Build `Pool<T>` from an element type `T`.
fn pool_of(elem: &Type) -> Type {
    Type::UnresolvedGeneric {
        name: "Pool".to_string(),
        args: vec![GenericArg::Type(Box::new(elem.clone()))],
    }
}

/// Check if a function body accesses fields on handle-typed variables.
/// Looks for patterns like `h.field` where `h` is one of the handle params.
fn body_accesses_handle_fields(stmts: &[Stmt], handle_names: &[&str]) -> bool {
    for stmt in stmts {
        if stmt_accesses_handle_fields(stmt, handle_names) {
            return true;
        }
    }
    false
}

fn stmt_accesses_handle_fields(stmt: &Stmt, handle_names: &[&str]) -> bool {
    match &stmt.kind {
        StmtKind::Expr(e) => expr_accesses_handle_fields(e, handle_names),
        StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
            expr_accesses_handle_fields(init, handle_names)
        }
        StmtKind::MutTuple { init, .. } | StmtKind::LetTuple { init, .. } => {
            expr_accesses_handle_fields(init, handle_names)
        }
        StmtKind::Assign { target, value, .. } => {
            expr_accesses_handle_fields(target, handle_names)
                || expr_accesses_handle_fields(value, handle_names)
        }
        StmtKind::Return(Some(e)) => expr_accesses_handle_fields(e, handle_names),
        StmtKind::While { cond, body, .. } => {
            expr_accesses_handle_fields(cond, handle_names)
                || body_accesses_handle_fields(body, handle_names)
        }
        StmtKind::WhileLet { expr, body, .. } => {
            expr_accesses_handle_fields(expr, handle_names)
                || body_accesses_handle_fields(body, handle_names)
        }
        StmtKind::Loop { body, .. } | StmtKind::For { body, .. } => {
            body_accesses_handle_fields(body, handle_names)
        }
        StmtKind::Ensure { body, .. } => body_accesses_handle_fields(body, handle_names),
        StmtKind::Comptime(body) | StmtKind::ComptimeFor { body, .. } => {
            body_accesses_handle_fields(body, handle_names)
        }
        _ => false,
    }
}

fn expr_accesses_handle_fields(expr: &Expr, handle_names: &[&str]) -> bool {
    match &expr.kind {
        // h.field — the key pattern
        ExprKind::Field { object, .. } => {
            if let ExprKind::Ident(name) = &object.kind {
                if handle_names.contains(&name.as_str()) {
                    return true;
                }
            }
            expr_accesses_handle_fields(object, handle_names)
        }
        // Recurse into subexpressions
        ExprKind::Binary { left, right, .. } => {
            expr_accesses_handle_fields(left, handle_names)
                || expr_accesses_handle_fields(right, handle_names)
        }
        ExprKind::Unary { operand, .. } => expr_accesses_handle_fields(operand, handle_names),
        ExprKind::Call { func, args } => {
            expr_accesses_handle_fields(func, handle_names)
                || args.iter().any(|a| expr_accesses_handle_fields(&a.expr, handle_names))
        }
        ExprKind::MethodCall { object, args, .. } => {
            expr_accesses_handle_fields(object, handle_names)
                || args.iter().any(|a| expr_accesses_handle_fields(&a.expr, handle_names))
        }
        ExprKind::Index { object, index } => {
            expr_accesses_handle_fields(object, handle_names)
                || expr_accesses_handle_fields(index, handle_names)
        }
        ExprKind::If { cond, then_branch, else_branch, .. } => {
            expr_accesses_handle_fields(cond, handle_names)
                || expr_accesses_handle_fields(then_branch, handle_names)
                || else_branch.as_ref().map_or(false, |e| expr_accesses_handle_fields(e, handle_names))
        }
        ExprKind::Block(stmts) => body_accesses_handle_fields(stmts, handle_names),
        _ => false,
    }
}
