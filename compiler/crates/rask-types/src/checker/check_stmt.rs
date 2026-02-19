// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement type checking.

use rask_ast::stmt::{ForBinding, Stmt, StmtKind};

use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Statement Checking
    // ------------------------------------------------------------------------

    pub(super) fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.infer_expr(expr);
                // ESAD Phase 1: Clear borrows at statement end (semicolon)
                self.clear_expression_borrows();
            }
            StmtKind::Let { name, name_span: _, ty, init } => {
                let (init_ty, declared_ty) = if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        let init_ty = self.infer_expr_expecting(init, &declared);
                        (init_ty, Some(declared))
                    } else {
                        (self.infer_expr(init), None)
                    }
                } else {
                    (self.infer_expr(init), None)
                };
                if let Some(declared) = declared_ty {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                    self.define_local(name.clone(), declared);
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Const { name, name_span: _, ty, init } => {
                let (init_ty, declared_ty) = if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        let init_ty = self.infer_expr_expecting(init, &declared);
                        (init_ty, Some(declared))
                    } else {
                        (self.infer_expr(init), None)
                    }
                } else {
                    (self.infer_expr(init), None)
                };
                if let Some(declared) = declared_ty {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                    self.define_local(name.clone(), declared);
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Assign { target, value } => {
                // Deref write (*ptr = value) requires unsafe
                if matches!(&target.kind, rask_ast::expr::ExprKind::Unary { op: rask_ast::expr::UnaryOp::Deref, .. }) && !self.in_unsafe {
                    self.errors.push(TypeError::UnsafeRequired {
                        operation: "pointer dereference write".to_string(),
                        span: stmt.span,
                    });
                }
                // Reject mutation of read-only parameters (default params are read-only)
                if let Some(root) = Self::root_ident_name(target) {
                    if self.is_local_read_only(&root) {
                        self.errors.push(TypeError::MutateReadOnlyParam {
                            name: root.clone(),
                            span: stmt.span,
                        });
                    }
                    // ESAD Phase 2: Reject mutation of persistently borrowed sources
                    if let Some(borrow) = self.check_persistent_borrow_conflict(&root) {
                        self.errors.push(TypeError::MutateBorrowedSource {
                            source_var: root,
                            view_var: borrow.view_var.clone(),
                            borrow_span: borrow.borrow_span,
                            mutate_span: stmt.span,
                        });
                    }
                }
                self.in_assign_target = true;
                let target_ty = self.infer_expr(target);
                self.in_assign_target = false;
                let value_ty = self.infer_expr_expecting(value, &target_ty);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    target_ty, value_ty, stmt.span,
                ));
                self.clear_expression_borrows();
            }
            StmtKind::Return(value) => {
                let ret_ty = if let Some(expr) = value {
                    if let Some(expected) = &self.current_return_type.clone() {
                        // If expecting Result<T, E>, propagate T as the expected type
                        // so literals like `return 42` infer the correct inner type
                        let effective = match &self.ctx.apply(expected) {
                            Type::Result { ok, .. } => (**ok).clone(),
                            _ => expected.clone(),
                        };
                        self.infer_expr_expecting(expr, &effective)
                    } else {
                        self.infer_expr(expr)
                    }
                } else {
                    Type::Unit
                };
                if let Some(expected) = &self.current_return_type {
                    // Defer auto-Ok wrapping — the solver resolves this after
                    // method/field constraints are solved, so we know if the
                    // return expression is already a Result or needs wrapping
                    self.ctx.add_constraint(TypeConstraint::ReturnValue {
                        ret_ty,
                        expected: expected.clone(),
                        span: stmt.span,
                    });
                }
                self.clear_expression_borrows();
            }
            StmtKind::While { cond, body, .. } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, stmt.span));
                self.push_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::For { binding, iter, body, .. } => {
                let iter_ty = self.infer_expr(iter);
                self.push_scope();
                let elem_ty = match &iter_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    _ => self.ctx.fresh_var(),
                };
                match binding {
                    ForBinding::Single(name) => self.define_local(name.clone(), elem_ty),
                    ForBinding::Tuple(names) => {
                        let vars: Vec<_> = names.iter().map(|_| self.ctx.fresh_var()).collect();
                        for (name, var) in names.iter().zip(vars) {
                            self.define_local(name.clone(), var);
                        }
                    }
                }
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::Break { value, .. } => {
                if let Some(v) = value {
                    self.infer_expr(v);
                }
            }
            StmtKind::Continue(_) => {}
            StmtKind::Ensure { body, else_handler } => {
                for s in body {
                    self.check_stmt(s);
                }
                if let Some((_name, handler)) = else_handler {
                    for s in handler {
                        self.check_stmt(s);
                    }
                }
            }
            StmtKind::Comptime(body) => {
                for s in body {
                    self.check_stmt(s);
                }
            }
            StmtKind::LetTuple { names, init } | StmtKind::ConstTuple { names, init } => {
                let init_ty = self.infer_expr(init);
                // Bind each destructured name to its tuple element type
                if let Type::Tuple(elems) = &init_ty {
                    for (i, name) in names.iter().enumerate() {
                        if let Some(elem_ty) = elems.get(i) {
                            self.define_local(name.clone(), elem_ty.clone());
                        }
                    }
                } else {
                    // Not a known tuple type — bind all names as the inferred type
                    for name in names {
                        self.define_local(name.clone(), init_ty.clone());
                    }
                }
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                let value_ty = self.infer_expr(expr);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, stmt.span);
                for (name, ty) in bindings {
                    self.define_local(name, ty);
                }
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::Loop { body, .. } => {
                self.push_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
        }
    }
}
