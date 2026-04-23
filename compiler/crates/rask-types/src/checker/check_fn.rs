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

        // ER21: public functions must not use `or _` (inferred error types)
        let has_inferred_error = f.ret_ty.as_ref().is_some_and(|t| t.ends_with(", _>"));
        if f.is_pub && has_inferred_error {
            self.errors.push(TypeError::PublicInferredError {
                function_name: f.name.clone(),
                span: f.span,
            });
        }

        // GC1/GC2: Reuse pre-registered type vars for inferred params/return
        let inferred = self.inferred_fn_types.get(&f.name).cloned();

        let ret_ty = if has_inferred_error {
            // `or _` — reuse the pre-registered Result with fresh error var
            if let Some((_, ref ret_var)) = inferred {
                ret_var.clone()
            } else {
                // Fallback: parse the ok type, create fresh error var
                let t = f.ret_ty.as_ref().unwrap();
                let ok_str = &t["Result<".len()..t.len() - ", _>".len()];
                let ok_ty = parse_type_string(ok_str, &self.types).unwrap_or(Type::Error);
                Type::Result {
                    ok: Box::new(ok_ty),
                    err: Box::new(self.ctx.fresh_var()),
                }
            }
        } else if let Some(t) = &f.ret_ty {
            parse_type_string(t, &self.types).unwrap_or(Type::Error)
        } else if let Some((_, ref ret_var)) = inferred {
            ret_var.clone()
        } else {
            Type::Unit
        };
        // ER3/ER4: validate every `T or E` that appears in the return type.
        self.validate_result_types_in(&ret_ty, f.span);
        self.current_return_type = Some(ret_ty);

        // ER20: Save outer accumulation state and detect if we should accumulate
        let old_accumulate = self.accumulate_errors;
        let old_inferred_errors = std::mem::take(&mut self.inferred_errors);
        let resolved_for_accum = self.ctx.apply(self.current_return_type.as_ref().unwrap());
        self.accumulate_errors = match &resolved_for_accum {
            Type::Var(_) => true,
            Type::Result { err, .. } => matches!(self.ctx.apply(err), Type::Var(_)),
            _ => false,
        };

        // CC1: reject `using Multitasking` / `using ThreadPool` on function signatures
        for cc in &f.context_clauses {
            if is_runtime_context(&cc.ty) {
                self.errors.push(TypeError::SignatureRuntimeContext {
                    ctx: cc.ty.clone(),
                    span: f.span,
                });
            }
        }

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

        // Reset multitasking depth for each function body
        self.multitasking_depth = 0;

        self.push_scope();
        for param in &f.params {
            if param.name == "self" {
                if let Some(self_ty) = self.current_self_type.clone() {
                    if inferred_self_mutate || param.is_mutate || param.is_take {
                        self.define_local("self".to_string(), self_ty.clone());
                    } else {
                        self.define_local_param("self".to_string(), self_ty.clone());
                    }
                    self.span_types.insert((param.name_span.start, param.name_span.end, param.name_span.file_id), self_ty);
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
            // ER3/ER4: validate nested `T or E` in parameter types.
            self.validate_result_types_in(&ty, param.name_span);
            if param.is_mutate || param.is_take {
                self.define_local(param.name.clone(), ty.clone());
            } else {
                self.define_local_param(param.name.clone(), ty.clone());
            }
            self.span_types.insert((param.name_span.start, param.name_span.end, param.name_span.file_id), ty);
        }

        for stmt in &f.body {
            self.check_stmt(stmt);
            // ER24: early-exit narrowing after each top-level stmt.
            // Solve pending constraints first so method-call return types
            // are resolved (otherwise scrutinee stays `Var`).
            self.solve_constraints();
            self.apply_early_exit_narrowing(stmt);
        }

        // ER20: Finalize error union from accumulated error types
        if self.accumulate_errors && !self.inferred_errors.is_empty() {
            let errors = std::mem::take(&mut self.inferred_errors);
            let error_union = Type::union(errors);
            let ret = self.current_return_type.as_ref().unwrap().clone();
            let resolved_ret = self.ctx.apply(&ret);
            match &resolved_ret {
                Type::Result { err, .. } => {
                    let resolved_err = self.ctx.apply(err);
                    if matches!(resolved_err, Type::Var(_)) {
                        let _ = self.unify(&error_union, &resolved_err, f.span);
                    }
                }
                Type::Var(_) => {
                    let ret_ok = self.ctx.fresh_var();
                    let ret_result = Type::Result {
                        ok: Box::new(ret_ok),
                        err: Box::new(error_union),
                    };
                    let _ = self.unify(&resolved_ret, &ret_result, f.span);
                }
                _ => {}
            }
        }

        let ret_ty = self.current_return_type.as_ref().unwrap();
        let resolved_ret_ty = self.ctx.apply(ret_ty);

        // Empty body with non-Unit return type is a missing return (unless it's
        // a trait method declaration with no body — those are handled separately).
        // Stdlib stubs are never passed through check_fn.

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
                        let end_span = Span::with_file(f.span.end.saturating_sub(1), f.span.end, f.span.file_id);
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
                    let end_span = Span::with_file(f.span.end.saturating_sub(1), f.span.end, f.span.file_id);
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

        // ER20: Restore outer accumulation state
        self.accumulate_errors = old_accumulate;
        self.inferred_errors = old_inferred_errors;

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
            StmtKind::Mut { init, .. } | StmtKind::Const { init, .. } => {
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
            ExprKind::If { cond, then_branch, else_branch, .. } => {
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
            // An unconditional loop either returns or diverges — either way,
            // code after it is unreachable and the function has a return path.
            StmtKind::Loop { .. } => true,
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
                    matches!(name.as_str(), "panic" | "todo" | "unreachable" | "skip")
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
            StmtKind::Assign { target, value } => {
                Self::expr_targets_self(target) || Self::expr_writes_self(value)
            }
            StmtKind::Expr(e) => Self::expr_writes_self(e),
            StmtKind::Const { init, .. } | StmtKind::Mut { init, .. } => {
                Self::expr_writes_self(init)
            }
            StmtKind::ConstTuple { init, .. } | StmtKind::MutTuple { init, .. } => {
                Self::expr_writes_self(init)
            }
            StmtKind::Return(Some(e)) => Self::expr_writes_self(e),
            StmtKind::Break { value: Some(v), .. } => Self::expr_writes_self(v),
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
                // Conservative: a direct method call on self (`self.foo()`)
                // is assumed to mutate. Without a second pass over all
                // declarations we can't know whether `foo` is `self` or
                // `mutate self`. Marking the enclosing private method as
                // mutate is safe — it's inference for the common case.
                if let ExprKind::Ident(name) = &object.kind {
                    if name == "self" {
                        return true;
                    }
                }
                false
            }
            ExprKind::Block(stmts) => Self::body_writes_self(stmts),
            ExprKind::If { then_branch, else_branch, .. }
            | ExprKind::IfLet { then_branch, else_branch, .. } => {
                Self::expr_writes_self(then_branch)
                    || else_branch.as_ref().map_or(false, |e| Self::expr_writes_self(e))
            }
            ExprKind::GuardPattern { else_branch, .. } => Self::expr_writes_self(else_branch),
            ExprKind::Match { arms, .. } => {
                arms.iter().any(|arm| Self::expr_writes_self(&arm.body))
            }
            ExprKind::Try { expr, .. } => Self::expr_writes_self(expr),
            ExprKind::Unwrap { expr, .. } | ExprKind::IsPresent { expr, .. } => {
                Self::expr_writes_self(expr)
            }
            ExprKind::Unsafe { body } | ExprKind::Comptime { body } => {
                Self::body_writes_self(body)
            }
            ExprKind::Loop { body, .. } => Self::body_writes_self(body),
            _ => false,
        }
    }
}

fn is_runtime_context(ty: &str) -> bool {
    matches!(ty, "Multitasking" | "MultiTasking" | "multitasking" | "ThreadPool" | "threadpool")
}
