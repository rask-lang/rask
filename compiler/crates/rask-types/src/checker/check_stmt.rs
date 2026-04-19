// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement type checking.

use rask_ast::expr::{Expr, ExprKind};
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
                let was = self.in_stmt_expr;
                self.in_stmt_expr = true;
                self.infer_expr(expr);
                self.in_stmt_expr = was;
                // E5: Bare sync access without chaining is a compile error
                self.check_bare_sync_access(expr);
                // ESAD Phase 1: Clear borrows at statement end (semicolon)
                self.clear_expression_borrows();
            }
            StmtKind::Mut { name, name_span, ty, init } => {
                let (init_ty, declared_ty) = if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        // ER3/ER4: validate `T or E` in let annotation.
                        self.validate_result_types_in(&declared, *name_span);
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
                self.span_types.insert((name_span.start, name_span.end, name_span.file_id), binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                // E5: Cannot store sync access result in a variable
                self.check_sync_access_in_binding(init);
                self.clear_expression_borrows();
            }
            StmtKind::Const { name, name_span, ty, init } => {
                let (init_ty, declared_ty) = if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        // ER3/ER4: validate `T or E` in const annotation.
                        self.validate_result_types_in(&declared, *name_span);
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
                self.span_types.insert((name_span.start, name_span.end, name_span.file_id), binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                // E5: Cannot store sync access result in a variable
                self.check_sync_access_in_binding(init);
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
            StmtKind::MutTuple { patterns, init } | StmtKind::ConstTuple { patterns, init } => {
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
                    self.span_types.insert((name_span.start, name_span.end, name_span.file_id), ty);
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

    // ── E5: Sync inline access validation ──────────────────────────────
    //
    // E5/R5/MX3: `.read()/.write()/.lock()` on Shared<T>/Mutex<T> produce
    // expression-scoped locks. Validation rules:
    //
    // 1. Must be chained: `shared.read().field` or `shared.read().method()`.
    //    Bare `shared.read()` is a compile error.
    // 2. Cannot be stored: `const x = shared.read()` is a compile error.
    //    Only Copy-out or inline mutation allowed.
    // 3. DL4: Multiple sync accesses in one expression is a compile error
    //    (deadlock risk).

    /// Validate E5 rules for a top-level expression statement.
    /// Called from check_stmt after type inference.
    fn check_bare_sync_access(&mut self, expr: &Expr) {
        // Rule 1: Bare sync access at statement level
        if let Some((ty_name, method, span)) = self.is_sync_access(expr) {
            self.errors.push(TypeError::BareSyncAccess {
                ty: ty_name,
                method,
                span,
            });
            return;
        }

        // Rule 3 (DL4): Count sync accesses within this expression tree.
        // Multiple locks in one expression risks deadlock.
        let accesses = self.collect_sync_accesses(expr);
        if accesses.len() > 1 {
            // Report on the second access
            let (ty_name, method, span) = &accesses[1];
            self.errors.push(TypeError::BareSyncAccess {
                ty: ty_name.clone(),
                method: format!("{} (multiple sync accesses in one expression — deadlock risk [conc.sync/DL4])", method),
                span: *span,
            });
        }
    }

    /// Validate E5 for let/const bindings: `const x = shared.read()` is an error.
    /// Only `const x = shared.read().field` (Copy out) is allowed.
    fn check_sync_access_in_binding(&mut self, init: &Expr) {
        if let Some((ty_name, method, span)) = self.is_sync_access(init) {
            self.errors.push(TypeError::BareSyncAccess {
                ty: ty_name,
                method,
                span,
            });
        }
    }

    /// Check if an expression is a sync access call (not chained).
    /// Returns Some if this is a bare `.read()/.write()/.lock()`.
    fn is_sync_access(&self, expr: &Expr) -> Option<(String, String, rask_ast::Span)> {
        match &expr.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                if args.is_empty() && matches!(method.as_str(), "read" | "write" | "lock") {
                    if let Some(ty_name) = self.sync_type_of(object) {
                        return Some((ty_name, method.clone(), expr.span));
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Recursively collect all sync access nodes within an expression tree.
    /// Each access is (type_name, method, span).
    fn collect_sync_accesses(&self, expr: &Expr) -> Vec<(String, String, rask_ast::Span)> {
        let mut accesses = Vec::new();
        self.walk_sync_accesses(expr, &mut accesses);
        accesses
    }

    fn walk_sync_accesses(&self, expr: &Expr, out: &mut Vec<(String, String, rask_ast::Span)>) {
        match &expr.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                // Check if this node itself is a sync access
                if args.is_empty() && matches!(method.as_str(), "read" | "write" | "lock") {
                    if let Some(ty_name) = self.sync_type_of(object) {
                        out.push((ty_name, method.clone(), expr.span));
                    }
                }
                // Recurse into object and args
                self.walk_sync_accesses(object, out);
                for arg in args {
                    self.walk_sync_accesses(&arg.expr, out);
                }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.walk_sync_accesses(object, out);
            }
            ExprKind::Call { func, args } => {
                self.walk_sync_accesses(func, out);
                for arg in args {
                    self.walk_sync_accesses(&arg.expr, out);
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.walk_sync_accesses(left, out);
                self.walk_sync_accesses(right, out);
            }
            ExprKind::Unary { operand, .. } => {
                self.walk_sync_accesses(operand, out);
            }
            ExprKind::Index { object, index } => {
                self.walk_sync_accesses(object, out);
                self.walk_sync_accesses(index, out);
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.walk_sync_accesses(cond, out);
                self.walk_sync_accesses(then_branch, out);
                if let Some(e) = else_branch {
                    self.walk_sync_accesses(e, out);
                }
            }
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems {
                    self.walk_sync_accesses(e, out);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.walk_sync_accesses(&f.value, out);
                }
                if let Some(s) = spread {
                    self.walk_sync_accesses(s, out);
                }
            }
            _ => {}
        }
    }

    /// Check if an expression's inferred type is Shared<T> or Mutex<T>.
    fn sync_type_of(&self, expr: &Expr) -> Option<String> {
        let ty = self.node_types.get(&expr.id)?;
        let resolved = self.ctx.apply(ty);
        match &resolved {
            Type::UnresolvedGeneric { name, .. }
                if matches!(name.as_str(), "Shared" | "Mutex") =>
            {
                Some(name.clone())
            }
            _ => None,
        }
    }
}
