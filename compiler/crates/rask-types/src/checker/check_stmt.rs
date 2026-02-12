// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement type checking.

use rask_ast::stmt::{Stmt, StmtKind};

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
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                        self.define_local(name.clone(), declared);
                    } else {
                        self.define_local(name.clone(), init_ty);
                    }
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Const { name, name_span: _, ty, init } => {
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                        self.define_local(name.clone(), declared);
                    } else {
                        self.define_local(name.clone(), init_ty);
                    }
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Assign { target, value } => {
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
                let target_ty = self.infer_expr(target);
                let value_ty = self.infer_expr(value);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    target_ty, value_ty, stmt.span,
                ));
                self.clear_expression_borrows();
            }
            StmtKind::Return(value) => {
                let ret_ty = if let Some(expr) = value {
                    self.infer_expr(expr)
                } else {
                    Type::Unit
                };
                if let Some(expected) = &self.current_return_type {
                    // Auto-wrap in Ok() if returning T where function expects T or E
                    let wrapped_ty = self.wrap_in_ok_if_needed(ret_ty, expected);

                    self.ctx.add_constraint(TypeConstraint::Equal(
                        expected.clone(),
                        wrapped_ty,
                        stmt.span,
                    ));
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
                self.define_local(binding.clone(), elem_ty);
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
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.infer_expr(init);
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
