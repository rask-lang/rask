// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Concurrency lowering: Shared read/write blocks, Mutex lock blocks.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    stmt::ClosureCapture, types::StructLayoutId, BlockBuilder, FunctionRef,
    MirOperand, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::expr::{CallArg, Expr, ExprKind};
use rask_ast::{NodeId, Span};

impl<'a> MirLowerer<'a> {
    /// Extract the inner type name from a Shared variable expression.
    pub(super) fn resolve_shared_inner_type_name(&self, object: &Expr) -> Option<String> {
        if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
            if let rask_types::Type::UnresolvedGeneric { args, .. } = raw_ty {
                if let Some(rask_types::GenericArg::Type(inner)) = args.first() {
                    if let rask_types::Type::UnresolvedNamed(name) = inner.as_ref() {
                        return Some(name.clone());
                    }
                    if let Some(prefix) = super::MirContext::type_prefix(inner, self.ctx.type_names) {
                        return Some(prefix);
                    }
                }
            }
        }
        if let ExprKind::Ident(var_name) = &object.kind {
            if let Some(full_type) = self.meta(var_name).and_then(|m| m.full_type.as_deref()) {
                let inner = full_type.split('<').nth(1)
                    .and_then(|s| s.strip_suffix('>'));
                if let Some(name) = inner {
                    return Some(name.to_string());
                }
            }
        }
        None
    }

    /// Lower `with shared.read() as d { body }` / `with shared.write() as d { body }`.
    pub(super) fn lower_shared_with_block(
        &mut self,
        object: &Expr,
        method: &str,
        binding_name: &str,
        body: &[rask_ast::stmt::Stmt],
    ) -> Result<TypedOperand, LoweringError> {
        let (shared_op, _) = self.lower_expr(object)?;

        let mut free_vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut bound = std::collections::HashSet::new();
        bound.insert(binding_name.to_string());
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free_vars);

        let closure_name = format!("{}__with_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for (_name, local_id, ty) in &free_vars {
            let size = ty.size();
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
            });
            env_offset = aligned_offset + size;
        }

        let mut closure_builder = BlockBuilder::new(closure_name.clone(), MirType::I64);
        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut data_param_ty = MirType::I64;
        let inner_type_name = self.resolve_shared_inner_type_name(object);
        if let Some(ref type_name) = inner_type_name {
            if let Some((layout_idx, sl)) = self.ctx.find_struct(type_name) {
                data_param_ty = MirType::Struct(StructLayoutId::new(layout_idx, sl.size, sl.align));
            }
            self.meta_mut(binding_name).type_prefix = Some(type_name.clone());
        }

        let data_param_id = closure_builder.add_param(binding_name.to_string(), data_param_ty.clone());

        let mut closure_locals = std::collections::HashMap::new();
        closure_locals.insert(binding_name.to_string(), (data_param_id, data_param_ty));

        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
                by_ref: false,
            }));
            closure_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        let body_result;
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            body_result = self.lower_block(body);

            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;
        }

        let (body_val, _) = body_result?;

        if closure_builder.current_block_unterminated() {
            closure_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(body_val),
            }));
        }

        let closure_fn = closure_builder.finish();
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
            scalar_mutate_params: Vec::new(),
        });
        self.synthesized_functions.push(closure_fn);

        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
        }));

        let func_name = if method == "read" {
            "Shared_read".to_string()
        } else {
            "Shared_write".to_string()
        };

        let result_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_local),
            func: FunctionRef::internal(func_name),
            args: vec![shared_op, MirOperand::Local(closure_local)],
        }));

        Ok((MirOperand::Local(result_local), MirType::I64))
    }

    /// Lower `with mutex as v { body }`.
    pub(super) fn lower_mutex_with_block(
        &mut self,
        object: &Expr,
        binding_name: &str,
        body: &[rask_ast::stmt::Stmt],
    ) -> Result<TypedOperand, LoweringError> {
        let (mutex_op, _) = self.lower_expr(object)?;

        let mut free_vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut bound = std::collections::HashSet::new();
        bound.insert(binding_name.to_string());
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free_vars);

        let closure_name = format!("{}__with_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        let mut captures = Vec::new();
        let mut env_offset = 0u32;
        for (_name, local_id, ty) in &free_vars {
            let size = ty.size();
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
            });
            env_offset = aligned_offset + size;
        }

        let mut closure_builder = BlockBuilder::new(closure_name.clone(), MirType::I64);
        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut data_param_ty = MirType::I64;
        let inner_type_name = self.resolve_shared_inner_type_name(object);
        if let Some(ref type_name) = inner_type_name {
            if let Some((layout_idx, sl)) = self.ctx.find_struct(type_name) {
                data_param_ty = MirType::Struct(StructLayoutId::new(layout_idx, sl.size, sl.align));
            }
            self.meta_mut(binding_name).type_prefix = Some(type_name.clone());
        }

        let data_param_id = closure_builder.add_param(binding_name.to_string(), data_param_ty.clone());

        let mut closure_locals = std::collections::HashMap::new();
        closure_locals.insert(binding_name.to_string(), (data_param_id, data_param_ty));

        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
                by_ref: false,
            }));
            closure_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        let body_result;
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            body_result = self.lower_block(body);

            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;
        }

        let (body_val, _) = body_result?;

        if closure_builder.current_block_unterminated() {
            closure_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                value: Some(body_val),
            }));
        }

        let closure_fn = closure_builder.finish();
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
            scalar_mutate_params: Vec::new(),
        });
        self.synthesized_functions.push(closure_fn);

        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
        }));

        let result_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(result_local),
            func: FunctionRef::internal("Mutex_lock".to_string()),
            args: vec![mutex_op, MirOperand::Local(closure_local)],
        }));

        Ok((MirOperand::Local(result_local), MirType::I64))
    }

    /// True when `object` has a Mutex type — either from the checker's type
    /// info or from a tracked `type_prefix` on a local/const.
    pub(super) fn is_mutex_expr(&self, object: &Expr) -> bool {
        let from_type = self.ctx.lookup_raw_type(object.id)
            .map(|ty| matches!(ty,
                rask_types::Type::UnresolvedGeneric { name, .. }
                | rask_types::Type::UnresolvedNamed(name)
                if name == "Mutex"
            ))
            .unwrap_or(false);
        let from_prefix = if let ExprKind::Ident(var_name) = &object.kind {
            self.meta(var_name)
                .and_then(|m| m.type_prefix.as_deref())
                .map(|p| p == "Mutex")
                .unwrap_or(false)
        } else {
            false
        };
        from_type || from_prefix
    }

    /// Lower `mutex.lock().method(args)` — a scoped lock used as a method
    /// receiver. The runtime lock hands the closure a pointer to the inner
    /// data, runs it, and unlocks. Run the trailing call inside that closure
    /// so the lock is held for exactly the call. Reuses the `with mutex as g
    /// { g.method(args) }` machinery by synthesizing that body.
    pub(super) fn lower_mutex_lock_method_call(
        &mut self,
        expr: &Expr,
        mutex_obj: &Expr,
        method: &str,
        args: &[CallArg],
    ) -> Result<TypedOperand, LoweringError> {
        let guard_name = format!("__lock_guard_{}", self.closure_counter);
        let guard_ident = Expr {
            id: NodeId::DUMMY,
            span: Span::new(0, 0),
            kind: ExprKind::Ident(guard_name.clone()),
        };
        let call_expr = Expr {
            id: NodeId::DUMMY,
            span: Span::new(0, 0),
            kind: ExprKind::MethodCall {
                object: Box::new(guard_ident),
                method: method.to_string(),
                type_args: None,
                args: args.to_vec(),
            },
        };
        let body = vec![rask_ast::stmt::Stmt {
            id: NodeId::DUMMY,
            span: Span::new(0, 0),
            kind: rask_ast::stmt::StmtKind::Expr(call_expr),
        }];

        let (op, _) = self.lower_mutex_with_block(mutex_obj, &guard_name, &body)?;

        // The scoped call yields the method's value; size the result with the
        // checker's type for the whole `.lock().method()` expression.
        let ret_ty = self.ctx.lookup_raw_type(expr.id)
            .map(|t| self.ctx.type_to_mir(t))
            .unwrap_or(MirType::I64);
        Ok((op, ret_ty))
    }

}
