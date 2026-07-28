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
            rewrite_expr(pass, caller, init);
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
        StmtKind::For { iter, body, .. } => {
            rewrite_expr(pass, caller, iter);
            rewrite_stmts(pass, caller, body);
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
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
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

        let kind = resolve_arg_kind(pass, caller, req);
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
) -> ExprKind {
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
        // CC8 (ambiguous) and NotFound both fall back to the hidden param name;
        // ambiguity is reported as a diagnostic before this pass runs.
        ResolveResult::Ambiguous(_) | ResolveResult::NotFound => {
            ExprKind::Ident(req.param_name.clone())
        }
    }
}
