// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure and spawn lowering.

use super::{LoweringError, MirLowerer, TypedOperand};
use crate::{
    stmt::ClosureCapture, BlockBuilder, FunctionRef, LocalId, MirOperand,
    MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};
use rask_ast::{
    expr::Expr,
    stmt::Stmt,
};

impl<'a> MirLowerer<'a> {
    /// Closure lowering: synthesize a separate MIR function for the body,
    /// build the environment, and emit ClosureCreate in the enclosing function.
    pub(super) fn lower_closure(
        &mut self,
        params: &[rask_ast::expr::ClosureParam],
        ret_ty: Option<&str>,
        body: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        // 1. Collect free variables (captures from enclosing scope)
        let free_vars = self.collect_free_vars(body, params);

        // 2. Generate unique name for the closure function
        let closure_name = format!("{}__closure_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // 3. Build the closure environment layout
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

        // 4. Synthesize a MIR function for the closure body.
        let closure_ret = ret_ty
            .map(|s| self.ctx.resolve_type_str(s))
            .unwrap_or(MirType::I64);
        let mut closure_builder = BlockBuilder::new(closure_name.clone(), closure_ret.clone());

        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut closure_locals = std::collections::HashMap::new();
        for param in params {
            let param_ty = param.ty.as_deref()
                .map(|s| self.ctx.resolve_type_str(s))
                .unwrap_or(MirType::I64);
            let param_id = closure_builder.add_param(param.name.clone(), param_ty.clone());
            closure_locals.insert(param.name.clone(), (param_id, param_ty.clone()));
            if let Some(prefix) = self.mir_type_name(&param_ty) {
                self.local_type_prefix.insert(param.name.clone(), prefix);
            } else if let Some(ty_str) = param.ty.as_deref() {
                if let Some(prefix) = super::type_prefix_from_str(ty_str) {
                    self.local_type_prefix.insert(param.name.clone(), prefix);
                }
            }
        }

        // Emit LoadCapture for each free variable
        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = closure_builder.alloc_local(name.clone(), ty.clone());
            closure_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
            }));
            closure_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        // Lower the closure body using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            let body_result = self.lower_expr(body);

            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;

            let (body_val, _body_ty) = body_result?;

            if closure_builder.current_block_unterminated() {
                closure_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(body_val),
                }));
            }
        }

        let closure_fn = closure_builder.finish();

        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: closure_ret,
        });

        self.synthesized_functions.push(closure_fn);

        // 5. In the parent function, emit ClosureCreate
        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
            dst: result_local,
            func_name: closure_name,
            captures,
            heap: true,
        }));

        Ok((MirOperand::Local(result_local), MirType::Ptr))
    }

    /// Spawn lowering: synthesize a closure function from the body block,
    /// emit ClosureCreate + Call to rask_closure_spawn.
    pub(super) fn lower_spawn(
        &mut self,
        body: &[Stmt],
    ) -> Result<TypedOperand, LoweringError> {
        // 1. Collect free variables from the spawn body block
        let free_vars = self.collect_free_vars_block(body);

        // 2. Generate unique name for the spawn function
        let spawn_name = format!("{}__spawn_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        // 3. Build the closure environment layout
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

        // 4. Synthesize a MIR function for the spawn body.
        let mut spawn_builder = BlockBuilder::new(spawn_name.clone(), MirType::Void);

        let env_param_id = spawn_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut spawn_locals = std::collections::HashMap::new();
        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = spawn_builder.alloc_local(name.clone(), ty.clone());
            spawn_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
            }));
            spawn_locals.insert(name.clone(), (local_id, ty.clone()));
        }

        // Lower the body statements using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, spawn_builder);
            let saved_locals = std::mem::replace(&mut self.locals, spawn_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            let mut body_result = Ok(());
            for stmt in body {
                if let Err(e) = self.lower_stmt(stmt) {
                    body_result = Err(e);
                    break;
                }
            }

            spawn_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;

            body_result?;

            if spawn_builder.current_block_unterminated() {
                spawn_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
            }
        }

        let spawn_fn = spawn_builder.finish();

        // Try the state machine transform for yield-point-containing spawns
        if let Some(sm_result) = crate::transform::state_machine::transform(&spawn_fn) {
            let poll_name = sm_result.poll_fn.name.clone();
            self.func_sigs.insert(poll_name.clone(), super::FuncSig {
                ret_ty: MirType::I32,
            });
            self.synthesized_functions.push(sm_result.poll_fn);

            let state_ptr = self.builder.alloc_temp(MirType::Ptr);
            let state_size_val = sm_result.state_size as i64;
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(state_ptr),
                func: FunctionRef::internal("rask_alloc".to_string()),
                args: vec![MirOperand::Constant(crate::operand::MirConst::Int(state_size_val))],
            }));

            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: state_ptr,
                offset: 0,
                value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                store_size: None,
            }));

            for &(env_offset, state_offset) in &sm_result.capture_stores {
                if let Some(cap) = captures.iter().find(|c| c.offset == env_offset) {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: state_ptr,
                        offset: state_offset,
                        value: MirOperand::Local(cap.local_id),
                        store_size: None,
                    }));
                }
            }

            let poll_fn_ptr = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: poll_fn_ptr,
                rvalue: MirRValue::Use(MirOperand::Constant(
                    crate::operand::MirConst::String(poll_name),
                )),
            }));

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handle_local),
                func: FunctionRef::internal("rask_green_spawn".to_string()),
                args: vec![
                    MirOperand::Local(poll_fn_ptr),
                    MirOperand::Local(state_ptr),
                    MirOperand::Constant(crate::operand::MirConst::Int(state_size_val)),
                ],
            }));

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        } else {
            self.func_sigs.insert(spawn_name.clone(), super::FuncSig {
                ret_ty: MirType::Void,
            });
            self.synthesized_functions.push(spawn_fn);

            let closure_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCreate {
                dst: closure_local,
                func_name: spawn_name,
                captures,
                heap: true,
            }));

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handle_local),
                func: FunctionRef::internal("spawn".to_string()),
                args: vec![MirOperand::Local(closure_local)],
            }));

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        }
    }

    /// Collect free variables from a block of statements (no params to bind).
    pub(super) fn collect_free_vars_block(
        &self,
        body: &[Stmt],
    ) -> Vec<(String, LocalId, MirType)> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bound = std::collections::HashSet::new();
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free);
        free
    }
}
