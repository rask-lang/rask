// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Expression type inference and specific type checks.

use rask_ast::expr::{BinOp, CallArg, Expr, ExprKind, MatchArm, Pattern};
use rask_ast::stmt::StmtKind;
use rask_ast::{NodeId, Span};
use rask_resolve::{SymbolId, SymbolKind};

use super::type_defs::TypeDef;
use super::borrow::BorrowMode;
use super::errors::TypeError;
use super::inference::{LiteralKind, TypeConstraint};
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

/// Split a type argument string by commas, respecting nested angle brackets.
/// "Map<string, bool>, i64" → ["Map<string, bool>", "i64"]
fn split_type_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                args.push(s[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        args.push(last.to_string());
    }
    args
}

/// Parse a type argument string into a Type, handling nested generics.
/// "Map<string, bool>" → UnresolvedGeneric { name: "Map", args: [string, bool] }
/// "Route" → UnresolvedNamed("Route")
fn parse_type_arg(s: &str) -> Type {
    if let Some(open) = s.find('<') {
        let base = &s[..open];
        let inner = &s[open+1..s.len()-1];
        let args = split_type_args(inner)
            .into_iter()
            .map(|a| GenericArg::Type(Box::new(parse_type_arg(&a))))
            .collect();
        Type::UnresolvedGeneric {
            name: base.to_string(),
            args,
        }
    } else {
        Type::UnresolvedNamed(s.to_string())
    }
}

impl TypeChecker {
    /// Infer expression type with an expected type hint for unsuffixed literals.
    /// Falls through to normal inference for non-literal or suffixed expressions.
    pub(super) fn infer_expr_expecting(&mut self, expr: &Expr, expected: &Type) -> Type {
        match &expr.kind {
            ExprKind::Int(_, None) if Self::is_integer_type(expected) => {
                let ty = expected.clone();
                self.node_types.insert(expr.id, ty.clone());
                return ty;
            }
            ExprKind::Float(_, None) if Self::is_float_type(expected) => {
                let ty = expected.clone();
                self.node_types.insert(expr.id, ty.clone());
                return ty;
            }
            _ => {}
        }
        self.infer_expr(expr)
    }

    fn is_integer_type(ty: &Type) -> bool {
        matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                    | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128)
    }

    fn is_float_type(ty: &Type) -> bool {
        matches!(ty, Type::F32 | Type::F64)
    }

    pub(super) fn infer_expr(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            // Literals
            ExprKind::Int(_, suffix) => {
                use rask_ast::token::IntSuffix;
                match suffix {
                    Some(IntSuffix::I8) => Type::I8,
                    Some(IntSuffix::I16) => Type::I16,
                    Some(IntSuffix::I32) => Type::I32,
                    Some(IntSuffix::I64) => Type::I64,
                    Some(IntSuffix::I128) => Type::I128,
                    Some(IntSuffix::Isize) => Type::I64,
                    Some(IntSuffix::U8) => Type::U8,
                    Some(IntSuffix::U16) => Type::U16,
                    Some(IntSuffix::U32) => Type::U32,
                    Some(IntSuffix::U64) => Type::U64,
                    Some(IntSuffix::U128) => Type::U128,
                    Some(IntSuffix::Usize) => Type::U64,
                    None => self.ctx.fresh_literal_var(LiteralKind::Integer),
                }
            }
            ExprKind::Float(_, suffix) => {
                use rask_ast::token::FloatSuffix;
                match suffix {
                    Some(FloatSuffix::F32) => Type::F32,
                    Some(FloatSuffix::F64) => Type::F64,
                    None => self.ctx.fresh_literal_var(LiteralKind::Float),
                }
            }
            ExprKind::String(_) | ExprKind::StringInterp(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Null => Type::RawPtr(Box::new(self.ctx.fresh_var())),

            ExprKind::Ident(name) => {
                // D1: use after discard is a compile error
                if let Some(&discard_span) = self.discarded_bindings.get(name.as_str()) {
                    self.errors.push(TypeError::UseAfterDiscard {
                        name: name.clone(),
                        discarded_at: discard_span,
                        span: expr.span,
                    });
                    return Type::Error;
                }
                if let Some(ty) = self.lookup_local(name) {
                    ty
                } else if let Some(&sym_id) = self.resolved.resolutions.get(&expr.id) {
                    self.get_symbol_type(sym_id)
                } else if let Some(type_id) = self.types.get_type_id(name) {
                    // Imported type name (struct/enum) without resolver entry
                    Type::Named(type_id)
                } else {
                    self.errors.push(TypeError::UndefinedName {
                        name: name.clone(),
                        span: expr.span,
                    });
                    Type::Error
                }
            }

            ExprKind::Binary { op, left, right } => {
                self.check_binary(*op, left, right, expr.span)
            }

            ExprKind::Unary { op, operand } => {
                let operand_ty = self.infer_expr(operand);
                match op {
                    rask_ast::expr::UnaryOp::Deref => {
                        self.unsafe_ops.push((expr.span, super::UnsafeCategory::PointerDeref));
                        if !self.in_unsafe {
                            self.errors.push(TypeError::UnsafeRequired {
                                operation: "pointer dereference".to_string(),
                                span: expr.span,
                            });
                        }
                        // *ptr where ptr: *T should yield T
                        let resolved = self.ctx.apply(&operand_ty);
                        match resolved {
                            Type::RawPtr(inner) => *inner,
                            _ => operand_ty,
                        }
                    }
                    _ => operand_ty,
                }
            }

            ExprKind::Call { func, args } => self.check_call(expr.id, func, args, expr.span),

            ExprKind::MethodCall {
                object,
                method,
                args,
                type_args,
            } => self.check_method_call(object, method, args, type_args.as_deref(), expr.span),

            ExprKind::Field { object, field } => self.check_field_access(object, field, expr.span),

            ExprKind::DynamicField { object, field_expr } => {
                // Infer both sub-expressions; actual comptime field resolution
                // happens in the comptime pass — here we just type-check children.
                let _obj_ty = self.infer_expr(object);
                let _field_ty = self.infer_expr(field_expr);
                Type::Error
            }

            ExprKind::Index { object, index } => {
                let raw_obj_ty = self.infer_expr(object);
                let _idx_ty = self.infer_expr(index);

                // Check if indexing with a range (slicing)
                let is_range = matches!(index.kind, rask_ast::expr::ExprKind::Range { .. });

                // Resolve type variables so Generic{} is visible
                let obj_ty = self.ctx.apply(&raw_obj_ty);
                match &obj_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => {
                        if is_range {
                            Type::Slice(elem.clone())
                        } else {
                            *elem.clone()
                        }
                    }
                    Type::String => {
                        if is_range {
                            Type::String
                        } else {
                            Type::Char
                        }
                    }
                    // Vec<T>, Map<K,V>, Pool<T> — extract element type from first type arg
                    Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                        if let Some(GenericArg::Type(elem)) = args.first() {
                            if is_range {
                                Type::Slice(elem.clone())
                            } else {
                                *elem.clone()
                            }
                        } else {
                            self.ctx.fresh_var()
                        }
                    }
                    _ => self.ctx.fresh_var(),
                }
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                // Capture and clear the statement-position flag so it doesn't
                // leak into nested expressions (e.g. const x = if ... { ... }).
                let is_stmt = self.in_stmt_expr;
                self.in_stmt_expr = false;

                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, expr.span));

                // Type narrowing: if the condition is `opt is Some` (OPT10),
                // rebind `opt` to the inner type inside the then-branch.
                let narrowing = self.extract_is_some_narrowing(cond);

                if let Some((ref var_name, ref inner_ty)) = narrowing {
                    self.push_scope();
                    self.define_local(var_name.clone(), inner_ty.clone());
                }
                let then_ty = self.infer_expr(then_branch);
                if narrowing.is_some() {
                    self.pop_scope();
                }

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(else_branch);
                    let resolved_then = self.ctx.apply(&then_ty);
                    let resolved_else = self.ctx.apply(&else_ty);
                    // Never coerces to any type (CF32) — don't constrain
                    if matches!(resolved_else, Type::Never) {
                        then_ty
                    } else if matches!(resolved_then, Type::Never) {
                        else_ty
                    } else if is_stmt {
                        // Statement position: value is discarded, branches
                        // don't need to agree. Return unit.
                        Type::Unit
                    } else {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            then_ty.clone(),
                            else_ty,
                            expr.span,
                        ));
                        then_ty
                    }
                } else {
                    Type::Unit
                }
            }

            ExprKind::IfLet {
                pattern,
                then_branch,
                else_branch,
                expr: value,
            } => {
                let value_ty = self.infer_expr(value);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);
                for (name, ty) in bindings {
                    if name.is_empty() {
                        // OPT10: `if opt is Some` with no explicit binding —
                        // rebind the original variable to the inner type.
                        if let ExprKind::Ident(var_name) = &value.kind {
                            self.define_local(var_name.clone(), ty);
                        }
                    } else {
                        self.define_local(name, ty);
                    }
                }
                let then_ty = self.infer_expr(then_branch);
                self.pop_scope();
                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(else_branch);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        then_ty.clone(),
                        else_ty,
                        expr.span,
                    ));
                }
                then_ty
            }

            ExprKind::GuardPattern {
                expr: value,
                pattern,
                else_branch,
            } => {
                let value_ty = self.infer_expr(value);

                // Check that else branch diverges (returns Never)
                let else_ty = self.infer_expr(else_branch);
                let resolved_else = self.ctx.apply(&else_ty);
                if !matches!(resolved_else, Type::Never) {
                    self.errors.push(TypeError::GuardElseMustDiverge {
                        found: resolved_else,
                        span: else_branch.span,
                    });
                }

                // Check pattern and extract bindings
                // Note: Bindings are NOT added to scope here - they're added by the stmt handler
                // We just return them via the expression type mechanism
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);

                // For a guard pattern like `const v = opt is Some else { return }`,
                // the expression itself evaluates to the inner type
                // The pattern binding happens at the statement level
                if let Some((_, inner_ty)) = bindings.first() {
                    inner_ty.clone()
                } else {
                    // If no explicit bindings, extract inner type from Option/Result
                    // This handles patterns like `Some` or `Ok` without explicit field binding
                    let resolved_value_ty = self.ctx.apply(&value_ty);
                    match &resolved_value_ty {
                        Type::Option(inner) => *inner.clone(),
                        Type::Result { ok, .. } => *ok.clone(),
                        _ => Type::Unit,
                    }
                }
            }

            ExprKind::IsPattern { expr: value, pattern } => {
                let value_ty = self.infer_expr(value);
                let _bindings = self.check_pattern(pattern, &value_ty, expr.span);
                Type::Bool
            }

            ExprKind::Match { scrutinee, arms } => {
                let is_stmt = self.in_stmt_expr;
                self.in_stmt_expr = false;

                let scrutinee_ty = self.infer_expr(scrutinee);
                let result_ty = self.ctx.fresh_var();
                for arm in arms {
                    self.push_scope();
                    let bindings = self.check_pattern(&arm.pattern, &scrutinee_ty, expr.span);
                    for (name, ty) in bindings {
                        self.define_local(name, ty);
                    }
                    if let Some(guard) = &arm.guard {
                        let guard_ty = self.infer_expr(guard);
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            Type::Bool,
                            guard_ty,
                            expr.span,
                        ));
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.pop_scope();
                    let resolved_arm_ty = self.ctx.apply(&arm_ty);
                    // In statement position, arm types don't need to agree.
                    if !is_stmt && !matches!(resolved_arm_ty, Type::Never) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            result_ty.clone(),
                            arm_ty,
                            expr.span,
                        ));
                    }
                }

                // Exhaustiveness check for enum scrutinees
                self.check_match_exhaustiveness(&scrutinee_ty, arms, expr.span);

                if is_stmt { Type::Unit } else { result_ty }
            }

            ExprKind::Block(stmts) => {
                self.push_scope();
                for stmt in stmts {
                    self.check_stmt(stmt);
                }
                let result = if let Some(last) = stmts.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => self.infer_expr(e),
                        StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => {
                            Type::Never
                        }
                        _ => Type::Unit,
                    }
                } else {
                    Type::Unit
                };
                self.pop_scope();
                result
            }

            ExprKind::StructLit { name, fields, .. } => {
                if let Some(ty) = self.types.lookup(name) {
                    if let Type::Named(type_id) = &ty {
                        let (struct_fields, type_params, private_fields) = match self.types.get(*type_id) {
                            Some(TypeDef::Struct { fields: sf, type_params: tp, private_fields: pf, .. }) => {
                                (sf.clone(), tp.clone(), pf.clone())
                            }
                            _ => (vec![], vec![], vec![]),
                        };

                        // V5: check private fields in struct literal construction
                        let is_self_type = self.current_self_type.as_ref()
                            .is_some_and(|st| matches!(st, Type::Named(id) if id == type_id));
                        if !is_self_type {
                            for field_init in fields.iter() {
                                if private_fields.contains(&field_init.name) {
                                    self.errors.push(TypeError::PrivateFieldAccess {
                                        ty: name.clone(),
                                        field: field_init.name.clone(),
                                        span: field_init.value.span,
                                    });
                                }
                            }
                        }

                        if type_params.is_empty() {
                            // Non-generic struct: constrain directly
                            for field_init in fields {
                                let expected_field = struct_fields.iter()
                                    .find(|(n, _)| n == &field_init.name)
                                    .map(|(_, t)| t.clone());
                                let field_ty = if let Some(ref exp) = expected_field {
                                    self.infer_expr_expecting(&field_init.value, exp)
                                } else {
                                    self.infer_expr(&field_init.value)
                                };
                                if let Some(expected) = expected_field {
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        expected,
                                        field_ty,
                                        field_init.value.span,
                                    ));
                                }
                            }
                            ty
                        } else {
                            // Generic struct: create fresh vars, substitute into fields
                            let fresh_args: Vec<GenericArg> = type_params.iter()
                                .map(|_| GenericArg::Type(Box::new(self.ctx.fresh_var())))
                                .collect();
                            let subst = Self::build_type_param_subst(&type_params, &fresh_args);

                            for field_init in fields {
                                let substituted = struct_fields.iter()
                                    .find(|(n, _)| n == &field_init.name)
                                    .map(|(_, t)| Self::substitute_type_params(t, &subst));
                                let field_ty = if let Some(ref sub) = substituted {
                                    self.infer_expr_expecting(&field_init.value, sub)
                                } else {
                                    self.infer_expr(&field_init.value)
                                };
                                if let Some(sub) = substituted {
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        sub,
                                        field_ty,
                                        field_init.value.span,
                                    ));
                                }
                            }

                            Type::Generic { base: *type_id, args: fresh_args }
                        }
                    } else {
                        ty
                    }
                } else {
                    Type::UnresolvedNamed(name.clone())
                }
            }

            ExprKind::Array(elements) => {
                if elements.is_empty() {
                    let elem_ty = self.ctx.fresh_var();
                    Type::Array {
                        elem: Box::new(elem_ty),
                        len: 0,
                    }
                } else {
                    let first_ty = self.infer_expr(&elements[0]);
                    for elem in &elements[1..] {
                        let elem_ty = self.infer_expr(elem);
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            first_ty.clone(),
                            elem_ty,
                            expr.span,
                        ));
                    }
                    Type::Array {
                        elem: Box::new(first_ty),
                        len: elements.len(),
                    }
                }
            }

            ExprKind::Tuple(elements) => {
                let elem_types: Vec<_> = elements.iter().map(|e| self.infer_expr(e)).collect();
                // Empty tuple () is Unit type
                if elem_types.is_empty() {
                    Type::Unit
                } else {
                    Type::Tuple(elem_types)
                }
            }

            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.infer_expr(s);
                }
                if let Some(e) = end {
                    self.infer_expr(e);
                }
                Type::UnresolvedNamed("Range".to_string())
            }

            ExprKind::Try { expr: inner, ref else_clause } => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Option(inner) => {
                        if else_clause.is_some() {
                            // try...else on Option doesn't make sense (no error value)
                            self.errors.push(TypeError::TryOnNonResult {
                                found: resolved.clone(),
                                span: expr.span,
                            });
                            return Type::Error;
                        }
                        *inner.clone()
                    }
                    Type::Result { ok, err } => {
                        if let Some(ec) = else_clause {
                            // try...else: bind error, infer handler, unify handler type with
                            // function's error return type
                            self.push_scope();
                            if let Some(scope) = self.local_types.last_mut() {
                                scope.insert(ec.error_binding.clone(), (*err.clone(), true));
                            }
                            let handler_ty = self.infer_expr(&ec.body);
                            self.pop_scope();
                            // Handler produces the transformed error; unify with function's error type
                            if let Some(return_ty) = &self.current_return_type {
                                let resolved_ret = self.ctx.apply(return_ty);
                                if self.accumulate_errors {
                                    // ER20: Collect instead of unifying
                                    self.inferred_errors.push(handler_ty);
                                } else if let Type::Result { err: ret_err, .. } = &resolved_ret {
                                    let _ = self.unify(&handler_ty, ret_err, expr.span);
                                } else if matches!(resolved_ret, Type::Var(_)) {
                                    // GC7/ER20: Return type is inferred — make it Result
                                    let ret_ok = self.ctx.fresh_var();
                                    let ret_result = Type::Result {
                                        ok: Box::new(ret_ok),
                                        err: Box::new(handler_ty),
                                    };
                                    let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                                }
                            }
                        } else {
                            // Plain try: unify error types directly
                            if let Some(return_ty) = &self.current_return_type {
                                let resolved_ret = self.ctx.apply(return_ty);
                                if self.accumulate_errors {
                                    // ER20: Collect instead of unifying
                                    self.inferred_errors.push(*err.clone());
                                } else if let Type::Result { err: ret_err, .. } = &resolved_ret {
                                    let _ = self.unify(err, ret_err, expr.span);
                                } else if matches!(resolved_ret, Type::Var(_)) {
                                    // GC7/ER20: Return type is inferred — make it Result
                                    let ret_ok = self.ctx.fresh_var();
                                    let ret_result = Type::Result {
                                        ok: Box::new(ret_ok),
                                        err: err.clone(),
                                    };
                                    let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                                }
                            }
                        }
                        *ok.clone()
                    }
                    Type::Var(_) => {
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            match &resolved_ret {
                                Type::Option(_) => {
                                    let inner_opt_ty = self.ctx.fresh_var();
                                    let option_ty = Type::Option(Box::new(inner_opt_ty.clone()));
                                    let _ = self.unify(&inner_ty, &option_ty, expr.span);
                                    inner_opt_ty
                                }
                                Type::Result { err: ret_err, .. } => {
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let result_ty = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty.clone()),
                                    };
                                    let _ = self.unify(&inner_ty, &result_ty, expr.span);
                                    if let Some(ec) = else_clause {
                                        // try...else with unresolved inner: bind error, infer handler,
                                        // unify handler type with function's error return type
                                        self.push_scope();
                                        if let Some(scope) = self.local_types.last_mut() {
                                            scope.insert(ec.error_binding.clone(), (err_ty, true));
                                        }
                                        let handler_ty = self.infer_expr(&ec.body);
                                        self.pop_scope();
                                        if self.accumulate_errors {
                                            self.inferred_errors.push(handler_ty);
                                        } else {
                                            let _ = self.unify(&handler_ty, ret_err, expr.span);
                                        }
                                    } else if self.accumulate_errors {
                                        // ER20: Collect instead of unifying with return
                                        self.inferred_errors.push(err_ty);
                                    } else {
                                        let _ = self.unify(&err_ty, ret_err, expr.span);
                                    }
                                    ok_ty
                                }
                                Type::Var(_) => {
                                    // GC7/ER20: Both inner and return type unresolved.
                                    // Create Result structure — try implies error propagation.
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let inner_result = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty.clone()),
                                    };
                                    let _ = self.unify(&inner_ty, &inner_result, expr.span);
                                    if let Some(ec) = else_clause {
                                        // try...else with both types unresolved
                                        self.push_scope();
                                        if let Some(scope) = self.local_types.last_mut() {
                                            scope.insert(ec.error_binding.clone(), (err_ty, true));
                                        }
                                        let handler_ty = self.infer_expr(&ec.body);
                                        self.pop_scope();
                                        if self.accumulate_errors {
                                            self.inferred_errors.push(handler_ty.clone());
                                        }
                                        let ret_ok = self.ctx.fresh_var();
                                        let ret_result = Type::Result {
                                            ok: Box::new(ret_ok),
                                            err: Box::new(handler_ty),
                                        };
                                        let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                                    } else if self.accumulate_errors {
                                        // ER20: Collect instead of unifying with return
                                        self.inferred_errors.push(err_ty);
                                    } else {
                                        let ret_ok = self.ctx.fresh_var();
                                        let ret_result = Type::Result {
                                            ok: Box::new(ret_ok),
                                            err: Box::new(err_ty),
                                        };
                                        let _ = self.unify(&resolved_ret, &ret_result, expr.span);
                                    }
                                    ok_ty
                                }
                                _ => {
                                    self.errors.push(TypeError::TryInNonPropagatingContext {
                                        return_ty: resolved_ret.clone(),
                                        span: expr.span,
                                    });
                                    Type::Error
                                }
                            }
                        } else {
                            self.errors.push(TypeError::TryOutsideFunction { span: expr.span });
                            Type::Error
                        }
                    }
                    _ => {
                        self.errors.push(TypeError::TryOnNonResult {
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Unwrap { expr: inner, message: _ } => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Option(inner) => {
                        // Extract the inner type from Option<T>
                        *inner.clone()
                    }
                    Type::Result { ok, err: _ } => {
                        // Extract the Ok type from Result<T, E>
                        *ok.clone()
                    }
                    Type::Var(_) => {
                        // Don't constrain yet - let later context determine if Option or Result
                        self.ctx.fresh_var()
                    }
                    _ => {
                        self.errors.push(TypeError::Mismatch {
                            expected: Type::Option(Box::new(self.ctx.fresh_var())),
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Closure { params, ret_ty: declared_ret, body } => {
                let param_types: Vec<_> = params
                    .iter()
                    .map(|p| {
                        p.ty.as_ref()
                            .and_then(|t| parse_type_string(t, &self.types).ok())
                            .unwrap_or_else(|| self.ctx.fresh_var())
                    })
                    .collect();

                // ESAD Phase 2: Check for aliasing violations in closure body
                self.check_closure_aliasing(params, body);

                // Save enclosing return type — `return` inside a closure
                // returns from the closure, not the enclosing function
                let outer_return_type = self.current_return_type.take();
                let outer_accumulate = self.accumulate_errors;
                let outer_inferred_errors = std::mem::take(&mut self.inferred_errors);
                self.accumulate_errors = false;
                let closure_return_type = self.ctx.fresh_var();
                self.current_return_type = Some(closure_return_type.clone());

                let inferred_ret = self.infer_expr(body);

                self.current_return_type = outer_return_type;
                self.accumulate_errors = outer_accumulate;
                self.inferred_errors = outer_inferred_errors;

                // Unify the closure body type with the return type from
                // return statements (if any)
                let _ = self.unify(&inferred_ret, &closure_return_type, expr.span);

                // Check declared return type if present
                let ret_ty = if let Some(declared) = declared_ret {
                    let expected_ret = parse_type_string(declared, &self.types)
                        .unwrap_or(Type::Error);
                    if let Err(err) = self.unify(&closure_return_type, &expected_ret, expr.span) {
                        self.errors.push(err);
                    }
                    expected_ret
                } else {
                    closure_return_type
                };

                Type::Fn {
                    params: param_types,
                    ret: Box::new(ret_ty),
                }
            }

            ExprKind::Cast { expr: inner, ty } => {
                let inner_ty = self.infer_expr(inner);
                let target = parse_type_string(ty, &self.types).unwrap_or(Type::Error);

                // Validate trait satisfaction for `as any Trait` casts
                if let Type::TraitObject { ref trait_name } = target {
                    if !matches!(inner_ty, Type::Var(_) | Type::Error) {
                        if !crate::traits::implements_trait(&self.types, &inner_ty, trait_name) {
                            let ty_desc = match &inner_ty {
                                Type::Named(id) => self.types.type_name(*id),
                                other => format!("{}", other),
                            };
                            self.errors.push(TypeError::TraitNotSatisfied {
                                ty: ty_desc,
                                trait_name: trait_name.clone(),
                                span: expr.span,
                            });
                        }
                    }
                }

                target
            }

            ExprKind::Loop { body, .. } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                // Loop-as-expression gets its type from break values
                self.ctx.fresh_var()
            }

            ExprKind::Unsafe { body } => {
                let was_unsafe = self.in_unsafe;
                self.in_unsafe = true;
                for stmt in body {
                    self.check_stmt(stmt);
                }
                let result = if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        self.infer_expr(e)
                    } else {
                        Type::Unit
                    }
                } else {
                    Type::Unit
                };
                self.in_unsafe = was_unsafe;
                result
            }

            ExprKind::Comptime { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            ExprKind::Spawn { body } => {
                // Spawn blocks are like anonymous functions - they have their own return type
                let outer_return_type = self.current_return_type.take();
                let outer_accumulate = self.accumulate_errors;
                let outer_inferred_errors = std::mem::take(&mut self.inferred_errors);
                self.accumulate_errors = false;
                let spawn_return_type = self.ctx.fresh_var();
                self.current_return_type = Some(spawn_return_type.clone());

                // Check all statements except the last (which we infer separately)
                let last_idx = body.len().saturating_sub(1);
                for (i, stmt) in body.iter().enumerate() {
                    if i < last_idx {
                        self.check_stmt(stmt);
                    }
                }

                // Infer the return type from the last statement (only process once)
                let inner_type = if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => self.infer_expr(e),
                        StmtKind::Return(_) => {
                            self.check_stmt(last);
                            Type::Never
                        }
                        _ => {
                            self.check_stmt(last);
                            Type::Unit
                        }
                    }
                } else {
                    Type::Unit
                };

                self.ctx.add_constraint(TypeConstraint::Equal(
                    spawn_return_type.clone(),
                    inner_type,
                    expr.span,
                ));

                self.current_return_type = outer_return_type;
                self.accumulate_errors = outer_accumulate;
                self.inferred_errors = outer_inferred_errors;

                Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(spawn_return_type))],
                }
            }

            ExprKind::UsingBlock { name, args, body } => {
                // Validate context name
                match name.as_str() {
                    "Multitasking" | "MultiTasking" | "multitasking"
                    | "ThreadPool" | "threadpool" => {}
                    _ => {
                        self.errors.push(TypeError::UnknownContext {
                            name: name.clone(),
                            span: expr.span,
                        });
                    }
                }
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                // Check if the block ends with a diverging statement (return/break/continue)
                if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => {
                            return Type::Never;
                        }
                        _ => {}
                    }
                }
                Type::Unit
            }

            ExprKind::WithAs { bindings, body } => {
                self.push_scope();
                for binding in bindings {
                    let elem_ty = self.infer_expr(&binding.source);
                    self.define_local(binding.name.clone(), elem_ty);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                let result = if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => self.infer_expr(e),
                        StmtKind::Return(_) | StmtKind::Break { .. } | StmtKind::Continue(_) => {
                            Type::Never
                        }
                        _ => Type::Unit,
                    }
                } else {
                    Type::Unit
                };
                self.pop_scope();
                result
            }

            ExprKind::BlockCall { body, .. } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            ExprKind::ArrayRepeat { value, count } => {
                let elem_ty = self.infer_expr(value);
                self.infer_expr(count);
                // Extract literal size when available, otherwise use 0 as placeholder
                let len = match &count.kind {
                    ExprKind::Int(n, _) => *n as usize,
                    _ => 0,
                };
                Type::Array {
                    elem: Box::new(elem_ty),
                    len,
                }
            }


            ExprKind::NullCoalesce { value, default } => {
                let val_ty = self.infer_expr(value);
                let def_ty = self.infer_expr(default);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    val_ty,
                    Type::Option(Box::new(def_ty.clone())),
                    expr.span,
                ));
                def_ty
            }

            ExprKind::OptionalField { object, field } => {
                let inferred = self.infer_expr(object);
                let obj_ty = self.ctx.apply(&inferred);
                // ?. unwraps Option, accesses field, wraps in Option (flatten if already Option)
                let inner_ty = match &obj_ty {
                    Type::Option(inner) => *inner.clone(),
                    _ => obj_ty.clone(),
                };
                let field_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty: inner_ty,
                    field: field.clone(),
                    expected: field_ty.clone(),
                    span: expr.span,
                    self_type: self.current_self_type.clone(),
                });
                // Flatten: if field is already Option<T>, return Option<T> not Option<Option<T>>
                let resolved_field = self.ctx.apply(&field_ty);
                if matches!(&resolved_field, Type::Option(_)) {
                    resolved_field
                } else {
                    Type::Option(Box::new(field_ty))
                }
            }

            ExprKind::Select { arms, .. } => {
                if arms.is_empty() {
                    self.errors.push(TypeError::GenericError(
                        "select must have at least one arm".to_string(),
                        expr.span,
                    ));
                    return Type::Unit;
                }
                let mut result_ty: Option<Type> = None;
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, binding: _ } => {
                            self.infer_expr(channel);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.infer_expr(channel);
                            self.infer_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    let body_ty = self.infer_expr(&arm.body);
                    if let Some(ref prev) = result_ty {
                        let _ = self.unify(prev, &body_ty, arm.body.span);
                    } else {
                        result_ty = Some(body_ty);
                    }
                }
                result_ty.unwrap_or(Type::Unit)
            }

            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                let cond_ty = self.infer_expr(condition);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    cond_ty,
                    Type::Bool,
                    condition.span,
                ));
                if let Some(msg) = message {
                    let msg_ty = self.infer_expr(msg);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        msg_ty,
                        Type::String,
                        msg.span,
                    ));
                }
                Type::Unit
            }
        };

        self.node_types.insert(expr.id, ty.clone());
        ty
    }

    // ------------------------------------------------------------------------
    // Specific Type Checks
    // ------------------------------------------------------------------------

    pub(super) fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Type {
        let left_ty = self.infer_expr(left);
        let right_ty = self.infer_expr(right);

        self.ctx.add_constraint(TypeConstraint::Equal(
            left_ty.clone(),
            right_ty.clone(),
            span,
        ));

        match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Type::Bool,
            BinOp::And | BinOp::Or => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, left_ty, span));
                Type::Bool
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => left_ty,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => left_ty,
        }
    }

    pub(super) fn check_call(&mut self, call_id: NodeId, func: &Expr, args: &[CallArg], span: Span) -> Type {
        if let ExprKind::Ident(name) = &func.kind {
            // transmute(val) — reinterpret bits, requires unsafe
            if name == "transmute" {
                self.unsafe_ops.push((span, super::UnsafeCategory::Transmute));
                if !self.in_unsafe {
                    self.errors.push(TypeError::UnsafeRequired {
                        operation: "transmute".to_string(),
                        span,
                    });
                }
                if args.len() != 1 {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: 1,
                        found: args.len(),
                        span,
                    });
                }
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                return self.ctx.fresh_var();
            }

            if self.is_builtin_function(name) {
                for arg in args {
                    self.infer_expr(&arg.expr);
                }
                return match name.as_str() {
                    "panic" | "todo" | "unreachable" | "skip" => Type::Never,
                    "format" => Type::String,
                    _ => Type::Unit,
                };
            }

            // Nominal type constructor: UserId(42)
            if let Some(type_id) = self.types.get_type_id(name) {
                if let Some(TypeDef::NominalAlias { underlying, .. }) = self.types.get(type_id) {
                    let underlying = underlying.clone();
                    if args.len() != 1 {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: 1,
                            found: args.len(),
                            span,
                        });
                        for arg in args { self.infer_expr(&arg.expr); }
                        return Type::Error;
                    }
                    let arg_ty = self.infer_expr_expecting(&args[0].expr, &underlying);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        underlying,
                        arg_ty,
                        span,
                    ));
                    return Type::Named(type_id);
                }
            }
        }

        // Extern and unsafe function calls require unsafe context
        if let ExprKind::Ident(_) = &func.kind {
            if let Some(&sym_id) = self.resolved.resolutions.get(&func.id) {
                if let Some(sym) = self.resolved.symbols.get(sym_id) {
                    let unsafe_category = match &sym.kind {
                        SymbolKind::ExternFunction { .. } => Some(super::UnsafeCategory::ExternCall),
                        SymbolKind::Function { is_unsafe: true, .. } => Some(super::UnsafeCategory::UnsafeFuncCall),
                        _ => None,
                    };
                    if let Some(category) = unsafe_category {
                        self.unsafe_ops.push((span, category));
                        if !self.in_unsafe {
                            let operation = match category {
                                super::UnsafeCategory::ExternCall => "extern function call",
                                _ => "unsafe function call",
                            };
                            self.errors.push(TypeError::UnsafeRequired {
                                operation: operation.to_string(),
                                span,
                            });
                        }
                    }
                }
            }
        }

        // Call-site annotations (mutate/own) are optional — IDE shows ghost
        // annotations but the compiler doesn't require them (spec decision).
        // Validate when present, but don't error on missing annotations.
        self.check_call_annotations(func, args, span);

        // For generic function calls, create fresh type vars for each type param
        // and build a substitution map (param name → fresh var). After getting
        // the function type, we apply this substitution so that UnresolvedNamed("T")
        // in the param/return types becomes the fresh var. Constraint solving then
        // links the fresh vars to concrete types from the call arguments.
        let generic_subst: Option<Vec<(String, Type)>> = if let ExprKind::Ident(_) = &func.kind {
            // Resolve the callee's SymbolId, then look up its type params
            self.resolved.resolutions.get(&func.id)
                .and_then(|sym_id| self.fn_type_params.get(sym_id).cloned())
                .map(|type_params| {
                    let pairs: Vec<(String, Type)> = type_params.into_iter()
                        .map(|name| {
                            let fresh = self.ctx.fresh_var();
                            (name, fresh)
                        })
                        .collect();
                    let fresh_vars: Vec<Type> = pairs.iter().map(|(_, v)| v.clone()).collect();
                    self.pending_call_type_args.push((call_id, fresh_vars));
                    pairs
                })
        } else {
            None
        };

        let func_ty = self.infer_expr(func);

        // Substitute type param names with fresh vars in the function signature
        let func_ty = if let Some(ref pairs) = generic_subst {
            let subst: std::collections::HashMap<&str, Type> = pairs.iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            Self::substitute_type_params(&func_ty, &subst)
        } else {
            func_ty
        };

        match func_ty {
            Type::Fn { ref params, ref ret } => {
                if params.is_empty() && !args.is_empty() {
                    for arg in args { self.infer_expr(&arg.expr); }
                    return *ret.clone();
                }

                if params.len() != args.len() {
                    for arg in args { self.infer_expr(&arg.expr); }
                    self.errors.push(TypeError::ArityMismatch {
                        expected: params.len(),
                        found: args.len(),
                        span,
                    });
                    return Type::Error;
                }

                // Propagate expected param types to arguments
                let ret = *ret.clone();
                for (param, arg) in params.clone().iter().zip(args.iter()) {
                    // TR5: record implicit trait coercion for MIR boxing
                    if let Type::TraitObject { ref trait_name } = param {
                        let is_explicit_cast = matches!(
                            &arg.expr.kind,
                            ExprKind::Cast { ty, .. } if ty.starts_with("any ")
                        );
                        if !is_explicit_cast {
                            let arg_ty = self.infer_expr(&arg.expr);
                            if !matches!(arg_ty, Type::TraitObject { .. } | Type::Error) {
                                self.trait_coercions.insert(
                                    arg.expr.id,
                                    trait_name.clone(),
                                );
                            }
                        }
                    }
                    let arg_ty = self.infer_expr_expecting(&arg.expr, param);
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(param.clone(), arg_ty, span));
                }

                ret
            }
            Type::Var(_) => {
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let ret = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    func_ty,
                    Type::Fn {
                        params: arg_types,
                        ret: Box::new(ret.clone()),
                    },
                    span,
                ));
                ret
            }
            Type::Error => {
                for arg in args { self.infer_expr(&arg.expr); }
                Type::Error
            }
            _ => {
                for arg in args { self.infer_expr(&arg.expr); }
                self.ctx.fresh_var()
            }
        }
    }

    pub(super) fn is_builtin_function(&self, name: &str) -> bool {
        matches!(name, "println" | "print" | "panic" | "todo" | "unreachable"
            | "assert" | "debug" | "format" | "fence" | "compiler_fence"
            | "assert_eq" | "skip" | "expect_fail")
    }

    /// Validate that call-site annotations match parameter declarations.
    fn check_call_annotations(&mut self, func: &Expr, args: &[CallArg], _span: Span) {
        use rask_ast::expr::ArgMode;
        use rask_resolve::SymbolKind;

        // Get the function's symbol ID
        let sym_id = if let ExprKind::Ident(_) = &func.kind {
            self.resolved.resolutions.get(&func.id).copied()
        } else {
            None
        };

        let Some(sym_id) = sym_id else { return };
        let Some(sym) = self.resolved.symbols.get(sym_id) else { return };

        // Get parameter symbols
        let param_ids = match &sym.kind {
            SymbolKind::Function { params, .. } => params.clone(),
            _ => return,
        };

        // Validate each argument annotation
        for (i, (arg, &param_id)) in args.iter().zip(param_ids.iter()).enumerate() {
            let Some(param_sym) = self.resolved.symbols.get(param_id) else { continue };
            let (is_take, is_mutate) = match &param_sym.kind {
                SymbolKind::Parameter { is_take, is_mutate } => (*is_take, *is_mutate),
                _ => continue,
            };

            let param_name = &param_sym.name;

            match (&arg.mode, is_take, is_mutate) {
                // Missing annotations are OK — call-site markers are optional.
                // IDE shows ghost annotations for visibility (spec decision).
                (ArgMode::Default, true, _) => {}
                (ArgMode::Default, _, true) => {}
                // Correct annotations are fine
                (ArgMode::Own, true, _) => {}
                (ArgMode::Mutate, _, true) => {}
                // Wrong annotation type: `mutate` where `take` expected
                (ArgMode::Mutate, true, false) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "mutate".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // Unexpected `own` annotation on borrow param
                (ArgMode::Own, false, _) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "own".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // Unexpected `mutate` annotation on borrow param
                (ArgMode::Mutate, _, false) => {
                    self.errors.push(TypeError::UnexpectedAnnotation {
                        annotation: "mutate".to_string(),
                        param_name: param_name.clone(),
                        param_index: i,
                        span: arg.expr.span,
                    });
                }
                // All other cases are valid
                _ => {}
            }
        }
    }

    pub(super) fn check_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[CallArg],
        type_args: Option<&[String]>,
        span: Span,
    ) -> Type {
        // Check if this is a builtin module method call (e.g., fs.open)
        if let ExprKind::Ident(name) = &object.kind {
            if self.types.builtin_modules.is_module(name) {
                return self.check_module_method(name, method, args, type_args, span);
            }
        }

        // Type-level namespaces: Vec.new(), Map.new(), Rng.new(), Pool.new()
        // These are type names, not variables — skip ESAD borrow check and
        // emit UnresolvedNamed directly instead of calling infer_expr
        // (which would return Type::Error for unregistered type names).
        // Also handles generic forms like Vec<Route>.from().
        if let ExprKind::Ident(name) = &object.kind {
            // Extract base type name for generic types (e.g. "Vec<Route>" → "Vec")
            let base_name = name.split('<').next().unwrap_or(name);
            if matches!(base_name, "Vec" | "Map" | "Pool" | "Rng" | "Thread" | "ThreadPool" | "Mutex" | "Shared")
                || rask_stdlib::StubRegistry::load().get_type(base_name).is_some()
            {
                let obj_ty = if name.contains('<') {
                    // Parse generic args, respecting nested angle brackets:
                    // "Shared<Map<string, bool>>" → ["Map<string, bool>"]
                    let inner = &name[base_name.len()+1..name.len()-1];
                    let generic_args = split_type_args(inner)
                        .into_iter()
                        .map(|s| GenericArg::Type(Box::new(parse_type_arg(&s))))
                        .collect();
                    Type::UnresolvedGeneric {
                        name: base_name.to_string(),
                        args: generic_args,
                    }
                } else {
                    Type::UnresolvedNamed(name.clone())
                };
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let ret_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: obj_ty,
                    method: method.to_string(),
                    args: arg_types,
                    ret: ret_ty.clone(),
                    span,
                });
                return ret_ty;
            }
        }

        // User-defined enum variant construction: LexError.UnexpectedChar(c, line)
        // The name might be shadowed in scope by a same-named variant from
        // another enum (e.g. CompileError { LexError(LexError) }). Check the
        // type table directly — it's authoritative for type names.
        if let ExprKind::Ident(name) = &object.kind {
            // Look up the type table (not scope) to avoid variant-name shadowing.
            let variant_fields = self.types.get_type_id(name).and_then(|type_id| {
                if let Some(TypeDef::Enum { variants, .. }) = self.types.get(type_id) {
                    variants.iter()
                        .find(|(v, _)| v == method)
                        .map(|(_, fields)| (type_id, fields.clone()))
                } else {
                    None
                }
            });
            if let Some((type_id, field_types)) = variant_fields {
                let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();
                let instantiated = self.instantiate_type_vars(&field_types);
                for (arg_ty, field_ty) in arg_types.iter().zip(instantiated.iter()) {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        arg_ty.clone(),
                        field_ty.clone(),
                        span,
                    ));
                }
                if arg_types.len() != instantiated.len() {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: instantiated.len(),
                        found: arg_types.len(),
                        span,
                    });
                }
                return Type::Named(type_id);
            }
        }

        // ESAD Phase 1: Push borrow for the object being called
        if let ExprKind::Ident(var_name) = &object.kind {
            let mode = self.method_borrow_mode(var_name, method);

            // ESAD Phase 2: Check persistent borrow conflict for exclusive methods
            if matches!(mode, BorrowMode::Exclusive) {
                if let Some(borrow) = self.check_persistent_borrow_conflict(var_name) {
                    self.errors.push(TypeError::MutateBorrowedSource {
                        source_var: var_name.clone(),
                        view_var: borrow.view_var.clone(),
                        borrow_span: borrow.borrow_span,
                        mutate_span: object.span,
                    });
                }
            }

            self.push_borrow(var_name.clone(), mode, object.span);
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();

        // SP3: zero step on range is a compile error
        // SP1/SP2: step direction mismatch is a warning
        if method == "step" {
            let is_range = matches!(
                &self.ctx.apply(&obj_ty),
                Type::UnresolvedNamed(n) if n == "Range"
            );
            if is_range {
                if let Some(first_arg) = args.first() {
                    let is_zero = matches!(
                        &first_arg.expr.kind,
                        rask_ast::expr::ExprKind::Int(0, _)
                    );
                    if is_zero {
                        self.errors.push(TypeError::ZeroStep { span: first_arg.expr.span });
                    } else {
                        // SP1/SP2: check direction mismatch when literals are available
                        self.check_step_direction(object, first_arg);
                    }
                }
            }
        }

        // Raw pointer methods — resolve directly instead of through HasMethod constraints
        let resolved_obj = self.ctx.apply(&obj_ty);
        if let Type::RawPtr(ref inner) = resolved_obj {
            if let Some(ret) = self.check_raw_ptr_method(inner, method, &arg_types, span) {
                return ret;
            }
        }

        let ret_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasMethod {
            ty: obj_ty,
            method: method.to_string(),
            args: arg_types,
            ret: ret_ty.clone(),
            span,
        });

        ret_ty
    }

    /// SP1/SP2: Check for step direction mismatch on range literals.
    /// Only fires when start, end, and step are all integer literals.
    fn check_step_direction(&mut self, range_expr: &rask_ast::expr::Expr, step_arg: &rask_ast::expr::CallArg) {
        use rask_ast::expr::ExprKind;

        // Extract step value from literal
        let step_val: Option<i64> = match &step_arg.expr.kind {
            ExprKind::Int(v, _) => Some(*v),
            // After desugar, `-1` becomes `(1).neg()`
            ExprKind::MethodCall { object, method: neg_method, args: neg_args, .. }
                if neg_method == "neg" && neg_args.is_empty() =>
            {
                if let ExprKind::Int(v, _) = &object.kind {
                    Some(-v)
                } else {
                    None
                }
            }
            _ => None,
        };

        let step_val = match step_val {
            Some(v) => v,
            None => return, // non-literal step, can't check at compile time
        };

        // Extract start/end from Range expression
        let (start_val, end_val, range_span) = match &range_expr.kind {
            ExprKind::Range { start, end, .. } => {
                let s = start.as_ref().and_then(|e| {
                    if let ExprKind::Int(v, _) = &e.kind { Some(*v) } else { None }
                });
                let e = end.as_ref().and_then(|e| {
                    if let ExprKind::Int(v, _) = &e.kind { Some(*v) } else { None }
                });
                (s, e, range_expr.span)
            }
            _ => return,
        };

        let (start, end) = match (start_val, end_val) {
            (Some(s), Some(e)) => (s, e),
            _ => return, // non-literal bounds, can't check
        };

        // SP1: positive step requires start < end (ascending)
        // SP2: negative step requires start > end (descending)
        let mismatch = if step_val > 0 && start >= end {
            Some(("descending", "positive"))
        } else if step_val < 0 && start <= end {
            Some(("ascending", "negative"))
        } else {
            None
        };

        if let Some((range_dir, step_dir)) = mismatch {
            self.errors.push(TypeError::StepDirectionMismatch {
                range_span,
                step_span: step_arg.expr.span,
                range_direction: range_dir.to_string(),
                step_direction: step_dir.to_string(),
            });
        }
    }

    /// Resolve methods on raw pointer types (*T).
    /// Returns Some(return_type) if the method is recognized, None otherwise.
    fn check_raw_ptr_method(
        &mut self,
        inner: &Type,
        method: &str,
        _args: &[Type],
        span: Span,
    ) -> Option<Type> {
        let requires_unsafe = method != "is_null";
        if requires_unsafe {
            let category = match method {
                "add" | "sub" | "offset" => super::UnsafeCategory::PointerArithmetic,
                _ => super::UnsafeCategory::PointerMethod,
            };
            self.unsafe_ops.push((span, category));
            if !self.in_unsafe {
                self.errors.push(TypeError::UnsafeRequired {
                    operation: format!("pointer method .{}()", method),
                    span,
                });
            }
        }

        match method {
            "read" => Some(inner.clone()),
            "write" => Some(Type::Unit),
            "add" | "sub" | "offset" => Some(Type::RawPtr(Box::new(inner.clone()))),
            "is_null" => Some(Type::Bool),
            "cast" => Some(Type::RawPtr(Box::new(self.ctx.fresh_var()))),
            "is_aligned" => Some(Type::Bool),
            "is_aligned_to" => Some(Type::Bool),
            "align_offset" => Some(Type::I64),
            _ => None,
        }
    }

    pub(super) fn check_module_method(
        &mut self,
        module: &str,
        method: &str,
        args: &[CallArg],
        type_args: Option<&[String]>,
        span: Span,
    ) -> Type {
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(&a.expr)).collect();

        if let Some(sig) = self.types.builtin_modules.get_method(module, method) {
            // Check parameter count — skip for wildcard params (_Any accepts anything)
            let has_wildcard = sig.params.iter().any(|p| {
                matches!(p, Type::UnresolvedNamed(n) if n == "_Any")
            });
            if !has_wildcard && sig.params.len() != arg_types.len() {
                self.errors.push(TypeError::ArityMismatch {
                    expected: sig.params.len(),
                    found: arg_types.len(),
                    span,
                });
                return Type::Error;
            }

            // Check parameter types (skip _Any wildcards)
            if !has_wildcard {
                for (param_ty, arg_ty) in sig.params.iter().zip(arg_types.iter()) {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        param_ty.clone(),
                        arg_ty.clone(),
                        span,
                    ));
                }
            }

            // If explicit type args provided (e.g., json.decode<Foo>),
            // substitute them directly instead of using unconstrained fresh vars
            let ret = sig.ret.clone();
            if let Some(ta) = type_args {
                if ta.len() == 1 {
                    let explicit_ty = self.resolve_type_name(&ta[0], span);
                    return self.freshen_module_return_type_with(&ret, &explicit_ty);
                }
            }

            // Replace placeholder types with fresh vars for generic module methods
            self.freshen_module_return_type(&ret)
        } else {
            self.errors.push(TypeError::NoSuchMethod {
                ty: Type::UnresolvedNamed(module.to_string()),
                method: method.to_string(),
                span,
            });
            Type::Error
        }
    }

    /// Replace internal placeholder types (_JsonDecodeResult, _Any) with fresh type vars.
    pub(super) fn freshen_module_return_type(&mut self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n.starts_with('_') => self.ctx.fresh_var(),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_module_return_type(ok)),
                err: Box::new(self.freshen_module_return_type(err)),
            },
            Type::Option(inner) => Type::Option(Box::new(self.freshen_module_return_type(inner))),
            _ => ty.clone(),
        }
    }

    /// Replace internal placeholder types with an explicit type (from type args).
    fn freshen_module_return_type_with(&mut self, ty: &Type, explicit: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n.starts_with('_') => explicit.clone(),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_module_return_type_with(ok, explicit)),
                err: Box::new(self.freshen_module_return_type_with(err, explicit)),
            },
            Type::Option(inner) => {
                Type::Option(Box::new(self.freshen_module_return_type_with(inner, explicit)))
            }
            _ => ty.clone(),
        }
    }

    /// Resolve a type name string to a Type.
    fn resolve_type_name(&self, name: &str, _span: Span) -> Type {
        let ty = Type::UnresolvedNamed(name.to_string());
        self.resolve_named(&ty)
    }

    pub(super) fn check_field_access(&mut self, object: &Expr, field: &str, span: Span) -> Type {
        // Primitive type constants: u64.MAX, i32.MIN, etc.
        if let ExprKind::Ident(name) = &object.kind {
            if let Some(ty) = Self::primitive_type_constant(name, field) {
                return ty;
            }
            // G4: @binary struct SIZE/SIZE_BITS constants
            if matches!(field, "SIZE" | "SIZE_BITS") {
                if let Some(type_id) = self.types.get_type_id(name) {
                    if self.types.is_binary_type_by_id(type_id) {
                        return Type::U64;
                    }
                }
            }
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);

        // UN2: union field reads require unsafe (UN3: writes are safe)
        if !self.in_assign_target {
            if let Type::Named(type_id) = &obj_ty {
                if let Some(TypeDef::Union { .. }) = self.types.get(*type_id) {
                    self.unsafe_ops.push((span, super::UnsafeCategory::UnionFieldAccess));
                    if !self.in_unsafe {
                        self.errors.push(TypeError::UnsafeRequired {
                            operation: "union field access".to_string(),
                            span,
                        });
                    }
                }
            }
        }

        let field_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasField {
            ty: obj_ty,
            field: field.to_string(),
            expected: field_ty.clone(),
            span,
            self_type: self.current_self_type.clone(),
        });

        field_ty
    }

    pub(super) fn primitive_type_constant(type_name: &str, field: &str) -> Option<Type> {
        if !matches!(field, "MAX" | "MIN" | "EPSILON" | "NAN" | "INFINITY") {
            return None;
        }
        match type_name {
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Some(Type::UnresolvedNamed(type_name.to_string())),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Some(Type::UnresolvedNamed(type_name.to_string())),
            "f32" | "f64" => Some(Type::UnresolvedNamed(type_name.to_string())),
            _ => None,
        }
    }

    pub(super) fn get_symbol_type(&mut self, sym_id: SymbolId) -> Type {
        if let Some(ty) = self.symbol_types.get(&sym_id) {
            return ty.clone();
        }

        if let Some(sym) = self.resolved.symbols.get(sym_id) {
            match &sym.kind {
                SymbolKind::Function { ret_ty, params, .. } => {
                    let param_types: Vec<_> = params
                        .iter()
                        .filter_map(|pid| {
                            self.resolved.symbols.get(*pid).and_then(|p| {
                                p.ty.as_ref()
                                    .and_then(|t| parse_type_string(t, &self.types).ok())
                            })
                        })
                        .collect();
                    let ret = ret_ty
                        .as_ref()
                        .and_then(|t| parse_type_string(t, &self.types).ok())
                        .unwrap_or(Type::Unit);
                    return Type::Fn {
                        params: param_types,
                        ret: Box::new(ret),
                    };
                }
                SymbolKind::ExternFunction { params, ret_ty, .. } => {
                    let param_types: Vec<_> = params
                        .iter()
                        .filter_map(|p| parse_type_string(p, &self.types).ok())
                        .collect();
                    let ret = ret_ty
                        .as_ref()
                        .and_then(|t| parse_type_string(t, &self.types).ok())
                        .unwrap_or(Type::Unit);
                    return Type::Fn {
                        params: param_types,
                        ret: Box::new(ret),
                    };
                }
                SymbolKind::Variable { .. } | SymbolKind::Parameter { .. } => {
                    if let Some(ty_str) = &sym.ty {
                        if let Ok(ty) = parse_type_string(ty_str, &self.types) {
                            return ty;
                        }
                    }
                }
                SymbolKind::Struct { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::Enum { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::EnumVariant { enum_id } => {
                    if let Some(enum_sym) = self.resolved.symbols.get(*enum_id) {
                        let type_id = if enum_sym.span == Span::new(0, 0) {
                            match enum_sym.name.as_str() {
                                "Result" => self.types.get_result_type_id(),
                                "Option" => self.types.get_option_type_id(),
                                _ => None,
                            }
                        } else {
                            self.types.get_type_id(&enum_sym.name)
                        };

                        if let Some(id) = type_id {
                            let variant_fields = self.types.get(id).and_then(|def| {
                                if let TypeDef::Enum { variants, .. } = def {
                                    variants.iter()
                                        .find(|(n, _)| n == &sym.name)
                                        .map(|(_, fields)| fields.clone())
                                } else {
                                    None
                                }
                            });

                            if let Some(fields) = variant_fields {
                                if fields.is_empty() {
                                    return Type::Named(id);
                                } else {
                                    let (param_types, ret_type) = if Some(id) == self.types.get_result_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let e_var = self.ctx.fresh_var();
                                        let params = match sym.name.as_str() {
                                            "Ok" => vec![t_var.clone()],
                                            "Err" => vec![e_var.clone()],
                                            _ => fields.clone(),
                                        };
                                        let ret = Type::Result {
                                            ok: Box::new(t_var),
                                            err: Box::new(e_var),
                                        };
                                        (params, ret)
                                    } else if Some(id) == self.types.get_option_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let params = if sym.name == "Some" {
                                            vec![t_var.clone()]
                                        } else {
                                            vec![]
                                        };
                                        let ret = Type::Option(Box::new(t_var));
                                        (params, ret)
                                    } else {
                                        let instantiated = self.instantiate_type_vars(&fields);
                                        (instantiated, Type::Named(id))
                                    };

                                    return Type::Fn {
                                        params: param_types,
                                        ret: Box::new(ret_type),
                                    };
                                }
                            } else {
                                return Type::Named(id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let var = self.ctx.fresh_var();
        self.symbol_types.insert(sym_id, var.clone());
        var
    }

    /// Check that a match on an enum covers all variants.
    fn check_match_exhaustiveness(&mut self, scrutinee_ty: &Type, arms: &[MatchArm], span: Span) {
        let resolved = self.ctx.apply(scrutinee_ty);

        // Only check enums
        let type_id = match &resolved {
            Type::Named(id) => *id,
            _ => return,
        };

        let all_variants: Vec<String> = match self.types.get(type_id) {
            Some(TypeDef::Enum { variants, .. }) => {
                variants.iter().map(|(name, _)| name.clone()).collect()
            }
            _ => return,
        };

        // Collect covered variant names from patterns
        let mut has_wildcard = false;
        let mut covered = std::collections::HashSet::new();
        for arm in arms {
            self.collect_covered_variants(&arm.pattern, &mut covered, &mut has_wildcard, &all_variants);
        }

        if has_wildcard {
            return;
        }

        let missing: Vec<String> = all_variants
            .into_iter()
            .filter(|v| !covered.contains(v))
            .collect();

        if !missing.is_empty() {
            self.errors.push(TypeError::NonExhaustiveMatch {
                missing,
                span,
            });
        }
    }

    fn collect_covered_variants(
        &self,
        pattern: &Pattern,
        covered: &mut std::collections::HashSet<String>,
        has_wildcard: &mut bool,
        enum_variants: &[String],
    ) {
        match pattern {
            Pattern::Wildcard => *has_wildcard = true,
            Pattern::Ident(name) => {
                // Bare identifier matching an enum variant name is a variant match,
                // not a catch-all binding
                if enum_variants.contains(name) {
                    covered.insert(name.clone());
                } else {
                    *has_wildcard = true;
                }
            }
            Pattern::Constructor { name, .. } => {
                // Qualified names like "Enum.Variant" — extract the variant part
                let variant = name.rsplit('.').next().unwrap_or(name);
                covered.insert(variant.to_string());
            }
            Pattern::Or(patterns) => {
                for p in patterns {
                    self.collect_covered_variants(p, covered, has_wildcard, enum_variants);
                }
            }
            _ => {}
        }
    }

    /// Detect `opt is Some` (no bindings) in an if-condition and extract
    /// the variable name and its narrowed inner type (OPT10 type narrowing).
    /// Also handles `opt is Some` within `&&` chains.
    fn extract_is_some_narrowing(&self, cond: &Expr) -> Option<(String, Type)> {
        match &cond.kind {
            ExprKind::IsPattern { expr: value, pattern } => {
                // Only narrow for `is Some` with no explicit binding
                if let Pattern::Constructor { name, fields } = pattern {
                    if name == "Some" && fields.is_empty() {
                        if let ExprKind::Ident(var_name) = &value.kind {
                            let var_ty = self.lookup_local(var_name)?;
                            let resolved = self.ctx.apply(&var_ty);
                            if let Type::Option(inner) = &resolved {
                                return Some((var_name.clone(), *inner.clone()));
                            }
                        }
                    }
                }
                None
            }
            // Handle `opt is Some && ...` — narrow on the left side
            ExprKind::Binary { op: rask_ast::expr::BinOp::And, left, .. } => {
                self.extract_is_some_narrowing(left)
            }
            _ => None,
        }
    }
}
