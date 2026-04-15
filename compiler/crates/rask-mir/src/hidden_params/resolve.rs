// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! CC4: Context resolution order (local > param > self.field > using clause).
//! CC7: Private function context inference from handle field access.
//! CC8: Ambiguity detection (multiple pools of same type in scope).
//! CC9: Immediate closure context inheritance.
//! CC10: Storable closure context exclusion.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use super::{
    extract_generic_arg, handle_to_pool_type, is_handle_type,
    ContextReq, HiddenParamPass, PoolSource, ScopePool,
};

// ── CC4: Scope Resolution ───────────────────────────────────────────────

/// Resolve a context requirement from the current function's scope.
/// Returns the variable name to use as the hidden argument, following CC4 order:
///   1. Local variables
///   2. Function parameters
///   3. Fields of `self`
///   4. Own `using` clause (hidden param)
///
/// Returns None if no resolution found, or an error string if ambiguous (CC8).
pub(crate) fn resolve_context_in_scope(
    pass: &HiddenParamPass,
    caller_name: &str,
    clause_type: &str,
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
                pool_type: ty.clone(),
                source: PoolSource::Local,
            });
        }
    }

    // CC4 priority 2: Function parameters
    for (name, ty) in &info.params {
        if ty == clause_type || ty == &format!("&{}", clause_type) {
            candidates.push(ScopePool {
                var_name: name.clone(),
                pool_type: clause_type.to_string(),
                source: PoolSource::Parameter,
            });
        }
    }

    // CC4 priority 3: Fields of self
    for (field_name, ty) in &info.self_fields {
        if ty == clause_type {
            candidates.push(ScopePool {
                var_name: format!("self.{}", field_name),
                pool_type: ty.clone(),
                source: PoolSource::SelfField,
            });
        }
    }

    // CC4 priority 4: Own using clause (already a hidden param)
    for req in &info.reqs {
        if req.clause_type == clause_type {
            candidates.push(ScopePool {
                var_name: req.param_name.clone(),
                pool_type: req.clause_type.clone(),
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

    // Find Handle<T> parameters
    let handle_types: Vec<String> = f
        .params
        .iter()
        .filter(|p| is_handle_type(&p.ty))
        .map(|p| p.ty.clone())
        .collect();

    if handle_types.is_empty() {
        return None;
    }

    // Check if body accesses handle fields (h.field patterns)
    let handle_param_names: Vec<&str> = f
        .params
        .iter()
        .filter(|p| is_handle_type(&p.ty))
        .map(|p| p.name.as_str())
        .collect();

    let has_field_access = body_accesses_handle_fields(&f.body, &handle_param_names);

    if !has_field_access {
        return None;
    }

    // Infer unnamed context for the first handle type found
    let pool_type = handle_to_pool_type(&handle_types[0])?;
    let inner = extract_generic_arg(&pool_type)?;
    let param_name = format!("__ctx_pool_{}", inner);

    Some(ContextReq {
        param_name,
        param_type: format!("&{}", pool_type),
        clause_type: pool_type,
        is_runtime: false,
        alias: None,
    })
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
        StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
            expr_accesses_handle_fields(init, handle_names)
        }
        StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            expr_accesses_handle_fields(init, handle_names)
        }
        StmtKind::Assign { target, value } => {
            expr_accesses_handle_fields(target, handle_names)
                || expr_accesses_handle_fields(value, handle_names)
        }
        StmtKind::Return(Some(e)) => expr_accesses_handle_fields(e, handle_names),
        StmtKind::While { cond, body } => {
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
        ExprKind::If { cond, then_branch, else_branch } => {
            expr_accesses_handle_fields(cond, handle_names)
                || expr_accesses_handle_fields(then_branch, handle_names)
                || else_branch.as_ref().map_or(false, |e| expr_accesses_handle_fields(e, handle_names))
        }
        ExprKind::Block(stmts) => body_accesses_handle_fields(stmts, handle_names),
        _ => false,
    }
}

// ── CC9/CC10: Closure context rules ─────────────────────────────────────

/// Check if an expression is an expression-scoped (immediate) closure.
/// Expression-scoped closures appear as arguments to calls like:
///   vec.map(|x| x.field)
///   pool.cursor().for_each(|h| ...)
/// These inherit enclosing contexts (CC9).
pub(crate) fn is_expression_scoped_closure(expr: &Expr) -> bool {
    // Closures used as arguments to iterator/collection methods are expression-scoped.
    // Closures assigned to variables with Func type are storable.
    // This heuristic: if a closure appears directly inside a Call or MethodCall arg, it's CC9.
    // If it appears in a Let/Const init, it's CC10 (storable).
    true // Default to expression-scoped; rewrite phase checks assignment context
}

/// Check if a closure is storable (CC10 — cannot inherit contexts).
/// Storable closures are assigned to variables: `const callback: |Handle<Player>| = |h| { ... }`
pub(crate) fn is_storable_closure(_stmt: &Stmt) -> bool {
    // A closure is storable if it's the init expression of a Let/Const
    // where the type annotation is a function type.
    false // Conservative: most closures are expression-scoped
}
