// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 2-3: Call graph construction and context propagation (CC5).

use std::collections::HashSet;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use super::{extract_callee_name, HiddenParamPass};

/// Phase 2: Build the call graph from function bodies.
pub fn build_call_graph(pass: &mut HiddenParamPass, decls: &[Decl]) {
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => {
                let callees = collect_callees_from_body(&f.body);
                if !callees.is_empty() {
                    pass.call_graph.insert(f.name.clone(), callees);
                }
            }
            DeclKind::Struct(s) => {
                for method in &s.methods {
                    let qname = format!("{}.{}", s.name, method.name);
                    let callees = collect_callees_from_body(&method.body);
                    if !callees.is_empty() {
                        pass.call_graph.insert(qname, callees);
                    }
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    let qname = format!("{}.{}", e.name, method.name);
                    let callees = collect_callees_from_body(&method.body);
                    if !callees.is_empty() {
                        pass.call_graph.insert(qname, callees);
                    }
                }
            }
            DeclKind::Impl(i) => {
                for method in &i.methods {
                    let qname = format!("{}.{}", i.target_ty, method.name);
                    let callees = collect_callees_from_body(&method.body);
                    if !callees.is_empty() {
                        pass.call_graph.insert(qname, callees);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Phase 3: Fixed-point propagation of context requirements (CC5).
/// If a function calls a context-needing function and can't resolve
/// the context from its own params/using clauses, it also needs it.
pub fn propagate(pass: &mut HiddenParamPass) {
    loop {
        let mut changed = false;

        let graph_snapshot: Vec<(String, HashSet<String>)> = pass
            .call_graph
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (caller, callees) in &graph_snapshot {
            for callee in callees {
                let callee_reqs = match pass.func_contexts.get(callee) {
                    Some(r) => r.clone(),
                    None => continue,
                };

                for req in &callee_reqs {
                    // Does caller already have this context?
                    let caller_has = pass
                        .func_contexts
                        .get(caller)
                        .map(|reqs| reqs.iter().any(|r| r.clause_type == req.clause_type))
                        .unwrap_or(false);

                    if caller_has {
                        continue;
                    }

                    // CC4: Check if caller can resolve from locals/params/self
                    if can_resolve_locally(pass, caller, &req.clause_type) {
                        continue;
                    }

                    // Public functions must declare contexts explicitly (CC6/PUB1)
                    if pass.public_funcs.contains(caller) {
                        continue;
                    }

                    // Private function: propagate context requirement (CC5/PUB2)
                    let new_req = req.clone();
                    pass.func_contexts
                        .entry(caller.clone())
                        .or_default()
                        .push(new_req);
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }
    }
}

/// CC4: Check if a function can resolve a context type from its own scope
/// (local variables, parameters, self fields) without needing propagation.
fn can_resolve_locally(pass: &HiddenParamPass, func_name: &str, clause_type: &str) -> bool {
    if let Some(info) = pass.func_info.get(func_name) {
        // Check parameters
        for (_, ty) in &info.params {
            if ty == clause_type || ty == &format!("&{}", clause_type) {
                return true;
            }
        }

        // Check locals
        for (_, ty) in &info.locals {
            if ty == clause_type {
                return true;
            }
        }

        // Check self fields
        for (_, ty) in &info.self_fields {
            if ty == clause_type {
                return true;
            }
        }
    }
    false
}

// ── Call graph collection helpers ───────────────────────────────────────

fn collect_callees_from_body(stmts: &[Stmt]) -> HashSet<String> {
    let mut callees = HashSet::new();
    for stmt in stmts {
        collect_callees_from_stmt(stmt, &mut callees);
    }
    callees
}

fn collect_callees_from_stmt(stmt: &Stmt, callees: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expr(e) => collect_callees_from_expr(e, callees),
        StmtKind::Mut { init, .. }
        | StmtKind::Const { init, .. }
        | StmtKind::MutTuple { init, .. }
        | StmtKind::ConstTuple { init, .. } => {
            collect_callees_from_expr(init, callees);
        }
        StmtKind::Assign { target, value } => {
            collect_callees_from_expr(target, callees);
            collect_callees_from_expr(value, callees);
        }
        StmtKind::Return(Some(e)) => collect_callees_from_expr(e, callees),
        StmtKind::Return(None) => {}
        StmtKind::Break {
            value: Some(v), ..
        } => collect_callees_from_expr(v, callees),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::While { cond, body } => {
            collect_callees_from_expr(cond, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::WhileLet { expr, body, .. } => {
            collect_callees_from_expr(expr, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::Loop { body, .. } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::For { iter, body, .. } => {
            collect_callees_from_expr(iter, callees);
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::Ensure {
            body,
            else_handler,
        } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
            if let Some((_, handler)) = else_handler {
                for s in handler {
                    collect_callees_from_stmt(s, callees);
                }
            }
        }
        StmtKind::Comptime(body) => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::ComptimeFor { body, .. } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        StmtKind::Discard { .. } => {}
    }
}

fn collect_callees_from_expr(expr: &Expr, callees: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Call { func, args } => {
            if let Some(name) = extract_callee_name(func) {
                callees.insert(name);
            }
            collect_callees_from_expr(func, callees);
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
        }
        ExprKind::MethodCall {
            object, args, method, ..
        } => {
            callees.insert(method.clone());
            collect_callees_from_expr(object, callees);
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_callees_from_expr(left, callees);
            collect_callees_from_expr(right, callees);
        }
        ExprKind::Unary { operand, .. } => collect_callees_from_expr(operand, callees),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            collect_callees_from_expr(object, callees);
        }
        ExprKind::DynamicField { object, field_expr } => {
            collect_callees_from_expr(object, callees);
            collect_callees_from_expr(field_expr, callees);
        }
        ExprKind::Index { object, index } => {
            collect_callees_from_expr(object, callees);
            collect_callees_from_expr(index, callees);
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_callees_from_expr(cond, callees);
            collect_callees_from_expr(then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::IfLet {
            expr,
            then_branch,
            else_branch,
            ..
        } => {
            collect_callees_from_expr(expr, callees);
            collect_callees_from_expr(then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_callees_from_expr(scrutinee, callees);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_callees_from_expr(g, callees);
                }
                collect_callees_from_expr(&arm.body, callees);
            }
        }
        ExprKind::Try { expr: e, ref else_clause } => {
            collect_callees_from_expr(e, callees);
            if let Some(ec) = else_clause {
                collect_callees_from_expr(&ec.body, callees);
            }
        }
        ExprKind::IsPresent { expr: e, .. } => {
            collect_callees_from_expr(e, callees);
        }
        ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
            collect_callees_from_expr(e, callees);
        }
        ExprKind::GuardPattern {
            expr, else_branch, ..
        } => {
            collect_callees_from_expr(expr, callees);
            collect_callees_from_expr(else_branch, callees);
        }
        ExprKind::IsPattern { expr, .. } => collect_callees_from_expr(expr, callees),
        ExprKind::NullCoalesce { value, default } => {
            collect_callees_from_expr(value, callees);
            collect_callees_from_expr(default, callees);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_callees_from_expr(s, callees);
            }
            if let Some(e) = end {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                collect_callees_from_expr(&f.value, callees);
            }
            if let Some(s) = spread {
                collect_callees_from_expr(s, callees);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                collect_callees_from_expr(e, callees);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            collect_callees_from_expr(value, callees);
            collect_callees_from_expr(count, callees);
        }
        ExprKind::WithAs { bindings, body } => {
            for binding in bindings {
                collect_callees_from_expr(&binding.source, callees);
            }
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::Closure { body, .. } => collect_callees_from_expr(body, callees),
        ExprKind::Spawn { body }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Loop { body, .. } => {
            for s in body {
                collect_callees_from_stmt(s, callees);
            }
        }
        ExprKind::Assert { condition, message }
        | ExprKind::Check { condition, message } => {
            collect_callees_from_expr(condition, callees);
            if let Some(m) = message {
                collect_callees_from_expr(m, callees);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        collect_callees_from_expr(channel, callees);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        collect_callees_from_expr(channel, callees);
                        collect_callees_from_expr(value, callees);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                collect_callees_from_expr(&arm.body, callees);
            }
        }
        ExprKind::UsingBlock { args, body, .. } => {
            for arg in args {
                collect_callees_from_expr(&arg.expr, callees);
            }
            for s in body {
                collect_callees_from_stmt(s, callees);
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
