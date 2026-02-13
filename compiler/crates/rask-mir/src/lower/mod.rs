// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR lowering - transform AST to MIR CFG.

mod expr;
mod stmt;

use crate::{
    operand::MirConst, BlockBuilder, FunctionRef, MirFunction, MirOperand, MirRValue, MirStmt,
    MirTerminator, MirType, BlockId, LocalId,
};
use rask_ast::{
    decl::{Decl, DeclKind},
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
};
use std::collections::HashMap;

/// Loop context for break/continue
struct LoopContext {
    label: Option<String>,
    /// Block to jump to on `continue`
    continue_block: BlockId,
    /// Block to jump to on `break`
    exit_block: BlockId,
    /// For `break value` - local to assign the value to
    result_local: Option<LocalId>,
}

pub struct MirLowerer {
    builder: BlockBuilder,
    locals: HashMap<String, LocalId>,
    /// Stack of enclosing loops (innermost last)
    loop_stack: Vec<LoopContext>,
}

impl MirLowerer {
    pub fn lower_function(decl: &Decl) -> Result<MirFunction, LoweringError> {
        let fn_decl = match &decl.kind {
            DeclKind::Fn(f) => f,
            _ => {
                return Err(LoweringError::InvalidConstruct(
                    "Expected function declaration".to_string(),
                ))
            }
        };

        // TODO: Parse return type string to MirType
        let ret_ty = MirType::Void; // Placeholder

        let mut lowerer = MirLowerer {
            builder: BlockBuilder::new(fn_decl.name.clone(), ret_ty),
            locals: HashMap::new(),
            loop_stack: Vec::new(),
        };

        // Add parameters
        for param in &fn_decl.params {
            // TODO: Parse param type
            let param_ty = MirType::I32; // Placeholder
            let local_id = lowerer.builder.add_param(param.name.clone(), param_ty);
            lowerer.locals.insert(param.name.clone(), local_id);
        }

        // Lower function body
        for stmt in &fn_decl.body {
            lowerer.lower_stmt(stmt)?;
        }

        // TODO: Add implicit void return if missing

        Ok(lowerer.builder.finish())
    }

    // =================================================================
    // Expression lowering
    // =================================================================

    fn lower_expr(&mut self, expr: &Expr) -> Result<MirOperand, LoweringError> {
        match &expr.kind {
            // Literals
            ExprKind::Int(val, _suffix) => Ok(MirOperand::Constant(MirConst::Int(*val))),
            ExprKind::Float(val, _suffix) => Ok(MirOperand::Constant(MirConst::Float(*val))),
            ExprKind::String(s) => Ok(MirOperand::Constant(MirConst::String(s.clone()))),
            ExprKind::Char(c) => Ok(MirOperand::Constant(MirConst::Char(*c))),
            ExprKind::Bool(b) => Ok(MirOperand::Constant(MirConst::Bool(*b))),

            // Variable reference
            ExprKind::Ident(name) => self
                .locals
                .get(name)
                .copied()
                .map(MirOperand::Local)
                .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone())),

            // Binary operations → method calls
            ExprKind::Binary { op, left, right } => {
                let left_op = self.lower_expr(left)?;
                let right_op = self.lower_expr(right)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                let method_name = binop_method_name(*op);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: method_name },
                    args: vec![left_op, right_op],
                });

                Ok(MirOperand::Local(result_local))
            }

            // Unary operations → method calls
            ExprKind::Unary { op, operand } => {
                let operand_op = self.lower_expr(operand)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                let method_name = unaryop_method_name(*op);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: method_name },
                    args: vec![operand_op],
                });

                Ok(MirOperand::Local(result_local))
            }

            // Function call
            ExprKind::Call { func, args } => {
                let func_name = match &func.kind {
                    ExprKind::Ident(name) => name.clone(),
                    _ => {
                        return Err(LoweringError::InvalidConstruct(
                            "Complex function expressions not yet supported".to_string(),
                        ))
                    }
                };

                let arg_operands: Result<Vec<_>, _> =
                    args.iter().map(|a| self.lower_expr(a)).collect();
                let arg_operands = arg_operands?;

                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: func_name },
                    args: arg_operands,
                });

                Ok(MirOperand::Local(result_local))
            }

            // If expression (spec L1)
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(cond, then_branch, else_branch.as_deref()),

            // Match expression (spec L2)
            ExprKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms),

            // Block expression
            ExprKind::Block(stmts) => self.lower_block(stmts),

            // Method call
            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => {
                let obj_op = self.lower_expr(object)?;
                let mut all_args = vec![obj_op];
                for arg in args {
                    all_args.push(self.lower_expr(arg)?);
                }
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: method.clone(),
                    },
                    args: all_args,
                });
                Ok(MirOperand::Local(result_local))
            }

            // Field access
            ExprKind::Field { object, field: _ } => {
                let obj_op = self.lower_expr(object)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Lookup field index
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index: 0, // TODO: Lookup from layout
                    },
                });
                Ok(MirOperand::Local(result_local))
            }

            // Index access
            ExprKind::Index { object, index } => {
                let obj_op = self.lower_expr(object)?;
                let idx_op = self.lower_expr(index)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: "index".to_string(),
                    },
                    args: vec![obj_op, idx_op],
                });
                Ok(MirOperand::Local(result_local))
            }

            // Array literal
            ExprKind::Array(elems) => {
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Array type
                for (i, elem) in elems.iter().enumerate() {
                    let elem_op = self.lower_expr(elem)?;
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * 4, // TODO: Proper element size
                        value: elem_op,
                    });
                }
                Ok(MirOperand::Local(result_local))
            }

            // Tuple literal
            ExprKind::Tuple(elems) => {
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Tuple type
                for (i, elem) in elems.iter().enumerate() {
                    let elem_op = self.lower_expr(elem)?;
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * 8, // TODO: Proper field offsets
                        value: elem_op,
                    });
                }
                Ok(MirOperand::Local(result_local))
            }

            // Struct literal
            ExprKind::StructLit { fields, .. } => {
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Struct type
                for (i, field) in fields.iter().enumerate() {
                    let val_op = self.lower_expr(&field.value)?;
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * 4, // TODO: Use layout
                        value: val_op,
                    });
                }
                Ok(MirOperand::Local(result_local))
            }

            // If-let (if expr is Pattern { then } else { else })
            ExprKind::IfLet {
                expr,
                pattern: _,
                then_branch,
                else_branch,
            } => {
                // TODO: Proper pattern matching with tag check
                // For now, treat like if-expression with the expr as condition
                let cond_op = self.lower_expr(expr)?;
                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::I32);

                self.builder.terminate(MirTerminator::Branch {
                    cond: cond_op,
                    then_block,
                    else_block,
                });

                self.builder.switch_to_block(then_block);
                let then_val = self.lower_expr(then_branch)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(then_val),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(else_block);
                if let Some(else_expr) = else_branch {
                    let else_val = self.lower_expr(else_expr)?;
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(else_val),
                    });
                }
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok(MirOperand::Local(result_local))
            }

            // Guard pattern (const v = expr is Pattern else { diverge })
            ExprKind::GuardPattern {
                expr,
                pattern: _,
                else_branch,
            } => {
                // TODO: Proper pattern match + bind
                let val = self.lower_expr(expr)?;
                let ok_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                // Treat as "always matches" for now
                self.builder.terminate(MirTerminator::Branch {
                    cond: val.clone(),
                    then_block: ok_block,
                    else_block,
                });

                self.builder.switch_to_block(else_block);
                self.lower_expr(else_branch)?;
                // else_branch should diverge (return/break), but terminate anyway
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok(val)
            }

            // Try expression (spec L3)
            ExprKind::Try(inner) => self.lower_try(inner),

            // Unwrap (postfix !) - panic on None/Err
            ExprKind::Unwrap(inner) => {
                let val = self.lower_expr(inner)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block, // tag != 0 means Err/None
                    else_block: ok_block,    // tag == 0 means Ok/Some
                });

                self.builder.switch_to_block(panic_block);
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: "panic_unwrap".to_string() },
                    args: vec![],
                });
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                // Extract payload
                let result_local = self.builder.alloc_temp(MirType::I32);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: val, field_index: 0 },
                });
                Ok(MirOperand::Local(result_local))
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let val = self.lower_expr(value)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::I32);

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                });

                self.builder.switch_to_block(some_block);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: val, field_index: 0 },
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(none_block);
                let default_val = self.lower_expr(default)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(default_val),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok(MirOperand::Local(result_local))
            }

            // Range expression
            ExprKind::Range { start, end, inclusive } => {
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Range type
                let mut args = Vec::new();
                if let Some(s) = start {
                    args.push(self.lower_expr(s)?);
                }
                if let Some(e) = end {
                    args.push(self.lower_expr(e)?);
                }
                let func_name = if *inclusive { "range_inclusive" } else { "range" };
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: func_name.to_string() },
                    args,
                });
                Ok(MirOperand::Local(result_local))
            }

            // Array repeat ([value; count])
            ExprKind::ArrayRepeat { value, count } => {
                let val = self.lower_expr(value)?;
                let cnt = self.lower_expr(count)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Array type
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: "array_repeat".to_string() },
                    args: vec![val, cnt],
                });
                Ok(MirOperand::Local(result_local))
            }

            // Optional chaining (a?.b)
            ExprKind::OptionalField { object, field: _ } => {
                // Desugar to: if obj is Some(v): Some(v.field) else: None
                let obj = self.lower_expr(object)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: obj.clone() },
                });

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::I32);

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                });

                self.builder.switch_to_block(some_block);
                // TODO: Extract inner value and access field
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: obj, field_index: 0 },
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(none_block);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))), // None
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok(MirOperand::Local(result_local))
            }

            // Closure
            ExprKind::Closure { params, body, .. } => {
                // Closures lower to: allocate env struct, store captures, create fat ptr
                // TODO: Full capture analysis - for now just lower the body
                let result_local = self.builder.alloc_temp(MirType::Ptr);

                // Register closure params as locals within the body
                let saved_locals = self.locals.clone();
                for param in params {
                    let param_local = self.builder.alloc_temp(MirType::I32);
                    self.locals.insert(param.name.clone(), param_local);
                }
                let _body_val = self.lower_expr(body)?;
                self.locals = saved_locals;

                // TODO: Create closure struct with function pointer + env
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))), // Placeholder
                });
                Ok(MirOperand::Local(result_local))
            }

            // Cast
            ExprKind::Cast { expr, .. } => {
                let val = self.lower_expr(expr)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Target type
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Cast {
                        value: val,
                        target_ty: MirType::I32,
                    },
                });
                Ok(MirOperand::Local(result_local))
            }

            // Using block
            ExprKind::UsingBlock { body, .. } => {
                // TODO: Set up context, lower body, tear down context
                self.lower_block_stmts(body)
            }

            // With-as binding
            ExprKind::WithAs { bindings, body } => {
                // Lower bindings, make them available, lower body
                for (bind_expr, name) in bindings {
                    let val = self.lower_expr(bind_expr)?;
                    let local = self.builder.alloc_local(name.clone(), MirType::I32);
                    self.locals.insert(name.clone(), local);
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: local,
                        rvalue: MirRValue::Use(val),
                    });
                }
                self.lower_block_stmts(body)
            }

            // Spawn
            ExprKind::Spawn { body } => {
                // TODO: Create task, capture env, schedule
                let result_local = self.builder.alloc_temp(MirType::I32);
                // Lower body statements for analysis (captures)
                let _body_val = self.lower_block_stmts(body)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                });
                Ok(MirOperand::Local(result_local))
            }

            // Block call (e.g., spawn_raw { ... })
            ExprKind::BlockCall { name, body } => {
                let body_val = self.lower_block_stmts(body)?;
                let result_local = self.builder.alloc_temp(MirType::I32);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: name.clone() },
                    args: vec![body_val],
                });
                Ok(MirOperand::Local(result_local))
            }

            // Unsafe block
            ExprKind::Unsafe { body } => {
                // Same as block - safety checks happen in type checker
                self.lower_block_stmts(body)
            }

            // Comptime expression
            ExprKind::Comptime { body } => {
                // TODO: Should be evaluated at compile time
                // For now just lower the body
                self.lower_block_stmts(body)
            }

            // Select (channel multiplexing)
            ExprKind::Select { arms, .. } => {
                // TODO: Full select lowering with channel polling
                let result_local = self.builder.alloc_temp(MirType::I32);
                let merge_block = self.builder.create_block();
                let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

                // For now: just jump to first arm (placeholder)
                if let Some(&first) = arm_blocks.first() {
                    self.builder.terminate(MirTerminator::Goto { target: first });
                } else {
                    self.builder.terminate(MirTerminator::Goto { target: merge_block });
                }

                for (i, arm) in arms.iter().enumerate() {
                    self.builder.switch_to_block(arm_blocks[i]);
                    let arm_val = self.lower_expr(&arm.body)?;
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(arm_val),
                    });
                    self.builder.terminate(MirTerminator::Goto { target: merge_block });
                }

                self.builder.switch_to_block(merge_block);
                Ok(MirOperand::Local(result_local))
            }

            // Assert
            ExprKind::Assert { condition, message } => {
                let cond_op = self.lower_expr(condition)?;
                let ok_block = self.builder.create_block();
                let fail_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: cond_op,
                    then_block: ok_block,
                    else_block: fail_block,
                });

                self.builder.switch_to_block(fail_block);
                let mut args = Vec::new();
                if let Some(msg) = message {
                    args.push(self.lower_expr(msg)?);
                }
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: "assert_fail".to_string() },
                    args,
                });
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                Ok(MirOperand::Constant(MirConst::Bool(true)))
            }

            // Check (like assert but continues)
            ExprKind::Check { condition, message } => {
                let cond_op = self.lower_expr(condition)?;
                let ok_block = self.builder.create_block();
                let fail_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::Bool);

                self.builder.terminate(MirTerminator::Branch {
                    cond: cond_op,
                    then_block: ok_block,
                    else_block: fail_block,
                });

                self.builder.switch_to_block(fail_block);
                let mut args = Vec::new();
                if let Some(msg) = message {
                    args.push(self.lower_expr(msg)?);
                }
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: "check_fail".to_string() },
                    args,
                });
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(ok_block);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok(MirOperand::Local(result_local))
            }
        }
    }

    // =================================================================
    // Control flow lowering
    // =================================================================

    /// If expression lowering (spec L1).
    ///
    /// ```text
    /// [current]  cond → branch then_block / else_block
    /// [then]     result = then_val; goto merge
    /// [else]     result = else_val; goto merge
    /// [merge]    continue with result
    /// ```
    fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
    ) -> Result<MirOperand, LoweringError> {
        let cond_op = self.lower_expr(cond)?;

        let then_block = self.builder.create_block();
        let else_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        // Result local - both branches assign into this
        let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

        // Terminate current block with branch
        self.builder.terminate(MirTerminator::Branch {
            cond: cond_op,
            then_block,
            else_block,
        });

        // Then branch
        self.builder.switch_to_block(then_block);
        let then_val = self.lower_expr(then_branch)?;
        self.builder.push_stmt(MirStmt::Assign {
            dst: result_local,
            rvalue: MirRValue::Use(then_val),
        });
        self.builder.terminate(MirTerminator::Goto {
            target: merge_block,
        });

        // Else branch
        self.builder.switch_to_block(else_block);
        if let Some(else_expr) = else_branch {
            let else_val = self.lower_expr(else_expr)?;
            self.builder.push_stmt(MirStmt::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(else_val),
            });
        }
        // else: result stays uninitialized (void if-statement, no else)
        self.builder.terminate(MirTerminator::Goto {
            target: merge_block,
        });

        // Continue in merge block
        self.builder.switch_to_block(merge_block);

        Ok(MirOperand::Local(result_local))
    }

    /// Match expression lowering (spec L2).
    ///
    /// ```text
    /// [current]  tag = enum_tag(scrutinee); switch tag → arm blocks
    /// [arm_0]    bind payload; result = body; goto merge
    /// [arm_1]    bind payload; result = body; goto merge
    /// [merge]    continue with result
    /// ```
    fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<MirOperand, LoweringError> {
        let scrutinee_op = self.lower_expr(scrutinee)?;
        let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

        // Extract tag
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag {
                value: scrutinee_op.clone(),
            },
        });

        let merge_block = self.builder.create_block();

        // Create arm blocks (don't switch yet - we still need to terminate current block)
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

        let cases: Vec<(u64, BlockId)> = arm_blocks
            .iter()
            .enumerate()
            .map(|(i, &block)| (i as u64, block))
            .collect();

        // Terminate current block with switch
        self.builder.terminate(MirTerminator::Switch {
            value: MirOperand::Local(tag_local),
            cases,
            default: merge_block,
        });

        // Lower each arm in its own block
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            // TODO: Bind pattern variables (extract payload fields from scrutinee)
            let body_val = self.lower_expr(&arm.body)?;

            self.builder.push_stmt(MirStmt::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(body_val),
            });
            self.builder.terminate(MirTerminator::Goto {
                target: merge_block,
            });
        }

        self.builder.switch_to_block(merge_block);
        Ok(MirOperand::Local(result_local))
    }

    /// Block expression: lower each statement, last expression is the value.
    fn lower_block(&mut self, stmts: &[Stmt]) -> Result<MirOperand, LoweringError> {
        let mut last_val = MirOperand::Constant(MirConst::Int(0)); // void placeholder
        for (i, stmt) in stmts.iter().enumerate() {
            if i == stmts.len() - 1 {
                // Last statement: if it's an expression, use its value
                if let StmtKind::Expr(e) = &stmt.kind {
                    last_val = self.lower_expr(e)?;
                    continue;
                }
            }
            self.lower_stmt(stmt)?;
        }
        Ok(last_val)
    }

    // =================================================================
    // Statement lowering
    // =================================================================

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                Ok(())
            }

            StmtKind::Let { name, init, .. } => {
                let init_op = self.lower_expr(init)?;
                let var_ty = MirType::I32; // TODO: Parse type
                let local_id = self.builder.alloc_local(name.clone(), var_ty);
                self.locals.insert(name.clone(), local_id);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Use(init_op),
                });
                Ok(())
            }

            StmtKind::Const { name, init, .. } => {
                let init_op = self.lower_expr(init)?;
                let var_ty = MirType::I32; // TODO: Parse type
                let local_id = self.builder.alloc_local(name.clone(), var_ty);
                self.locals.insert(name.clone(), local_id);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Use(init_op),
                });
                Ok(())
            }

            StmtKind::Return(opt_expr) => {
                let value = if let Some(e) = opt_expr {
                    Some(self.lower_expr(e)?)
                } else {
                    None
                };
                self.builder.terminate(MirTerminator::Return { value });
                Ok(())
            }

            StmtKind::Assign { target, value } => {
                let val_op = self.lower_expr(value)?;
                // Target must be a local variable or field access
                match &target.kind {
                    ExprKind::Ident(name) => {
                        let local_id = self
                            .locals
                            .get(name)
                            .copied()
                            .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone()))?;
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: local_id,
                            rvalue: MirRValue::Use(val_op),
                        });
                    }
                    _ => {
                        // TODO: Handle field/index assignment
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

            // Let tuple destructuring
            StmtKind::LetTuple { names, init } => {
                let init_op = self.lower_expr(init)?;
                for (i, name) in names.iter().enumerate() {
                    let local_id = self.builder.alloc_local(name.clone(), MirType::I32);
                    self.locals.insert(name.clone(), local_id);
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

            // Const tuple destructuring
            StmtKind::ConstTuple { names, init } => {
                let init_op = self.lower_expr(init)?;
                for (i, name) in names.iter().enumerate() {
                    let local_id = self.builder.alloc_local(name.clone(), MirType::I32);
                    self.locals.insert(name.clone(), local_id);
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

            // While-let pattern loop
            StmtKind::WhileLet { pattern: _, expr, body } => {
                // Desugar: while expr matches pattern, execute body
                // TODO: Proper pattern matching
                let check_block = self.builder.create_block();
                let body_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Goto { target: check_block });

                self.builder.switch_to_block(check_block);
                let val = self.lower_expr(expr)?;
                // Check tag == 0 (Some/Ok)
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val },
                });
                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag),
                    then_block: exit_block,   // tag != 0 → None/Err → exit
                    else_block: body_block,   // tag == 0 → Some/Ok → body
                });

                self.builder.switch_to_block(body_block);
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

                // Register cleanup
                self.builder.push_stmt(MirStmt::EnsurePush { cleanup_block });

                // Lower body
                for s in body {
                    self.lower_stmt(s)?;
                }

                // Pop cleanup on normal path
                self.builder.push_stmt(MirStmt::EnsurePop);
                self.builder.terminate(MirTerminator::Goto { target: continue_block });

                // Cleanup block
                self.builder.switch_to_block(cleanup_block);
                if let Some((param_name, handler_body)) = else_handler {
                    let param_local = self.builder.alloc_local(param_name.clone(), MirType::I32);
                    self.locals.insert(param_name.clone(), param_local);
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
                // TODO: Mark as comptime-only
                for s in stmts {
                    self.lower_stmt(s)?;
                }
                Ok(())
            }
        }
    }

    // =================================================================
    // Loop lowering
    // =================================================================

    /// While loop (spec L5).
    ///
    /// ```text
    /// [current]  goto check
    /// [check]    cond → branch body / exit
    /// [body]     stmts; goto check
    /// [exit]     continue
    /// ```
    fn lower_while(&mut self, cond: &Expr, body: &[Stmt]) -> Result<(), LoweringError> {
        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        // Jump to check
        self.builder.terminate(MirTerminator::Goto {
            target: check_block,
        });

        // Check block: evaluate condition, branch
        self.builder.switch_to_block(check_block);
        let cond_op = self.lower_expr(cond)?;
        self.builder.terminate(MirTerminator::Branch {
            cond: cond_op,
            then_block: body_block,
            else_block: exit_block,
        });

        // Body block: push loop context, lower body, jump back to check
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

        // Continue after loop
        self.builder.switch_to_block(exit_block);
        Ok(())
    }

    /// For loop: desugar to iterator + while.
    ///
    /// ```text
    /// iter_local = lower(iter_expr)
    /// [check]    has_next = iter.next(); branch has_next / exit
    /// [body]     binding = current; stmts; goto check
    /// [exit]     continue
    /// ```
    fn lower_for(
        &mut self,
        label: Option<&str>,
        binding: &str,
        iter_expr: &Expr,
        body: &[Stmt],
    ) -> Result<(), LoweringError> {
        // Lower iterator expression
        let iter_op = self.lower_expr(iter_expr)?;
        let iter_local = self.builder.alloc_temp(MirType::I32); // TODO: Iterator type
        self.builder.push_stmt(MirStmt::Assign {
            dst: iter_local,
            rvalue: MirRValue::Use(iter_op),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto {
            target: check_block,
        });

        // Check: call next() on iterator, branch on result
        self.builder.switch_to_block(check_block);
        let next_result = self.builder.alloc_temp(MirType::I32); // TODO: Option type
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(next_result),
            func: FunctionRef {
                name: "next".to_string(),
            },
            args: vec![MirOperand::Local(iter_local)],
        });
        // TODO: Proper Option check - for now treat as bool-like
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(next_result),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body: bind loop variable, lower body
        self.builder.switch_to_block(body_block);
        let binding_local = self.builder.alloc_local(binding.to_string(), MirType::I32);
        self.locals.insert(binding.to_string(), binding_local);
        // TODO: Extract value from Option
        self.builder.push_stmt(MirStmt::Assign {
            dst: binding_local,
            rvalue: MirRValue::Use(MirOperand::Local(next_result)),
        });

        self.loop_stack.push(LoopContext {
            label: label.map(|s| s.to_string()),
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

    /// Infinite loop.
    ///
    /// ```text
    /// [loop]  body; goto loop
    /// [exit]  continue (reached via break)
    /// ```
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
            let val_op = self.lower_expr(val_expr)?;
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

        // Create unreachable block for any code after break
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

    /// Try expression lowering (spec L3).
    ///
    /// ```text
    /// [current]  result = call expr; tag = enum_tag(result)
    ///            branch tag==0 → ok_block, err_block
    /// [ok]       value = field(result, 0); goto merge
    /// [err]      err = field(result, 0); cleanup_return err
    /// [merge]    continue with value
    /// ```
    fn lower_try(&mut self, inner: &Expr) -> Result<MirOperand, LoweringError> {
        let result = self.lower_expr(inner)?;

        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag {
                value: result.clone(),
            },
        });

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block, // tag != 0 → Err
            else_block: ok_block,  // tag == 0 → Ok
        });

        // Err path: extract error, return it (with cleanup)
        self.builder.switch_to_block(err_block);
        let err_val = self.builder.alloc_temp(MirType::I32);
        self.builder.push_stmt(MirStmt::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
            },
        });
        // TODO: Run ensure cleanup chain before returning
        self.builder.terminate(MirTerminator::Return {
            value: Some(MirOperand::Local(err_val)),
        });

        // Ok path: extract value, continue
        self.builder.switch_to_block(ok_block);
        let ok_val = self.builder.alloc_temp(MirType::I32);
        self.builder.push_stmt(MirStmt::Assign {
            dst: ok_val,
            rvalue: MirRValue::Field {
                base: result,
                field_index: 0,
            },
        });
        self.builder.terminate(MirTerminator::Goto {
            target: merge_block,
        });

        self.builder.switch_to_block(merge_block);
        Ok(MirOperand::Local(ok_val))
    }

    /// Lower a list of statements as a block, returning the last expression value.
    fn lower_block_stmts(&mut self, stmts: &[Stmt]) -> Result<MirOperand, LoweringError> {
        let mut last_val = MirOperand::Constant(MirConst::Int(0));
        for (i, stmt) in stmts.iter().enumerate() {
            if i == stmts.len() - 1 {
                if let StmtKind::Expr(e) = &stmt.kind {
                    last_val = self.lower_expr(e)?;
                    continue;
                }
            }
            self.lower_stmt(stmt)?;
        }
        Ok(last_val)
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

/// Get method name for binary operator
fn binop_method_name(op: BinOp) -> String {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Gt => "gt",
        BinOp::Le => "le",
        BinOp::Ge => "ge",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::BitAnd => "bitand",
        BinOp::BitOr => "bitor",
        BinOp::BitXor => "bitxor",
        BinOp::Shl => "shl",
        BinOp::Shr => "shr",
    }
    .to_string()
}

/// Get method name for unary operator
fn unaryop_method_name(op: UnaryOp) -> String {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bitnot",
        UnaryOp::Ref => "ref",
        UnaryOp::Deref => "deref",
    }
    .to_string()
}

#[derive(Debug)]
pub enum LoweringError {
    UnresolvedVariable(String),
    UnresolvedGeneric(String),
    InvalidConstruct(String),
}
