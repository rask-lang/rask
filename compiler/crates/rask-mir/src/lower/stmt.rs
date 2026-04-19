// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Statement lowering.

use super::{LoopContext, LoweringError, MirLowerer};
use crate::{
    operand::{BinOp, MirConst},
    types::StructLayoutId,
    FunctionRef, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind,
    MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, UnaryOp},
    stmt::{ForBinding, Stmt, StmtKind},
};

impl<'a> MirLowerer<'a> {
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        self.builder.set_span(stmt.span);
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                // C1/C2: if this is a consuming method call on an ensure receiver,
                // emit ResourceConsume so the ensure is cancelled at cleanup time.
                self.check_resource_consume(e);
                Ok(())
            }

            StmtKind::Mut { name, ty, init, .. } => {
                self.lower_binding(name, ty.as_deref(), init)
            }

            StmtKind::Const { name, ty, init, .. } => {
                // If this const was evaluated at compile time, emit a global reference
                if let Some(meta) = self.ctx.comptime_globals.get(name) {
                    if meta.type_prefix == "Vec" {
                        // Array: store pointer for later Vec wrapping
                        let mir_ty = if let Some(ty_str) = ty.as_deref() {
                            self.ctx.resolve_type_str(ty_str)
                        } else {
                            MirType::Ptr
                        };
                        let local_id = self.builder.alloc_local(name.to_string(), mir_ty.clone());
                        self.locals.insert(name.to_string(), (local_id, mir_ty));
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                            dst: local_id,
                            name: name.clone(),
                        }));
                    } else {
                        // Scalar: load the data pointer, then deref to get the value
                        let mir_ty = match meta.type_prefix.as_str() {
                            "bool" => MirType::Bool,
                            "i32" => MirType::I32,
                            "i64" => MirType::I64,
                            "f32" => MirType::F32,
                            "f64" => MirType::F64,
                            _ => if let Some(ty_str) = ty.as_deref() {
                                self.ctx.resolve_type_str(ty_str)
                            } else {
                                MirType::I64
                            },
                        };
                        let ptr_local = self.builder.alloc_temp(MirType::Ptr);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                            dst: ptr_local,
                            name: name.clone(),
                        }));
                        let local_id = self.builder.alloc_local(name.to_string(), mir_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: local_id,
                            rvalue: MirRValue::Deref(MirOperand::Local(ptr_local)),
                        }));
                        self.locals.insert(name.to_string(), (local_id, mir_ty));
                    }
                    self.meta_mut(name).type_prefix = Some(meta.type_prefix.clone());
                    return Ok(());
                }
                self.lower_binding(name, ty.as_deref(), init)
            }

            StmtKind::Return(opt_expr) => {
                let value = if let Some(e) = opt_expr {
                    let (op, _) = self.lower_expr(e)?;
                    Some(op)
                } else {
                    None
                };
                // Inside an inlined closure (e.g. fold callback), redirect
                // return to an assignment + goto instead of a real return.
                if let Some((dst_local, cont_block)) = self.inline_return_target {
                    if let Some(val) = value {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: dst_local,
                            rvalue: MirRValue::Use(val),
                        }));
                    }
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: cont_block }));
                } else if self.ensure_stack.is_empty() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value }));
                } else {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                        value,
                        cleanup_chain: self.cleanup_chain(),
                    }));
                }
                Ok(())
            }

            StmtKind::Assign { target, value } => {
                let (val_op, _) = self.lower_expr(value)?;
                match &target.kind {
                    ExprKind::Ident(name) => {
                        let (local_id, _) = self
                            .locals
                            .get(name)
                            .cloned()
                            .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone()))?;
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: local_id,
                            rvalue: MirRValue::Use(val_op),
                        }));
                    }
                    // Field assignment: obj.field = value → Store at field offset
                    ExprKind::Field { object, field } => {
                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let offset = if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                            if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                layout.fields.iter()
                                    .find(|f| f.name == *field)
                                    .map(|f| f.offset)
                                    .unwrap_or(0)
                            } else { 0 }
                        } else if let Some((_, _, Some(bo), _)) = Self::resolve_tuple_field(&obj_ty, field) {
                            bo
                        } else {
                            // Base is a raw pointer (I64/Ptr) — field offset unknown.
                            // With correct element type tracking, pool[h] and pool.get(h)
                            // return Struct-typed results so this path shouldn't fire
                            // for pool operations.
                            0
                        };
                        let base_local = match obj_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(obj_ty);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(obj_op),
                                }));
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: base_local,
                            offset,
                            value: val_op,
                            store_size: None,
                        }));
                    }
                    // Index assignment: a[i] = val
                    ExprKind::Index { object, index } => {
                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let (idx_op, _) = self.lower_expr(index)?;

                        if let MirType::Array { ref elem, .. } = obj_ty {
                            // Fixed-size array: direct store at base + index * elem_size
                            if let MirOperand::Local(base_id) = obj_op {
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ArrayStore {
                                    base: base_id,
                                    index: idx_op,
                                    elem_size: elem.size(),
                                    value: val_op,
                                }));
                            }
                        } else {
                            // Vec/Map: dispatch through runtime
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: None,
                                func: FunctionRef::internal("Vec_set".to_string()),
                                args: vec![obj_op, idx_op, val_op],
                            }));
                        }
                    }
                    // Deref assignment: *ptr = value → Store through pointer
                    ExprKind::Unary { op: UnaryOp::Deref, operand } => {
                        let (addr_op, _) = self.lower_expr(operand)?;
                        let addr_local = match addr_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(addr_op),
                                }));
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: addr_local,
                            offset: 0,
                            value: val_op,
                            store_size: None,
                        }));
                    }
                    _ => {
                        return Err(LoweringError::InvalidConstruct(
                            format!("unsupported assignment target: {:?}", target.kind),
                        ));
                    }
                }
                Ok(())
            }

            // While loop (spec L5)
            StmtKind::While { cond, body } => self.lower_while(cond, body),

            // For loop - desugar to while with iterator
            StmtKind::For {
                label,
                binding,
                mutate,
                iter,
                body,
            } => self.lower_for(label.as_deref(), binding, *mutate, iter, body),

            // Infinite loop
            StmtKind::Loop { label, body } => self.lower_loop(label.as_deref(), body),

            // Break
            StmtKind::Break { label, value } => self.lower_break(label.as_deref(), value.as_ref()),

            // Continue
            StmtKind::Continue(label) => self.lower_continue(label.as_deref()),

            // Tuple destructuring
            StmtKind::MutTuple { patterns, init }
            | StmtKind::ConstTuple { patterns, init } => {
                let names: Vec<String> = rask_ast::stmt::tuple_pats_flat_names(patterns)
                    .into_iter().map(|s| s.to_string()).collect();
                self.lower_tuple_destructure(&names, init)
            }

            // While-let pattern loop
            StmtKind::WhileLet { pattern, expr, body } => {
                let check_block = self.builder.create_block();
                let body_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

                self.builder.switch_to_block(check_block);
                let (val, _) = self.lower_expr(expr)?;
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                }));
                // Compare tag against expected variant
                let expected = self.pattern_tag(pattern);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(crate::operand::MirConst::Int(expected)),
                    },
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: body_block,
                    else_block: exit_block,
                }));

                self.builder.switch_to_block(body_block);
                // Bind payload variables from the pattern
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload(pattern, val, payload_ty);
                let ensure_depth = self.ensure_stack.len();
                self.loop_stack.push(LoopContext {
                    label: None,
                    continue_block: check_block,
                    exit_block,
                    result_local: None,
                    ensure_depth,
                });
                for s in body {
                    self.lower_stmt(s)?;
                }
                // EN7: run loop-scoped ensures at iteration end
                self.emit_loop_cleanup(ensure_depth);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));
                self.loop_stack.pop();
                self.ensure_stack.truncate(ensure_depth);

                self.builder.switch_to_block(exit_block);
                Ok(())
            }

            // Ensure (EN1–EN7): schedule cleanup to run at scope exit.
            // Body is lowered into a cleanup block; CleanupReturn terminators
            // at return/try sites chain through these blocks.
            StmtKind::Ensure { body, else_handler } => {
                let cleanup_block = self.builder.create_block();
                let continue_block = self.builder.create_block();

                // Marker for MIRI/analysis
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::EnsurePush { cleanup_block }));

                // C1/C2: extract receiver variable from body (e.g. `ensure tx.rollback()` → "tx").
                // Register a resource_id so consumption can be tracked at runtime.
                let receiver_name = Self::extract_ensure_receiver(body);
                if let Some(ref name) = receiver_name {
                    if let Some((local_id, _)) = self.locals.get(name) {
                        let resource_id = self.builder.alloc_local(
                            format!("__ensure_res_{}", cleanup_block.0),
                            MirType::I64,
                        );
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceRegister {
                            dst: resource_id,
                            type_name: name.clone(),
                            scope_depth: 0,
                        }));
                        self.ensure_receivers.insert(cleanup_block, (name.clone(), resource_id));
                        // Store resource_id in local_meta so method calls on this
                        // receiver can find it for ResourceConsume.
                        self.meta_mut(name).resource_id = Some(resource_id);
                    }
                }
                self.ensure_stack.push(cleanup_block);

                // Main flow skips to continue block (body runs at scope exit)
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: continue_block }));

                // Lower ensure body into cleanup block.
                // C1/C2: if receiver has a resource_id, check consumption first.
                self.builder.switch_to_block(cleanup_block);
                if let Some((_, resource_id)) = receiver_name
                    .as_ref()
                    .and_then(|name| self.ensure_receivers.get(&cleanup_block).cloned())
                {
                    // Check if resource was consumed → skip cleanup
                    let consumed = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(consumed),
                        func: crate::FunctionRef::internal("rask_resource_is_consumed".to_string()),
                        args: vec![MirOperand::Local(resource_id)],
                    }));
                    let body_block = self.builder.create_block();
                    let skip_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(consumed),
                        then_block: skip_block,
                        else_block: body_block,
                    }));
                    // skip_block: sentinel (consumed → skip cleanup)
                    self.builder.switch_to_block(skip_block);
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    // body_block: run the actual cleanup
                    self.builder.switch_to_block(body_block);
                }

                for s in body {
                    self.lower_stmt(s)?;
                }

                if let Some((param_name, handler_body)) = else_handler {
                    // ER2: route errors from body to else handler.
                    // The body's last call may return a Result — check its tag.
                    let handler_block = self.builder.create_block();
                    let done_block = self.builder.create_block();

                    if let Some(call_dst) = self.builder.last_call_dst() {
                        // Check Result tag: 0=Ok, 1=Err
                        let tag = self.builder.alloc_temp(MirType::U8);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: tag,
                            rvalue: MirRValue::EnumTag { value: MirOperand::Local(call_dst) },
                        }));
                        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                            cond: MirOperand::Local(tag),
                            then_block: handler_block,
                            else_block: done_block,
                        }));

                        // Handler block: bind error, run handler body.
                        // Infer error type from the call's return type.
                        let err_ty = self.builder.local_type(call_dst)
                            .and_then(|t| match t {
                                MirType::Result { err, .. } => Some(*err),
                                _ => None,
                            })
                            .unwrap_or(MirType::I64);
                        self.builder.switch_to_block(handler_block);
                        let err_local = self.builder.alloc_local(param_name.clone(), err_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: err_local,
                            rvalue: MirRValue::Field {
                                base: MirOperand::Local(call_dst),
                                field_index: 0,
                                byte_offset: None,
                                field_size: None,
                            },
                        }));
                        self.locals.insert(param_name.clone(), (err_local, err_ty));
                        for s in handler_body {
                            self.lower_stmt(s)?;
                        }
                        if self.builder.current_block_unterminated() {
                            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
                        }
                    } else {
                        // No call in body — handler never fires
                        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: done_block }));
                    }

                    // Done block: sentinel for end of cleanup sub-CFG
                    self.builder.switch_to_block(done_block);
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                } else {
                    // No handler — terminate with sentinel
                    if self.builder.current_block_unterminated() {
                        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    }
                }

                self.builder.switch_to_block(continue_block);
                Ok(())
            }

            // Discard (D1): value dropped, binding invalidated.
            // At MIR level this is a no-op — the value becomes dead.
            // Ownership checker handles use-after-discard errors.
            StmtKind::Discard { name, .. } => {
                // If the local exists, remove it from scope so later
                // references fail during lowering rather than silently.
                self.locals.remove(name);
                Ok(())
            }

            StmtKind::Discard { .. } => Ok(()),

            // Comptime (compile-time evaluated)
            StmtKind::Comptime(stmts) => {
                // Try to evaluate comptime if at compile time (CC1)
                if let Some(ref interp_cell) = self.ctx.comptime_interp {
                    if let Some(taken) = self.try_eval_comptime_if(stmts, interp_cell)? {
                        for s in taken {
                            self.lower_stmt(s)?;
                        }
                        return Ok(());
                    }
                }
                for s in stmts {
                    self.lower_stmt(s)?;
                }
                Ok(())
            }

            // CT48: comptime for — must be unrolled before MIR lowering
            StmtKind::ComptimeFor { .. } => {
                Err(LoweringError::InvalidConstruct(
                    "comptime for must be unrolled at monomorphization time before MIR lowering".into()
                ))
            }
        }
    }

    /// Try to evaluate a `comptime if` block at compile time.
    ///
    /// Returns `Some(stmts)` with the taken branch's statements if the condition
    /// evaluates successfully, or `None` if the block isn't a comptime if pattern.
    fn try_eval_comptime_if<'b>(
        &self,
        stmts: &'b [Stmt],
        interp_cell: &std::cell::RefCell<rask_comptime::ComptimeInterpreter>,
    ) -> Result<Option<&'b [Stmt]>, LoweringError> {
        // Pattern: comptime { if cond { then } else { else } }
        if stmts.len() != 1 {
            return Ok(None);
        }
        let inner = match &stmts[0].kind {
            StmtKind::Expr(e) => e,
            _ => return Ok(None),
        };
        let (cond, then_branch, else_branch) = match &inner.kind {
            ExprKind::If { cond, then_branch, else_branch, .. } => (cond, then_branch, else_branch),
            _ => return Ok(None),
        };

        let mut interp = interp_cell.borrow_mut();
        match interp.eval_expr(cond) {
            Ok(val) => {
                let taken = val.as_bool().unwrap_or(false);
                if taken {
                    // Lower the then branch — it's a Block(stmts) expression
                    if let ExprKind::Block(block_stmts) = &then_branch.kind {
                        Ok(Some(block_stmts))
                    } else {
                        Ok(None)
                    }
                } else if let Some(else_br) = else_branch {
                    if let ExprKind::Block(block_stmts) = &else_br.kind {
                        Ok(Some(block_stmts))
                    } else {
                        Ok(None)
                    }
                } else {
                    // No else branch, condition is false — emit nothing
                    Ok(Some(&[]))
                }
            }
            Err(_) => {
                // Condition not evaluable — fall through to normal lowering
                Ok(None)
            }
        }
    }

    /// Lower a let/const binding: evaluate init, assign to a new local.
    fn lower_binding(&mut self, name: &str, ty: Option<&str>, init: &Expr) -> Result<(), LoweringError> {
        let is_closure = matches!(&init.kind, ExprKind::Closure { .. });
        let (init_op, inferred_ty) = self.lower_expr(init)?;
        let var_ty = ty.map(|s| self.ctx.resolve_type_str(s)).unwrap_or(inferred_ty);
        let local_id = self.builder.alloc_local(name.to_string(), var_ty.clone());
        self.locals.insert(name.to_string(), (local_id, var_ty.clone()));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: local_id,
            rvalue: MirRValue::Use(init_op),
        }));

        // Track collection element types for for-in iteration heuristics
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                match (obj_name.as_str(), method.as_str()) {
                    ("cli", "args") | ("fs", "read_lines") => {
                        self.meta_mut(name).elem_type = Some(MirType::String);
                    }
                    ("fs", "read_bytes") => {
                        self.meta_mut(name).elem_type = Some(MirType::U8);
                    }
                    _ => {}
                }
            }
            // String methods that always return Vec<string>
            match method.as_str() {
                "lines" | "split" | "split_whitespace" => {
                    self.meta_mut(name).elem_type = Some(MirType::String);
                }
                _ => {}
            }
        }

        // Track stdlib type prefix for variables assigned from type constructors,
        // known module functions, or method calls on tracked variables,
        // so later method calls dispatch correctly.
        // Unwrap try/unwrap wrappers to see the underlying expression.
        let init_inner = match &init.kind {
            ExprKind::Try { expr, .. } => expr.as_ref(),
            ExprKind::Unwrap { expr, .. } => expr.as_ref(),
            _ => init,
        };
        if let ExprKind::MethodCall { object, method, .. } = &init_inner.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                if super::is_type_constructor_name(obj_name) {
                    // Type.method() → prefix is the type name.
                    // Covers stdlib (Vec, Map, string) and user types (Person, Document).
                    // Strip generic args: Map<string, JsonValue> → Map
                    let base_name = obj_name.split('<').next().unwrap_or(obj_name);
                    let is_module = rask_stdlib::mir_metadata::stdlib_module_names()
                        .contains(base_name);
                    if !is_module && (super::MirContext::stdlib_type_prefix(
                        &rask_types::Type::UnresolvedNamed(base_name.to_string())
                    ).is_some()
                        || base_name.chars().next().map_or(false, |c| c.is_uppercase()))
                    {
                        self.meta_mut(name).type_prefix = Some(base_name.to_string());
                    } else {
                        // Module function (fs.open) → check return type prefix
                        let func_name = format!("{}_{}", obj_name, method);
                        if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                            self.meta_mut(name).type_prefix = Some(prefix.to_string());
                        }
                    }
                } else if let Some(obj_prefix) = self.meta(obj_name).and_then(|m| m.type_prefix.clone()) {
                    // Instance method on tracked variable (file.lines() → File_lines)
                    let func_name = format!("{}_{}", obj_prefix, method);
                    if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                        self.meta_mut(name).type_prefix = Some(prefix.to_string());
                    }
                    // Propagate full generic type through clone (Shared, Sender, Receiver)
                    if method == "clone" {
                        if let Some(full_ty) = self.meta(obj_name).and_then(|m| m.full_type.clone()) {
                            self.meta_mut(name).full_type = Some(full_ty);
                        }
                    }
                }
            }
            // Module.Type.method() pattern: http.HttpServer.listen() → prefix "HttpServer"
            if let ExprKind::Field { object: inner_obj, field: type_name } = &object.kind {
                if let ExprKind::Ident(module_name) = &inner_obj.kind {
                    if !self.locals.contains_key(module_name)
                        && super::is_type_constructor_name(module_name)
                    {
                        self.meta_mut(name).type_prefix = Some(type_name.clone());
                    }
                }
            }
        }
        // Track full generic type for Shared.new(data) calls:
        // infer inner type from the constructor argument.
        if let ExprKind::MethodCall { object, method, args: call_args, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                if obj_name == "Shared" && method == "new" && !call_args.is_empty() {
                    // Infer inner type from the first arg
                    let inner_name = match &call_args[0].expr.kind {
                        ExprKind::StructLit { name: sn, .. } => Some(sn.clone()),
                        ExprKind::MethodCall { object: inner_obj, .. } => {
                            if let ExprKind::Ident(tn) = &inner_obj.kind {
                                if tn.chars().next().map_or(false, |c| c.is_uppercase()) {
                                    Some(tn.clone())
                                } else { None }
                            } else { None }
                        }
                        ExprKind::Ident(vn) => {
                            // Look up variable type from local_meta
                            self.meta(vn).and_then(|m| m.type_prefix.clone())
                        }
                        _ => None,
                    };
                    if let Some(inner) = inner_name {
                        self.meta_mut(name).full_type = Some(
                            format!("Shared<{}>", inner),
                        );
                    }
                }
            }
        }
        // Track element type for Pool<T>.new() constructors:
        // const pool = Pool<Node>.new() → collection_elem_types["pool"] = Struct(Node)
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                let base_name = obj_name.split('<').next().unwrap_or(obj_name);
                if base_name == "Pool" && method == "new" {
                    if let Some(inner) = obj_name.split('<').nth(1).and_then(|s| s.strip_suffix('>')) {
                        let elem_mir = self.ctx.resolve_type_str(inner);
                        if !matches!(elem_mir, MirType::Ptr | MirType::I64) {
                            self.meta_mut(name).elem_type = Some(elem_mir);
                        }
                    }
                }
            }
        }
        // Iterator terminal .collect() returns a Vec
        if let ExprKind::MethodCall { method, .. } = &init.kind {
            if method == "collect" {
                self.meta_mut(name).type_prefix = Some("Vec".to_string());
            }
        }
        // Also track for simple function calls (e.g. cli.args())
        if let ExprKind::Call { func, .. } = &init.kind {
            if let ExprKind::Ident(func_name) = &func.kind {
                if let Some(prefix) = super::func_return_type_prefix(func_name) {
                    self.meta_mut(name).type_prefix = Some(prefix.to_string());
                }
            }
        }
        // Index expression: args[1] → if args has known element type, propagate it
        if let ExprKind::Index { object, .. } = &init.kind {
            if let ExprKind::Ident(coll_name) = &object.kind {
                if let Some(elem_ty) = self.meta(coll_name).and_then(|m| m.elem_type.clone()) {
                    if let Some(prefix) = self.mir_type_name(&elem_ty) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                    }
                }
            }
        }

        // Fallback: derive prefix from the MIR type (catches String, Struct, Enum)
        // or from the type annotation string (catches Ptr types like Vec<T>, Map<K,V>)
        if self.meta(name).and_then(|m| m.type_prefix.as_ref()).is_none() {
            if let Some(prefix) = self.mir_type_name(&var_ty) {
                self.meta_mut(name).type_prefix = Some(prefix);
            } else if let Some(ty_str) = ty {
                if let Some(prefix) = super::type_prefix_from_str(ty_str) {
                    self.meta_mut(name).type_prefix = Some(prefix);
                }
            }
        }

        // Track collection element types from type annotations (Vec<u8>, Pool<T>)
        if self.meta(name).and_then(|m| m.elem_type.as_ref()).is_none() {
            if let Some(ty_str) = ty {
                if let Some(elem_str) = ty_str.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
                    let elem_mir = self.ctx.resolve_type_str(elem_str);
                    self.meta_mut(name).elem_type = Some(elem_mir);
                } else if let Some(elem_str) = ty_str.strip_prefix("Pool<").and_then(|s| s.strip_suffix('>')) {
                    let elem_mir = self.ctx.resolve_type_str(elem_str);
                    self.meta_mut(name).elem_type = Some(elem_mir);
                }
            }
        }

        // Track closure bindings and alias the func_sig so callers can
        // look up the return type by variable name.
        if is_closure {
            self.closure_locals.insert(name.to_string());
            let closure_fn = format!("{}__closure_{}", self.parent_name, self.closure_counter - 1);
            if let Some(sig) = self.func_sigs.get(&closure_fn).cloned() {
                self.func_sigs.insert(name.to_string(), sig);
            }
        }

        // Track locals assigned from calls that return closure types.
        // e.g., `const add_5 = make_adder(5)` where make_adder returns |i32| -> i32.
        // Check via type checker's node_types: if init expr has Type::Fn, it's a closure.
        if !is_closure {
            if let Some(rask_types::Type::Fn { ret, .. }) = self.ctx.node_types.get(&init.id) {
                self.closure_locals.insert(name.to_string());
                let ret_mir = self.ctx.type_to_mir(ret);
                self.func_sigs.insert(name.to_string(), super::FuncSig { ret_ty: ret_mir });
            }
        }

        // Propagate Vec element types from "self.field" to "<name>.field"
        // so struct field access like `state.data.get(i)` finds the right type.
        if let ExprKind::StructLit { fields, .. } = &init.kind {
            let shared = self.ctx.shared_elem_types.borrow();
            let mut to_add = Vec::new();
            for field in fields {
                let self_key = format!("self.{}", field.name);
                if let Some(elem_ty) = shared.get(&self_key) {
                    let var_key = format!("{}.{}", name, field.name);
                    to_add.push((var_key, elem_ty.clone()));
                }
                // Also check if the source variable directly has an element type
                if let ExprKind::Ident(src_var) = &field.value.kind {
                    if let Some(elem_ty) = self.meta(src_var).and_then(|m| m.elem_type.as_ref())
                        .or_else(|| shared.get(src_var))
                    {
                        let var_key = format!("{}.{}", name, field.name);
                        to_add.push((var_key, elem_ty.clone()));
                    }
                }
            }
            drop(shared);
            for (key, ty) in to_add {
                self.meta_mut(&key).elem_type = Some(ty.clone());
                self.ctx.shared_elem_types.borrow_mut().insert(key, ty);
            }
        }

        Ok(())
    }

    /// Lower tuple destructuring: evaluate init, extract each element by field index.
    fn lower_tuple_destructure(&mut self, names: &[String], init: &Expr) -> Result<(), LoweringError> {
        // Channel.buffered()/unbuffered() returns a raw channel pointer in
        // codegen, not a (Sender, Receiver) tuple. Emit channel_tx/channel_rx
        // calls to extract the handles instead of field extraction.
        let is_channel_create = match &init.kind {
            ExprKind::MethodCall { object, method, .. } => {
                if let ExprKind::Ident(type_name) = &object.kind {
                    let base = type_name.split('<').next().unwrap_or(type_name);
                    base == "Channel" && (method == "buffered" || method == "unbuffered")
                } else { false }
            }
            _ => false,
        };

        let (init_op, init_mir_ty) = self.lower_expr(init)?;
        // Extract tuple element types from type checker for type prefix tracking.
        // e.g. Channel<T>.buffered() → (Sender<T>, Receiver<T>)
        let tuple_elems: Option<Vec<rask_types::Type>> =
            self.ctx.lookup_raw_type(init.id).and_then(|ty| {
                if let rask_types::Type::Tuple(elems) = ty {
                    Some(elems.clone())
                } else {
                    None
                }
            });

        // Extract per-element MIR types from the tuple type.
        let mir_elem_types: Option<Vec<MirType>> = match &init_mir_ty {
            MirType::Tuple(fields) => Some(fields.clone()),
            _ => None,
        };

        for (i, name) in names.iter().enumerate() {
            let elem_ty = if is_channel_create {
                // Channel tx/rx handles are opaque i64 pointers
                MirType::I64
            } else {
                mir_elem_types.as_ref()
                    .and_then(|elems| elems.get(i).cloned())
                    .or_else(|| self.lookup_expr_type(init))
                    .unwrap_or(MirType::I64)
            };
            let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
            self.locals.insert(name.clone(), (local_id, elem_ty));

            if is_channel_create && names.len() == 2 {
                // Extract tx (index 0) or rx (index 1) from the raw channel ptr.
                let extract_fn = if i == 0 { "channel_tx" } else { "channel_rx" };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(local_id),
                    func: FunctionRef::internal(extract_fn.to_string()),
                    args: vec![init_op.clone()],
                }));
            } else {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Field {
                        base: init_op.clone(),
                        field_index: i as u32,
                        byte_offset: None,
                        field_size: None,
                    },
                }));
            }

            // Track type prefix so method calls get qualified names.
            // First try type-checker info (works when types are fully resolved).
            let mut found_prefix = false;
            if let Some(ref elems) = tuple_elems {
                if let Some(elem_type) = elems.get(i) {
                    if let Some(prefix) = super::MirContext::type_prefix(elem_type, self.ctx.type_names) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                        found_prefix = true;
                    }
                }
            }
            // Fallback: detect Channel<T>.buffered/unbuffered pattern directly.
            // Returns (Sender<T>, Receiver<T>) — set prefixes by position.
            if !found_prefix {
                if let ExprKind::MethodCall { object, method, .. } = &init.kind {
                    if let ExprKind::Ident(type_name) = &object.kind {
                        let base = type_name.split('<').next().unwrap_or(type_name);
                        if base == "Channel" && (method == "buffered" || method == "unbuffered") {
                            let prefix = match i {
                                0 => "Sender",
                                1 => "Receiver",
                                _ => continue,
                            };
                            self.meta_mut(name).type_prefix = Some(prefix.to_string());
                            // Track channel element size for struct-aware recv
                            let inner = type_name.split('<').nth(1)
                                .and_then(|s| s.strip_suffix('>'));
                            let elem_size = if let Some(tn) = inner {
                                self.ctx.find_struct(tn)
                                    .map(|(_, l)| l.size as i64)
                                    .unwrap_or(8)
                            } else {
                                8
                            };
                            self.meta_mut(name).channel_elem_size = Some(elem_size);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // =================================================================
    // Loop lowering
    // =================================================================

    /// While loop (spec L5).
    fn lower_while(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), LoweringError> {
        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: check_block,
        }));

        self.builder.switch_to_block(check_block);
        let (cond_op, _) = self.lower_expr(cond)?;
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: cond_op,
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);
        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: None,
            continue_block: check_block,
            exit_block,
            result_local: None,
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: check_block,
        }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// For loop: counter-based while for ranges, iterator protocol otherwise.
    fn lower_for(
        &mut self,
        label: Option<&str>,
        binding: &ForBinding,
        mutate: bool,
        iter_expr: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        // Extract single name for range/iter-chain delegation (tuple not supported there)
        let single_name = match binding {
            ForBinding::Single(name) => name.as_str(),
            ForBinding::Tuple(names) => names.first().map_or("_", |n| n.as_str()),
        };

        // Range expressions desugar to a simple counter loop
        if let ExprKind::Range { start, end, inclusive } = &iter_expr.kind {
            return self.lower_for_range(label, single_name, start.as_deref(), end.as_deref(), *inclusive, body);
        }

        // Iterator chain: for x in vec.iter().filter(...).map(...) { ... }
        // Fuse into index loop with inlined adapter closures.
        if let Some(chain) = self.try_parse_iter_chain(iter_expr) {
            return self.lower_for_iter_chain(label, single_name, &chain, body, binding);
        }

        // pool.entries(): for (h, val) in pool.entries() { ... }
        // Desugars to: handles = Pool_handles(pool); for i in 0..len { h = handles[i]; val = Pool_get(pool, h); body }
        if let ForBinding::Tuple(names) = binding {
            if let ExprKind::MethodCall { object, method, .. } = &iter_expr.kind {
                if method == "entries" {
                    let obj_is_pool = self.ctx.lookup_raw_type(object.id).map_or(false, |ty| {
                        matches!(ty, rask_types::Type::UnresolvedNamed(n) if n == "Pool")
                            || matches!(ty, rask_types::Type::UnresolvedGeneric { name, .. } if name == "Pool")
                    });
                    if obj_is_pool {
                        return self.lower_for_pool_entries(label, names, object, body, mutate);
                    }
                }
            }
        }

        // Pool iteration: `for h in pool` desugars to snapshot handle iteration.
        // Calls Pool_handles(pool) → Vec<Handle>, then iterates the Vec.
        let is_pool = self.ctx.lookup_raw_type(iter_expr.id).map_or(false, |ty| {
            matches!(
                ty,
                rask_types::Type::UnresolvedNamed(n) if n == "Pool"
            ) || matches!(
                ty,
                rask_types::Type::UnresolvedGeneric { name, .. } if name == "Pool"
            )
        });

        // LP13: Detect Map iteration for correct writeback target.
        let is_map = self.ctx.lookup_raw_type(iter_expr.id).map_or(false, |ty| {
            matches!(
                ty,
                rask_types::Type::UnresolvedNamed(n) if n == "Map"
            ) || matches!(
                ty,
                rask_types::Type::UnresolvedGeneric { name, .. } if name == "Map"
            )
        });

        // Index-based iteration: for item in collection { ... }
        // Desugars to: _i = 0; _len = collection.len(); while _i < _len { item = collection[_i]; ...; _i += 1 }
        let (iter_op, iter_ty) = self.lower_expr(iter_expr)?;

        // For pools: convert pool → Vec<Handle> via Pool_handles snapshot
        let (iter_op, iter_ty) = if is_pool {
            let pool_tmp = self.builder.alloc_temp(iter_ty.clone());
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: pool_tmp,
                rvalue: MirRValue::Use(iter_op),
            }));
            let handles_vec = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(handles_vec),
                func: FunctionRef::internal("Pool_handles".to_string()),
                args: vec![MirOperand::Local(pool_tmp)],
            }));
            (MirOperand::Local(handles_vec), MirType::I64)
        } else {
            (iter_op, iter_ty)
        };

        let is_array = matches!(&iter_ty, MirType::Array { .. });
        let (array_len, array_elem_size) = match &iter_ty {
            MirType::Array { elem, len } => (Some(*len), Some(elem.size())),
            _ => (None, None),
        };

        let collection = self.builder.alloc_temp(iter_ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: collection,
            rvalue: MirRValue::Use(iter_op),
        }));

        // _len = collection.len()
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
            // Fixed-size array: compile-time constant length
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: len_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(arr_len as i64))),
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(len_local),
                func: FunctionRef::internal("Vec_len".to_string()),
                args: vec![MirOperand::Local(collection)],
            }));
        }

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // For `for mutate`, create writeback blocks that call Vec_set
        // before continuing or breaking out of the loop.
        let (wb_block, break_wb_block) = if mutate && !is_array {
            let wb = self.builder.create_block();
            let break_wb = self.builder.create_block();
            (Some(wb), Some(break_wb))
        } else {
            (None, None)
        };

        // continue target: writeback block (if mutate), otherwise inc_block
        let continue_target = wb_block.unwrap_or(inc_block);
        // break target: break-writeback block (if mutate), otherwise exit_block
        let break_target = break_wb_block.unwrap_or(exit_block);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        // body: item = collection[_i]
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(iter_expr)
            .unwrap_or(MirType::I64);
        let binding_local = self.builder.alloc_local(single_name.to_string(), elem_ty.clone());
        if let Some(prefix) = self.mir_type_name(&elem_ty) {
            self.meta_mut(single_name).type_prefix = Some(prefix);
        } else {
            // MirType is Ptr — try to derive element prefix from iterable context.
            // Method calls like .chunks() return Vec elements, .handles() returns Handle elements.
            if let ExprKind::MethodCall { method, .. } = &iter_expr.kind {
                match method.as_str() {
                    "chunks" => {
                        self.meta_mut(single_name).type_prefix = Some("Vec".to_string());
                    }
                    "handles" | "cursor" => {
                        self.meta_mut(single_name).type_prefix = Some("Handle".to_string());
                    }
                    _ => {}
                }
            }
        }
        self.locals.insert(single_name.to_string(), (binding_local, elem_ty));
        if is_array {
            // Fixed-size array: direct memory load
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: binding_local,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(binding_local),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            }));
        }

        // Tuple destructuring: for (a, b) in collection { ... }
        // Extract fields from the loaded element into each binding.
        // LP13: Track value local for Map writeback (key=field0, value=field1).
        let mut map_value_local = None;
        if let ForBinding::Tuple(names) = binding {
            for (i, name) in names.iter().enumerate() {
                if i == 0 { continue; }
                let field_local = self.builder.alloc_local(name.clone(), MirType::I64);
                self.locals.insert(name.clone(), (field_local, MirType::I64));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: field_local,
                    rvalue: MirRValue::Field {
                        base: MirOperand::Local(binding_local),
                        field_index: i as u32,
                        byte_offset: None,
                        field_size: None,
                    },
                }));
                // Track value local (field 1) for Map writeback
                if i == 1 && is_map {
                    map_value_local = Some(field_local);
                }
            }
            // Re-extract field 0 into the first binding (was whole tuple)
            let first_field = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: first_field,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(binding_local),
                    field_index: 0,
                    byte_offset: None,
                    field_size: None,
                },
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: binding_local,
                rvalue: MirRValue::Use(MirOperand::Local(first_field)),
            }));
        }

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: continue_target,
            exit_block: break_target,
            result_local: None,
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: continue_target }));

        // Writeback blocks for `for mutate`
        // LP13: Vec uses Vec_set(vec, idx, elem), Map uses Map_set(map, key, value)
        if let Some(wb) = wb_block {
            self.builder.switch_to_block(wb);
            if let Some(val_local) = map_value_local {
                // Map writeback: Map_set(collection, key, value)
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Map_set".to_string()),
                    args: vec![
                        MirOperand::Local(collection),
                        MirOperand::Local(binding_local), // key (field 0)
                        MirOperand::Local(val_local),     // value (field 1)
                    ],
                }));
            } else {
                // Vec writeback: Vec_set(collection, idx, elem)
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Vec_set".to_string()),
                    args: vec![
                        MirOperand::Local(collection),
                        MirOperand::Local(idx),
                        MirOperand::Local(binding_local),
                    ],
                }));
            }
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: inc_block }));
        }
        if let Some(break_wb) = break_wb_block {
            self.builder.switch_to_block(break_wb);
            if let Some(val_local) = map_value_local {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Map_set".to_string()),
                    args: vec![
                        MirOperand::Local(collection),
                        MirOperand::Local(binding_local),
                        MirOperand::Local(val_local),
                    ],
                }));
            } else {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("Vec_set".to_string()),
                    args: vec![
                        MirOperand::Local(collection),
                        MirOperand::Local(idx),
                        MirOperand::Local(binding_local),
                    ],
                }));
            }
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
        }

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Pool entries iteration: `for (h, val) in pool.entries()`
    /// Desugars to snapshot handle iteration with Pool_get for each handle.
    /// LP11-LP13: `for mutate` adds Pool_set writeback.
    fn lower_for_pool_entries(
        &mut self,
        label: Option<&str>,
        names: &[String],
        pool_expr: &Expr,
        body: &[Stmt],
        mutate: bool,
    ) -> Result<(), LoweringError> {
        let (pool_op, _) = self.lower_expr(pool_expr)?;
        let pool_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: pool_local,
            rvalue: MirRValue::Use(pool_op),
        }));

        // handles_vec = Pool_handles(pool)
        let handles_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handles_vec),
            func: FunctionRef::internal("Pool_handles".to_string()),
            args: vec![MirOperand::Local(pool_local)],
        }));

        // _len = Vec_len(handles_vec)
        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(len_local),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(handles_vec)],
        }));

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // LP11-LP13: for mutate writeback blocks for Pool_set
        let (wb_block, break_wb_block) = if mutate && names.len() > 1 {
            let wb = self.builder.create_block();
            let break_wb = self.builder.create_block();
            (Some(wb), Some(break_wb))
        } else {
            (None, None)
        };
        let continue_target = wb_block.unwrap_or(inc_block);
        let break_target = break_wb_block.unwrap_or(exit_block);

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        // body: h = handles_vec[_i]; val = Pool_get(pool, h)
        self.builder.switch_to_block(body_block);

        // Bind handle (first name)
        let handle_name = names.first().map_or("_h", |n| n.as_str());
        let handle_local = self.builder.alloc_local(handle_name.to_string(), MirType::I64);
        self.locals.insert(handle_name.to_string(), (handle_local, MirType::I64));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(handle_local),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(handles_vec), MirOperand::Local(idx)],
        }));

        // Bind value (second name) via Pool_get
        let val_local = if names.len() > 1 {
            let val_name = &names[1];
            let val_local = self.builder.alloc_local(val_name.clone(), MirType::I64);
            self.locals.insert(val_name.clone(), (val_local, MirType::I64));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(val_local),
                func: FunctionRef::internal("Pool_get".to_string()),
                args: vec![MirOperand::Local(pool_local), MirOperand::Local(handle_local)],
            }));
            Some(val_local)
        } else {
            None
        };

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: continue_target,
            exit_block: break_target,
            result_local: None,
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: continue_target }));

        // LP13: Pool_set writeback blocks for `for mutate`
        if let (Some(wb), Some(vl)) = (wb_block, val_local) {
            self.builder.switch_to_block(wb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Pool_set".to_string()),
                args: vec![
                    MirOperand::Local(pool_local),
                    MirOperand::Local(handle_local),
                    MirOperand::Local(vl),
                ],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: inc_block }));
        }
        if let (Some(break_wb), Some(vl)) = (break_wb_block, val_local) {
            self.builder.switch_to_block(break_wb);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal("Pool_set".to_string()),
                args: vec![
                    MirOperand::Local(pool_local),
                    MirOperand::Local(handle_local),
                    MirOperand::Local(vl),
                ],
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: exit_block }));
        }

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Range for-loop: `for i in start..end` desugars to a counter-based while.
    fn lower_for_range(
        &mut self,
        label: Option<&str>,
        binding: &str,
        start: Option<&Expr>,
        end: Option<&Expr>,
        inclusive: bool,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let (start_op, start_ty) = if let Some(s) = start {
            self.lower_expr(s)?
        } else {
            (MirOperand::Constant(MirConst::Int(0)), MirType::I64)
        };
        let (end_op, _) = if let Some(e) = end {
            self.lower_expr(e)?
        } else {
            return Err(LoweringError::InvalidConstruct("Unbounded range in for loop".to_string()));
        };

        // Mutable counter initialized to start
        let counter = self.builder.alloc_local(binding.to_string(), start_ty.clone());
        self.locals.insert(binding.to_string(), (counter, start_ty.clone()));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(start_op),
        }));

        // Evaluate end once
        let end_local = self.builder.alloc_temp(start_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: end_local,
            rvalue: MirRValue::Use(end_op),
        }));

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));
        self.builder.switch_to_block(check_block);

        // counter < end (or <= for inclusive)
        let cmp_op = if inclusive { BinOp::Le } else { BinOp::Lt };
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: cmp_op,
                left: MirOperand::Local(counter),
                right: MirOperand::Local(end_local),
            },
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        }));

        self.builder.switch_to_block(body_block);
        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: inc_block }));

        // counter = counter + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        }));
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: check_block }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Infinite loop.
    pub(super) fn lower_loop(&mut self, label: Option<&str>, body: &[Stmt]) -> Result<(), LoweringError> {
        let loop_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // CF25: allocate result slot so break-with-value can store to it
        let result_local = self.builder.alloc_local(
            "__loop_result".to_string(),
            MirType::I64,
        );

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: loop_block,
        }));

        self.builder.switch_to_block(loop_block);

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: loop_block,
            exit_block,
            result_local: Some(result_local),
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: loop_block,
        }));

        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Break statement - jump to enclosing loop's exit block.
    /// EX4: runs loop-scoped ensures before exiting.
    fn lower_break(
        &mut self,
        label: Option<&str>,
        value: Option<&Expr>,
    ) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let exit_block = ctx.exit_block;
        let result_local = ctx.result_local;
        let ensure_depth = ctx.ensure_depth;

        if let Some(val_expr) = value {
            let (val_op, _) = self.lower_expr(val_expr)?;
            if let Some(result) = result_local {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result,
                    rvalue: MirRValue::Use(val_op),
                }));
            }
        }

        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: exit_block,
        }));

        let dead_block = self.builder.create_block();
        self.builder.switch_to_block(dead_block);

        Ok(())
    }

    /// Continue statement - jump to enclosing loop's check block.
    /// EX4: runs loop-scoped ensures before continuing.
    fn lower_continue(&mut self, label: Option<&str>) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let continue_block = ctx.continue_block;
        let ensure_depth = ctx.ensure_depth;

        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: continue_block,
        }));

        let dead_block = self.builder.create_block();
        self.builder.switch_to_block(dead_block);

        Ok(())
    }

    /// Find the loop context for a break/continue, optionally by label.
    fn find_loop(&self, label: Option<&str>) -> Result<&LoopContext, LoweringError> {
        match label {
            None => self.loop_stack.last().ok_or_else(|| {
                LoweringError::InvalidConstruct("break/continue outside of loop".to_string())
            }),
            Some(lbl) => self
                .loop_stack
                .iter()
                .rev()
                .find(|ctx| ctx.label.as_deref() == Some(lbl))
                .ok_or_else(|| {
                    LoweringError::InvalidConstruct(format!("No loop with label '{}'", lbl))
                }),
        }
    }

    /// For-in over an iterator chain: fused index loop with inlined adapters.
    fn lower_for_iter_chain(
        &mut self,
        label: Option<&str>,
        binding_name: &str,
        chain: &super::IterChain<'_>,
        body: &[Stmt],
        for_binding: &ForBinding,
    ) -> Result<(), LoweringError> {
        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty.clone(),
            setup.inc_block, setup.idx,
        )?;

        // Bind final value to the loop variable
        let binding_local = self.builder.alloc_local(binding_name.to_string(), final_ty.clone());
        if let Some(prefix) = self.mir_type_name(&final_ty) {
            self.meta_mut(binding_name).type_prefix = Some(prefix);
        }
        self.locals.insert(binding_name.to_string(), (binding_local, final_ty));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: binding_local,
            rvalue: MirRValue::Use(final_op),
        }));

        // Tuple destructuring for iter chains
        if let ForBinding::Tuple(names) = for_binding {
            for (i, name) in names.iter().enumerate() {
                if i == 0 { continue; }
                let field_local = self.builder.alloc_local(name.clone(), MirType::I64);
                self.locals.insert(name.clone(), (field_local, MirType::I64));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: field_local,
                    rvalue: MirRValue::Field {
                        base: MirOperand::Local(binding_local),
                        field_index: i as u32,
                        byte_offset: None,
                        field_size: None,
                    },
                }));
            }
            // Re-extract field 0 into the first binding
            let first_field = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: first_field,
                rvalue: MirRValue::Field {
                    base: MirOperand::Local(binding_local),
                    field_index: 0,
                    byte_offset: None,
                    field_size: None,
                },
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: binding_local,
                rvalue: MirRValue::Use(MirOperand::Local(first_field)),
            }));
        }

        let ensure_depth = self.ensure_stack.len();
        self.loop_stack.push(super::LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: setup.inc_block,
            exit_block: setup.exit_block,
            result_local: None,
            ensure_depth,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }

        // EN7: run loop-scoped ensures at iteration end
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: setup.inc_block }));
        self.loop_stack.pop();
        self.ensure_stack.truncate(ensure_depth);

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok(())
    }

}
