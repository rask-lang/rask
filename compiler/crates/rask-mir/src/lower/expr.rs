// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use super::{
    binop_result_type, is_type_constructor_name, is_variant_name, lower_binop, lower_unaryop,
    mir_type_size, operator_method_to_binop, operator_method_to_unaryop, LoweringError,
    MirLowerer, TypedOperand,
};
use crate::{
    operand::MirConst, stmt::ClosureCapture, types::{EnumLayoutId, StructLayoutId}, BlockBuilder,
    BlockId, FunctionRef, LocalId, MirOperand, MirRValue, MirStmt, MirTerminator, MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
    token::{FloatSuffix, IntSuffix},
};
use rask_mono::StructLayout;

impl<'a> MirLowerer<'a> {
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
                MirType::String,
            )),
            ExprKind::Char(c) => Ok((MirOperand::Constant(MirConst::Char(*c)), MirType::Char)),
            ExprKind::Bool(b) => Ok((MirOperand::Constant(MirConst::Bool(*b)), MirType::Bool)),

            // Variable reference (or bare enum variant like None)
            ExprKind::Ident(name) => {
                if let Some((id, ty)) = self.locals.get(name).cloned() {
                    Ok((MirOperand::Local(id), ty))
                } else if name == "None" {
                    // Fieldless variant → tag-only value (tag 1 for None)
                    Ok((MirOperand::Constant(MirConst::Int(1)), MirType::Ptr))
                } else {
                    Err(LoweringError::UnresolvedVariable(name.clone()))
                }
            }

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

            // Function call — direct or through closure
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
                    let (op, _) = self.lower_expr(&a.expr)?;
                    arg_operands.push(op);
                }

                // If the callee is a known closure variable, emit ClosureCall
                if self.closure_locals.contains(&func_name) {
                    if let Some((closure_local, _)) = self.locals.get(&func_name).cloned() {
                        let ret_ty = self.func_sigs
                            .get(&func_name)
                            .map(|s| s.ret_ty.clone())
                            .unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::ClosureCall {
                            dst: Some(result_local),
                            closure: closure_local,
                            args: arg_operands,
                        });
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                }

                // Built-in variant constructors: Ok(v), Err(v), Some(v)
                match func_name.as_str() {
                    "Ok" | "Some" | "Err" => {
                        let tag = self.variant_tag(&func_name);
                        let result_local = self.builder.alloc_temp(MirType::Ptr);
                        self.builder.push_stmt(MirStmt::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(tag)),
                        });
                        if let Some(payload) = arg_operands.first() {
                            self.builder.push_stmt(MirStmt::Store {
                                addr: result_local,
                                offset: 8,
                                value: payload.clone(),
                            });
                        }
                        return Ok((MirOperand::Local(result_local), MirType::Ptr));
                    }
                    _ => {}
                }

                let ret_ty = self
                    .func_sigs
                    .get(&func_name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);

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

            // Method call — operator methods from desugar become BinaryOp/UnaryOp,
            // type constructors become enum construction or static calls.
            ExprKind::MethodCall {
                object,
                method,
                args,
                type_args,
            } => {
                // When the object is a type name (not a local variable), intercept
                // before lowering it as a value expression.
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        // Enum variant constructor: Shape.Circle(r)
                        // Extract layout data before mutable borrows in lower_expr
                        let enum_variant = self.ctx.find_enum(name).and_then(|(idx, layout)| {
                            let variant = layout.variants.iter().find(|v| v.name == *method)?;
                            Some((
                                idx,
                                layout.tag_offset,
                                variant.tag,
                                variant.payload_offset,
                                variant.fields.clone(),
                            ))
                        });

                        if let Some((idx, tag_offset, tag_val, payload_offset, fields)) =
                            enum_variant
                        {
                            let enum_ty = MirType::Enum(EnumLayoutId(idx));
                            let result_local = self.builder.alloc_temp(enum_ty.clone());

                            // Store discriminant tag
                            self.builder.push_stmt(MirStmt::Store {
                                addr: result_local,
                                offset: tag_offset,
                                value: MirOperand::Constant(MirConst::Int(tag_val as i64)),
                            });

                            // Store payload fields
                            for (i, arg) in args.iter().enumerate() {
                                let (val, _) = self.lower_expr(&arg.expr)?;
                                let offset = if i < fields.len() {
                                    payload_offset + fields[i].offset
                                } else {
                                    payload_offset + (i as u32 * 8)
                                };
                                self.builder.push_stmt(MirStmt::Store {
                                    addr: result_local,
                                    offset,
                                    value: val,
                                });
                            }

                            return Ok((MirOperand::Local(result_local), enum_ty));
                        }

                        // .variants() on enum types: build a Vec of tag values
                        if method == "variants" && args.is_empty() {
                            if let Some((_idx, layout)) = self.ctx.find_enum(name) {
                                // Create a new Vec
                                let vec_local = self.builder.alloc_temp(MirType::I64);
                                self.builder.push_stmt(MirStmt::Call {
                                    dst: Some(vec_local),
                                    func: FunctionRef { name: "Vec_new".to_string() },
                                    args: vec![],
                                });
                                // Push each variant's tag value
                                for variant in &layout.variants {
                                    self.builder.push_stmt(MirStmt::Call {
                                        dst: None,
                                        func: FunctionRef { name: "push".to_string() },
                                        args: vec![
                                            MirOperand::Local(vec_local),
                                            MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                        ],
                                    });
                                }
                                return Ok((MirOperand::Local(vec_local), MirType::I64));
                            }
                        }

                        // json.encode — expand struct serialization at MIR level
                        if name == "json" && method == "encode" && args.len() == 1 {
                            let (arg_op, arg_ty) = self.lower_expr(&args[0].expr)?;
                            if let MirType::Struct(StructLayoutId(id)) = &arg_ty {
                                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                    return self.lower_json_encode_struct(arg_op, layout.clone());
                                }
                            }
                            // Non-struct: string or integer
                            let helper = if matches!(arg_ty, MirType::String) {
                                "json_encode_string"
                            } else {
                                "json_encode_i64"
                            };
                            let result_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::Call {
                                dst: Some(result_local),
                                func: FunctionRef { name: helper.to_string() },
                                args: vec![arg_op],
                            });
                            return Ok((MirOperand::Local(result_local), MirType::I64));
                        }

                        // json.decode<T> — expand struct deserialization at MIR level
                        if name == "json" && method == "decode" && args.len() == 1 {
                            let (str_op, _) = self.lower_expr(&args[0].expr)?;
                            if let Some(ta) = type_args {
                                if let Some(target_name) = ta.first() {
                                    if let Some((_, layout)) = self.ctx.find_struct(target_name) {
                                        return self.lower_json_decode_struct(str_op, layout.clone());
                                    }
                                }
                            }
                            // Fallback: opaque decode
                            let result_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::Call {
                                dst: Some(result_local),
                                func: FunctionRef { name: "json_decode".to_string() },
                                args: vec![str_op],
                            });
                            return Ok((MirOperand::Local(result_local), MirType::I64));
                        }

                        // Static method on a type: Vec.new(), string.new()
                        let is_known_type = self.ctx.find_struct(name).is_some()
                            || self.ctx.find_enum(name).is_some()
                            || is_type_constructor_name(name);

                        if is_known_type {
                            let func_name = format!("{}_{}", name, method);
                            let mut arg_operands = Vec::new();
                            for arg in args {
                                let (op, _) = self.lower_expr(&arg.expr)?;
                                arg_operands.push(op);
                            }
                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or(MirType::I64);
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::Call {
                                dst: Some(result_local),
                                func: FunctionRef { name: func_name },
                                args: arg_operands,
                            });
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Detect binary operator methods (desugared from a + b → a.add(b))
                if let Some(mir_binop) = operator_method_to_binop(method) {
                    if args.len() == 1 {
                        let (rhs, _) = self.lower_expr(&args[0].expr)?;
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

                // concat(): string concatenation from interpolation desugaring
                if method == "concat" && args.len() == 1 && matches!(obj_ty, MirType::String) {
                    let (arg_op, _) = self.lower_expr(&args[0].expr)?;
                    let result_local = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::Call {
                        dst: Some(result_local),
                        func: FunctionRef { name: "concat".to_string() },
                        args: vec![obj_op, arg_op],
                    });
                    return Ok((MirOperand::Local(result_local), MirType::String));
                }

                // to_string(): route to type-specific runtime function
                if method == "to_string" && args.is_empty() {
                    let func_name = match &obj_ty {
                        MirType::String => {
                            // String.to_string() is identity
                            return Ok((obj_op, MirType::String));
                        }
                        MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8
                        | MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8 => "i64_to_string",
                        MirType::Bool => "bool_to_string",
                        _ => "i64_to_string", // fallback
                    };
                    let result_local = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::Call {
                        dst: Some(result_local),
                        func: FunctionRef { name: func_name.to_string() },
                        args: vec![obj_op],
                    });
                    return Ok((MirOperand::Local(result_local), MirType::String));
                }

                // Regular method call
                let mut all_args = vec![obj_op];
                for arg in args {
                    let (op, _) = self.lower_expr(&arg.expr)?;
                    all_args.push(op);
                }
                let ret_ty = self
                    .func_sigs
                    .get(method)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);
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
            ExprKind::Field { object, field } => {
                // Enum variant access: Color.Red (no parens, fieldless variant)
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        if let Some((idx, layout)) = self.ctx.find_enum(name) {
                            if let Some(variant) = layout.variants.iter().find(|v| v.name == *field) {
                                let enum_ty = MirType::Enum(EnumLayoutId(idx));
                                let result_local = self.builder.alloc_temp(enum_ty.clone());
                                // Store discriminant tag
                                self.builder.push_stmt(MirStmt::Store {
                                    addr: result_local,
                                    offset: layout.tag_offset,
                                    value: MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                });
                                return Ok((MirOperand::Local(result_local), enum_ty));
                            }
                        }
                        // Unknown enum type (built-in Error, etc.) — produce a
                        // tag-only stub so codegen can proceed.
                        if is_type_constructor_name(name) {
                            let tag = self.variant_tag(field);
                            return Ok((MirOperand::Constant(MirConst::Int(tag)), MirType::Ptr));
                        }
                    }
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Resolve field index and type from struct layout
                let (field_index, result_ty) = if let MirType::Struct(StructLayoutId(id)) = &obj_ty {
                    if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                        if let Some((idx, fl)) = layout.fields.iter().enumerate()
                            .find(|(_, f)| f.name == *field)
                        {
                            (idx as u32, self.ctx.resolve_type_str(&format!("{}", fl.ty)))
                        } else {
                            (0, MirType::I32) // field not found — fallback
                        }
                    } else {
                        (0, MirType::I32)
                    }
                } else {
                    (0, MirType::I32) // non-struct field access — fallback
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index,
                    },
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Index access
            ExprKind::Index { object, index } => {
                let (obj_op, obj_ty) = self.lower_expr(object)?;
                let (idx_op, _) = self.lower_expr(index)?;
                let result_ty = match &obj_ty {
                    MirType::Array { elem, .. } => *elem.clone(),
                    _ => MirType::I32, // fallback for non-array indexing
                };
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
                // Lower elements first to determine the element type
                let mut lowered = Vec::new();
                let mut elem_ty = MirType::I32;
                for (i, elem) in elems.iter().enumerate() {
                    let (elem_op, ty) = self.lower_expr(elem)?;
                    if i == 0 {
                        elem_ty = ty;
                    }
                    lowered.push(elem_op);
                }
                let elem_size = mir_type_size(&elem_ty);
                let array_ty = MirType::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len() as u32,
                };
                let result_local = self.builder.alloc_temp(array_ty.clone());
                for (i, elem_op) in lowered.into_iter().enumerate() {
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset: i as u32 * elem_size,
                        value: elem_op,
                    });
                }
                Ok((MirOperand::Local(result_local), array_ty))
            }

            // Tuple literal
            ExprKind::Tuple(elems) => {
                let result_local = self.builder.alloc_temp(MirType::Ptr);
                let mut offset = 0u32;
                for elem in elems.iter() {
                    let (elem_op, elem_ty) = self.lower_expr(elem)?;
                    let elem_size = mir_type_size(&elem_ty);
                    let elem_align = elem_size.max(1);
                    // Align offset for this element
                    offset = (offset + elem_align - 1) & !(elem_align - 1);
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset,
                        value: elem_op,
                    });
                    offset += elem_size;
                }
                Ok((MirOperand::Local(result_local), MirType::Ptr))
            }

            // Struct literal
            ExprKind::StructLit { name, fields, .. } => {
                let (result_ty, layout) = if let Some((idx, sl)) = self.ctx.find_struct(name) {
                    (MirType::Struct(StructLayoutId(idx)), Some(sl))
                } else {
                    (MirType::Ptr, None)
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                for field in fields.iter() {
                    let (val_op, _) = self.lower_expr(&field.value)?;
                    // Look up field offset from layout
                    let offset = layout
                        .and_then(|sl| sl.fields.iter().find(|f| f.name == field.name))
                        .map(|f| f.offset)
                        .unwrap_or(0);
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset,
                        value: val_op,
                    });
                }
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // If-let (if expr is Pattern { then } else { else })
            ExprKind::IfLet {
                expr,
                pattern,
                then_branch,
                else_branch,
            } => {
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
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                });

                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(matches),
                    then_block,
                    else_block,
                });

                // Then block: bind payload, evaluate body
                self.builder.switch_to_block(then_block);
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload(pattern, val, payload_ty);
                let (then_val, then_ty) = self.lower_expr(then_branch)?;
                let result_local = self.builder.alloc_temp(then_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(then_val),
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                // Else block: evaluate else branch or default to zero-value
                self.builder.switch_to_block(else_block);
                if let Some(else_expr) = else_branch {
                    let (else_val, _) = self.lower_expr(else_expr)?;
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(else_val),
                    });
                } else {
                    // No else branch — initialize to default zero value
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                    });
                }
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), then_ty))
            }

            // Guard pattern (const v = expr is Pattern else { diverge })
            ExprKind::GuardPattern {
                expr,
                pattern,
                else_branch,
            } => {
                let (val, _) = self.lower_expr(expr)?;
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val.clone() },
                });

                let expected = self.pattern_tag(pattern);
                let matches = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                });

                let ok_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: ok_block,
                    else_block,
                });

                // Else branch diverges (return, panic, etc.)
                self.builder.switch_to_block(else_block);
                self.lower_expr(else_branch)?;
                self.builder.terminate(MirTerminator::Unreachable);

                // Ok block: bind payload and continue
                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload(pattern, val.clone(), payload_ty.clone());
                // Extract the payload value for the result
                let payload = self.builder.alloc_temp(payload_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: payload,
                    rvalue: MirRValue::Field { base: val, field_index: 0 },
                });
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(payload), payload_ty))
            }

            // Pattern test (expr is Pattern) — evaluates to bool
            ExprKind::IsPattern { expr: inner, pattern } => {
                let (val, _ty) = self.lower_expr(inner)?;
                let tag = self.builder.alloc_temp(MirType::U8);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: tag,
                    rvalue: MirRValue::EnumTag { value: val },
                });
                let expected = self.pattern_tag(pattern);
                let result = self.builder.alloc_temp(MirType::Bool);
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                });
                Ok((MirOperand::Local(result), MirType::Bool))
            }

            // Try expression (spec L3)
            ExprKind::Try(inner) => self.lower_try(inner),

            // Unwrap (postfix !) - panic on None/Err
            ExprKind::Unwrap { expr: inner, message: _ } => {
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
                // Infer payload type from the Option/Result being unwrapped
                let payload_ty = self.extract_payload_type(inner)
                    .unwrap_or(MirType::I64);
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
                // Infer payload type from the Option being coalesced
                let payload_ty = self.extract_payload_type(value)
                    .unwrap_or(MirType::I64);
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
                // Dynamic-length array — use Ptr since Array { elem, len }
                // requires compile-time length. Element type is preserved
                // in elem_ty for future DynamicArray MirType variant.
                let result_ty = MirType::Ptr;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                // Pass elem size to array_repeat for proper allocation
                let elem_size = self.elem_size_for_type(&elem_ty);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: "array_repeat".to_string() },
                    args: vec![val, cnt, MirOperand::Constant(MirConst::Int(elem_size))],
                });
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
                // Infer field type from the optional value
                let result_ty = self.extract_payload_type(object)
                    .unwrap_or(MirType::I64);
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

            // Closure — synthesize a separate MIR function and emit ClosureCreate
            ExprKind::Closure { params, ret_ty, body } => {
                self.lower_closure(params, ret_ty.as_deref(), body)
            }

            // Cast
            ExprKind::Cast { expr, ty } => {
                let (val, _) = self.lower_expr(expr)?;
                let target_ty = self.ctx.resolve_type_str(ty);
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

            // Using block — emit runtime init/shutdown for Multitasking
            ExprKind::UsingBlock { name, body, .. } => {
                if name == "Multitasking" || name == "multitasking" {
                    // rask_runtime_init(0) — 0 = auto-detect worker count
                    self.builder.push_stmt(MirStmt::Call {
                        dst: None,
                        func: FunctionRef { name: "rask_runtime_init".to_string() },
                        args: vec![MirOperand::Constant(crate::operand::MirConst::Int(0))],
                    });
                    let result = self.lower_block(body);
                    // rask_runtime_shutdown()
                    self.builder.push_stmt(MirStmt::Call {
                        dst: None,
                        func: FunctionRef { name: "rask_runtime_shutdown".to_string() },
                        args: vec![],
                    });
                    result
                } else {
                    self.lower_block(body)
                }
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

            // Spawn — synthesize a closure function and call rask_closure_spawn
            ExprKind::Spawn { body } => {
                self.lower_spawn(body)
            }

            // Block call (e.g., spawn_raw { ... })
            ExprKind::BlockCall { name, body } => {
                let (body_val, _) = self.lower_block(body)?;
                let ret_ty = self
                    .func_sigs
                    .get(name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);
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
    /// Handles both enum matches (extract tag, switch on discriminant)
    /// and value matches (switch on scrutinee directly).
    fn lower_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[rask_ast::expr::MatchArm],
    ) -> Result<TypedOperand, LoweringError> {
        use rask_ast::expr::Pattern;

        let (scrutinee_op, scrutinee_ty) = self.lower_expr(scrutinee)?;

        let is_enum = matches!(scrutinee_ty, MirType::Enum(_));

        // Detect Result/Option via raw type info from type checker
        let is_result_or_option = if !is_enum {
            self.ctx.lookup_raw_type(scrutinee.id).map_or(false, |ty| {
                matches!(ty, rask_types::Type::Result { .. } | rask_types::Type::Option(_))
            })
        } else {
            false
        };

        // Pattern-based detection: if any arm uses a known variant pattern,
        // treat the match as tagged dispatch even without type info.
        let patterns_imply_enum = if !is_enum && !is_result_or_option {
            arms.iter().any(|arm| match &arm.pattern {
                Pattern::Constructor { name, .. } => is_variant_name(name),
                Pattern::Ident(name) => {
                    self.resolve_pattern_tag(name).is_some()
                        || matches!(name.as_str(), "Ok" | "Err" | "Some" | "None")
                }
                _ => false,
            })
        } else {
            false
        };
        let has_tag = is_enum || is_result_or_option || patterns_imply_enum;

        // Extract payload types for Result/Option
        let ok_payload_ty = self.extract_payload_type(scrutinee)
            .unwrap_or(MirType::I64);
        let err_payload_ty = self.extract_err_type(scrutinee)
            .unwrap_or(MirType::I64);

        // Determine the switch value
        let switch_val = if has_tag {
            let tag_local = self.builder.alloc_temp(MirType::U8);
            self.builder.push_stmt(MirStmt::Assign {
                dst: tag_local,
                rvalue: MirRValue::EnumTag {
                    value: scrutinee_op.clone(),
                },
            });
            MirOperand::Local(tag_local)
        } else {
            scrutinee_op.clone()
        };

        let merge_block = self.builder.create_block();
        let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

        // Build switch cases: map pattern → (value, block)
        let mut cases: Vec<(u64, BlockId)> = Vec::new();
        let mut default_block = merge_block;

        for (i, arm) in arms.iter().enumerate() {
            match &arm.pattern {
                Pattern::Wildcard => {
                    default_block = arm_blocks[i];
                }
                Pattern::Ident(name) => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag && is_variant_name(name) {
                        // Bare variant name (Ok, None, etc.) → use well-known tag
                        cases.push((self.variant_tag(name) as u64, arm_blocks[i]));
                    } else {
                        // Binding variable — acts as default
                        default_block = arm_blocks[i];
                    }
                }
                Pattern::Constructor { name, .. } => {
                    if let Some(tag) = self.resolve_pattern_tag(name) {
                        cases.push((tag, arm_blocks[i]));
                    } else if has_tag {
                        cases.push((self.variant_tag(name) as u64, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                Pattern::Literal(lit_expr) => {
                    if let ExprKind::Int(v, _) = &lit_expr.kind {
                        cases.push((*v as u64, arm_blocks[i]));
                    } else if let ExprKind::Bool(b) = &lit_expr.kind {
                        cases.push((if *b { 1 } else { 0 }, arm_blocks[i]));
                    } else {
                        cases.push((i as u64, arm_blocks[i]));
                    }
                }
                _ => {
                    cases.push((i as u64, arm_blocks[i]));
                }
            }
        }

        self.builder.terminate(MirTerminator::Switch {
            value: switch_val,
            cases,
            default: default_block,
        });

        // Lower each arm
        let mut result_ty = MirType::Void;
        let result_local = self.builder.alloc_temp(MirType::I64);
        for (i, arm) in arms.iter().enumerate() {
            self.builder.switch_to_block(arm_blocks[i]);

            // Bind pattern variables for enum payloads
            if has_tag {
                if let Pattern::Constructor { name, fields } = &arm.pattern {
                    // Determine field types from enum layout or Result/Option
                    let variant_fields: Option<Vec<(MirType, u32)>> =
                        if let MirType::Enum(crate::types::EnumLayoutId(idx)) = &scrutinee_ty {
                            self.ctx.enum_layouts.get(*idx as usize).and_then(|layout| {
                                layout.variants.iter().find(|v| v.name == *name).map(|v| {
                                    v.fields.iter().map(|f| {
                                        (self.ctx.type_to_mir(&f.ty), f.offset)
                                    }).collect()
                                })
                            })
                        } else {
                            None
                        };

                    for (j, field_pat) in fields.iter().enumerate() {
                        if let Pattern::Ident(binding) = field_pat {
                            let field_ty = if let Some(ref vf) = variant_fields {
                                vf.get(j).map(|(ty, _)| ty.clone()).unwrap_or(MirType::I64)
                            } else {
                                // Result/Option: use payload type based on variant
                                match name.as_str() {
                                    "Err" => err_payload_ty.clone(),
                                    _ => ok_payload_ty.clone(),
                                }
                            };
                            let payload_local = self.builder.alloc_local(
                                binding.clone(), field_ty.clone(),
                            );
                            self.builder.push_stmt(MirStmt::Assign {
                                dst: payload_local,
                                rvalue: MirRValue::Field {
                                    base: scrutinee_op.clone(),
                                    field_index: j as u32,
                                },
                            });
                            self.locals.insert(binding.clone(), (payload_local, field_ty));
                        }
                    }
                }
            }

            let (body_val, arm_ty) = self.lower_expr(&arm.body)?;
            if i == 0 {
                result_ty = arm_ty;
            }

            // Only emit Assign+Goto if the arm body didn't already terminate
            // (e.g., via return, break, or continue).
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(body_val),
                });
                self.builder.terminate(MirTerminator::Goto {
                    target: merge_block,
                });
            }
        }

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(result_local), result_ty))
    }

    /// Resolve enum variant name to its tag value from the layout.
    /// Handles "Color.Red" → 0, "Color.Green" → 1, etc.
    fn resolve_pattern_tag(&self, name: &str) -> Option<u64> {
        // Pattern name could be "Color.Red" (qualified) or just "Red" (variant only)
        let parts: Vec<&str> = name.splitn(2, '.').collect();
        let (enum_name, variant_name) = if parts.len() == 2 {
            (parts[0], parts[1])
        } else {
            return None;
        };
        let (_, layout) = self.ctx.find_enum(enum_name)?;
        let variant = layout.variants.iter().find(|v| v.name == variant_name)?;
        Some(variant.tag)
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

    /// Closure lowering: synthesize a separate MIR function for the body,
    /// build the environment, and emit ClosureCreate in the enclosing function.
    fn lower_closure(
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
            let size = mir_type_size(ty);
            // 8-byte alignment
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
            });
            env_offset = aligned_offset + size;
        }

        // 4. Synthesize a MIR function for the closure body.
        //    Signature: fn closure_name(env_ptr: ptr, params...) -> ret
        let closure_ret = ret_ty
            .map(|s| self.ctx.resolve_type_str(s))
            .unwrap_or(MirType::I64);
        let mut closure_builder = BlockBuilder::new(closure_name.clone(), closure_ret.clone());

        // env_ptr is the implicit first parameter
        let env_param_id = closure_builder.add_param("__env".to_string(), MirType::Ptr);

        // Add explicit closure parameters
        let mut closure_locals = std::collections::HashMap::new();
        for param in params {
            let param_ty = param.ty.as_deref()
                .map(|s| self.ctx.resolve_type_str(s))
                .unwrap_or(MirType::I64);
            let param_id = closure_builder.add_param(param.name.clone(), param_ty.clone());
            closure_locals.insert(param.name.clone(), (param_id, param_ty));
        }

        // Emit LoadCapture for each free variable
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

        // Lower the closure body using a temporary lowerer
        {
            let saved_builder = std::mem::replace(&mut self.builder, closure_builder);
            let saved_locals = std::mem::replace(&mut self.locals, closure_locals);
            let saved_loop_stack = std::mem::take(&mut self.loop_stack);

            let body_result = self.lower_expr(body);

            // Restore parent state
            closure_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;

            let (body_val, _body_ty) = body_result?;

            // Add implicit return of body value if block is unterminated
            if closure_builder.current_block_unterminated() {
                closure_builder.terminate(MirTerminator::Return {
                    value: Some(body_val),
                });
            }
        }

        let closure_fn = closure_builder.finish();

        // Register the closure function signature for return type lookup
        self.func_sigs.insert(closure_name.clone(), super::FuncSig {
            ret_ty: closure_ret,
        });

        self.synthesized_functions.push(closure_fn);

        // 5. In the parent function, emit ClosureCreate (heap by default;
        //    the closure optimization pass downgrades to stack when safe).
        let result_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::ClosureCreate {
            dst: result_local,
            func_name: closure_name,
            captures,
            heap: true,
        });

        Ok((MirOperand::Local(result_local), MirType::Ptr))
    }

    /// Spawn lowering: synthesize a closure function from the body block,
    /// emit ClosureCreate + Call to rask_closure_spawn.
    ///
    /// The closure function has signature `fn(env_ptr: ptr)` — no params, void return.
    /// rask_closure_spawn extracts func_ptr from the closure, spawns an OS thread,
    /// and frees the closure allocation when the task completes.
    fn lower_spawn(
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
            let size = mir_type_size(ty);
            let aligned_offset = (env_offset + 7) & !7;
            captures.push(ClosureCapture {
                local_id: *local_id,
                offset: aligned_offset,
                size,
            });
            env_offset = aligned_offset + size;
        }

        // 4. Synthesize a MIR function for the spawn body.
        //    Signature: fn spawn_name(env_ptr: ptr) -> void
        let mut spawn_builder = BlockBuilder::new(spawn_name.clone(), MirType::Void);

        // env_ptr is the implicit first parameter (standard closure convention)
        let env_param_id = spawn_builder.add_param("__env".to_string(), MirType::Ptr);

        // Emit LoadCapture for each free variable
        let mut spawn_locals = std::collections::HashMap::new();
        for (i, (name, _outer_id, ty)) in free_vars.iter().enumerate() {
            let cap = &captures[i];
            let local_id = spawn_builder.alloc_local(name.clone(), ty.clone());
            spawn_builder.push_stmt(MirStmt::LoadCapture {
                dst: local_id,
                env_ptr: env_param_id,
                offset: cap.offset,
            });
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

            // Restore parent state
            spawn_builder = std::mem::replace(&mut self.builder, saved_builder);
            self.locals = saved_locals;
            self.loop_stack = saved_loop_stack;

            body_result?;

            // Add implicit void return if unterminated
            if spawn_builder.current_block_unterminated() {
                spawn_builder.terminate(MirTerminator::Return { value: None });
            }
        }

        let spawn_fn = spawn_builder.finish();

        // Try the state machine transform for yield-point-containing spawns
        if let Some(sm_result) = crate::transform::state_machine::transform(&spawn_fn) {
            // Register the poll function
            let poll_name = sm_result.poll_fn.name.clone();
            self.func_sigs.insert(poll_name.clone(), super::FuncSig {
                ret_ty: MirType::I32,
            });
            self.synthesized_functions.push(sm_result.poll_fn);

            // Allocate state struct, store initial tag = 0, captured vars
            let state_ptr = self.builder.alloc_temp(MirType::Ptr);
            let state_size_val = sm_result.state_size as i64;
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(state_ptr),
                func: FunctionRef { name: "rask_alloc".to_string() },
                args: vec![MirOperand::Constant(crate::operand::MirConst::Int(state_size_val))],
            });

            // Store state_tag = 0
            self.builder.push_stmt(MirStmt::Store {
                addr: state_ptr,
                offset: 0,
                value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
            });

            // Store captured variables into the state struct
            for field in &sm_result.state_fields {
                if let Some(orig_local_id) = field.local_id {
                    // Find the capture that corresponds to this local
                    if let Some(cap) = captures.iter().find(|c| c.local_id == orig_local_id) {
                        let _ = cap; // the local is in scope — just store it
                    }
                    // Store the local's current value into the state struct
                    self.builder.push_stmt(MirStmt::Store {
                        addr: state_ptr,
                        offset: field.offset,
                        value: MirOperand::Local(orig_local_id),
                    });
                }
            }

            // Emit: rask_green_spawn(poll_fn, state_ptr, state_size)
            let poll_fn_ptr = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::Assign {
                dst: poll_fn_ptr,
                rvalue: MirRValue::Use(MirOperand::Constant(
                    crate::operand::MirConst::String(poll_name),
                )),
            });

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(handle_local),
                func: FunctionRef { name: "rask_green_spawn".to_string() },
                args: vec![
                    MirOperand::Local(poll_fn_ptr),
                    MirOperand::Local(state_ptr),
                    MirOperand::Constant(crate::operand::MirConst::Int(state_size_val)),
                ],
            });

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        } else {
            // No yield points — use the closure bridge
            // Register the spawn function signature
            self.func_sigs.insert(spawn_name.clone(), super::FuncSig {
                ret_ty: MirType::Void,
            });
            self.synthesized_functions.push(spawn_fn);

            // Emit ClosureCreate + rask_green_closure_spawn call
            let closure_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::ClosureCreate {
                dst: closure_local,
                func_name: spawn_name,
                captures,
                heap: true,
            });

            let handle_local = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(handle_local),
                func: FunctionRef { name: "spawn".to_string() },
                args: vec![MirOperand::Local(closure_local)],
            });

            Ok((MirOperand::Local(handle_local), MirType::Ptr))
        }
    }

    /// Collect free variables from a block of statements (no params to bind).
    fn collect_free_vars_block(
        &self,
        body: &[Stmt],
    ) -> Vec<(String, LocalId, MirType)> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bound = std::collections::HashSet::new();
        self.walk_free_vars_block(body, &bound, &mut seen, &mut free);
        free
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

        // Err path — extract the error payload and return it
        self.builder.switch_to_block(err_block);
        let err_ty = self.extract_err_type(inner).unwrap_or(MirType::I64);
        let err_val = self.builder.alloc_temp(err_ty);
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
        // Infer Ok payload type from the Result being tried
        let ok_ty = self.extract_payload_type(inner)
            .unwrap_or(MirType::I64);
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

    /// Expand `json.encode(struct_val)` into a sequence of json_buf_* calls.
    fn lower_json_encode_struct(
        &mut self,
        struct_op: MirOperand,
        layout: StructLayout,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        let buf = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(buf),
            func: FunctionRef { name: "json_buf_new".to_string() },
            args: vec![],
        });

        for (idx, field) in layout.fields.iter().enumerate() {
            let field_val = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::Assign {
                dst: field_val,
                rvalue: MirRValue::Field {
                    base: struct_op.clone(),
                    field_index: idx as u32,
                },
            });

            let helper = match &field.ty {
                Type::String => "json_buf_add_string",
                Type::Bool => "json_buf_add_bool",
                Type::F32 | Type::F64 => "json_buf_add_f64",
                _ => "json_buf_add_i64",
            };

            self.builder.push_stmt(MirStmt::Call {
                dst: None,
                func: FunctionRef { name: helper.to_string() },
                args: vec![
                    MirOperand::Local(buf),
                    MirOperand::Constant(MirConst::String(field.name.clone())),
                    MirOperand::Local(field_val),
                ],
            });
        }

        let result = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(result),
            func: FunctionRef { name: "json_buf_finish".to_string() },
            args: vec![MirOperand::Local(buf)],
        });

        Ok((MirOperand::Local(result), MirType::I64))
    }

    /// Expand `json.decode<T>(str)` into json_parse + field extraction + struct construction.
    fn lower_json_decode_struct(
        &mut self,
        str_op: MirOperand,
        layout: StructLayout,
    ) -> Result<TypedOperand, LoweringError> {
        use rask_types::Type;

        // Parse JSON string into opaque object
        let parsed = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(parsed),
            func: FunctionRef { name: "json_parse".to_string() },
            args: vec![str_op],
        });

        // Find struct type ID for result
        let struct_id = self.ctx.find_struct(&layout.name)
            .map(|(id, _)| StructLayoutId(id));
        let struct_ty = struct_id
            .map(MirType::Struct)
            .unwrap_or(MirType::I64);

        // Allocate struct and extract each field
        let result = self.builder.alloc_temp(struct_ty.clone());
        for (_idx, field) in layout.fields.iter().enumerate() {
            let helper = match &field.ty {
                Type::String => "json_get_string",
                Type::Bool => "json_get_bool",
                Type::F32 | Type::F64 => "json_get_f64",
                _ => "json_get_i64",
            };

            let field_val = self.builder.alloc_temp(MirType::I64);
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(field_val),
                func: FunctionRef { name: helper.to_string() },
                args: vec![
                    MirOperand::Local(parsed),
                    MirOperand::Constant(MirConst::String(field.name.clone())),
                ],
            });

            self.builder.push_stmt(MirStmt::Store {
                addr: result,
                offset: field.offset,
                value: MirOperand::Local(field_val),
            });
        }

        Ok((MirOperand::Local(result), struct_ty))
    }

    /// Size in bytes for a MIR type (used for runtime allocation).
    fn elem_size_for_type(&self, ty: &MirType) -> i64 {
        match ty {
            MirType::Bool | MirType::I8 | MirType::U8 => 1,
            MirType::I16 | MirType::U16 => 2,
            MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
            MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr
            | MirType::String | MirType::FuncPtr(_) => 8,
            MirType::Struct(StructLayoutId(id)) => {
                self.ctx.struct_layouts.get(*id as usize)
                    .map(|l| l.size as i64)
                    .unwrap_or(8)
            }
            MirType::Enum(EnumLayoutId(id)) => {
                self.ctx.enum_layouts.get(*id as usize)
                    .map(|l| l.size as i64)
                    .unwrap_or(8)
            }
            MirType::Array { elem, len } => self.elem_size_for_type(elem) * (*len as i64),
            MirType::Void => 0,
        }
    }
}
