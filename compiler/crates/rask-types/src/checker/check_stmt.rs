// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement type checking.

use rask_ast::coercion::CoercionSite;
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::Span;

use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::check_expr::ContainerElem;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    /// Type a `for` binding takes when iterating `iter_ty`.
    ///
    /// The element type is spelled out wherever it can be read off the
    /// container: leaving it a free var loses the element's identity, and a
    /// `Vec<any Trait>` binding then has no trait to dispatch against.
    ///
    /// Everything else gets a fresh var plus an `ElementOf` constraint — either
    /// because the container is a field access whose type hasn't resolved yet,
    /// or because it genuinely can't say (a bare `Range` carries no element
    /// type, so the body's arithmetic pins the width). The constraint is what
    /// reports a container that turns out not to be iterable at all.
    ///
    /// This is deliberately *not* the `Index` constraint, though both wait on
    /// the same field to resolve: indexing a container and iterating it don't
    /// agree on Map or Pool. `m[k]` is a `V` while `for e in m` is a `(K, V)`,
    /// and `p[h]` is a `T` while `for h in p` is a `Handle<T>`.
    fn iter_elem_type(&mut self, iter_ty: &Type, span: Span) -> Type {
        let resolved = self.ctx.apply(iter_ty);
        if let ContainerElem::Known(elem) = self.container_elem_type(&resolved) {
            return elem;
        }
        let elem = self.ctx.fresh_var();
        // A field's type is a deferred `HasField`, so the container can still be
        // unresolved here. Tie the element to it and come back once it settles —
        // otherwise `for t in self.tables` leaves `t` open forever and every
        // binding derived from it reports E0361 (#632).
        self.ctx.add_constraint(TypeConstraint::ElementOf {
            container: iter_ty.clone(),
            elem: elem.clone(),
            span,
        });
        elem
    }

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
                    // AN8: an annotation name here would leave the binding
                    // asking for a value nothing can produce, and the type
                    // error would blame the initializer for it.
                    self.reject_annotation_binding_type(ty_str, *name_span);
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
                    // ER11/optionals: at binding position, only the optional
                    // shape (T or none) widens. Bare T into T or E (E ≠ none)
                    // is rejected so the error-branch coercion stays visible.
                    self.coerce_into(
                        CoercionSite::AnnotatedBinding,
                        init_ty,
                        declared.clone(),
                        stmt.span,
                    );
                    self.define_local(name.clone(), declared.clone());
                    declared
                } else {
                    self.define_local(name.clone(), init_ty.clone());
                    init_ty
                };
                self.span_types.insert((name_span.start, name_span.end, name_span.file_id), binding_ty.clone());
                // RC1/RC3: a `Vec`/`Map` binding can't hold linear elements.
                self.note_linear_container_site(*name_span, binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                // E5: Cannot store sync access result in a variable
                self.check_sync_access_in_binding(init);
                self.clear_expression_borrows();
            }
            StmtKind::Let { name, name_span, ty, init } => {
                let (init_ty, declared_ty) = if let Some(ty_str) = ty {
                    // AN8: an annotation name here would leave the binding
                    // asking for a value nothing can produce, and the type
                    // error would blame the initializer for it.
                    self.reject_annotation_binding_type(ty_str, *name_span);
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
                    // ER11/optionals: at binding position, only the optional
                    // shape (T or none) widens — same rule as Mut above.
                    self.coerce_into(
                        CoercionSite::AnnotatedBinding,
                        init_ty,
                        declared.clone(),
                        stmt.span,
                    );
                    self.define_local_const(name.clone(), declared.clone());
                    declared
                } else {
                    self.define_local_const(name.clone(), init_ty.clone());
                    init_ty
                };
                self.span_types.insert((name_span.start, name_span.end, name_span.file_id), binding_ty.clone());
                // RC1/RC3: a `Vec`/`Map` binding can't hold linear elements.
                self.note_linear_container_site(*name_span, binding_ty);
                // ESAD Phase 2: Track view creation
                self.check_view_at_binding(name, init, stmt.span);
                // E5: Cannot store sync access result in a variable
                self.check_sync_access_in_binding(init);
                self.clear_expression_borrows();
            }
            StmtKind::Assign { target, value, .. } => {
                // Reject mutation of read-only bindings (const) and read-only
                // parameters (default params). `const` is deep: rebinding,
                // index/field assign, and mutating method calls all forbidden.
                if let Some(root) = Self::root_ident_name(target) {
                    // Writing *through* a reference is not mutating the binding
                    // that holds it: `h.field = v` on a `Handle<T>` lands in pool
                    // storage (mem.context/CC1), and `l.field = v` on a `Link<T>`
                    // lands in the node, because a link is the node's address.
                    // Only a bare rebind (`h = other`) mutates the binding.
                    //
                    // Whether the root is such a reference depends on its type,
                    // and at this point the type is often still a variable — a
                    // link bound by `if e.target? as t` comes from a deferred
                    // `HasField`. So field and index writes are always judged in
                    // `validate_pending_mutations`, after solving, and never here.
                    // One decision site, reading a resolved type.
                    let writes_through_place = matches!(
                        &target.kind,
                        ExprKind::Field { .. } | ExprKind::Index { .. }
                    );
                    if writes_through_place {
                        if let Some(kind) = self.lookup_binding_kind(&root) {
                            self.pending_mutations.push(super::PendingMutation {
                                root: root.clone(),
                                ty: self.lookup_local(&root).unwrap_or(Type::Error),
                                kind,
                                span: stmt.span,
                            });
                        }
                    } else {
                        match self.lookup_binding_kind(&root) {
                            Some(super::BindingKind::Let) => {
                                self.errors.push(TypeError::MutateConst {
                                    name: root.clone(),
                                    span: stmt.span,
                                });
                            }
                            Some(super::BindingKind::WithRead) => {
                                self.errors.push(TypeError::MutateWithBinding {
                                    name: root.clone(),
                                    span: stmt.span,
                                });
                            }
                            Some(super::BindingKind::Param) => {
                                self.errors.push(TypeError::MutateReadOnlyParam {
                                    name: root.clone(),
                                    span: stmt.span,
                                });
                            }
                            Some(super::BindingKind::Bound(from)) => {
                                self.errors.push(TypeError::MutateBoundName {
                                    name: root.clone(),
                                    from,
                                    span: stmt.span,
                                });
                            }
                            _ => {}
                        }
                    }
                    // mem.pools/PF5: a write through a handle whose element type is
                    // backed by a frozen context is rejected. Needs the element
                    // type, so it waits for solving too.
                    if writes_through_place {
                        self.pending_frozen_writes.push(super::PendingFrozenWrite {
                            ty: self.lookup_local(&root).unwrap_or(Type::Error),
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

                // `*x = v` requires unsafe only when `x` really is a raw
                // pointer. Writing through an `Owned` is a mutable borrow
                // (mem.owned/OW3), not a raw-pointer store. Checked here rather
                // than before the target is inferred, because "what is x" is
                // the whole question and there's no answer until then (#737).
                if let rask_ast::expr::ExprKind::Unary {
                    op: rask_ast::expr::UnaryOp::Deref, operand,
                } = &target.kind {
                    let operand_ty = self
                        .node_types
                        .get(&operand.id)
                        .map(|t| self.ctx.apply(t));
                    let needs_unsafe = match operand_ty {
                        Some(t) => matches!(t, Type::RawPtr(_) | Type::Var(_)),
                        // Never inferred — can't prove it's safe.
                        None => true,
                    };
                    if needs_unsafe {
                        self.unsafe_ops.push((stmt.span, super::UnsafeCategory::PointerDerefWrite));
                        if !self.in_unsafe {
                            self.errors.push(TypeError::UnsafeRequired {
                                operation: "pointer dereference write".to_string(),
                                span: stmt.span,
                            });
                        }
                    }
                }
                let value_ty = self.infer_expr_expecting(value, &target_ty);
                // Assignment is a widening position (optionals/O-widen, SYNTAX L521):
                // the optional shape `T` widens to `T?` at the lvalue, same as a
                // binding. Bind keeps `T or E` (E ≠ none) strict.
                self.coerce_into(CoercionSite::Assignment, value_ty, target_ty, stmt.span);
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
                    // Defer auto-wrap — the solver resolves this after
                    // method/field constraints are solved, so we know if the
                    // return expression is already a Result or needs wrapping.
                    // Return position permits the full ER9 wrap.
                    self.coerce_into_node(
                        CoercionSite::Return,
                        ret_ty,
                        expected.clone(),
                        value.as_ref().map(|e| e.id),
                        stmt.span,
                    );
                }
                self.clear_expression_borrows();
            }
            StmtKind::While { cond, body, .. } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, stmt.span));
                self.push_scope();
                // OPT19 on a loop: `while expr? as v` binds the payload for the
                // body. The test narrows nothing — this is a binding, re-read
                // once per iteration, exactly as in the `if` form.
                if let Some((name, payload_ty, _)) = self.extract_presence_binding(cond) {
                    self.define_local_bound(name, payload_ty, super::BoundFrom::Payload);
                }
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::For { binding, iter, body, mutate, .. } => {
                let iter_ty = self.infer_expr(iter);
                self.push_scope();
                let elem_ty = self.iter_elem_type(&iter_ty, iter.span);
                // std.iteration/I1: a plain `for` yields elements read-only;
                // `for mutate x in xs` is the mode whose writes reach the
                // collection. Nothing enforced this, so `for c in xs { c.n += 1 }`
                // compiled and then the backends disagreed — the interpreter
                // wrote through to the element, native dropped the write.
                let mut define = |c: &mut Self, name: String, ty: Type| {
                    if *mutate {
                        c.define_local(name, ty);
                    } else {
                        c.define_local_bound(name, ty, super::BoundFrom::Element);
                    }
                };
                match binding {
                    ForBinding::Single(name) => define(self, name.clone(), elem_ty),
                    ForBinding::Tuple(names) => {
                        // Each name takes its position in the element tuple.
                        // They used to get fresh unconstrained variables, so
                        // `for (k, v) in m { k.len() }` left `k` with no type at
                        // all — and the program compiled only because MIR guessed
                        // the receiver from the variable's tracked prefix, the
                        // last of the nine dispatch fallbacks (#425).
                        let elems: Vec<Type> = match self.ctx.apply(&elem_ty) {
                            Type::Tuple(elems) if elems.len() == names.len() => elems,
                            // Still open, or not a tuple: fresh variables, which
                            // is what every case got before.
                            _ => names.iter().map(|_| self.ctx.fresh_var()).collect(),
                        };
                        for (name, ty) in names.iter().zip(elems) {
                            define(self, name.clone(), ty);
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
                    let ty = self.infer_expr(v);
                    if let Some(loop_ty) = self.loop_value_types.last().cloned() {
                        if let Err(e) = self.unify(&ty, &loop_ty, v.span) {
                            self.errors.push(e);
                        }
                    }
                }
            }
            StmtKind::Continue(_) => {}
            StmtKind::Ensure { body, else_handler } => {
                // ER4/ER3: cleanup has no caller to propagate to. Report the
                // `try` itself, then keep checking — the rest of the body is
                // still worth type-checking.
                Self::report_try_in_ensure(body, "inside an `ensure` body", &mut self.errors);
                let mut body_ty = None;
                for s in body {
                    self.check_stmt(s);
                    if let Some(e) = Self::ensure_body_value(&s.kind) {
                        body_ty = self.node_types.get(&e.id).cloned();
                    }
                }
                if let Some((name, handler)) = else_handler {
                    Self::report_try_in_ensure(handler, "in an `ensure` error handler", &mut self.errors);
                    // ER2: the handler binds the error branch of whatever the
                    // body's last expression produced. That type is still a
                    // variable here — the body is a method call — so tie the two
                    // together and let the solver settle it. Without a binding at
                    // all, `e.message()` had no receiver type and died in MIR
                    // lowering as an unresolved receiver.
                    let err_ty = self.ctx.fresh_var();
                    if let Some(body_ty) = body_ty {
                        self.ctx.add_constraint(TypeConstraint::ErrorBranch {
                            value: body_ty,
                            result: err_ty.clone(),
                            span: stmt.span,
                        });
                    }
                    self.push_scope();
                    self.define_local_const(name.clone(), err_ty);
                    for s in handler {
                        self.check_stmt(s);
                    }
                    self.pop_scope();
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
            StmtKind::MutTuple { patterns, init } | StmtKind::LetTuple { patterns, init } => {
                let is_const = matches!(&stmt.kind, StmtKind::LetTuple { .. });
                let init_ty = self.infer_expr(init);
                self.bind_tuple_patterns(patterns, &init_ty, is_const, stmt.span);
            }
            // `let Point { x, .. } = p` — the pattern types its own bindings, the
            // same way a match arm's does.
            StmtKind::LetStruct { pattern, init, is_mut } => {
                let init_ty = self.infer_expr(init);
                let bindings = self.check_pattern(pattern, &init_ty, stmt.span);
                for (name, ty) in bindings {
                    if *is_mut {
                        self.define_local(name, ty);
                    } else {
                        self.define_local_const(name, ty);
                    }
                }
            }
            StmtKind::WhileLet { pattern, expr, body, .. } => {
                let value_ty = self.infer_expr(expr);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, stmt.span);
                for (name, ty) in bindings {
                    self.define_local_bound(name, ty, super::BoundFrom::Payload);
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
                            self.define_local_const(name.clone(), elem_ty);
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
                            self.define_local_const(name.clone(), var);
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
    pub(super) fn check_bare_sync_access(&mut self, expr: &Expr) {
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
    fn collect_sync_accesses(&mut self, expr: &Expr) -> Vec<(String, String, rask_ast::Span)> {
        let mut accesses = Vec::new();
        self.walk_sync_accesses(expr, &mut accesses);
        let staged = std::mem::take(&mut self.staged_outside_with);
        for (name, span) in staged {
            self.errors.push(TypeError::StagedOutsideWith { name, span });
        }
        accesses
    }

    /// How to spell a sync receiver back to the author, for the suggestion.
    fn sync_source_text(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, field } => {
                Some(format!("{}.{}", Self::sync_source_text(object)?, field))
            }
            _ => None,
        }
    }

    fn walk_sync_accesses(&mut self, expr: &Expr, out: &mut Vec<(String, String, rask_ast::Span)>) {
        match &expr.kind {
            ExprKind::MethodCall { object, method, args, .. } => {
                // Check if this node itself is a sync access
                if args.is_empty() && matches!(method.as_str(), "read" | "write" | "lock") {
                    if let Some(ty_name) = self.sync_type_of(object) {
                        out.push((ty_name, method.clone(), expr.span));
                    }
                }
                // ST1: `staged()` has no expression-scoped form, so reaching one
                // here at all means it is outside a `with` source — this walk
                // never descends into a `with` binding.
                if args.is_empty() && method == "staged" && self.sync_type_of(object).is_some() {
                    self.staged_outside_with.push((
                        Self::sync_source_text(object).unwrap_or_else(|| "the box".to_string()),
                        expr.span,
                    ));
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
            ExprKind::If { cond, then_branch, else_branch, .. } => {
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

    /// The expression an `ensure` body statement leaves as its result — whatever
    /// the `else` handler binds the error branch of. A binding counts: writing
    /// `let n = s.close()` can fail in exactly the way a bare `s.close()` can,
    /// and taking only bare expressions left the handler's parameter untyped.
    fn ensure_body_value(kind: &StmtKind) -> Option<&Expr> {
        use rask_ast::stmt::StmtKind as SK;
        match kind {
            SK::Expr(e)
            | SK::Let { init: e, .. }
            | SK::Mut { init: e, .. }
            | SK::LetTuple { init: e, .. }
            | SK::MutTuple { init: e, .. }
            | SK::LetStruct { init: e, .. }
            | SK::Assign { value: e, .. } => Some(e),
            _ => None,
        }
    }

    /// ER4/ER3: report every `try` that would run in the `ensure`'s own frame.
    ///
    /// A closure or `spawn` body nested in here is its own frame with its own
    /// caller, so the walk stops at those. Everything else recurses.
    fn report_try_in_ensure(
        body: &[Stmt],
        region: &'static str,
        errors: &mut Vec<TypeError>,
    ) {
        for stmt in body {
            Self::scan_stmt_for_try(stmt, region, errors);
        }
    }

    fn scan_stmt_for_try(stmt: &Stmt, region: &'static str, errors: &mut Vec<TypeError>) {
        use rask_ast::stmt::StmtKind as SK;
        let mut exprs: Vec<&Expr> = Vec::new();
        let mut bodies: Vec<&Vec<Stmt>> = Vec::new();
        match &stmt.kind {
            SK::Let { init, .. } | SK::Mut { init, .. } => exprs.push(init),
            SK::LetTuple { init, .. } | SK::MutTuple { init, .. } => exprs.push(init),
            SK::LetStruct { init, .. } => exprs.push(init),
            SK::Expr(e) => exprs.push(e),
            SK::Return(Some(e)) => exprs.push(e),
            SK::Assign { target, value, .. } => {
                exprs.push(target);
                exprs.push(value);
            }
            SK::While { cond, body, .. } => {
                exprs.push(cond);
                bodies.push(body);
            }
            SK::WhileLet { expr, body, .. } => {
                exprs.push(expr);
                bodies.push(body);
            }
            SK::For { iter, body, .. } => {
                exprs.push(iter);
                bodies.push(body);
            }
            SK::Loop { body, .. } | SK::Comptime(body) => bodies.push(body),
            SK::ComptimeFor { iter, body, .. } => {
                exprs.push(iter);
                bodies.push(body);
            }
            // A nested `ensure` reports against its own region on its own pass.
            _ => {}
        }
        for e in exprs {
            Self::scan_expr_for_try(e, region, errors);
        }
        for b in bodies {
            for s in b {
                Self::scan_stmt_for_try(s, region, errors);
            }
        }
    }

    fn scan_expr_for_try(expr: &Expr, region: &'static str, errors: &mut Vec<TypeError>) {
        use rask_ast::expr::ExprKind as EK;
        let mut kids: Vec<&Expr> = Vec::new();
        let mut bodies: Vec<&Vec<Stmt>> = Vec::new();
        match &expr.kind {
            EK::Try { .. } => {
                errors.push(TypeError::TryInEnsure { region, span: expr.span });
                return;
            }
            // Own frame, own caller — `try` there is the callee's business.
            EK::Closure { .. } | EK::Spawn { .. } => return,

            EK::Binary { left, right, .. } => {
                kids.push(left);
                kids.push(right);
            }
            EK::Unary { operand, .. } => kids.push(operand),
            EK::Call { func, args } => {
                kids.push(func);
                kids.extend(args.iter().map(|a| &a.expr));
            }
            EK::MethodCall { object, args, .. } => {
                kids.push(object);
                kids.extend(args.iter().map(|a| &a.expr));
            }
            EK::Field { object, .. } | EK::OptionalField { object, .. } => kids.push(object),
            EK::IsPresent { expr: inner, .. }
            | EK::Unwrap { expr: inner, .. }
            | EK::Cast { expr: inner, .. }
            | EK::Convert { expr: inner, .. }
            | EK::IsPattern { expr: inner, .. } => kids.push(inner),
            EK::GuardPattern { expr: inner, else_branch, .. } => {
                kids.push(inner);
                kids.push(else_branch);
            }
            EK::DynamicField { object, field_expr } => {
                kids.push(object);
                kids.push(field_expr);
            }
            EK::Index { object, index } => {
                kids.push(object);
                kids.push(index);
            }
            EK::NullCoalesce { value, default } => {
                kids.push(value);
                kids.push(default);
            }
            EK::Catch { value, .. } => kids.push(value),
            EK::Take { place } => kids.push(place),
            EK::If { cond, then_branch, else_branch, .. } => {
                kids.push(cond);
                kids.push(then_branch);
                if let Some(e) = else_branch {
                    kids.push(e);
                }
            }
            EK::IfLet { expr: scrut, then_branch, else_branch, .. } => {
                kids.push(scrut);
                kids.push(then_branch);
                if let Some(e) = else_branch {
                    kids.push(e);
                }
            }
            EK::Match { scrutinee, arms } => {
                kids.push(scrutinee);
                kids.extend(arms.iter().map(|a| a.body.as_ref()));
            }
            EK::Range { start, end, .. } => {
                kids.extend(start.iter().map(|b| b.as_ref()));
                kids.extend(end.iter().map(|b| b.as_ref()));
            }
            EK::StructLit { fields, .. } => kids.extend(fields.iter().map(|f| &f.value)),
            EK::Array(items) | EK::Tuple(items) => kids.extend(items.iter()),
            EK::ArrayRepeat { value, count } => {
                kids.push(value);
                kids.push(count);
            }
            EK::WithAs { bindings, body } => {
                kids.extend(bindings.iter().map(|b| &b.source));
                bodies.push(body);
            }
            EK::Block(body)
            | EK::Loop { body, .. }
            | EK::UsingBlock { body, .. }
            | EK::Unsafe { body }
            | EK::Comptime { body } => bodies.push(body),
            EK::Assert { condition, message } | EK::Check { condition, message } => {
                kids.push(condition);
                kids.extend(message.iter().map(|m| m.as_ref()));
            }
            EK::StringInterp(segments) => {
                use rask_ast::expr::StringSegment;
                for seg in segments {
                    if let StringSegment::Expr(e, _) = seg {
                        kids.push(e);
                    }
                }
            }
            _ => {}
        }
        for k in kids {
            Self::scan_expr_for_try(k, region, errors);
        }
        for b in bodies {
            for s in b {
                Self::scan_stmt_for_try(s, region, errors);
            }
        }
    }


}
