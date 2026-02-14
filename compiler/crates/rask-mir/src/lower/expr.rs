// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use super::{
    binop_result_type, lower_binop, lower_unaryop, operator_method_to_binop,
    operator_method_to_unaryop, parse_type_str, LoweringError, MirLowerer, TypedOperand,
};
use crate::{
    operand::MirConst, BlockId, FunctionRef, MirOperand, MirRValue, MirStmt, MirTerminator,
    MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
    token::{FloatSuffix, IntSuffix},
};

impl MirLowerer {
    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        match &expr.kind {
            // Literals
            ExprKind::Int(val, suffix) => {
                let ty = match suffix {
                    Some(IntSuffix::I8) => MirType::I8,
                    Some(IntSuffix::I16) => MirType::I16,
                    Some(IntSuffix::I32) => MirType::I32,
                    Some(IntSuffix::I64) | None => MirType::I64,
                    Some(IntSuffix::U8) => MirType::U8,
                    Some(IntSuffix::U16) => MirType::U16,
                    Some(IntSuffix::U32) => MirType::U32,
                    Some(IntSuffix::U64) => MirType::U64,
                    Some(IntSuffix::I128 | IntSuffix::U128 | IntSuffix::Isize | IntSuffix::Usize) => MirType::I64,
                };
                Ok((MirOperand::Constant(MirConst::Int(*val)), ty))
            }
            ExprKind::Float(val, suffix) => {
                let ty = match suffix {
                    Some(FloatSuffix::F32) => MirType::F32,
                    Some(FloatSuffix::F64) | None => MirType::F64,
                };
                Ok((MirOperand::Constant(MirConst::Float(*val)), ty))
            }
            ExprKind::String(s) => Ok((
                MirOperand::Constant(MirConst::String(s.clone())),
                MirType::FatPtr, // string is a fat pointer (ptr + len)
            )),
            ExprKind::Char(c) => Ok((MirOperand::Constant(MirConst::Char(*c)), MirType::Char)),
            ExprKind::Bool(b) => Ok((MirOperand::Constant(MirConst::Bool(*b)), MirType::Bool)),

            // Variable reference
            ExprKind::Ident(name) => self
                .locals
                .get(name)
                .cloned()
                .map(|(id, ty)| (MirOperand::Local(id), ty))
                .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone())),

            // Binary operations (only &&/|| survive desugar)
            ExprKind::Binary { op, left, right } => {
                let (left_op, _) = self.lower_expr(left)?;
                let (right_op, _) = self.lower_expr(right)?;
                let result_local = self.builder.alloc_temp(MirType::Bool);

                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::BinaryOp {
                        op: lower_binop(*op),
                        left: left_op,
                        right: right_op,
                    },
                });

                Ok((MirOperand::Local(result_local), MirType::Bool))
            }

            // Unary operations (only !, &, * survive desugar)
            ExprKind::Unary { op, operand } => {
                let (operand_op, operand_ty) = self.lower_expr(operand)?;
                let (result_ty, rvalue) = match op {
                    UnaryOp::Ref => {
                        let rv = match operand_op {
                            MirOperand::Local(id) => MirRValue::Ref(id),
                            _ => MirRValue::Use(operand_op),
                        };
                        (MirType::Ptr, rv)
                    }
                    UnaryOp::Deref => (operand_ty.clone(), MirRValue::Deref(operand_op)),
                    UnaryOp::Not => (MirType::Bool, MirRValue::UnaryOp {
                        op: lower_unaryop(*op),
                        operand: operand_op,
                    }),
                    _ => (operand_ty.clone(), MirRValue::UnaryOp {
                        op: lower_unaryop(*op),
                        operand: operand_op,
                    }),
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue,
                });

                Ok((MirOperand::Local(result_local), result_ty))
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

                let mut arg_operands = Vec::new();
                for a in args {
                    let (op, _) = self.lower_expr(a)?;
                    arg_operands.push(op);
                }

                let ret_ty = self
                    .func_sigs
                    .get(&func_name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I32);

                let result_local = self.builder.alloc_temp(ret_ty.clone());

                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: func_name },
                    args: arg_operands,
                });

                Ok((MirOperand::Local(result_local), ret_ty))
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

            // Method call — operator methods from desugar become BinaryOp/UnaryOp
            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => {
                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Detect binary operator methods (desugared from a + b → a.add(b))
                if let Some(mir_binop) = operator_method_to_binop(method) {
                    if args.len() == 1 {
                        let (rhs, _) = self.lower_expr(&args[0])?;
                        let result_ty = binop_result_type(&mir_binop, &obj_ty);
                        let result_local = self.builder.alloc_temp(result_ty.clone());
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: result_local,
                            rvalue: MirRValue::BinaryOp {
                                op: mir_binop,
                                left: obj_op,
                                right: rhs,
                            },
                        });
                        return Ok((MirOperand::Local(result_local), result_ty));
                    }
                }

                // Detect unary operator methods (desugared from -a → a.neg())
                if let Some(mir_unop) = operator_method_to_unaryop(method) {
                    if args.is_empty() {
                        let result_local = self.builder.alloc_temp(obj_ty.clone());
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: result_local,
                            rvalue: MirRValue::UnaryOp {
                                op: mir_unop,
                                operand: obj_op,
                            },
                        });
                        return Ok((MirOperand::Local(result_local), obj_ty));
                    }
                }

                // Regular method call
                let mut all_args = vec![obj_op];
                for arg in args {
                    let (op, _) = self.lower_expr(arg)?;
                    all_args.push(op);
                }
                let ret_ty = self
                    .func_sigs
                    .get(method)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I32);
                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: method.clone(),
                    },
                    args: all_args,
                });
                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // Field access
            ExprKind::Field { object, field: _ } => {
                let (obj_op, _obj_ty) = self.lower_expr(object)?;
                // TODO: Lookup field type from layout
                let result_ty = MirType::I32;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index: 0, // TODO: Lookup from layout
                    },
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Index access
            ExprKind::Index { object, index } => {
                let (obj_op, _) = self.lower_expr(object)?;
                let (idx_op, _) = self.lower_expr(index)?;
                // TODO: Infer element type from collection type
                let result_ty = MirType::I32;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: "index".to_string(),
                    },
                    args: vec![obj_op, idx_op],
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Array literal
            ExprKind::Array(elems) => {
                let mut elem_ty = MirType::I32;
                for (i, elem) in elems.iter().enumerate() {
                    let (elem_op, ty) = self.lower_expr(elem)?;
                    if i == 0 {
                        elem_ty = ty;
                    }
                    self.builder.push_stmt(MirStmt::Store {
                        addr: crate::operand::LocalId(0), // Placeholder — filled in after alloc
                        offset: i as u32 * 4, // TODO: Proper element size
                        value: elem_op,
                    });
                }
                let array_ty = MirType::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len() as u32,
                };
                let result_local = self.builder.alloc_temp(array_ty.clone());
                // TODO: Fix stores to use result_local
                Ok((MirOperand::Local(result_local), array_ty))
            }

            // Tuple literal
            ExprKind::Tuple(elems) => {
                let result_local = self.builder.alloc_temp(MirType::Ptr);
                for (i, elem) in elems.iter().enumerate() {
                    let (elem_op, _) = self.lower_expr(elem)?;
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * 8, // TODO: Proper field offsets
                        value: elem_op,
                    });
                }
                Ok((MirOperand::Local(result_local), MirType::Ptr))
            }

            // Struct literal
            ExprKind::StructLit { fields, .. } => {
                // TODO: Look up struct layout for proper type
                let result_local = self.builder.alloc_temp(MirType::Ptr);
                for (i, field) in fields.iter().enumerate() {
                    let (val_op, _) = self.lower_expr(&field.value)?;
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * 4, // TODO: Use layout
                        value: val_op,
                    });
                }
                Ok((MirOperand::Local(result_local), MirType::Ptr))
            }

            // If-let (if expr is Pattern { then } else { else })
            ExprKind::IfLet {
                expr,
                pattern: _,
                then_branch,
                else_branch,
            } => {
                // TODO: Proper pattern matching with tag check
                let (cond_op, _) = self.lower_expr(expr)?;
                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: cond_op,
                    then_block,
                    else_block,
                });

                self.builder.switch_to_block(then_block);
                let (then_val, then_ty) = self.lower_expr(then_branch)?;
                let result_local = self.builder.alloc_temp(then_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(then_val),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(else_block);
                if let Some(else_expr) = else_branch {
                    let (else_val, _) = self.lower_expr(else_expr)?;
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(else_val),
                    });
                }
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), then_ty))
            }

            // Guard pattern (const v = expr is Pattern else { diverge })
            ExprKind::GuardPattern {
                expr,
                pattern: _,
                else_branch,
            } => {
                // TODO: Proper pattern match + bind
                let (val, val_ty) = self.lower_expr(expr)?;
                let ok_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: val.clone(),
                    then_block: ok_block,
                    else_block,
                });

                self.builder.switch_to_block(else_block);
                self.lower_expr(else_branch)?;
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((val, val_ty))
            }

            // Pattern test (expr is Pattern) — evaluates to bool
            ExprKind::IsPattern { expr: inner, pattern: _ } => {
                // TODO: Proper pattern tag check
                let (_val, _ty) = self.lower_expr(inner)?;
                Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
            }

            // Try expression (spec L3)
            ExprKind::Try(inner) => self.lower_try(inner),

            // Unwrap (postfix !) - panic on None/Err
            ExprKind::Unwrap(inner) => {
                let (val, _inner_ty) = self.lower_expr(inner)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block,
                    else_block: ok_block,
                });

                self.builder.switch_to_block(panic_block);
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: "panic_unwrap".to_string() },
                    args: vec![],
                });
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                // TODO: Infer payload type from inner type
                let payload_ty = MirType::I32;
                let result_local = self.builder.alloc_temp(payload_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: val, field_index: 0 },
                });
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let (val, _) = self.lower_expr(value)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                });

                self.builder.switch_to_block(some_block);
                // TODO: Infer payload type
                let payload_ty = MirType::I32;
                let result_local = self.builder.alloc_temp(payload_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: val, field_index: 0 },
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(none_block);
                let (default_val, _) = self.lower_expr(default)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(default_val),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Range expression
            ExprKind::Range { start, end, inclusive } => {
                let result_ty = MirType::Ptr; // Range is an opaque struct
                let result_local = self.builder.alloc_temp(result_ty.clone());
                let mut args = Vec::new();
                if let Some(s) = start {
                    let (op, _) = self.lower_expr(s)?;
                    args.push(op);
                }
                if let Some(e) = end {
                    let (op, _) = self.lower_expr(e)?;
                    args.push(op);
                }
                let func_name = if *inclusive { "range_inclusive" } else { "range" };
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: func_name.to_string() },
                    args,
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Array repeat ([value; count])
            ExprKind::ArrayRepeat { value, count } => {
                let (val, elem_ty) = self.lower_expr(value)?;
                let (cnt, _) = self.lower_expr(count)?;
                let result_ty = MirType::Ptr; // Dynamic array
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: "array_repeat".to_string() },
                    args: vec![val, cnt],
                });
                let _ = elem_ty; // TODO: Use for proper array type
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Optional chaining (a?.b)
            ExprKind::OptionalField { object, field: _ } => {
                let (obj, _) = self.lower_expr(object)?;
                let tag_local = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag_local,
                    rvalue: MirRValue::EnumTag { value: obj.clone() },
                });

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                // TODO: Infer inner field type
                let result_ty = MirType::I32;
                let result_local = self.builder.alloc_temp(result_ty.clone());

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                });

                self.builder.switch_to_block(some_block);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field { base: obj, field_index: 0 },
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(none_block);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Closure
            ExprKind::Closure { params, body, .. } => {
                let result_local = self.builder.alloc_temp(MirType::Ptr);

                let saved_locals = self.locals.clone();
                for param in params {
                    let param_ty = MirType::I32; // TODO: Closure param types
                    let param_local = self.builder.alloc_temp(param_ty.clone());
                    self.locals.insert(param.name.clone(), (param_local, param_ty));
                }
                let _body_val = self.lower_expr(body)?;
                self.locals = saved_locals;

                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                });
                Ok((MirOperand::Local(result_local), MirType::Ptr))
            }

            // Cast
            ExprKind::Cast { expr, ty } => {
                let (val, _) = self.lower_expr(expr)?;
                let target_ty = parse_type_str(ty);
                let result_local = self.builder.alloc_temp(target_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Cast {
                        value: val,
                        target_ty: target_ty.clone(),
                    },
                });
                Ok((MirOperand::Local(result_local), target_ty))
            }

            // Using block
            ExprKind::UsingBlock { body, .. } => {
                self.lower_block(body)
            }

            // With-as binding
            ExprKind::WithAs { bindings, body } => {
                for (bind_expr, name) in bindings {
                    let (val, val_ty) = self.lower_expr(bind_expr)?;
                    let local = self.builder.alloc_local(name.clone(), val_ty.clone());
                    self.locals.insert(name.clone(), (local, val_ty));
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: local,
                        rvalue: MirRValue::Use(val),
                    });
                }
                self.lower_block(body)
            }

            // Spawn
            ExprKind::Spawn { body } => {
                let result_local = self.builder.alloc_temp(MirType::Ptr);
                let _body_val = self.lower_block(body)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                });
                Ok((MirOperand::Local(result_local), MirType::Ptr))
            }

            // Block call (e.g., spawn_raw { ... })
            ExprKind::BlockCall { name, body } => {
                let (body_val, _) = self.lower_block(body)?;
                let ret_ty = self
                    .func_sigs
                    .get(name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I32);
                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: name.clone() },
                    args: vec![body_val],
                });
                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // Unsafe block
            ExprKind::Unsafe { body } => {
                self.lower_block(body)
            }

            // Comptime expression
            ExprKind::Comptime { body } => {
                self.lower_block(body)
            }

            // Select (channel multiplexing)
            ExprKind::Select { arms, .. } => {
                let merge_block = self.builder.create_block();
                let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

                if let Some(&first) = arm_blocks.first() {
                    self.builder.terminate(MirTerminator::Goto { target: first });
                } else {
                    self.builder.terminate(MirTerminator::Goto { target: merge_block });
                }

                let mut result_ty = MirType::Void;
                let result_local = self.builder.alloc_temp(MirType::I32);
                for (i, arm) in arms.iter().enumerate() {
                    self.builder.switch_to_block(arm_blocks[i]);
                    let (arm_val, arm_ty) = self.lower_expr(&arm.body)?;
                    if i == 0 {
                        result_ty = arm_ty;
                    }
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(arm_val),
                    });
                    self.builder.terminate(MirTerminator::Goto { target: merge_block });
                }

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Assert
            ExprKind::Assert { condition, message } => {
                let (cond_op, _) = self.lower_expr(condition)?;
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
                    let (msg_op, _) = self.lower_expr(msg)?;
                    args.push(msg_op);
                }
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef { name: "assert_fail".to_string() },
                    args,
                });
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
            }

            // Check (like assert but continues)
            ExprKind::Check { condition, message } => {
                let (cond_op, _) = self.lower_expr(condition)?;
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
                    let (msg_op, _) = self.lower_expr(msg)?;
                    args.push(msg_op);
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
                Ok((MirOperand::Local(result_local), MirType::Bool))
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
    ) -> Result<TypedOperand, LoweringError> {
        let (cond_op, _) = self.lower_expr(cond)?;

        let then_block = self.builder.create_block();
        let else_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Branch {
            cond: cond_op,
            then_block,
            else_block,
        });

        // Then branch
        self.builder.switch_to_block(then_block);
        let (then_val, then_ty) = self.lower_expr(then_branch)?;
        let result_local = self.builder.alloc_temp(then_ty.clone());
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
            let (else_val, _) = self.lower_expr(else_expr)?;
            self.builder.push_stmt(MirStmt::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(else_val),
            });
        }
        self.builder.terminate(MirTerminator::Goto {
            target: merge_block,
        });

        self.builder.switch_to_block(merge_block);

        Ok((MirOperand::Local(result_local), then_ty))
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
    ) -> Result<TypedOperand, LoweringError> {
        let (scrutinee_op, _) = self.lower_expr(scrutinee)?;

        // Extract tag
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag {
                value: scrutinee_op.clone(),
            },
        });

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

        let cases: Vec<(u64, BlockId)> = arm_blocks
            .iter()
            .enumerate()
            .map(|(i, &block)| (i as u64, block))
            .collect();

        self.builder.terminate(MirTerminator::Switch {
            value: MirOperand::Local(tag_local),
            cases,
            default: merge_block,
        });

        // Lower each arm — infer result type from first arm
        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I32); // Placeholder, updated below
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            // TODO: Bind pattern variables (extract payload fields from scrutinee)
            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 {
                result_ty = arm_ty;
            }

            self.builder.push_stmt(MirStmt::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(body_val),
            });
            self.builder.terminate(MirTerminator::Goto {
                target: merge_block,
            });
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Block expression: lower each statement, last expression is the value.
    pub(super) fn lower_block(&mut self, stmts: &[Stmt]) -> Result<TypedOperand, LoweringError> {
        let mut last_val = MirOperand::Constant(MirConst::Int(0));
        let mut last_ty = MirType::Void;
        for (i, stmt) in stmts.iter().enumerate() {
            if i == stmts.len() - 1 {
                if let StmtKind::Expr(e) = &stmt.kind {
                    let (val, ty) = self.lower_expr(e)?;
                    last_val = val;
                    last_ty = ty;
                    continue;
                }
            }
            self.lower_stmt(stmt)?;
        }
        Ok((last_val, last_ty))
    }

    /// Try expression lowering (spec L3).
    fn lower_try(&mut self, inner: &Expr) -> Result<TypedOperand, LoweringError> {
        let (result, _) = self.lower_expr(inner)?;

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
            then_block: err_block,
            else_block: ok_block,
        });

        // Err path
        self.builder.switch_to_block(err_block);
        let err_val = self.builder.alloc_temp(MirType::I32);
        self.builder.push_stmt(MirStmt::Assign {
            dst: err_val,
            rvalue: MirRValue::Field {
                base: result.clone(),
                field_index: 0,
            },
        });
        self.builder.terminate(MirTerminator::Return {
            value: Some(MirOperand::Local(err_val)),
        });

        // Ok path
        self.builder.switch_to_block(ok_block);
        // TODO: Infer Ok payload type
        let ok_ty = MirType::I32;
        let ok_val = self.builder.alloc_temp(ok_ty.clone());
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
        Ok((MirOperand::Local(ok_val), ok_ty))
    }
}
