// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 4-6: Rewrite signatures, call sites, and using blocks.

use rask_ast::decl::{DeclKind, FnDecl, Param};
use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;

use super::resolve::{resolve_context_in_scope, ResolveResult};
use super::{extract_callee_name, HiddenParamPass, PoolSource};

/// Rewrite all declarations.
pub fn rewrite_decls(pass: &mut HiddenParamPass, decls: &mut [rask_ast::decl::Decl]) {
    for decl in decls.iter_mut() {
        match &mut decl.kind {
            DeclKind::Fn(f) => {
                let name = f.name.clone();
                rewrite_fn(pass, &name, f);
            }
            DeclKind::Struct(s) => {
                let type_name = s.name.clone();
                for method in &mut s.methods {
                    let qname = format!("{}.{}", type_name, method.name);
                    rewrite_fn(pass, &qname, method);
                }
            }
            DeclKind::Enum(e) => {
                let type_name = e.name.clone();
                for method in &mut e.methods {
                    let qname = format!("{}.{}", type_name, method.name);
                    rewrite_fn(pass, &qname, method);
                }
            }
            DeclKind::Impl(i) => {
                let type_name = i.target_ty.clone();
                for method in &mut i.methods {
                    let qname = format!("{}.{}", type_name, method.name);
                    rewrite_fn(pass, &qname, method);
                }
            }
            DeclKind::Trait(t) => {
                let type_name = t.name.clone();
                for method in &mut t.methods {
                    let qname = format!("{}.{}", type_name, method.name);
                    rewrite_fn(pass, &qname, method);
                }
            }
            DeclKind::Test(t) => {
                rewrite_stmts(pass, "", &mut t.body);
            }
            DeclKind::Benchmark(b) => {
                rewrite_stmts(pass, "", &mut b.body);
            }
            _ => {}
        }
    }
}

/// Rewrite a single function: add hidden params + rewrite body.
fn rewrite_fn(pass: &mut HiddenParamPass, qname: &str, f: &mut FnDecl) {
    // Phase 4 (SIG1-SIG6): Add hidden params to signature
    if let Some(reqs) = pass.func_contexts.get(qname).cloned() {
        // SIG2: named contexts (`using players: Pool<T>`) need a local alias
        // so body code written against the context name resolves to the hidden
        // param. Collect them while adding params, then prepend to the body.
        let mut aliases: Vec<(String, String)> = Vec::new();
        for req in &reqs {
            // Check idempotency (HP4): skip if param already exists
            if f.params.iter().any(|p| p.name == req.param_name) {
                continue;
            }

            f.params.push(Param {
                name: req.param_name.clone(),
                name_span: Span::new(0, 0),
                ty: req.param_type.clone(),
                is_take: false,
                is_mutate: false,
                default: None,
            });

            if let Some(alias) = &req.alias {
                aliases.push((alias.clone(), req.param_name.clone()));
            }
        }

        // Clear context clauses — they're now expressed as params
        f.context_clauses.clear();

        // Prepend `const <alias> = <param_name>` bindings. Insert in reverse so
        // the declared order is preserved once each lands at index 0.
        for (alias, param_name) in aliases.into_iter().rev() {
            let init = Expr {
                id: pass.fresh_id(),
                kind: ExprKind::Ident(param_name),
                span: Span::new(0, 0),
            };
            let alias_stmt = Stmt {
                id: pass.fresh_id(),
                kind: StmtKind::Const {
                    name: alias,
                    name_span: Span::new(0, 0),
                    ty: None,
                    init,
                },
                span: Span::new(0, 0),
            };
            f.body.insert(0, alias_stmt);
        }
    }

    // Phase 5-6: Rewrite body (call sites and using blocks)
    let caller_name = qname.to_string();
    rewrite_stmts(pass, &caller_name, &mut f.body);
}

fn rewrite_stmts(pass: &mut HiddenParamPass, caller: &str, stmts: &mut [Stmt]) {
    for stmt in stmts.iter_mut() {
        rewrite_stmt(pass, caller, stmt);
    }
}

fn rewrite_stmt(pass: &mut HiddenParamPass, caller: &str, stmt: &mut Stmt) {
    match &mut stmt.kind {
        StmtKind::Expr(e) => rewrite_expr(pass, caller, e),
        StmtKind::Mut { init, .. } | StmtKind::Const { init, .. } => {
            // CC10: a closure bound to a name is storable — it can escape the
            // enclosing pool scope. Rewrite its body under the storable rule so
            // contexts resolve from the closure's own params, not ambient ones.
            if matches!(&init.kind, ExprKind::Closure { .. }) {
                rewrite_storable_closure(pass, caller, init);
            } else {
                rewrite_expr(pass, caller, init);
            }
        }
        StmtKind::MutTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
            rewrite_expr(pass, caller, init);
        }
        StmtKind::Assign { target, value } => {
            rewrite_expr(pass, caller, target);
            rewrite_expr(pass, caller, value);
        }
        StmtKind::Return(Some(e)) => rewrite_expr(pass, caller, e),
        StmtKind::Return(None) => {}
        StmtKind::Break {
            value: Some(v), ..
        } => rewrite_expr(pass, caller, v),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::While { cond, body } => {
            rewrite_expr(pass, caller, cond);
            rewrite_stmts(pass, caller, body);
        }
        StmtKind::WhileLet { expr, body, .. } => {
            rewrite_expr(pass, caller, expr);
            rewrite_stmts(pass, caller, body);
        }
        StmtKind::Loop { body, .. } => rewrite_stmts(pass, caller, body),
        StmtKind::For { iter, body, binding, .. } => {
            rewrite_expr(pass, caller, iter);
            // Record the loop variable's element type so the handle-deref
            // rewrite fires on it inside the body (the checker leaves it an
            // inference var). Save/restore to respect shadowing across loops.
            let saved = match binding {
                rask_ast::stmt::ForBinding::Single(name) => {
                    let prev = pass.loop_var_types.remove(name);
                    if let Some(elem) = pass.iterable_elem_type(iter) {
                        pass.loop_var_types.insert(name.clone(), elem);
                    }
                    Some((name.clone(), prev))
                }
                _ => None,
            };
            rewrite_stmts(pass, caller, body);
            if let Some((name, prev)) = saved {
                match prev {
                    Some(t) => { pass.loop_var_types.insert(name, t); }
                    None => { pass.loop_var_types.remove(&name); }
                }
            }
        }
        StmtKind::Ensure {
            body,
            else_handler,
        } => {
            rewrite_stmts(pass, caller, body);
            if let Some((_, handler)) = else_handler {
                rewrite_stmts(pass, caller, handler);
            }
        }
        StmtKind::Comptime(body) => rewrite_stmts(pass, caller, body),
        StmtKind::ComptimeFor { body, .. } => rewrite_stmts(pass, caller, body),
        StmtKind::Discard { .. } => {}
    }
}

fn rewrite_expr(pass: &mut HiddenParamPass, caller: &str, expr: &mut Expr) {
    match &mut expr.kind {
        // Phase 5 (CALL1-CALL6): Insert hidden args at call sites
        ExprKind::Call { func, args } => {
            let span = expr.span;
            let key = pass.callee_key(expr.id).or_else(|| extract_callee_name(func));
            rewrite_expr(pass, caller, func);
            for arg in args.iter_mut() {
                rewrite_expr(pass, caller, &mut arg.expr);
            }
            // CALL6: key the callee by the recorded dispatch target, so the
            // hidden args match the callee's rewritten signature.
            if let Some(key) = key {
                insert_hidden_args(pass, caller, &key, args, span);
            }
        }

        ExprKind::MethodCall { object, args, .. } => {
            // CALL6: a method callee (`recv.m()` / `self.m()` / `T.m()`) keys by
            // the recorded `Type.method` dispatch target — same as its declaration
            // — so its context requirement threads in exactly like a free call's.
            let span = expr.span;
            let key = pass.callee_key(expr.id);
            rewrite_expr(pass, caller, object);
            for arg in args.iter_mut() {
                rewrite_expr(pass, caller, &mut arg.expr);
            }
            if let Some(key) = key {
                insert_hidden_args(pass, caller, &key, args, span);
            }
        }

        // Phase 6 (BLK1-BLK4): Desugar `using` blocks
        ExprKind::UsingBlock { name, args, body } => {
            if name == "Multitasking" || name == "multitasking" {
                // Keep UsingBlock intact — MIR lowering emits
                // rask_runtime_init/rask_runtime_shutdown directly.
                rewrite_stmts(
                    pass,
                    caller,
                    match &mut expr.kind {
                        ExprKind::UsingBlock { body, .. } => body,
                        _ => unreachable!(),
                    },
                );
            } else if name == "ThreadPool" {
                // ThreadPool blocks keep their structure for now
                rewrite_stmts(
                    pass,
                    caller,
                    match &mut expr.kind {
                        ExprKind::UsingBlock { body, .. } => body,
                        _ => unreachable!(),
                    },
                );
            } else {
                // Unknown using block — just recurse
                for arg in args.iter_mut() {
                    rewrite_expr(pass, caller, &mut arg.expr);
                }
                rewrite_stmts(pass, caller, body);
            }
        }

        // Recurse into all other expression kinds
        ExprKind::Binary { left, right, .. } => {
            rewrite_expr(pass, caller, left);
            rewrite_expr(pass, caller, right);
        }
        ExprKind::Unary { operand, .. } => rewrite_expr(pass, caller, operand),
        ExprKind::Field { object, .. } => {
            // mem.context/CC1: `h.field` on a `Handle<T>` lowers to `pool[h].field`,
            // resolving the Pool<T> from scope (CC4) exactly like a hidden argument.
            // A loop variable's type isn't recorded on its node, so fall back to
            // the tracked for-loop element type when the object is a bare ident.
            let elem = match &object.kind {
                ExprKind::Ident(name) => pass.ident_handle_elem(name, object.id),
                _ => pass.node_handle_elem(object.id),
            };
            rewrite_expr(pass, caller, object);
            if let Some(elem) = elem {
                wrap_handle_deref(pass, caller, object, &elem, expr.span);
            }
        }
        ExprKind::OptionalField { object, .. } => {
            rewrite_expr(pass, caller, object);
        }
        ExprKind::DynamicField { object, field_expr } => {
            rewrite_expr(pass, caller, object);
            rewrite_expr(pass, caller, field_expr);
        }
        ExprKind::Index { object, index } => {
            rewrite_expr(pass, caller, object);
            rewrite_expr(pass, caller, index);
        }
        ExprKind::Block(stmts) => rewrite_stmts(pass, caller, stmts),
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(pass, caller, cond);
            rewrite_expr(pass, caller, then_branch);
            if let Some(e) = else_branch {
                rewrite_expr(pass, caller, e);
            }
        }
        ExprKind::IfLet {
            expr,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(pass, caller, expr);
            rewrite_expr(pass, caller, then_branch);
            if let Some(e) = else_branch {
                rewrite_expr(pass, caller, e);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            rewrite_expr(pass, caller, scrutinee);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    rewrite_expr(pass, caller, g);
                }
                rewrite_expr(pass, caller, &mut arm.body);
            }
        }
        ExprKind::Try { expr: e, ref mut else_clause } => {
            rewrite_expr(pass, caller, e);
            if let Some(ec) = else_clause {
                rewrite_expr(pass, caller, &mut ec.body);
            }
        }
        ExprKind::IsPresent { expr: e, .. } => {
            rewrite_expr(pass, caller, e);
        }
        ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } | ExprKind::Convert { expr: e, .. } => {
            rewrite_expr(pass, caller, e);
        }
        ExprKind::GuardPattern {
            expr, else_branch, ..
        } => {
            rewrite_expr(pass, caller, expr);
            rewrite_expr(pass, caller, else_branch);
        }
        ExprKind::IsPattern { expr, .. } => rewrite_expr(pass, caller, expr),
        ExprKind::NullCoalesce { value, default } => {
            rewrite_expr(pass, caller, value);
            rewrite_expr(pass, caller, default);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_expr(pass, caller, s);
            }
            if let Some(e) = end {
                rewrite_expr(pass, caller, e);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                rewrite_expr(pass, caller, &mut f.value);
            }
            if let Some(s) = spread {
                rewrite_expr(pass, caller, s);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                rewrite_expr(pass, caller, e);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            rewrite_expr(pass, caller, value);
            rewrite_expr(pass, caller, count);
        }
        ExprKind::WithAs { bindings, body } => {
            for binding in bindings {
                rewrite_expr(pass, caller, &mut binding.source);
            }
            rewrite_stmts(pass, caller, body);
        }
        ExprKind::Closure { body, .. } => {
            // CC9: Expression-scoped closures inherit context.
            // The closure body is rewritten with the same caller context,
            // so hidden params from the enclosing scope are accessible.
            rewrite_expr(pass, caller, body);
        }
        ExprKind::Spawn { body }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Loop { body, .. } => {
            rewrite_stmts(pass, caller, body);
        }
        ExprKind::Assert { condition, message }
        | ExprKind::Check { condition, message } => {
            rewrite_expr(pass, caller, condition);
            if let Some(m) = message {
                rewrite_expr(pass, caller, m);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &mut arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        rewrite_expr(pass, caller, channel);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        rewrite_expr(pass, caller, channel);
                        rewrite_expr(pass, caller, value);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                rewrite_expr(pass, caller, &mut arm.body);
            }
        }
        // Leaves
        ExprKind::Int(_, _)
        | ExprKind::Float(_, _)
        | ExprKind::String(_) | ExprKind::StringInterp(_)
        | ExprKind::Char(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::None
        | ExprKind::Ident(_) => {}
    }
}

/// CC10: rewrite a storable closure's body. Contexts inside resolve only from
/// the closure's own pool-typed parameters — it may outlive the enclosing pool
/// scope, so it can't capture ambient contexts. A needed context with no
/// matching param is a CC10 error (raised in `resolve_arg_kind`).
fn rewrite_storable_closure(pass: &mut HiddenParamPass, caller: &str, init: &mut Expr) {
    let params: Vec<(String, super::Type)> = match &init.kind {
        ExprKind::Closure { params, .. } => params
            .iter()
            .filter_map(|p| Some((p.name.clone(), pass.parse_ty(p.ty.as_ref()?)?)))
            .collect(),
        _ => Vec::new(),
    };
    // Save/restore so nested closures don't clobber the outer state.
    let prev = pass.storable_closure.replace(params);
    if let ExprKind::Closure { body, .. } = &mut init.kind {
        rewrite_expr(pass, caller, body);
    }
    pass.storable_closure = prev;
}

/// mem.context/CC1: rewrite the handle `object` of an `h.field` access into
/// `pool[h]`, resolving the backing `Pool<T>` from the caller's scope (CC4).
/// Reuses `resolve_arg_kind` so the pool expression, ambiguity (CC8) and
/// storable-closure (CC10) handling match hidden-argument resolution exactly.
fn wrap_handle_deref(
    pass: &mut HiddenParamPass,
    caller: &str,
    object: &mut Box<Expr>,
    elem: &super::Type,
    span: Span,
) {
    let elem_name = match pass.type_head_name(elem) {
        Some(n) => n,
        None => return,
    };
    let pool_ty = pass.canonical_type(&super::Type::UnresolvedGeneric {
        name: "Pool".to_string(),
        args: vec![rask_types::GenericArg::Type(Box::new(elem.clone()))],
    });
    let req = super::ContextReq {
        param_name: format!("__ctx_pool_{}", elem_name),
        param_type: format!("&{}", pass.type_to_source(&pool_ty)),
        clause_type: pool_ty,
        alias: None,
    };
    let pool_kind = resolve_arg_kind(pass, caller, &req, span);
    let pool_expr = Expr {
        id: pass.fresh_id(),
        kind: pool_kind,
        span,
    };
    let placeholder = Expr {
        id: pass.fresh_id(),
        kind: ExprKind::Null,
        span,
    };
    let handle = std::mem::replace(object, Box::new(placeholder));
    *object = Box::new(Expr {
        id: pass.fresh_id(),
        kind: ExprKind::Index {
            object: Box::new(pool_expr),
            index: handle,
        },
        span,
    });
}

/// CALL3: append the hidden context arguments a callee requires to `args`,
/// resolving each from the caller's scope (CC4). Shared by free calls and
/// method calls — both identify their callee by the recorded dispatch target.
fn insert_hidden_args(
    pass: &mut HiddenParamPass,
    caller: &str,
    callee_key: &str,
    args: &mut Vec<CallArg>,
    span: Span,
) {
    let reqs = match pass.func_contexts.get(callee_key).cloned() {
        Some(r) => r,
        None => return,
    };
    for req in &reqs {
        // HP4: don't re-append a hidden arg that's already present.
        let already_has = args
            .iter()
            .any(|a| matches!(&a.expr.kind, ExprKind::Ident(name) if name == &req.param_name));
        if already_has {
            continue;
        }

        let kind = resolve_arg_kind(pass, caller, req, span);
        args.push(CallArg {
            name: None,
            mode: ArgMode::Default,
            expr: Expr {
                id: pass.fresh_id(),
                kind,
                span,
            },
        });
    }
}

/// CC4: Build the argument expression for a context requirement, resolving from
/// the caller's scope. A `self.field` pool becomes a real field-access
/// expression (`self.players`), not an ident whose name happens to contain a
/// dot — MIR can't resolve the latter. Locals/params/hidden-params are plain
/// idents. Falls back to the hidden param name when scope resolution finds
/// nothing (propagation should have added it to the signature).
fn resolve_arg_kind(
    pass: &mut HiddenParamPass,
    caller: &str,
    req: &super::ContextReq,
    call_span: Span,
) -> ExprKind {
    // CC10: a storable closure resolves contexts only from its own params; it
    // cannot inherit the enclosing function's ambient pools.
    if let Some(params) = pass.storable_closure.clone() {
        if let Some((name, _)) = params.iter().find(|(_, ty)| ty == &req.clause_type) {
            return ExprKind::Ident(name.clone());
        }
        let diag = cc10_needs_explicit(pass, &req.clause_type, call_span);
        pass.diagnostics.push(diag);
        return ExprKind::Ident(req.param_name.clone());
    }

    if caller.is_empty() {
        return ExprKind::Ident(req.param_name.clone());
    }

    match resolve_context_in_scope(pass, caller, &req.clause_type) {
        ResolveResult::Resolved(pool) => match pool.source {
            PoolSource::SelfField => {
                // var_name is "self.<field>"; rebuild as a Field access so the
                // backend sees a struct field read, not an unknown variable.
                let field = pool
                    .var_name
                    .strip_prefix("self.")
                    .unwrap_or(&pool.var_name)
                    .to_string();
                let object = Expr {
                    id: pass.fresh_id(),
                    kind: ExprKind::Ident("self".to_string()),
                    span: Span::new(0, 0),
                };
                ExprKind::Field {
                    object: Box::new(object),
                    field,
                }
            }
            _ => ExprKind::Ident(pool.var_name),
        },
        // CC8: two-plus pools of the same type at the same priority — the
        // compiler can't pick one. Report it here (this pass owns context
        // resolution) instead of letting the fallback ident fail later as an
        // unresolved-variable error in MIR lowering.
        ResolveResult::Ambiguous(candidates) => {
            let diag = cc8_ambiguous(pass, &req.clause_type, &candidates, call_span);
            pass.diagnostics.push(diag);
            ExprKind::Ident(req.param_name.clone())
        }
        ResolveResult::NotFound => ExprKind::Ident(req.param_name.clone()),
    }
}

/// Build the CC8 "ambiguous context" diagnostic: the call needs a pool the
/// caller has more than one of at the same priority.
fn cc8_ambiguous(
    pass: &HiddenParamPass,
    clause_type: &super::Type,
    candidates: &[super::ScopePool],
    call_span: Span,
) -> rask_diagnostics::Diagnostic {
    use rask_diagnostics::Diagnostic;
    let pool = pass.type_to_source(clause_type);
    let names: Vec<String> = candidates.iter().map(|c| c.var_name.clone()).collect();
    Diagnostic::error(format!("ambiguous context — multiple {pool} in scope"))
        .with_code("mem.context/CC8")
        .with_primary(call_span, format!("which pool satisfies {pool}?"))
        .with_why(format!(
            "{} are both in scope and either could satisfy the {pool} context.",
            names.join(" and ")
        ))
        .with_fix(format!(
            "Pass the pool explicitly as a regular parameter, or index it \
             directly (e.g. `{}[h]`) instead of relying on auto-resolution.",
            names.first().map(String::as_str).unwrap_or("pool")
        ))
}

/// Build the CC10 "storable closure can't auto-resolve" diagnostic: a named
/// closure needs a pool context it doesn't take as a parameter.
fn cc10_needs_explicit(
    pass: &HiddenParamPass,
    clause_type: &super::Type,
    call_span: Span,
) -> rask_diagnostics::Diagnostic {
    use rask_diagnostics::Diagnostic;
    let pool = pass.type_to_source(clause_type);
    Diagnostic::error(format!(
        "storable closure cannot auto-resolve {pool} context"
    ))
    .with_code("mem.context/CC10")
    .with_primary(call_span, format!("needs {pool}, but the closure can't inherit it"))
    .with_why(
        "A closure bound to a name can outlive the scope that owns the pool, so \
         it can't capture an ambient context the way an inline callback does."
            .to_string(),
    )
    .with_fix(format!(
        "Take the pool as an explicit closure parameter, e.g. \
         `|pool: {pool}, h| ...`, and pass it in at each call."
    ))
}
