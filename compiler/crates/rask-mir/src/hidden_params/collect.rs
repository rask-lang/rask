// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Phase 1: Collect context requirements from `using` clauses.

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::stmt::StmtKind;

use super::{context_clause_to_req, ContextReq, FuncInfo, HiddenParamPass};

impl<'a> HiddenParamPass<'a> {
    /// Collect explicit context requirements from all function declarations.
    pub fn collect_contexts(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(f) => {
                    self.collect_fn_context(&f.name, f, None);
                }
                DeclKind::Struct(s) => {
                    for method in &s.methods {
                        let qname = format!("{}.{}", s.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&s.name));
                    }
                }
                DeclKind::Enum(e) => {
                    for method in &e.methods {
                        let qname = format!("{}.{}", e.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&e.name));
                    }
                }
                DeclKind::Impl(i) => {
                    for method in &i.methods {
                        let qname = format!("{}.{}", i.target_ty, method.name);
                        self.collect_fn_context(&qname, method, Some(&i.target_ty));
                    }
                }
                DeclKind::Trait(t) => {
                    for method in &t.methods {
                        let qname = format!("{}.{}", t.name, method.name);
                        self.collect_fn_context(&qname, method, Some(&t.name));
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_fn_context(&mut self, qname: &str, f: &FnDecl, self_type: Option<&str>) {
        if f.is_pub {
            self.public_funcs.insert(qname.to_string());
        }

        // Collect parameter info
        let params: Vec<(String, String)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty.clone()))
            .collect();

        // Collect local variable info from body
        let locals = collect_locals_from_body(&f.body);

        // Collect self fields (if this is a method on a struct)
        let self_fields = if let Some(ty_name) = self_type {
            self.struct_fields
                .get(ty_name)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // Collect explicit context requirements
        let reqs: Vec<ContextReq> = f
            .context_clauses
            .iter()
            .map(|cc| context_clause_to_req(cc))
            .collect();

        if !reqs.is_empty() {
            self.func_contexts
                .insert(qname.to_string(), reqs.clone());
        }

        self.func_info.insert(
            qname.to_string(),
            FuncInfo {
                reqs,
                is_public: f.is_pub,
                params,
                self_fields,
                locals,
            },
        );
    }
}

/// Collect local variable declarations from a function body.
/// Returns (name, type_string) pairs.
fn collect_locals_from_body(stmts: &[rask_ast::stmt::Stmt]) -> Vec<(String, String)> {
    let mut locals = Vec::new();
    for stmt in stmts {
        collect_locals_from_stmt(stmt, &mut locals);
    }
    locals
}

fn collect_locals_from_stmt(
    stmt: &rask_ast::stmt::Stmt,
    locals: &mut Vec<(String, String)>,
) {
    match &stmt.kind {
        StmtKind::Let { name, ty, init, .. } | StmtKind::Const { name, ty, init, .. } => {
            // If explicit type annotation, use it
            if let Some(t) = ty {
                locals.push((name.clone(), t.clone()));
            } else {
                // Try to infer Pool type from init expression
                if let Some(pool_ty) = infer_pool_type_from_expr(init) {
                    locals.push((name.clone(), pool_ty));
                }
            }
        }
        StmtKind::While { body, .. }
        | StmtKind::WhileLet { body, .. }
        | StmtKind::Loop { body, .. }
        | StmtKind::For { body, .. }
        | StmtKind::Comptime(body)
        | StmtKind::ComptimeFor { body, .. } => {
            for s in body {
                collect_locals_from_stmt(s, locals);
            }
        }
        StmtKind::Ensure { body, .. } => {
            for s in body {
                collect_locals_from_stmt(s, locals);
            }
        }
        _ => {}
    }
}

/// Try to infer a Pool<T> type from an initializer expression.
/// Recognizes patterns like `Pool.new()`, `Pool::<Player>.new()`.
fn infer_pool_type_from_expr(expr: &rask_ast::expr::Expr) -> Option<String> {
    use rask_ast::expr::ExprKind;
    match &expr.kind {
        // Pool.new() — look for Type.method pattern
        ExprKind::Call { func, .. } => {
            if let ExprKind::Field { object, field } = &func.kind {
                if field == "new" {
                    if let ExprKind::Ident(name) = &object.kind {
                        if name == "Pool" {
                            // Bare Pool.new() — type comes from context
                            return Some("Pool<_>".to_string());
                        }
                        if name.starts_with("Pool") && name.contains('<') {
                            return Some(name.clone());
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Check if any parameter in a function is a Handle<T> type.
pub(crate) fn find_handle_params(f: &FnDecl) -> Vec<String> {
    f.params
        .iter()
        .filter(|p| super::is_handle_type(&p.ty))
        .map(|p| p.ty.clone())
        .collect()
}
