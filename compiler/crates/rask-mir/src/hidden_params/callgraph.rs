// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 2-3: Call graph construction and context propagation (CC5).

use std::collections::HashSet;

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_types::Type;

use super::{extract_callee_name, HiddenParamPass};

/// Phase 2: Build the call graph from function bodies.
///
/// Collected in two phases so the callee keying (which reads the recorded
/// dispatch targets on `pass`) borrows `pass` immutably while gathering, then
/// mutates `pass.call_graph` once gathering is done.
pub fn build_call_graph(pass: &mut HiddenParamPass, decls: &[Decl]) {
    let mut entries: Vec<(String, HashSet<String>)> = Vec::new();
    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => {
                let callees = collect_callees_from_body(pass, &f.body);
                if !callees.is_empty() {
                    entries.push((f.name.clone(), callees));
                }
            }
            DeclKind::Struct(s) => {
                for method in &s.methods {
                    let qname = format!("{}.{}", s.name, method.name);
                    let callees = collect_callees_from_body(pass, &method.body);
                    if !callees.is_empty() {
                        entries.push((qname, callees));
                    }
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    let qname = format!("{}.{}", e.name, method.name);
                    let callees = collect_callees_from_body(pass, &method.body);
                    if !callees.is_empty() {
                        entries.push((qname, callees));
                    }
                }
            }
            DeclKind::Impl(i) => {
                for method in &i.methods {
                    let qname = format!("{}.{}", i.target_ty, method.name);
                    let callees = collect_callees_from_body(pass, &method.body);
                    if !callees.is_empty() {
                        entries.push((qname, callees));
                    }
                }
            }
            _ => {}
        }
    }
    for (qname, callees) in entries {
        pass.call_graph.insert(qname, callees);
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
pub(crate) fn can_resolve_locally(pass: &HiddenParamPass, func_name: &str, clause_type: &Type) -> bool {
    if let Some(info) = pass.func_info.get(func_name) {
        let candidates = info
            .params
            .iter()
            .chain(&info.locals)
            .chain(&info.self_fields);
        candidates.map(|(_, ty)| ty).any(|ty| ty == clause_type)
    } else {
        false
    }
}

// ── Call graph collection helpers ───────────────────────────────────────

fn collect_callees_from_body(pass: &HiddenParamPass, stmts: &[Stmt]) -> HashSet<String> {
    let mut callees = HashSet::new();
    for stmt in stmts {
        collect_callees_from_stmt(pass, stmt, &mut callees);
    }
    callees
}

fn collect_callees_from_stmt(pass: &HiddenParamPass, stmt: &Stmt, callees: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expr(e) => collect_callees_from_expr(pass, e, callees),
        StmtKind::Mut { init, .. }
        | StmtKind::Let { init, .. }
        | StmtKind::MutTuple { init, .. }
        | StmtKind::LetTuple { init, .. }
        | StmtKind::LetStruct { init, .. } => {
            collect_callees_from_expr(pass, init, callees);
        }
        StmtKind::Assign { target, value, .. } => {
            collect_callees_from_expr(pass, target, callees);
            collect_callees_from_expr(pass, value, callees);
        }
        StmtKind::Return(Some(e)) => collect_callees_from_expr(pass, e, callees),
        StmtKind::Return(None) => {}
        StmtKind::Break {
            value: Some(v), ..
        } => collect_callees_from_expr(pass, v, callees),
        StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
        StmtKind::While { cond, body, .. } => {
            collect_callees_from_expr(pass, cond, callees);
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::WhileLet { expr, body, .. } => {
            collect_callees_from_expr(pass, expr, callees);
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::Loop { body, .. } => {
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::For { iter, body, .. } => {
            collect_callees_from_expr(pass, iter, callees);
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::Ensure {
            body,
            else_handler,
        } => {
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
            if let Some((_, handler)) = else_handler {
                for s in handler {
                    collect_callees_from_stmt(pass, s, callees);
                }
            }
        }
        StmtKind::Comptime(body) => {
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::ComptimeFor { body, .. } => {
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        StmtKind::Discard { .. } => {}
    }
}

fn collect_callees_from_expr(pass: &HiddenParamPass, expr: &Expr, callees: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Call { func, args } => {
            // CALL6: key by the recorded target; fall back to name extraction
            // for builtins / calls the checker didn't record.
            if let Some(key) = pass.callee_key(expr.id).or_else(|| extract_callee_name(func)) {
                callees.insert(key);
            }
            collect_callees_from_expr(pass, func, callees);
            for arg in args {
                collect_callees_from_expr(pass, &arg.expr, callees);
            }
        }
        ExprKind::MethodCall {
            object, args, method, ..
        } => {
            // CALL6: key by `Type.method` from the recorded target instead of the
            // bare method name, so a method callee matches its declaration.
            if let Some(key) = pass.callee_key(expr.id) {
                callees.insert(key);
            } else {
                callees.insert(method.clone());
            }
            collect_callees_from_expr(pass, object, callees);
            for arg in args {
                collect_callees_from_expr(pass, &arg.expr, callees);
            }
        }
        ExprKind::Binary { left, right, .. } => {
            collect_callees_from_expr(pass, left, callees);
            collect_callees_from_expr(pass, right, callees);
        }
        ExprKind::Unary { operand, .. } => collect_callees_from_expr(pass, operand, callees),
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            collect_callees_from_expr(pass, object, callees);
        }
        ExprKind::DynamicField { object, field_expr } => {
            collect_callees_from_expr(pass, object, callees);
            collect_callees_from_expr(pass, field_expr, callees);
        }
        ExprKind::Index { object, index } => {
            collect_callees_from_expr(pass, object, callees);
            collect_callees_from_expr(pass, index, callees);
        }
        ExprKind::Block(stmts) => {
            for s in stmts {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        ExprKind::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_callees_from_expr(pass, cond, callees);
            collect_callees_from_expr(pass, then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(pass, e, callees);
            }
        }
        ExprKind::IfLet {
            expr,
            then_branch,
            else_branch,
            ..
        } => {
            collect_callees_from_expr(pass, expr, callees);
            collect_callees_from_expr(pass, then_branch, callees);
            if let Some(e) = else_branch {
                collect_callees_from_expr(pass, e, callees);
            }
        }
        ExprKind::Match { scrutinee, arms } => {
            collect_callees_from_expr(pass, scrutinee, callees);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_callees_from_expr(pass, g, callees);
                }
                collect_callees_from_expr(pass, &arm.body, callees);
            }
        }
        ExprKind::Try { expr: e } | ExprKind::Take { place: e } => {
            collect_callees_from_expr(pass, e, callees);
        }
        ExprKind::Catch { value, ref clause } => {
            collect_callees_from_expr(pass, value, callees);
            collect_callees_from_expr(pass, &clause.body, callees);
        }
        ExprKind::IsPresent { expr: e, .. } => {
            collect_callees_from_expr(pass, e, callees);
        }
        ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } | ExprKind::Convert { expr: e, .. } => {
            collect_callees_from_expr(pass, e, callees);
        }
        ExprKind::GuardPattern {
            expr, else_branch, ..
        } => {
            collect_callees_from_expr(pass, expr, callees);
            collect_callees_from_expr(pass, else_branch, callees);
        }
        ExprKind::IsPattern { expr, .. } => collect_callees_from_expr(pass, expr, callees),
        ExprKind::NullCoalesce { value, default } => {
            collect_callees_from_expr(pass, value, callees);
            collect_callees_from_expr(pass, default, callees);
        }
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_callees_from_expr(pass, s, callees);
            }
            if let Some(e) = end {
                collect_callees_from_expr(pass, e, callees);
            }
        }
        ExprKind::StructLit { fields, spread, .. } => {
            for f in fields {
                collect_callees_from_expr(pass, &f.value, callees);
            }
            if let Some(s) = spread {
                collect_callees_from_expr(pass, s, callees);
            }
        }
        ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
            for e in elems {
                collect_callees_from_expr(pass, e, callees);
            }
        }
        ExprKind::ArrayRepeat { value, count } => {
            collect_callees_from_expr(pass, value, callees);
            collect_callees_from_expr(pass, count, callees);
        }
        ExprKind::WithAs { bindings, body } => {
            for binding in bindings {
                collect_callees_from_expr(pass, &binding.source, callees);
            }
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        ExprKind::Closure { body, .. } => collect_callees_from_expr(pass, body, callees),
        ExprKind::Spawn { body }
        | ExprKind::Unsafe { body }
        | ExprKind::Comptime { body }
        | ExprKind::BlockCall { body, .. }
        | ExprKind::Loop { body, .. } => {
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
            }
        }
        ExprKind::Assert { condition, message }
        | ExprKind::Check { condition, message } => {
            collect_callees_from_expr(pass, condition, callees);
            if let Some(m) = message {
                collect_callees_from_expr(pass, m, callees);
            }
        }
        ExprKind::Select { arms, .. } => {
            for arm in arms {
                match &arm.kind {
                    rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                        collect_callees_from_expr(pass, channel, callees);
                    }
                    rask_ast::expr::SelectArmKind::Send { channel, value } => {
                        collect_callees_from_expr(pass, channel, callees);
                        collect_callees_from_expr(pass, value, callees);
                    }
                    rask_ast::expr::SelectArmKind::Default => {}
                }
                collect_callees_from_expr(pass, &arm.body, callees);
            }
        }
        ExprKind::UsingBlock { args, body, .. } => {
            for arg in args {
                collect_callees_from_expr(pass, &arg.expr, callees);
            }
            for s in body {
                collect_callees_from_stmt(pass, s, callees);
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
