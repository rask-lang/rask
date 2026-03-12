// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Concurrency lowering: Shared read/write blocks, Mutex lock blocks.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    stmt::ClosureCapture, types::StructLayoutId, BlockBuilder, FunctionRef,
    MirOperand, MirStmt, MirTerminator, MirType,
};
use rask_ast::expr::{Expr, ExprKind};

impl<'a> MirLowerer<'a> {
    /// Extract the inner type name from a Shared variable expression.
    pub(super) fn resolve_shared_inner_type_name(&self, object: &Expr) -> Option<String> {
        if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
            if let rask_types::Type::UnresolvedGeneric { args, .. } = raw_ty {
                if let Some(rask_types::GenericArg::Type(inner)) = args.first() {
                    if let rask_types::Type::UnresolvedNamed(name) = inner.as_ref() {
                        return Some(name.clone());
                    }
                    if let Some(prefix) = super::MirContext::type_prefix(inner) {
                        return Some(prefix);
                    }
                }
            }
        }
        if let ExprKind::Ident(var_name) = &object.kind {
            if let Some(full_type) = self.local_full_type.get(var_name) {
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
            if let Some((layout_idx, _)) = self.ctx.find_struct(type_name) {
                data_param_ty = MirType::Struct(StructLayoutId(layout_idx));
            }
            self.local_type_prefix.insert(binding_name.to_string(), type_name.clone());
        }

        let data_param_id = closure_builder.add_param(binding_name.to_string(), data_param_ty.clone());

        let mut closure_locals = std::collections::HashMap::new();
        closure_locals.insert(binding_name.to_string(), (data_param_id, data_param_ty));

        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
            });
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
            closure_builder.terminate(MirTerminator::Return {
                value: Some(body_val),
            });
        }

        let closure_fn = closure_builder.finish();
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
        });
        self.synthesized_functions.push(closure_fn);

        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
        });

        let func_name = if method == "read" {
            "Shared_read".to_string()
        } else {
            "Shared_write".to_string()
        };

        let result_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(result_local),
            func: FunctionRef::internal(func_name),
            args: vec![shared_op, MirOperand::Local(closure_local)],
        });

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
            if let Some((layout_idx, _)) = self.ctx.find_struct(type_name) {
                data_param_ty = MirType::Struct(StructLayoutId(layout_idx));
            }
            self.local_type_prefix.insert(binding_name.to_string(), type_name.clone());
        }

        let data_param_id = closure_builder.add_param(binding_name.to_string(), data_param_ty.clone());

        let mut closure_locals = std::collections::HashMap::new();
        closure_locals.insert(binding_name.to_string(), (data_param_id, data_param_ty));

        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
            });
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
            closure_builder.terminate(MirTerminator::Return {
                value: Some(body_val),
            });
        }

        let closure_fn = closure_builder.finish();
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: MirType::I64,
        });
        self.synthesized_functions.push(closure_fn);

        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::ClosureCreate {
            dst: closure_local,
            func_name: closure_name,
            captures,
            heap: false,
        });

        let result_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(result_local),
            func: FunctionRef::internal("Mutex_lock".to_string()),
            args: vec![mutex_op, MirOperand::Local(closure_local)],
        });

        Ok((MirOperand::Local(result_local), MirType::I64))
    }

    /// Extract the inner struct size from a generic type name like "Channel<LogEntry>".
    pub(super) fn generic_inner_struct_size(&self, generic_name: &str) -> i64 {
        let inner = generic_name.split('<').nth(1)
            .and_then(|s| s.strip_suffix('>'));
        if let Some(type_name) = inner {
            if let Some((_, layout)) = self.ctx.find_struct(type_name) {
                return layout.size as i64;
            }
        }
        8 // scalar default
    }
}
