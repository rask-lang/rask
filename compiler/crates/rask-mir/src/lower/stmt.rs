// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Statement lowering.

use super::{LoopContext, LoweringError, MirLowerer};
use crate::{
    operand::{BinOp, MirConst},
    types::StructLayoutId,
    FunctionRef, MirOperand, MirRValue, MirStmt, MirTerminator, MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, UnaryOp},
    stmt::{ForBinding, Stmt, StmtKind},
};

impl<'a> MirLowerer<'a> {
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        self.emit_source_location(&stmt.span);
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                Ok(())
            }

            StmtKind::Let { name, ty, init, .. } => {
                self.lower_binding(name, ty.as_deref(), init)
            }

            StmtKind::Const { name, ty, init, .. } => {
                // If this const was evaluated at compile time, emit a global reference
                if let Some(meta) = self.ctx.comptime_globals.get(name) {
                    let mir_ty = if let Some(ty_str) = ty.as_deref() {
                        self.ctx.resolve_type_str(ty_str)
                    } else {
                        MirType::Ptr
                    };
                    let local_id = self.builder.alloc_local(name.to_string(), mir_ty.clone());
                    self.locals.insert(name.to_string(), (local_id, mir_ty));
                    self.builder.push_stmt(MirStmt::GlobalRef {
                        dst: local_id,
                        name: name.clone(),
                    });
                    // Track type prefix for method dispatch
                    self.local_type_prefix.insert(name.to_string(), meta.type_prefix.clone());
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
                self.builder.terminate(MirTerminator::Return { value });
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
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: local_id,
                            rvalue: MirRValue::Use(val_op),
                        });
                    }
                    // Field assignment: obj.field = value → Store at field offset
                    ExprKind::Field { object, field } => {
                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let offset = if let MirType::Struct(StructLayoutId(id)) = &obj_ty {
                            if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                layout.fields.iter()
                                    .find(|f| f.name == *field)
                                    .map(|f| f.offset)
                                    .unwrap_or(0)
                            } else { 0 }
                        } else { 0 };
                        let base_local = match obj_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(obj_ty);
                                self.builder.push_stmt(MirStmt::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(obj_op),
                                });
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::Store {
                            addr: base_local,
                            offset,
                            value: val_op,
                        });
                    }
                    // Index assignment: a[i] = val
                    ExprKind::Index { object, index } => {
                        let (obj_op, obj_ty) = self.lower_expr(object)?;
                        let (idx_op, _) = self.lower_expr(index)?;

                        if let MirType::Array { ref elem, .. } = obj_ty {
                            // Fixed-size array: direct store at base + index * elem_size
                            if let MirOperand::Local(base_id) = obj_op {
                                self.builder.push_stmt(MirStmt::ArrayStore {
                                    base: base_id,
                                    index: idx_op,
                                    elem_size: elem.size(),
                                    value: val_op,
                                });
                            }
                        } else {
                            // Vec/Map: dispatch through runtime
                            self.builder.push_stmt(MirStmt::Call {
                                dst: None,
                                func: FunctionRef::internal("Vec_set".to_string()),
                                args: vec![obj_op, idx_op, val_op],
                            });
                        }
                    }
                    // Deref assignment: *ptr = value → Store through pointer
                    ExprKind::Unary { op: UnaryOp::Deref, operand } => {
                        let (addr_op, _) = self.lower_expr(operand)?;
                        let addr_local = match addr_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(addr_op),
                                });
                                tmp
                            }
                        };
                        self.builder.push_stmt(MirStmt::Store {
                            addr: addr_local,
                            offset: 0,
                            value: val_op,
                        });
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
                iter,
                body,
            } => self.lower_for(label.as_deref(), binding, iter, body),

            // Infinite loop
            StmtKind::Loop { label, body } => self.lower_loop(label.as_deref(), body),

            // Break
            StmtKind::Break { label, value } => self.lower_break(label.as_deref(), value.as_ref()),

            // Continue
            StmtKind::Continue(label) => self.lower_continue(label.as_deref()),

            // Tuple destructuring
            StmtKind::LetTuple { names, init }
            | StmtKind::ConstTuple { names, init } => {
                self.lower_tuple_destructure(names, init)
            }

            // While-let pattern loop
            StmtKind::WhileLet { pattern, expr, body } => {
                let check_block = self.builder.create_block();
                let body_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Goto { target: check_block });

                self.builder.switch_to_block(check_block);
                let (val, _) = self.lower_expr(expr)?;
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });
                // Compare tag against expected variant
                let expected = self.pattern_tag(pattern);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(crate::operand::MirConst::Int(expected)),
                    },
                });
                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: body_block,
                    else_block: exit_block,
                });

                self.builder.switch_to_block(body_block);
                // Bind payload variables from the pattern
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload(pattern, val, payload_ty);
                self.loop_stack.push(LoopContext {
                    label: None,
                    continue_block: check_block,
                    exit_block,
                    result_local: None,
                });
                for s in body {
                    self.lower_stmt(s)?;
                }
                self.builder.terminate(MirTerminator::Goto { target: check_block });
                self.loop_stack.pop();

                self.builder.switch_to_block(exit_block);
                Ok(())
            }

            // Ensure (spec L4)
            StmtKind::Ensure { body, else_handler } => {
                let cleanup_block = self.builder.create_block();
                let continue_block = self.builder.create_block();

                self.builder.push_stmt(MirStmt::EnsurePush { cleanup_block });

                for s in body {
                    self.lower_stmt(s)?;
                }

                self.builder.push_stmt(MirStmt::EnsurePop);
                self.builder.terminate(MirTerminator::Goto { target: continue_block });

                self.builder.switch_to_block(cleanup_block);
                if let Some((param_name, handler_body)) = else_handler {
                    // Error type - would need full type inference to determine exact type
                    // For now, use I32 as a placeholder for error values
                    let param_ty = MirType::I32;
                    let param_local = self.builder.alloc_local(param_name.clone(), param_ty.clone());
                    self.locals.insert(param_name.clone(), (param_local, param_ty));
                    for s in handler_body {
                        self.lower_stmt(s)?;
                    }
                }
                self.builder.push_stmt(MirStmt::EnsurePop);
                self.builder.terminate(MirTerminator::Goto { target: continue_block });

                self.builder.switch_to_block(continue_block);
                Ok(())
            }

            // Comptime (compile-time evaluated)
            StmtKind::Comptime(stmts) => {
                for s in stmts {
                    self.lower_stmt(s)?;
                }
                Ok(())
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: local_id,
            rvalue: MirRValue::Use(init_op),
        });

        // Track collection element types for for-in iteration heuristics
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                match (obj_name.as_str(), method.as_str()) {
                    ("cli", "args") | ("fs", "read_lines") => {
                        self.collection_elem_types.insert(name.to_string(), MirType::String);
                    }
                    _ => {}
                }
            }
            // String methods that always return Vec<string>
            match method.as_str() {
                "lines" | "split" | "split_whitespace" => {
                    self.collection_elem_types.insert(name.to_string(), MirType::String);
                }
                _ => {}
            }
        }

        // Track stdlib type prefix for variables assigned from type constructors,
        // known module functions, or method calls on tracked variables,
        // so later method calls dispatch correctly.
        if let ExprKind::MethodCall { object, method, .. } = &init.kind {
            if let ExprKind::Ident(obj_name) = &object.kind {
                if super::is_type_constructor_name(obj_name) {
                    // Type.method() → prefix is the type name.
                    // Covers stdlib (Vec, Map, string) and user types (Person, Document).
                    if super::MirContext::stdlib_type_prefix(
                        &rask_types::Type::UnresolvedNamed(obj_name.clone())
                    ).is_some()
                        || obj_name.chars().next().map_or(false, |c| c.is_uppercase())
                    {
                        self.local_type_prefix.insert(name.to_string(), obj_name.clone());
                    } else {
                        // Module function (fs.open) → check return type prefix
                        let func_name = format!("{}_{}", obj_name, method);
                        if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                            self.local_type_prefix.insert(name.to_string(), prefix.to_string());
                        }
                    }
                } else if let Some(obj_prefix) = self.local_type_prefix.get(obj_name).cloned() {
                    // Instance method on tracked variable (file.lines() → File_lines)
                    let func_name = format!("{}_{}", obj_prefix, method);
                    if let Some(prefix) = super::func_return_type_prefix(&func_name) {
                        self.local_type_prefix.insert(name.to_string(), prefix.to_string());
                    }
                }
            }
        }
        // Iterator terminal .collect() returns a Vec
        if let ExprKind::MethodCall { method, .. } = &init.kind {
            if method == "collect" {
                self.local_type_prefix.insert(name.to_string(), "Vec".to_string());
            }
        }
        // Also track for simple function calls (e.g. cli.args())
        if let ExprKind::Call { func, .. } = &init.kind {
            if let ExprKind::Ident(func_name) = &func.kind {
                if let Some(prefix) = super::func_return_type_prefix(func_name) {
                    self.local_type_prefix.insert(name.to_string(), prefix.to_string());
                }
            }
        }
        // Index expression: args[1] → if args has known element type, propagate it
        if let ExprKind::Index { object, .. } = &init.kind {
            if let ExprKind::Ident(coll_name) = &object.kind {
                if let Some(elem_ty) = self.collection_elem_types.get(coll_name).cloned() {
                    if let Some(prefix) = self.mir_type_name(&elem_ty) {
                        self.local_type_prefix.insert(name.to_string(), prefix);
                    }
                }
            }
        }

        // Fallback: derive prefix from the MIR type (catches String, Struct, Enum)
        // or from the type annotation string (catches Ptr types like Vec<T>, Map<K,V>)
        if !self.local_type_prefix.contains_key(name) {
            if let Some(prefix) = self.mir_type_name(&var_ty) {
                self.local_type_prefix.insert(name.to_string(), prefix);
            } else if let Some(ty_str) = ty {
                if let Some(prefix) = super::type_prefix_from_str(ty_str) {
                    self.local_type_prefix.insert(name.to_string(), prefix);
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

        Ok(())
    }

    /// Lower tuple destructuring: evaluate init, extract each element by field index.
    fn lower_tuple_destructure(&mut self, names: &[String], init: &Expr) -> Result<(), LoweringError> {
        let (init_op, _) = self.lower_expr(init)?;

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

        for (i, name) in names.iter().enumerate() {
            let elem_ty = self.lookup_expr_type(init)
                .or_else(|| Some(MirType::I32))
                .unwrap_or(MirType::I32);
            let local_id = self.builder.alloc_local(name.clone(), elem_ty.clone());
            self.locals.insert(name.clone(), (local_id, elem_ty));
            self.builder.push_stmt(MirStmt::Assign {
                dst: local_id,
                rvalue: MirRValue::Field {
                    base: init_op.clone(),
                    field_index: i as u32,
                },
            });

            // Track type prefix so method calls get qualified names.
            // First try type-checker info (works when types are fully resolved).
            let mut found_prefix = false;
            if let Some(ref elems) = tuple_elems {
                if let Some(elem_type) = elems.get(i) {
                    if let Some(prefix) = super::MirContext::type_prefix(elem_type) {
                        self.local_type_prefix.insert(name.clone(), prefix);
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
                            self.local_type_prefix.insert(name.clone(), prefix.to_string());
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

        self.builder.terminate(MirTerminator::Goto {
            target: check_block,
        });

        self.builder.switch_to_block(check_block);
        let (cond_op, _) = self.lower_expr(cond)?;
        self.builder.terminate(MirTerminator::Branch {
            cond: cond_op,
            then_block: body_block,
            else_block: exit_block,
        });

        self.builder.switch_to_block(body_block);
        self.loop_stack.push(LoopContext {
            label: None,
            continue_block: check_block,
            exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        self.builder.terminate(MirTerminator::Goto {
            target: check_block,
        });

        self.loop_stack.pop();
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// For loop: counter-based while for ranges, iterator protocol otherwise.
    fn lower_for(
        &mut self,
        label: Option<&str>,
        binding: &ForBinding,
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
            return self.lower_for_iter_chain(label, single_name, &chain, body);
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
                        return self.lower_for_pool_entries(label, names, object, body);
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

        // Index-based iteration: for item in collection { ... }
        // Desugars to: _i = 0; _len = collection.len(); while _i < _len { item = collection[_i]; ...; _i += 1 }
        let (iter_op, iter_ty) = self.lower_expr(iter_expr)?;

        // For pools: convert pool → Vec<Handle> via Pool_handles snapshot
        let (iter_op, iter_ty) = if is_pool {
            let pool_tmp = self.builder.alloc_temp(iter_ty.clone());
            self.builder.push_stmt(MirStmt::Assign {
                dst: pool_tmp,
                rvalue: MirRValue::Use(iter_op),
            });
            let handles_vec = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(handles_vec),
                func: FunctionRef::internal("Pool_handles".to_string()),
                args: vec![MirOperand::Local(pool_tmp)],
            });
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: collection,
            rvalue: MirRValue::Use(iter_op),
        });

        // _len = collection.len()
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
            // Fixed-size array: compile-time constant length
            self.builder.push_stmt(MirStmt::Assign {
                dst: len_local,
                rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(arr_len as i64))),
            });
        } else {
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(len_local),
                func: FunctionRef::internal("Vec_len".to_string()),
                args: vec![MirOperand::Local(collection)],
            });
        }

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto { target: check_block });

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        });
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // body: item = collection[_i]
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(iter_expr)
            .unwrap_or(MirType::I64);
        let binding_local = self.builder.alloc_local(single_name.to_string(), elem_ty.clone());
        if let Some(prefix) = self.mir_type_name(&elem_ty) {
            self.local_type_prefix.insert(single_name.to_string(), prefix);
        } else {
            // MirType is Ptr — try to derive element prefix from iterable context.
            // Method calls like .chunks() return Vec elements, .handles() returns Handle elements.
            if let ExprKind::MethodCall { method, .. } = &iter_expr.kind {
                match method.as_str() {
                    "chunks" => {
                        self.local_type_prefix.insert(single_name.to_string(), "Vec".to_string());
                    }
                    "handles" | "cursor" => {
                        self.local_type_prefix.insert(single_name.to_string(), "Handle".to_string());
                    }
                    _ => {}
                }
            }
        }
        self.locals.insert(single_name.to_string(), (binding_local, elem_ty));
        if is_array {
            // Fixed-size array: direct memory load
            self.builder.push_stmt(MirStmt::Assign {
                dst: binding_local,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            });
        } else {
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(binding_local),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            });
        }

        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        self.builder.terminate(MirTerminator::Goto { target: inc_block });

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });
        self.builder.terminate(MirTerminator::Goto { target: check_block });

        self.loop_stack.pop();
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Pool entries iteration: `for (h, val) in pool.entries()`
    /// Desugars to snapshot handle iteration with Pool_get for each handle.
    fn lower_for_pool_entries(
        &mut self,
        label: Option<&str>,
        names: &[String],
        pool_expr: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let (pool_op, _) = self.lower_expr(pool_expr)?;
        let pool_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: pool_local,
            rvalue: MirRValue::Use(pool_op),
        });

        // handles_vec = Pool_handles(pool)
        let handles_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(handles_vec),
            func: FunctionRef::internal("Pool_handles".to_string()),
            args: vec![MirOperand::Local(pool_local)],
        });

        // _len = Vec_len(handles_vec)
        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(len_local),
            func: FunctionRef::internal("Vec_len".to_string()),
            args: vec![MirOperand::Local(handles_vec)],
        });

        // _i = 0
        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto { target: check_block });

        // check: _i < _len
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Lt,
                left: MirOperand::Local(idx),
                right: MirOperand::Local(len_local),
            },
        });
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // body: h = handles_vec[_i]; val = Pool_get(pool, h)
        self.builder.switch_to_block(body_block);

        // Bind handle (first name)
        let handle_name = names.first().map_or("_h", |n| n.as_str());
        let handle_local = self.builder.alloc_local(handle_name.to_string(), MirType::I64);
        self.locals.insert(handle_name.to_string(), (handle_local, MirType::I64));
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(handle_local),
            func: FunctionRef::internal("Vec_get".to_string()),
            args: vec![MirOperand::Local(handles_vec), MirOperand::Local(idx)],
        });

        // Bind value (second name) via Pool_get
        if names.len() > 1 {
            let val_name = &names[1];
            let val_local = self.builder.alloc_local(val_name.clone(), MirType::I64);
            self.locals.insert(val_name.clone(), (val_local, MirType::I64));
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(val_local),
                func: FunctionRef::internal("Pool_get".to_string()),
                args: vec![MirOperand::Local(pool_local), MirOperand::Local(handle_local)],
            });
        }

        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        self.builder.terminate(MirTerminator::Goto { target: inc_block });

        // inc: _i = _i + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });
        self.builder.terminate(MirTerminator::Goto { target: check_block });

        self.loop_stack.pop();
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
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(start_op),
        });

        // Evaluate end once
        let end_local = self.builder.alloc_temp(start_ty);
        self.builder.push_stmt(MirStmt::Assign {
            dst: end_local,
            rvalue: MirRValue::Use(end_op),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto { target: check_block });
        self.builder.switch_to_block(check_block);

        // counter < end (or <= for inclusive)
        let cmp_op = if inclusive { BinOp::Le } else { BinOp::Lt };
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: cmp_op,
                left: MirOperand::Local(counter),
                right: MirOperand::Local(end_local),
            },
        });
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        self.builder.switch_to_block(body_block);
        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: inc_block,
            exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        self.builder.terminate(MirTerminator::Goto { target: inc_block });

        // counter = counter + 1
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });
        self.builder.terminate(MirTerminator::Goto { target: check_block });

        self.loop_stack.pop();
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Infinite loop.
    fn lower_loop(&mut self, label: Option<&str>, body: &[Stmt]) -> Result<(), LoweringError> {
        let loop_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto {
            target: loop_block,
        });

        self.builder.switch_to_block(loop_block);

        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: loop_block,
            exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }
        self.builder.terminate(MirTerminator::Goto {
            target: loop_block,
        });

        self.loop_stack.pop();
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// Break statement - jump to enclosing loop's exit block.
    fn lower_break(
        &mut self,
        label: Option<&str>,
        value: Option<&Expr>,
    ) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let exit_block = ctx.exit_block;
        let result_local = ctx.result_local;

        if let Some(val_expr) = value {
            let (val_op, _) = self.lower_expr(val_expr)?;
            if let Some(result) = result_local {
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result,
                    rvalue: MirRValue::Use(val_op),
                });
            }
        }

        self.builder.terminate(MirTerminator::Goto {
            target: exit_block,
        });

        let dead_block = self.builder.create_block();
        self.builder.switch_to_block(dead_block);

        Ok(())
    }

    /// Continue statement - jump to enclosing loop's check block.
    fn lower_continue(&mut self, label: Option<&str>) -> Result<(), LoweringError> {
        let ctx = self.find_loop(label)?;
        let continue_block = ctx.continue_block;

        self.builder.terminate(MirTerminator::Goto {
            target: continue_block,
        });

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
        binding: &str,
        chain: &super::IterChain<'_>,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty.clone(),
            setup.inc_block, setup.idx,
        )?;

        // Bind final value to the loop variable
        let binding_local = self.builder.alloc_local(binding.to_string(), final_ty.clone());
        if let Some(prefix) = self.mir_type_name(&final_ty) {
            self.local_type_prefix.insert(binding.to_string(), prefix);
        }
        self.locals.insert(binding.to_string(), (binding_local, final_ty));
        self.builder.push_stmt(MirStmt::Assign {
            dst: binding_local,
            rvalue: MirRValue::Use(final_op),
        });

        self.loop_stack.push(super::LoopContext {
            label: label.map(|s| s.to_string()),
            continue_block: setup.inc_block,
            exit_block: setup.exit_block,
            result_local: None,
        });

        for stmt in body {
            self.lower_stmt(stmt)?;
        }

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
        self.loop_stack.pop();

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok(())
    }
}
