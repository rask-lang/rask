// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Expression type inference and specific type checks.

use rask_ast::expr::{BinOp, Expr, ExprKind};
use rask_ast::stmt::StmtKind;
use rask_ast::Span;
use rask_resolve::{SymbolId, SymbolKind};

use super::type_defs::TypeDef;
use super::borrow::BorrowMode;
use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::{GenericArg, Type};

impl TypeChecker {
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
                    None => Type::I32, // Default to i32, not unconstrained variable
                }
            }
            ExprKind::Float(_, suffix) => {
                use rask_ast::token::FloatSuffix;
                match suffix {
                    Some(FloatSuffix::F32) => Type::F32,
                    Some(FloatSuffix::F64) => Type::F64,
                    None => Type::F64, // Default to f64, not unconstrained variable
                }
            }
            ExprKind::String(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Bool(_) => Type::Bool,

            ExprKind::Ident(name) => {
                if let Some(ty) = self.lookup_local(name) {
                    ty
                } else if let Some(&sym_id) = self.resolved.resolutions.get(&expr.id) {
                    self.get_symbol_type(sym_id)
                } else {
                    Type::Error
                }
            }

            ExprKind::Binary { op, left, right } => {
                self.check_binary(*op, left, right, expr.span)
            }

            ExprKind::Unary { op: _, operand } => {
                self.infer_expr(operand)
            }

            ExprKind::Call { func, args } => self.check_call(func, args, expr.span),

            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => self.check_method_call(object, method, args, expr.span),

            ExprKind::Field { object, field } => self.check_field_access(object, field, expr.span),

            ExprKind::Index { object, index } => {
                let obj_ty = self.infer_expr(object);
                let _idx_ty = self.infer_expr(index);
                match &obj_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    Type::String => Type::Char,
                    _ => self.ctx.fresh_var(),
                }
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, expr.span));

                let then_ty = self.infer_expr(then_branch);

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(else_branch);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        then_ty.clone(),
                        else_ty,
                        expr.span,
                    ));
                    then_ty
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
                    self.define_local(name, ty);
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

            ExprKind::Match { scrutinee, arms } => {
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
                    if !matches!(resolved_arm_ty, Type::Never) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            result_ty.clone(),
                            arm_ty,
                            expr.span,
                        ));
                    }
                }
                result_ty
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
                        let (struct_fields, type_params) = match self.types.get(*type_id) {
                            Some(TypeDef::Struct { fields: sf, type_params: tp, .. }) => {
                                (sf.clone(), tp.clone())
                            }
                            _ => (vec![], vec![]),
                        };

                        if type_params.is_empty() {
                            // Non-generic struct: constrain directly
                            for field_init in fields {
                                let field_ty = self.infer_expr(&field_init.value);
                                if let Some((_, expected)) =
                                    struct_fields.iter().find(|(n, _)| n == &field_init.name)
                                {
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        expected.clone(),
                                        field_ty,
                                        expr.span,
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
                                let field_ty = self.infer_expr(&field_init.value);
                                if let Some((_, expected)) =
                                    struct_fields.iter().find(|(n, _)| n == &field_init.name)
                                {
                                    let substituted = Self::substitute_type_params(expected, &subst);
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        substituted,
                                        field_ty,
                                        expr.span,
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

            ExprKind::Try(inner) => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Option(inner) => {
                        // For Option, just return the inner type
                        // The function return type should also be Option (checked elsewhere)
                        *inner.clone()
                    }
                    Type::Result { ok, err } => {
                        // For Result, extract the ok type and ensure error types match
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            if let Type::Result { err: ret_err, .. } = &resolved_ret {
                                // Unify the Result's error type with the function's error type
                                let _ = self.unify(err, ret_err, expr.span);
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
                                Type::Result { .. } => {
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let result_ty = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty),
                                    };
                                    let _ = self.unify(&inner_ty, &result_ty, expr.span);
                                    ok_ty
                                }
                                Type::Var(_) => {
                                    self.ctx.fresh_var()
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
                        self.errors.push(TypeError::Mismatch {
                            expected: Type::Result {
                                ok: Box::new(self.ctx.fresh_var()),
                                err: Box::new(self.ctx.fresh_var()),
                            },
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Unwrap(inner) => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Option(inner) => {
                        // For Unwrap, extract the inner type from Option
                        *inner.clone()
                    }
                    Type::Var(_) => {
                        // If we don't know the type yet, constrain it to be an Option
                        let inner_opt_ty = self.ctx.fresh_var();
                        let option_ty = Type::Option(Box::new(inner_opt_ty.clone()));
                        let _ = self.unify(&inner_ty, &option_ty, expr.span);
                        inner_opt_ty
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

                let inferred_ret = self.infer_expr(body);

                // Check declared return type if present
                let ret_ty = if let Some(declared) = declared_ret {
                    let expected_ret = parse_type_string(declared, &self.types)
                        .unwrap_or(Type::Error);
                    if let Err(err) = self.unify(&inferred_ret, &expected_ret, expr.span) {
                        self.errors.push(err);
                    }
                    expected_ret
                } else {
                    inferred_ret
                };

                Type::Fn {
                    params: param_types,
                    ret: Box::new(ret_ty),
                }
            }

            ExprKind::Cast { expr: inner, ty } => {
                self.infer_expr(inner);
                parse_type_string(ty, &self.types).unwrap_or(Type::Error)
            }

            ExprKind::Unsafe { body } => {
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

                Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(spawn_return_type))],
                }
            }

            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.infer_expr(arg);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            ExprKind::WithAs { bindings, body } => {
                self.push_scope();
                for (source_expr, binding_name) in bindings {
                    let elem_ty = self.infer_expr(source_expr);
                    self.define_local(binding_name.clone(), elem_ty);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                self.pop_scope();
                Type::Unit
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
                let obj_ty = self.infer_expr(object);
                let field_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty: obj_ty,
                    field: field.clone(),
                    expected: field_ty.clone(),
                    span: expr.span,
                });
                Type::Option(Box::new(field_ty))
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

    pub(super) fn check_call(&mut self, func: &Expr, args: &[Expr], span: Span) -> Type {
        if let ExprKind::Ident(name) = &func.kind {
            if self.is_builtin_function(name) {
                for arg in args {
                    self.infer_expr(arg);
                }
                return match name.as_str() {
                    "panic" => Type::Never,
                    "format" => Type::String,
                    _ => Type::Unit,
                };
            }
        }

        let func_ty = self.infer_expr(func);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

        match func_ty {
            Type::Fn { params, ret } => {
                if params.is_empty() && !arg_types.is_empty() {
                    return *ret;
                }

                if params.len() != arg_types.len() {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: params.len(),
                        found: arg_types.len(),
                        span,
                    });
                    return Type::Error;
                }

                for (param, arg) in params.iter().zip(arg_types.iter()) {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(param.clone(), arg.clone(), span));
                }

                *ret
            }
            Type::Var(_) => {
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
            Type::Error => Type::Error,
            _ => {
                self.ctx.fresh_var()
            }
        }
    }

    pub(super) fn is_builtin_function(&self, name: &str) -> bool {
        matches!(name, "println" | "print" | "panic" | "assert" | "debug" | "format")
    }

    pub(super) fn check_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        // Check if this is a builtin module method call (e.g., fs.open)
        if let ExprKind::Ident(name) = &object.kind {
            if self.types.builtin_modules.is_module(name) {
                return self.check_module_method(name, method, args, span);
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
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

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

    pub(super) fn check_module_method(
        &mut self,
        module: &str,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

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

            // Replace placeholder types with fresh vars for generic module methods
            self.freshen_module_return_type(&sig.ret.clone())
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

    pub(super) fn check_field_access(&mut self, object: &Expr, field: &str, span: Span) -> Type {
        // Primitive type constants: u64.MAX, i32.MIN, etc.
        if let ExprKind::Ident(name) = &object.kind {
            if let Some(ty) = Self::primitive_type_constant(name, field) {
                return ty;
            }
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);
        let field_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasField {
            ty: obj_ty,
            field: field.to_string(),
            expected: field_ty.clone(),
            span,
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
}
