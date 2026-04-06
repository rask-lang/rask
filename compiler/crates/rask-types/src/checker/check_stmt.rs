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
            StmtKind::Let { name, name_span, ty, init } => {
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
                let binding_ty = if let Some(declared) = declared_ty {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                    self.define_local(name.clone(), declared.clone());
                    declared
                } else {
                    self.define_local(name.clone(), init_ty.clone());
                    init_ty
                };
                self.span_types.insert((name_span.start, name_span.end), binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Const { name, name_span, ty, init } => {
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
                let binding_ty = if let Some(declared) = declared_ty {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                    self.define_local_read_only(name.clone(), declared.clone());
                    declared
                } else {
                    self.define_local_read_only(name.clone(), init_ty.clone());
                    init_ty
                };
                self.span_types.insert((name_span.start, name_span.end), binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                self.clear_expression_borrows();
            }
            StmtKind::Assign { target, value } => {
                // Deref write (*ptr = value) requires unsafe
                if matches!(&target.kind, rask_ast::expr::ExprKind::Unary { op: rask_ast::expr::UnaryOp::Deref, .. }) {
                    self.unsafe_ops.push((stmt.span, super::UnsafeCategory::PointerDerefWrite));
                    if !self.in_unsafe {
                        self.errors.push(TypeError::UnsafeRequired {
                            operation: "pointer dereference write".to_string(),
                            span: stmt.span,
                        });
                    }
                }
                // Reject mutation of read-only parameters (default params are read-only).
                // Exception: index assignment on collection types (Vec, Map, Pool)
                // is interior mutation, not rebinding — allowed on const bindings.
                if let Some(root) = Self::root_ident_name(target) {
                    // Index assignment (v[i] = x) is interior mutation — allowed
                    // on const bindings. Collections (Vec, Map) are heap-allocated;
                    // index assignment doesn't rebind the variable.
                    let is_index_assign = matches!(&target.kind, rask_ast::expr::ExprKind::Index { .. });
                    if self.is_local_read_only(&root) && !is_index_assign {
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
            StmtKind::ComptimeFor { binding, iter, body, .. } => {
                // CT48–CT54: comptime for loop type checking
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
            StmtKind::LetTuple { patterns, init } | StmtKind::ConstTuple { patterns, init } => {
                let is_const = matches!(&stmt.kind, StmtKind::ConstTuple { .. });
                let init_ty = self.infer_expr(init);
                self.bind_tuple_patterns(patterns, &init_ty, is_const, stmt.span);
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
            StmtKind::Discard { name, name_span } => {
                if let Some(ty) = self.lookup_local(name) {
                    let resolved = self.ctx.apply(&ty);
                    // D3: @resource types cannot be discarded
                    if self.is_resource_type(&resolved) {
                        self.errors.push(TypeError::DiscardResourceType {
                            name: name.clone(),
                            ty: resolved,
                            span: stmt.span,
                        });
                    }
                    // D2: Copy types — accepted but semantically a no-op.
                    // Warning emitted by the lint pass, not the type checker,
                    // because D2 is advisory, not a blocking error.
                    self.span_types.insert((name_span.start, name_span.end), ty);
                    // D1: Invalidate the binding
                    self.discarded_bindings.insert(name.clone(), stmt.span);
                } else {
                    self.errors.push(TypeError::UndefinedName {
                        name: name.clone(),
                        span: *name_span,
                    });
                }
            }
        }
    }

    /// Recursively bind tuple destructuring patterns to types.
    /// Handles nested patterns like `(a, (b, c))` matched against `(i32, (i32, i32))`.
    fn bind_tuple_patterns(
        &mut self,
        patterns: &[rask_ast::stmt::TuplePat],
        init_ty: &Type,
        is_const: bool,
        span: rask_ast::Span,
    ) {
        use rask_ast::stmt::TuplePat;

        let resolved = self.ctx.apply(init_ty);
        if let Type::Tuple(elems) = &resolved {
            for (i, pat) in patterns.iter().enumerate() {
                let elem_ty = elems.get(i).cloned().unwrap_or(Type::Error);
                match pat {
                    TuplePat::Name(name) => {
                        if is_const {
                            self.define_local_read_only(name.clone(), elem_ty);
                        } else {
                            self.define_local(name.clone(), elem_ty);
                        }
                    }
                    TuplePat::Wildcard => {} // discard
                    TuplePat::Nested(sub_pats) => {
                        self.bind_tuple_patterns(sub_pats, &elem_ty, is_const, span);
                    }
                }
            }
        } else {
            // Type not yet resolved — create fresh vars for each element
            let elem_vars: Vec<Type> = patterns.iter()
                .map(|_| self.ctx.fresh_var())
                .collect();
            let tuple_ty = Type::Tuple(elem_vars.clone());
            let _ = self.unify(init_ty, &tuple_ty, span);
            for (pat, var) in patterns.iter().zip(elem_vars) {
                match pat {
                    TuplePat::Name(name) => {
                        if is_const {
                            self.define_local_read_only(name.clone(), var);
                        } else {
                            self.define_local(name.clone(), var);
                        }
                    }
                    TuplePat::Wildcard => {}
                    TuplePat::Nested(sub_pats) => {
                        self.bind_tuple_patterns(sub_pats, &var, is_const, span);
                    }
                }
            }
        }
    }

    /// Check if a type is a primitive Copy type (trivially cleaned up).
    fn is_copy_type(&self, ty: &Type) -> bool {
        matches!(
            ty,
            Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
            | Type::F32 | Type::F64 | Type::Char | Type::Unit
        )
    }

    /// Check if a type is marked @resource.
    fn is_resource_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(id) => self.types.is_resource_type_by_id(*id),
            Type::UnresolvedNamed(name) => self.types.is_resource_type(name),
            _ => false,
        }
    }
}
