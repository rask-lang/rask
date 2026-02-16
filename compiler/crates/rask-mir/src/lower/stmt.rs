// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Statement lowering.

use super::{LoopContext, LoweringError, MirLowerer};
use crate::{
    operand::{BinOp, MirConst},
    FunctionRef, MirOperand, MirRValue, MirStmt, MirTerminator, MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
};

impl<'a> MirLowerer<'a> {
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                Ok(())
            }

            StmtKind::Let { name, ty, init, .. }
            | StmtKind::Const { name, ty, init, .. } => {
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
                    _ => {
                        return Err(LoweringError::InvalidConstruct(
                            "Complex assignment targets not yet supported".to_string(),
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
        self.locals.insert(name.to_string(), (local_id, var_ty));
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
        for (i, name) in names.iter().enumerate() {
            // Infer element type - would need full tuple type parsing
            // For now, try to look up from type checker, otherwise default to I32
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
        binding: &str,
        iter_expr: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        // Range expressions desugar to a simple counter loop
        if let ExprKind::Range { start, end, inclusive } = &iter_expr.kind {
            return self.lower_for_range(label, binding, start.as_deref(), end.as_deref(), *inclusive, body);
        }

        // Index-based iteration: for item in collection { ... }
        // Desugars to: _i = 0; _len = collection.len(); while _i < _len { item = collection.get(_i); ...; _i += 1 }
        let (iter_op, iter_ty) = self.lower_expr(iter_expr)?;
        let collection = self.builder.alloc_temp(iter_ty.clone());
        self.builder.push_stmt(MirStmt::Assign {
            dst: collection,
            rvalue: MirRValue::Use(iter_op),
        });

        // _len = collection.len()
        let len_local = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(len_local),
            func: FunctionRef { name: "len".to_string() },
            args: vec![MirOperand::Local(collection)],
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

        // body: item = collection.get(_i)
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(iter_expr)
            .unwrap_or(MirType::I64);
        let binding_local = self.builder.alloc_local(binding.to_string(), elem_ty.clone());
        self.locals.insert(binding.to_string(), (binding_local, elem_ty));
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(binding_local),
            func: FunctionRef { name: "get".to_string() },
            args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
        });

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
}
