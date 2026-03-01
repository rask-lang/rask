// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function-level checking, return analysis, and @no_alloc enforcement.

use rask_ast::decl::FnDecl;
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;

use super::errors::TypeError;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    pub(super) fn check_fn(&mut self, f: &FnDecl) {
        // GC5: public functions must have full type annotations
        let unannotated_params: Vec<String> = f.params.iter()
            .filter(|p| p.name != "self" && p.ty.is_empty())
            .map(|p| p.name.clone())
            .collect();
        let missing_return = f.ret_ty.is_none()
            && f.is_pub
            && self.has_explicit_return(&f.body);
        if f.is_pub && (!unannotated_params.is_empty() || missing_return) {
            self.errors.push(TypeError::PublicMissingAnnotation {
                function_name: f.name.clone(),
                params: unannotated_params.clone(),
                missing_return,
                span: f.span,
            });
        }

        // GC1/GC2: Reuse pre-registered type vars for inferred params/return
        let inferred = self.inferred_fn_types.get(&f.name).cloned();

        let ret_ty = if let Some(t) = &f.ret_ty {
            parse_type_string(t, &self.types).unwrap_or(Type::Error)
        } else if let Some((_, ref ret_var)) = inferred {
            ret_var.clone()
        } else {
            Type::Unit
        };
        self.current_return_type = Some(ret_ty);

        // UF1: unsafe func body is implicitly unsafe
        let was_unsafe = self.in_unsafe;
        if f.is_unsafe {
            self.in_unsafe = true;
        }

        // GC9: Infer self mode for private methods with unmodified self
        let self_param = f.params.iter().find(|p| p.name == "self");
        let inferred_self_mutate = if let Some(sp) = self_param {
            if !sp.is_mutate && !sp.is_take && !f.is_pub {
                // Scan body for self mutations
                Self::body_writes_self(&f.body)
            } else {
                sp.is_mutate || sp.is_take
            }
        } else {
            false
        };

        // GC10: Public methods must declare self mode explicitly
        if let Some(sp) = self_param {
            if f.is_pub && !sp.is_mutate && !sp.is_take && Self::body_writes_self(&f.body) {
                // Public method writes to self but doesn't declare mutate
                self.errors.push(TypeError::MutateReadOnlyParam {
                    name: "self".to_string(),
                    span: f.span,
                });
            }
        }

        self.push_scope();
        for param in &f.params {
            if param.name == "self" {
                if let Some(self_ty) = &self.current_self_type {
                    if inferred_self_mutate || param.is_mutate || param.is_take {
                        self.define_local("self".to_string(), self_ty.clone());
                    } else {
                        self.define_local_read_only("self".to_string(), self_ty.clone());
                    }
                }
                continue;
            }
            // GC1: Look up pre-created type var for inferred params
            let ty = if param.ty.is_empty() {
                if let Some((ref pvars, _)) = inferred {
                    pvars.iter()
                        .find(|(name, _)| name == &param.name)
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or_else(|| self.ctx.fresh_var())
                } else {
                    self.ctx.fresh_var()
                }
            } else if let Ok(ty) = parse_type_string(&param.ty, &self.types) {
                ty
            } else {
                continue;
            };
            if param.is_mutate || param.is_take {
                self.define_local(param.name.clone(), ty);
            } else {
                self.define_local_read_only(param.name.clone(), ty);
            }
        }

        for stmt in &f.body {
            self.check_stmt(stmt);
        }

        let ret_ty = self.current_return_type.as_ref().unwrap();
        let resolved_ret_ty = self.ctx.apply(ret_ty);

        match &resolved_ret_ty {
            Type::Unit | Type::Never => {
                // No return needed
            }
            Type::Result { ok, err: _ } => {
                let resolved_ok = self.ctx.apply(ok);
                if matches!(resolved_ok, Type::Unit) {
                    // Function is () or E - implicit Ok(()) is valid
                } else {
                    // Function is T or E where T != () - require explicit return
                    if !self.has_explicit_return(&f.body) {
                        let end_span = Span::new(f.span.end.saturating_sub(1), f.span.end);
                        self.errors.push(TypeError::MissingReturn {
                            function_name: f.name.clone(),
                            expected_type: ret_ty.clone(),
                            span: end_span,
                        });
                    }
                }
            }
            _ => {
                // Non-Result, non-Unit - require explicit return
                if !self.has_explicit_return(&f.body) {
                    let end_span = Span::new(f.span.end.saturating_sub(1), f.span.end);
                    self.errors.push(TypeError::MissingReturn {
                        function_name: f.name.clone(),
                        expected_type: ret_ty.clone(),
                        span: end_span,
                    });
                }
            }
        }

        self.pop_scope();
        self.current_return_type = None;
        self.in_unsafe = was_unsafe;

        // @no_alloc enforcement: scan body for heap allocations
        if f.attrs.iter().any(|a| a == "no_alloc") {
            self.check_no_alloc(&f.name, &f.body);
        }
    }

    /// Check that a @no_alloc function body has no heap allocations.
    pub(super) fn check_no_alloc(&mut self, fn_name: &str, body: &[Stmt]) {
        for stmt in body {
            self.check_no_alloc_stmt(fn_name, stmt);
        }
    }

    pub(super) fn check_no_alloc_stmt(&mut self, fn_name: &str, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.check_no_alloc_expr(fn_name, e),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.check_no_alloc_expr(fn_name, init);
            }
            StmtKind::Assign { value, .. } => self.check_no_alloc_expr(fn_name, value),
            StmtKind::Return(Some(e)) => self.check_no_alloc_expr(fn_name, e),
            StmtKind::While { cond, body, .. } => {
                self.check_no_alloc_expr(fn_name, cond);
                self.check_no_alloc(fn_name, body);
            }
            StmtKind::For { iter, body, .. } => {
                self.check_no_alloc_expr(fn_name, iter);
                self.check_no_alloc(fn_name, body);
            }
            _ => {}
        }
    }

    pub(super) fn check_no_alloc_expr(&mut self, fn_name: &str, expr: &Expr) {
        match &expr.kind {
            // Vec.new(), Map.new(), string.new() — heap allocation
            ExprKind::MethodCall { object, method, args, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if matches!(name.as_str(), "Vec" | "Map" | "Pool" | "string")
                        && method == "new"
                    {
                        self.errors.push(TypeError::NoAllocViolation {
                            reason: format!("{}.new() allocates on the heap", name),
                            function_name: fn_name.to_string(),
                            span: expr.span,
                        });
                    }
                }
                self.check_no_alloc_expr(fn_name, object);
                for a in args { self.check_no_alloc_expr(fn_name, &a.expr); }
            }
            // format() — allocates a string
            ExprKind::Call { func, args } => {
                if let ExprKind::Ident(name) = &func.kind {
                    if name == "format" {
                        self.errors.push(TypeError::NoAllocViolation {
                            reason: "format() allocates a new string".to_string(),
                            function_name: fn_name.to_string(),
                            span: expr.span,
                        });
                    }
                }
                self.check_no_alloc_expr(fn_name, func);
                for a in args { self.check_no_alloc_expr(fn_name, &a.expr); }
            }
            // Recurse into subexpressions
            ExprKind::Binary { left, right, .. } => {
                self.check_no_alloc_expr(fn_name, left);
                self.check_no_alloc_expr(fn_name, right);
            }
            ExprKind::Unary { operand, .. } => {
                self.check_no_alloc_expr(fn_name, operand);
            }
            ExprKind::Field { object, .. } => {
                self.check_no_alloc_expr(fn_name, object);
            }
            ExprKind::Index { object, index } => {
                self.check_no_alloc_expr(fn_name, object);
                self.check_no_alloc_expr(fn_name, index);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.check_no_alloc_expr(fn_name, cond);
                self.check_no_alloc_expr(fn_name, then_branch);
                if let Some(e) = else_branch { self.check_no_alloc_expr(fn_name, e); }
            }
            ExprKind::Block(stmts) => self.check_no_alloc(fn_name, stmts),
            ExprKind::Match { scrutinee, arms } => {
                self.check_no_alloc_expr(fn_name, scrutinee);
                for arm in arms { self.check_no_alloc_expr(fn_name, &arm.body); }
            }
            _ => {}
        }
    }

    pub(super) fn has_explicit_return(&self, body: &[Stmt]) -> bool {
        // Any statement in the body that always returns means the function returns
        body.iter().any(|stmt| self.stmt_always_returns(stmt))
    }

    pub(super) fn stmt_always_returns(&self, stmt: &Stmt) -> bool {
        use rask_ast::stmt::StmtKind;

        match &stmt.kind {
            StmtKind::Return(_) => true,
            StmtKind::Expr(expr) => self.expr_always_returns(expr),
            _ => false,
        }
    }

    pub(super) fn expr_always_returns(&self, expr: &rask_ast::expr::Expr) -> bool {
        use rask_ast::expr::ExprKind;

        match &expr.kind {
            ExprKind::Block(stmts) | ExprKind::Unsafe { body: stmts } => {
                stmts.iter().any(|s| self.stmt_always_returns(s))
            }
            ExprKind::UsingBlock { body, .. } | ExprKind::WithAs { body, .. } => {
                body.iter().any(|s| self.stmt_always_returns(s))
            }
            ExprKind::Match { arms, .. } => {
                !arms.is_empty() && arms.iter().all(|arm| self.expr_always_returns(&arm.body))
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                else_branch.as_ref().map_or(false, |else_br| {
                    self.expr_always_returns(then_branch) && self.expr_always_returns(else_br)
                })
            }
            ExprKind::IfLet { then_branch, else_branch, .. } => {
                else_branch.as_ref().map_or(false, |else_br| {
                    self.expr_always_returns(then_branch) && self.expr_always_returns(else_br)
                })
            }
            ExprKind::Call { func, .. } => {
                if let ExprKind::Ident(name) = &func.kind {
                    matches!(name.as_str(), "panic" | "todo" | "unreachable")
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// GC9: Check if body writes to self fields (implies mutate self).
    pub(super) fn body_writes_self(body: &[Stmt]) -> bool {
        body.iter().any(|stmt| Self::stmt_writes_self(stmt))
    }

    fn stmt_writes_self(stmt: &Stmt) -> bool {
        match &stmt.kind {
            StmtKind::Assign { target, .. } => Self::expr_targets_self(target),
            StmtKind::Expr(e) => Self::expr_writes_self(e),
            StmtKind::While { body, .. } | StmtKind::For { body, .. }
            | StmtKind::Loop { body, .. } | StmtKind::WhileLet { body, .. } => {
                Self::body_writes_self(body)
            }
            _ => false,
        }
    }

    /// Check if an expression is an assignment target involving self.
    fn expr_targets_self(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(name) if name == "self" => true,
            ExprKind::Field { object, .. } => Self::expr_targets_self(object),
            ExprKind::Index { object, .. } => Self::expr_targets_self(object),
            _ => false,
        }
    }

    /// Check if an expression contains self-mutating method calls.
    fn expr_writes_self(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::MethodCall { object, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if name == "self" {
                        // Conservative: method calls on self could mutate
                        // Precise check would need type info we don't have yet
                        return false;
                    }
                }
                false
            }
            ExprKind::Block(stmts) => Self::body_writes_self(stmts),
            ExprKind::If { then_branch, else_branch, .. } => {
                Self::expr_writes_self(then_branch)
                    || else_branch.as_ref().map_or(false, |e| Self::expr_writes_self(e))
            }
            ExprKind::Match { arms, .. } => {
                arms.iter().any(|arm| Self::expr_writes_self(&arm.body))
            }
            _ => false,
        }
    }
}
