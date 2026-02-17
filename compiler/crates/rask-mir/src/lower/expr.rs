// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use super::{
    binop_result_type, is_type_constructor_name, is_variant_name, lower_binop, lower_unaryop,
    operator_method_to_binop, operator_method_to_unaryop, LoweringError,
    MirLowerer, TypedOperand, HANDLE_NONE_SENTINEL,
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
    /// Resolve a MirType to its named type prefix using struct/enum layouts.
    pub(super) fn mir_type_name(&self, ty: &MirType) -> Option<String> {
        match ty {
            MirType::Struct(crate::types::StructLayoutId(idx)) => {
                self.ctx.struct_layouts.get(*idx as usize).map(|l| l.name.clone())
            }
            MirType::Enum(crate::types::EnumLayoutId(idx)) => {
                self.ctx.enum_layouts.get(*idx as usize).map(|l| l.name.clone())
            }
            MirType::String => Some("string".to_string()),
            MirType::F64 | MirType::F32 => Some("f64".to_string()),
            MirType::Bool => Some("bool".to_string()),
            MirType::Char => Some("char".to_string()),
            _ => None,
        }
    }

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
            ExprKind::Null => {
                // Null pointer literal — zero value
                Ok((MirOperand::Constant(MirConst::Int(0)), MirType::Ptr))
            }

            // Variable reference (or bare enum variant like None)
            ExprKind::Ident(name) => {
                if let Some((id, ty)) = self.locals.get(name).cloned() {
                    Ok((MirOperand::Local(id), ty))
                } else if name == "None" {
                    // Niche: Option<Handle<T>> uses sentinel instead of tag
                    if self.is_niche_option_expr(expr) {
                        Ok((MirOperand::Constant(MirConst::Int(HANDLE_NONE_SENTINEL)), MirType::Handle))
                    } else {
                        Ok((MirOperand::Constant(MirConst::Int(1)), MirType::Ptr))
                    }
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
                let mut arg_operands = Vec::new();
                for a in args {
                    let (op, _) = self.lower_expr(&a.expr)?;
                    arg_operands.push(op);
                }

                // Non-ident callees: field access, returned functions, etc.
                // Lower the callee expression and emit an indirect ClosureCall.
                let func_name = match &func.kind {
                    ExprKind::Ident(name) => name.clone(),
                    _ => {
                        let (callee_op, _callee_ty) = self.lower_expr(func)?;
                        let callee_local = match callee_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(callee_op),
                                });
                                tmp
                            }
                        };
                        let ret_ty = self.lookup_expr_type(expr).unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::ClosureCall {
                            dst: Some(result_local),
                            closure: callee_local,
                            args: arg_operands,
                        });
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                };

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
                    "Some" if self.is_niche_option_expr(expr) => {
                        // Niche: Some(handle) is just the handle value
                        let val = arg_operands.into_iter().next()
                            .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                        return Ok((val, MirType::Handle));
                    }
                    "Ok" | "Some" | "Err" => {
                        let tag = self.variant_tag(&func_name);
                        // Derive the result MirType from type checker info if available
                        let result_ty = self.lookup_expr_type(expr).unwrap_or(MirType::Ptr);
                        let result_local = self.builder.alloc_temp(result_ty.clone());
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
                        return Ok((MirOperand::Local(result_local), result_ty));
                    }
                    _ => {}
                }

                let ret_ty = self
                    .func_sigs
                    .get(&func_name)
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or(MirType::I64);

                let result_local = self.builder.alloc_temp(ret_ty.clone());

                let func_ref = if self.ctx.extern_funcs.contains(&func_name) {
                    FunctionRef::extern_c(func_name)
                } else {
                    FunctionRef::internal(func_name)
                };
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: func_ref,
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
                // Iterator terminal methods: .collect(), .fold(), .any(), .all(), etc.
                // Try to recognize an iterator chain on the receiver and fuse it inline.
                if let Some(result) = self.try_lower_iter_terminal(expr, object, method, args)? {
                    return Ok(result);
                }

                // Module.Type.method() pattern: time.Instant.now() → Instant_now
                // Detect field access on a module name and flatten to a qualified call.
                if let ExprKind::Field { object: inner_obj, field: type_name } = &object.kind {
                    if let ExprKind::Ident(module_name) = &inner_obj.kind {
                        if !self.locals.contains_key(module_name)
                            && is_type_constructor_name(module_name)
                        {
                            let func_name = format!("{}_{}", type_name, method);
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
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            });
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                // When the object is a type name (not a local variable), intercept
                // before lowering it as a value expression.
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        // Comptime global: TABLE.get(0) → GlobalRef + Vec_get
                        if let Some(meta) = self.ctx.comptime_globals.get(name) {
                            let type_prefix = meta.type_prefix.clone();
                            let elem_count = meta.elem_count;

                            // Load the comptime global data pointer
                            let global_local = self.builder.alloc_temp(MirType::Ptr);
                            self.builder.push_stmt(MirStmt::GlobalRef {
                                dst: global_local,
                                name: name.clone(),
                            });

                            // Wrap raw data into a Vec: rask_vec_from_static(ptr, count)
                            let vec_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::Call {
                                dst: Some(vec_local),
                                func: FunctionRef::internal("rask_vec_from_static".to_string()),
                                args: vec![
                                    MirOperand::Local(global_local),
                                    MirOperand::Constant(MirConst::Int(elem_count as i64)),
                                ],
                            });

                            // Dispatch method using the type prefix
                            let func_name = format!("{}_{}", type_prefix, method);
                            let mut arg_operands = vec![MirOperand::Local(vec_local)];
                            for arg in args {
                                let (op, _) = self.lower_expr(&arg.expr)?;
                                arg_operands.push(op);
                            }
                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            });
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }

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
                                    func: FunctionRef::internal("Vec_new".to_string()),
                                    args: vec![],
                                });
                                // Push each variant's tag value
                                for variant in &layout.variants {
                                    self.builder.push_stmt(MirStmt::Call {
                                        dst: None,
                                        func: FunctionRef::internal("Vec_push".to_string()),
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
                                func: FunctionRef::internal(helper.to_string()),
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
                                func: FunctionRef::internal("json_decode".to_string()),
                                args: vec![str_op],
                            });
                            return Ok((MirOperand::Local(result_local), MirType::I64));
                        }

                        // Static method on a type: Vec.new(), string.new()
                        let is_known_type = self.ctx.find_struct(name).is_some()
                            || self.ctx.find_enum(name).is_some()
                            || is_type_constructor_name(name);

                        if is_known_type {
                            // Strip generic parameters: "Channel<i64>" → "Channel"
                            let base_name = name.split('<').next().unwrap_or(name);
                            let func_name = format!("{}_{}", base_name, method);
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
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            });
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Skip native binop for types that need C runtime calls (SIMD vectors)
                // or special method dispatch (raw pointers: ptr.add != arithmetic add).
                let skip_binop = if let ExprKind::Ident(var_name) = &object.kind {
                    self.local_type_prefix.get(var_name)
                        .map(|p| matches!(p.as_str(), "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8" | "Ptr"))
                        .unwrap_or(false)
                    || matches!(obj_ty, MirType::Ptr)
                } else {
                    matches!(obj_ty, MirType::Ptr)
                };

                // Detect binary operator methods (desugared from a + b → a.add(b))
                // Skip for SIMD types and raw pointers — they use method dispatch.
                if !skip_binop {
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
                } // end if !skip_binop

                // concat(): string concatenation from interpolation desugaring
                if method == "concat" && args.len() == 1 && matches!(obj_ty, MirType::String) {
                    let (arg_op, _) = self.lower_expr(&args[0].expr)?;
                    let result_local = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("concat".to_string()),
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
                        MirType::F64 | MirType::F32 => "f64_to_string",
                        MirType::Bool => "bool_to_string",
                        MirType::Char => "char_to_string",
                        _ => "i64_to_string", // fallback
                    };
                    let result_local = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal(func_name.to_string()),
                        args: vec![obj_op],
                    });
                    return Ok((MirOperand::Local(result_local), MirType::String));
                }

                // map_err: inline expansion — branch on tag, transform error payload
                if method == "map_err" && args.len() == 1 {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { params, .. } if params.len() == 1) {
                        return self.lower_map_err(obj_op, &args[0].expr);
                    }
                    // Variant constructor: result.map_err(MyError)
                    if let ExprKind::Ident(name) = &args[0].expr.kind {
                        return self.lower_map_err_constructor(obj_op, name);
                    }
                }

                // Array.len() → compile-time constant (no runtime call)
                if method == "len" && args.is_empty() {
                    if let MirType::Array { len, .. } = &obj_ty {
                        return Ok((
                            MirOperand::Constant(MirConst::Int(*len as i64)),
                            MirType::I64,
                        ));
                    }
                }

                // Regular method call
                let mut all_args = vec![obj_op];
                for arg in args {
                    let (op, _) = self.lower_expr(&arg.expr)?;
                    all_args.push(op);
                }

                // Qualify method name with receiver type to avoid dispatch
                // ambiguity (e.g. Vec.get vs Map.get vs Pool.get).
                // Check local_type_prefix first (tracks actual codegen types),
                // then fall back to type-checker info (handles both stdlib
                // and user-defined types from extend blocks).
                let qualified_name = if let ExprKind::Ident(var_name) = &object.kind {
                        self.local_type_prefix.get(var_name).cloned()
                    } else {
                        None
                    }
                    .or_else(|| {
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| super::MirContext::type_prefix(ty))
                    })
                    // Fallback: derive prefix from MIR type (catches F64, String, etc.)
                    .or_else(|| super::mir_type_method_prefix(&obj_ty).map(|s| s.to_string()))
                    .map(|prefix| format!("{}_{}", prefix, method))
                    .unwrap_or_else(|| method.clone());

                let ret_ty = self
                    .func_sigs
                    .get(method)
                    .or_else(|| self.func_sigs.get(&qualified_name))
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or_else(|| super::stdlib_return_mir_type(&qualified_name));
                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(qualified_name),
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

                // Fixed-size arrays: direct memory access (base + index * elem_size)
                if let MirType::Array { ref elem, .. } = obj_ty {
                    let elem_size = elem.size();
                    let result_ty = *elem.clone();
                    let result_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::Assign {
                        dst: result_local,
                        rvalue: MirRValue::ArrayIndex {
                            base: obj_op,
                            index: idx_op,
                            elem_size,
                        },
                    });
                    return Ok((MirOperand::Local(result_local), result_ty));
                }

                // Vec/Map/etc: dispatch through runtime
                let result_ty = MirType::I32; // fallback for non-array indexing
                let index_name = if let ExprKind::Ident(var_name) = &object.kind {
                        self.local_type_prefix.get(var_name).cloned()
                    } else {
                        None
                    }
                    .or_else(|| {
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| super::MirContext::type_prefix(ty))
                    })
                    .map(|prefix| format!("{}_index", prefix))
                    .unwrap_or_else(|| "index".to_string());
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(index_name),
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
                let elem_size = elem_ty.size();
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
                let mut elem_types = Vec::new();
                let mut lowered_elems = Vec::new();
                for elem in elems.iter() {
                    let (elem_op, elem_ty) = self.lower_expr(elem)?;
                    lowered_elems.push(elem_op);
                    elem_types.push(elem_ty);
                }
                let tuple_ty = MirType::Tuple(elem_types.clone());
                let result_local = self.builder.alloc_temp(tuple_ty.clone());
                let mut offset = 0u32;
                for (elem_op, elem_ty) in lowered_elems.into_iter().zip(elem_types.iter()) {
                    let elem_size = elem_ty.size();
                    let elem_align = elem_ty.align().max(1);
                    offset = (offset + elem_align - 1) & !(elem_align - 1);
                    self.builder.push_stmt(MirStmt::Store {
                        addr: result_local,
                        offset,
                        value: elem_op,
                    });
                    offset += elem_size;
                }
                Ok((MirOperand::Local(result_local), tuple_ty))
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
                let is_niche = self.is_niche_option_expr(expr);
                let (val, _) = self.lower_expr(expr)?;
                let tag = self.emit_option_tag(&val, is_niche);

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
                self.bind_pattern_payload_niche(pattern, val, payload_ty, is_niche);
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
                let is_niche = self.is_niche_option_expr(expr);
                let (val, _) = self.lower_expr(expr)?;
                let tag = self.emit_option_tag(&val, is_niche);

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
                self.bind_pattern_payload_niche(pattern, val.clone(), payload_ty.clone(), is_niche);
                // Extract the payload value for the result
                let payload = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::Goto { target: merge_block });

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(payload), payload_ty))
            }

            // Pattern test (expr is Pattern) — evaluates to bool
            ExprKind::IsPattern { expr: inner, pattern } => {
                let is_niche = self.is_niche_option_expr(inner);
                let (val, _ty) = self.lower_expr(inner)?;
                let tag = self.emit_option_tag(&val, is_niche);
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
                let is_niche = self.is_niche_option_expr(inner);
                let (val, _inner_ty) = self.lower_expr(inner)?;
                let tag_local = self.emit_option_tag(&val, is_niche);

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block,
                    else_block: ok_block,
                });

                self.builder.switch_to_block(panic_block);
                self.emit_source_location(&expr.span);
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef::internal("panic_unwrap".to_string()),
                    args: vec![],
                });
                self.builder.terminate(MirTerminator::Unreachable);

                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(inner)
                    .unwrap_or(MirType::I64);
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                Ok((MirOperand::Local(result_local), payload_ty))
            }

            // Null coalescing (a ?? b)
            ExprKind::NullCoalesce { value, default } => {
                let is_niche = self.is_niche_option_expr(value);
                let (val, _) = self.lower_expr(value)?;
                let tag_local = self.emit_option_tag(&val, is_niche);

                let some_block = self.builder.create_block();
                let none_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                });

                self.builder.switch_to_block(some_block);
                let payload_ty = self.extract_payload_type(value)
                    .unwrap_or(MirType::I64);
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
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
                    func: FunctionRef::internal(func_name.to_string()),
                    args,
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Array repeat ([value; count])
            ExprKind::ArrayRepeat { value, count } => {
                // Constant count → expand to a fixed-size array (same as literal)
                if let ExprKind::Int(n, _) = &count.kind {
                    let (val, elem_ty) = self.lower_expr(value)?;
                    let len = *n as u32;
                    let elem_size = elem_ty.size();
                    let array_ty = MirType::Array { elem: Box::new(elem_ty), len };
                    let result_local = self.builder.alloc_temp(array_ty.clone());
                    for i in 0..len {
                        self.builder.push_stmt(MirStmt::Store {
                            addr: result_local,
                            offset: i * elem_size,
                            value: val.clone(),
                        });
                    }
                    return Ok((MirOperand::Local(result_local), array_ty));
                }

                // Dynamic count: keep existing Ptr-based fallback
                let (val, elem_ty) = self.lower_expr(value)?;
                let (cnt, _) = self.lower_expr(count)?;
                let result_ty = MirType::Ptr;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                let elem_size = self.elem_size_for_type(&elem_ty);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal("array_repeat".to_string()),
                    args: vec![val, cnt, MirOperand::Constant(MirConst::Int(elem_size))],
                });
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Optional chaining (a?.b)
            ExprKind::OptionalField { object, field: _ } => {
                let is_niche = self.is_niche_option_expr(object);
                let (obj, _) = self.lower_expr(object)?;
                let tag_local = self.emit_option_tag(&obj, is_niche);

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
                let rvalue = if is_niche {
                    MirRValue::Use(obj)
                } else {
                    MirRValue::Field { base: obj, field_index: 0 }
                };
                self.builder.push_stmt(MirStmt::Assign {
                    dst: result_local,
                    rvalue,
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
                        func: FunctionRef::internal("rask_runtime_init".to_string()),
                        args: vec![MirOperand::Constant(crate::operand::MirConst::Int(0))],
                    });
                    let result = self.lower_block(body);
                    // rask_runtime_shutdown()
                    self.builder.push_stmt(MirStmt::Call {
                        dst: None,
                        func: FunctionRef::internal("rask_runtime_shutdown".to_string()),
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
                    func: FunctionRef::internal(name.clone()),
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
                self.emit_source_location(&expr.span);
                let mut args = Vec::new();
                if let Some(msg) = message {
                    let (msg_op, _) = self.lower_expr(msg)?;
                    args.push(msg_op);
                }
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef::internal("assert_fail".to_string()),
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
                    func: FunctionRef::internal("check_fail".to_string()),
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

        let is_niche = self.is_niche_option_expr(scrutinee);
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
        let has_tag = is_enum || is_result_or_option || patterns_imply_enum || is_niche;

        // Extract payload types for Result/Option
        let ok_payload_ty = self.extract_payload_type(scrutinee)
            .unwrap_or(MirType::I64);
        let err_payload_ty = self.extract_err_type(scrutinee)
            .unwrap_or(MirType::I64);

        // Determine the switch value
        let switch_val = if has_tag {
            let tag_local = self.emit_option_tag(&scrutinee_op, is_niche);
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
                            let rvalue = if is_niche {
                                // Niche: the scrutinee IS the payload
                                MirRValue::Use(scrutinee_op.clone())
                            } else {
                                MirRValue::Field {
                                    base: scrutinee_op.clone(),
                                    field_index: j as u32,
                                }
                            };
                            self.builder.push_stmt(MirStmt::Assign {
                                dst: payload_local,
                                rvalue,
                            });
                            // Set local_type_prefix so method calls on match-bound
                            // variables get the correct type qualification.
                            // Derive from MIR type (not node_types which may lack
                            // monomorphized expression IDs).
                            let prefix = self.mir_type_name(&field_ty)

                                .or_else(|| {
                                    // For user-defined enums: look up field type from layout
                                    if let MirType::Enum(crate::types::EnumLayoutId(idx)) = &scrutinee_ty {
                                        self.ctx.enum_layouts.get(*idx as usize).and_then(|layout| {
                                            layout.variants.iter().find(|v| v.name == *name).and_then(|v| {
                                                v.fields.get(j).and_then(|f| {
                                                    super::MirContext::type_prefix(&f.ty)
                                                })
                                            })
                                        })
                                    } else {
                                        None
                                    }
                                })
                                .or_else(|| {
                                    // For Result/Option: extract payload MirType from scrutinee
                                    let payload_mir = match (&scrutinee_ty, name.as_str()) {
                                        (MirType::Result { err, .. }, "Err") => Some(err.as_ref()),
                                        (MirType::Result { ok, .. }, _) => Some(ok.as_ref()),
                                        (MirType::Option(inner), _) => Some(inner.as_ref()),
                                        _ => None,
                                    };
                                    payload_mir.and_then(|t| self.mir_type_name(t))
                                });
                            if let Some(p) = prefix {
                                self.local_type_prefix.insert(binding.clone(), p);
                            }
                            self.locals.insert(binding.clone(), (payload_local, field_ty));
                        }
                    }
                }
            }

            // If the arm has a guard, evaluate it and conditionally skip
            if let Some(guard_expr) = &arm.guard {
                let (guard_val, _) = self.lower_expr(guard_expr)?;
                let guard_fail_block = if i + 1 < arm_blocks.len() {
                    arm_blocks[i + 1]
                } else {
                    default_block
                };
                let guard_pass_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::Branch {
                    cond: guard_val,
                    then_block: guard_pass_block,
                    else_block: guard_fail_block,
                });
                self.builder.switch_to_block(guard_pass_block);
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
            let size = ty.size();
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

            // Allocate state struct, init tag = 0, store captured vars
            let state_ptr = self.builder.alloc_temp(MirType::Ptr);
            let state_size_val = sm_result.state_size as i64;
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(state_ptr),
                func: FunctionRef::internal("rask_alloc".to_string()),
                args: vec![MirOperand::Constant(crate::operand::MirConst::Int(state_size_val))],
            });

            // Store state_tag = 0
            self.builder.push_stmt(MirStmt::Store {
                addr: state_ptr,
                offset: 0,
                value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
            });

            // Store captured variables into the state struct.
            // capture_stores maps (env_offset → state_offset). Match each
            // entry to the ClosureCapture with the same env_offset to find
            // the parent's local_id.
            for &(env_offset, state_offset) in &sm_result.capture_stores {
                if let Some(cap) = captures.iter().find(|c| c.offset == env_offset) {
                    self.builder.push_stmt(MirStmt::Store {
                        addr: state_ptr,
                        offset: state_offset,
                        value: MirOperand::Local(cap.local_id),
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
                func: FunctionRef::internal("rask_green_spawn".to_string()),
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
                func: FunctionRef::internal("spawn".to_string()),
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
        let (result, result_ty) = self.lower_expr(inner)?;

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
        let err_ty = self.extract_err_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { err, .. } => Some(err.as_ref().clone()),
                _ => None,
            })
            .unwrap_or(MirType::I64);
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
        // Infer Ok payload type: type checker → MIR type → walk AST for base call
        let ok_ty = self.extract_payload_type(inner)
            .or_else(|| match &result_ty {
                MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
                _ => None,
            })
            .or_else(|| {
                // Walk through method chains (e.g. expr.map_err(...)) to find
                // the base function call and extract Ok type from its return sig.
                let mut expr = inner;
                loop {
                    match &expr.kind {
                        ExprKind::MethodCall { object, .. } => { expr = object; }
                        ExprKind::Call { func, .. } => {
                            let name = match &func.kind {
                                ExprKind::Ident(n) => n.clone(),
                                ExprKind::Field { object: o, field: f } => {
                                    if let ExprKind::Ident(mod_name) = &o.kind {
                                        format!("{}_{}", mod_name, f)
                                    } else { break; }
                                }
                                _ => break,
                            };
                            let ret = self.func_sigs.get(&name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&name));
                            return match ret {
                                MirType::Result { ok, .. } => Some(*ok),
                                MirType::Option(inner) => Some(*inner),
                                _ => None,
                            };
                        }
                        _ => break,
                    }
                }
                None
            })
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

    /// Inline expansion of `result.map_err(|e| transform(e))`.
    ///
    /// Checks the result tag: Ok (tag=0) passes through unchanged,
    /// Err (tag=1) extracts the payload, calls the closure, and rewraps.
    fn lower_map_err(
        &mut self,
        result_op: MirOperand,
        closure_expr: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        // Lower the closure to get a callable local
        let (closure_op, _) = self.lower_expr(closure_expr)?;
        let closure_local = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::Assign {
            dst: closure_local,
            rvalue: MirRValue::Use(closure_op),
        });

        // Extract tag
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: result_op.clone() },
        });

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        let out = self.builder.alloc_temp(MirType::Ptr);

        // tag=0 → Ok, tag=1 → Err
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        });

        // Ok path: pass through unchanged
        self.builder.switch_to_block(ok_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: out,
            rvalue: MirRValue::Use(result_op.clone()),
        });
        self.builder.terminate(MirTerminator::Goto { target: merge_block });

        // Err path: extract payload, call closure, wrap as Err
        self.builder.switch_to_block(err_block);
        let err_payload = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: err_payload,
            rvalue: MirRValue::Field { base: result_op, field_index: 0 },
        });
        let new_err = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::ClosureCall {
            dst: Some(new_err),
            closure: closure_local,
            args: vec![MirOperand::Local(err_payload)],
        });
        // Construct Err(new_err): tag=1, payload=new_err
        self.builder.push_stmt(MirStmt::Store {
            addr: out,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
        });
        self.builder.push_stmt(MirStmt::Store {
            addr: out,
            offset: 8,
            value: MirOperand::Local(new_err),
        });
        self.builder.terminate(MirTerminator::Goto { target: merge_block });

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(out), MirType::Ptr))
    }

    /// Inline expansion of `result.map_err(VariantConstructor)`.
    ///
    /// Same logic as the closure version: Ok passes through, Err extracts
    /// the payload and wraps it with the variant constructor.
    fn lower_map_err_constructor(
        &mut self,
        result_op: MirOperand,
        constructor_name: &str,
    ) -> Result<TypedOperand, LoweringError> {
        // Extract tag
        let tag_local = self.builder.alloc_temp(MirType::U8);
        self.builder.push_stmt(MirStmt::Assign {
            dst: tag_local,
            rvalue: MirRValue::EnumTag { value: result_op.clone() },
        });

        let ok_block = self.builder.create_block();
        let err_block = self.builder.create_block();
        let merge_block = self.builder.create_block();

        let out = self.builder.alloc_temp(MirType::Ptr);

        // tag=0 → Ok, tag=1 → Err
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(tag_local),
            then_block: err_block,
            else_block: ok_block,
        });

        // Ok path: pass through unchanged
        self.builder.switch_to_block(ok_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: out,
            rvalue: MirRValue::Use(result_op.clone()),
        });
        self.builder.terminate(MirTerminator::Goto { target: merge_block });

        // Err path: extract payload, wrap with constructor, re-wrap as Err
        self.builder.switch_to_block(err_block);
        let err_payload = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: err_payload,
            rvalue: MirRValue::Field { base: result_op, field_index: 0 },
        });
        // Wrap payload in variant: tag + payload
        let constructor_tag = self.variant_tag(constructor_name);
        let wrapped = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::Store {
            addr: wrapped,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(constructor_tag)),
        });
        self.builder.push_stmt(MirStmt::Store {
            addr: wrapped,
            offset: 8,
            value: MirOperand::Local(err_payload),
        });
        // Re-wrap as Err(wrapped)
        self.builder.push_stmt(MirStmt::Store {
            addr: out,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)), // Err tag
        });
        self.builder.push_stmt(MirStmt::Store {
            addr: out,
            offset: 8,
            value: MirOperand::Local(wrapped),
        });
        self.builder.terminate(MirTerminator::Goto { target: merge_block });

        self.builder.switch_to_block(merge_block);
        Ok((MirOperand::Local(out), MirType::Ptr))
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
            func: FunctionRef::internal("json_buf_new".to_string()),
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

            // Nested struct: recursively encode and add as raw JSON
            let nested_struct = match &field.ty {
                Type::UnresolvedNamed(name) => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
                Type::UnresolvedGeneric { name, .. } => self.ctx.find_struct(name).map(|(_, l)| l.clone()),
                _ => None,
            };

            if let Some(nested_layout) = nested_struct {
                let (nested_json, _) = self.lower_json_encode_struct(
                    MirOperand::Local(field_val),
                    nested_layout,
                )?;
                self.builder.push_stmt(MirStmt::Call {
                    dst: None,
                    func: FunctionRef::internal("json_buf_add_raw".to_string()),
                    args: vec![
                        MirOperand::Local(buf),
                        MirOperand::Constant(MirConst::String(field.name.clone())),
                        nested_json,
                    ],
                });
                continue;
            }

            let helper = match &field.ty {
                Type::String => "json_buf_add_string",
                Type::Bool => "json_buf_add_bool",
                Type::F32 | Type::F64 => "json_buf_add_f64",
                _ => "json_buf_add_i64",
            };

            self.builder.push_stmt(MirStmt::Call {
                dst: None,
                func: FunctionRef::internal(helper.to_string()),
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
            func: FunctionRef::internal("json_buf_finish".to_string()),
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
            func: FunctionRef::internal("json_parse".to_string()),
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
                func: FunctionRef::internal(helper.to_string()),
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
            | MirType::String | MirType::FuncPtr(_) | MirType::Handle => 8,
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
            MirType::Tuple(_) | MirType::Slice(_) | MirType::Option(_)
            | MirType::Result { .. } | MirType::Union(_)
            | MirType::SimdVector { .. } => ty.size() as i64,
            MirType::Void => 0,
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Iterator chain recognition + inline expansion
    // ═══════════════════════════════════════════════════════════

    /// Walk a method call chain backward to find .iter() and collect adapters.
    ///
    /// vec.iter().filter(|x| p(x)).map(|x| f(x))
    ///                                 ↑ start here, walk left
    ///
    /// Returns None if the chain doesn't end in .iter() or uses unsupported adapters.
    pub(super) fn try_parse_iter_chain<'e>(&self, expr: &'e Expr) -> Option<super::IterChain<'e>> {
        let mut adapters = Vec::new();
        let mut current = expr;

        loop {
            match &current.kind {
                ExprKind::MethodCall { object, method, args, .. } => {
                    match method.as_str() {
                        "iter" if args.is_empty() => {
                            // Found the source — reverse adapters (we collected outside-in)
                            adapters.reverse();
                            return Some(super::IterChain {
                                source: object,
                                adapters,
                            });
                        }
                        "filter" if args.len() == 1 => {
                            if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                                adapters.push(super::IterAdapter::Filter { closure: &args[0].expr });
                                current = object;
                            } else {
                                return None;
                            }
                        }
                        "map" if args.len() == 1 => {
                            if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                                adapters.push(super::IterAdapter::Map { closure: &args[0].expr });
                                current = object;
                            } else {
                                return None;
                            }
                        }
                        "take" if args.len() == 1 => {
                            adapters.push(super::IterAdapter::Take { count: &args[0].expr });
                            current = object;
                        }
                        "skip" if args.len() == 1 => {
                            adapters.push(super::IterAdapter::Skip { count: &args[0].expr });
                            current = object;
                        }
                        "enumerate" if args.is_empty() => {
                            adapters.push(super::IterAdapter::Enumerate);
                            current = object;
                        }
                        _ => return None, // Unknown adapter
                    }
                }
                _ => return None, // Chain must be method calls all the way down
            }
        }
    }

    /// Try to handle an iterator terminal method (.collect, .fold, .any, etc.)
    /// by recognizing the chain and emitting a fused loop.
    ///
    /// Returns Some if handled, None to fall through to regular method call.
    fn try_lower_iter_terminal(
        &mut self,
        _full_expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[rask_ast::expr::CallArg],
    ) -> Result<Option<TypedOperand>, LoweringError> {
        match method {
            "collect" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_collect(&chain)?;
                    return Ok(Some(result));
                }
            }
            "fold" if args.len() == 2 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_fold(&chain, &args[0].expr, &args[1].expr)?;
                    return Ok(Some(result));
                }
            }
            "any" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_any(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            "all" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_all(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            "count" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_count(&chain)?;
                    return Ok(Some(result));
                }
            }
            "sum" if args.is_empty() => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    let result = self.lower_iter_sum(&chain)?;
                    return Ok(Some(result));
                }
            }
            "find" if args.len() == 1 => {
                if let Some(chain) = self.try_parse_iter_chain(object) {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { .. }) {
                        let result = self.lower_iter_find(&chain, &args[0].expr)?;
                        return Ok(Some(result));
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Inline a closure body: substitute the closure parameter with a value
    /// and lower the body expression. Used to fuse iterator adapters.
    ///
    /// |x| x * 2  +  arg_op  →  lower(x * 2) with x bound to arg_op
    pub(super) fn inline_closure_body(
        &mut self,
        closure: &Expr,
        arg_op: MirOperand,
        arg_ty: MirType,
    ) -> Result<TypedOperand, LoweringError> {
        if let ExprKind::Closure { params, body, .. } = &closure.kind {
            if let Some(param) = params.first() {
                let param_name = &param.name;
                // Save existing binding
                let saved = self.locals.remove(param_name);
                // Bind parameter to the argument value
                let param_local = self.builder.alloc_local(param_name.clone(), arg_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: param_local,
                    rvalue: MirRValue::Use(arg_op),
                });
                self.locals.insert(param_name.clone(), (param_local, arg_ty));
                // Lower closure body
                let result = self.lower_expr(body)?;
                // Restore previous binding
                self.locals.remove(param_name);
                if let Some(prev) = saved {
                    self.locals.insert(param_name.clone(), prev);
                }
                return Ok(result);
            }
        }
        Err(LoweringError::InvalidConstruct("expected closure".to_string()))
    }

    /// Set up the index loop infrastructure for an iterator chain.
    /// Returns (collection_local, idx_local, len_local, elem_ty, is_array, check_block, body_block, inc_block, exit_block).
    pub(super) fn setup_iter_chain_loop(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<IterLoopSetup, LoweringError> {
        let (source_op, source_ty) = self.lower_expr(chain.source)?;
        let is_array = matches!(&source_ty, MirType::Array { .. });
        let (array_len, array_elem_size) = match &source_ty {
            MirType::Array { elem, len } => (Some(*len), Some(elem.size())),
            _ => (None, None),
        };

        let collection = self.builder.alloc_temp(source_ty.clone());
        self.builder.push_stmt(MirStmt::Assign {
            dst: collection,
            rvalue: MirRValue::Use(source_op),
        });

        // len
        let len_local = self.builder.alloc_temp(MirType::I64);
        if let Some(arr_len) = array_len {
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

        // Process Skip/Take adapters to adjust start/end bounds
        let mut start_val: Option<MirOperand> = None;
        let mut end_op = MirOperand::Local(len_local);

        for adapter in &chain.adapters {
            match adapter {
                super::IterAdapter::Skip { count } => {
                    let (skip_op, _) = self.lower_expr(count)?;
                    start_val = Some(skip_op);
                }
                super::IterAdapter::Take { count } => {
                    let (take_op, _) = self.lower_expr(count)?;
                    // end = min(start + take, len) — simplified to start + take
                    if let Some(ref start) = start_val {
                        let adjusted = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::Assign {
                            dst: adjusted,
                            rvalue: MirRValue::BinaryOp {
                                op: crate::operand::BinOp::Add,
                                left: start.clone(),
                                right: take_op,
                            },
                        });
                        end_op = MirOperand::Local(adjusted);
                    } else {
                        end_op = take_op;
                    }
                }
                _ => {} // Filter/Map/Enumerate handled inside the loop body
            }
        }

        let idx = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(start_val.unwrap_or(MirOperand::Constant(MirConst::Int(0)))),
        });

        let check_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let inc_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.terminate(MirTerminator::Goto { target: check_block });

        // check: idx < end
        self.builder.switch_to_block(check_block);
        let cond = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: cond,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Lt,
                left: MirOperand::Local(idx),
                right: end_op,
            },
        });
        self.builder.terminate(MirTerminator::Branch {
            cond: MirOperand::Local(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // body: load element
        self.builder.switch_to_block(body_block);
        let elem_ty = self.extract_iterator_elem_type(chain.source)
            .unwrap_or(MirType::I64);
        let elem_local = self.builder.alloc_temp(elem_ty.clone());
        if is_array {
            self.builder.push_stmt(MirStmt::Assign {
                dst: elem_local,
                rvalue: MirRValue::ArrayIndex {
                    base: MirOperand::Local(collection),
                    index: MirOperand::Local(idx),
                    elem_size: array_elem_size.unwrap_or(8),
                },
            });
        } else {
            self.builder.push_stmt(MirStmt::Call {
                dst: Some(elem_local),
                func: FunctionRef::internal("Vec_get".to_string()),
                args: vec![MirOperand::Local(collection), MirOperand::Local(idx)],
            });
        }

        Ok(IterLoopSetup {
            idx,
            elem_local,
            elem_ty,
            inc_block,
            exit_block,
            check_block,
        })
    }

    /// Apply filter/map/enumerate adapters inside a loop body.
    /// Returns the final (operand, type) after all adapters.
    /// For filter adapters, emits a branch that skips to inc_block on false.
    pub(super) fn apply_iter_adapters(
        &mut self,
        chain: &super::IterChain<'_>,
        elem_op: MirOperand,
        elem_ty: MirType,
        inc_block: crate::BlockId,
        idx: crate::LocalId,
    ) -> Result<TypedOperand, LoweringError> {
        let mut current_op = elem_op;
        let mut current_ty = elem_ty;
        // enumerate_idx is used below when building the tuple

        for adapter in &chain.adapters {
            match adapter {
                super::IterAdapter::Filter { closure } => {
                    let (pred_op, _) = self.inline_closure_body(closure, current_op.clone(), current_ty.clone())?;
                    // Continue block for elements that pass the filter
                    let pass_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::Branch {
                        cond: pred_op,
                        then_block: pass_block,
                        else_block: inc_block,
                    });
                    self.builder.switch_to_block(pass_block);
                }
                super::IterAdapter::Map { closure } => {
                    let (mapped_op, mapped_ty) = self.inline_closure_body(closure, current_op, current_ty)?;
                    current_op = mapped_op;
                    current_ty = mapped_ty;
                }
                super::IterAdapter::Enumerate => {
                    // Build (index, element) tuple on the stack
                    let tuple_ty = MirType::Tuple(vec![MirType::I64, current_ty.clone()]);
                    let tuple_local = self.builder.alloc_temp(tuple_ty.clone());
                    // field 0: index
                    self.builder.push_stmt(MirStmt::Store {
                        addr: tuple_local,
                        offset: 0,
                        value: MirOperand::Local(idx),
                    });
                    // field 1: element (offset 8, aligned)
                    self.builder.push_stmt(MirStmt::Store {
                        addr: tuple_local,
                        offset: 8,
                        value: current_op,
                    });
                    current_op = MirOperand::Local(tuple_local);
                    current_ty = tuple_ty;
                }
                super::IterAdapter::Skip { .. } | super::IterAdapter::Take { .. } => {
                    // Already handled in setup (start/end bounds)
                }
            }
        }

        Ok((current_op, current_ty))
    }

    /// Emit the increment block: idx += 1, goto check
    pub(super) fn emit_iter_increment(
        &mut self,
        idx: crate::LocalId,
        inc_block: crate::BlockId,
        check_block: crate::BlockId,
    ) {
        self.builder.switch_to_block(inc_block);
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(idx),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: idx,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });
        self.builder.terminate(MirTerminator::Goto { target: check_block });
    }

    /// .collect() — fused loop that pushes each result into a new Vec.
    fn lower_iter_collect(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        // Create result Vec
        let result_vec = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Call {
            dst: Some(result_vec),
            func: FunctionRef::internal("Vec_new".to_string()),
            args: vec![],
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // Push into result Vec
        self.builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("Vec_push".to_string()),
            args: vec![MirOperand::Local(result_vec), final_op],
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result_vec), MirType::I64))
    }

    /// .fold(init, |acc, x| body) — fused loop with accumulator.
    fn lower_iter_fold(
        &mut self,
        chain: &super::IterChain<'_>,
        init: &Expr,
        closure: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let (init_op, init_ty) = self.lower_expr(init)?;
        let acc = self.builder.alloc_temp(init_ty.clone());
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(init_op),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // Inline the fold closure with two args: (acc, elem)
        if let ExprKind::Closure { params, body, .. } = &closure.kind {
            if params.len() == 2 {
                let acc_name = &params[0].name;
                let elem_name = &params[1].name;

                let saved_acc = self.locals.remove(acc_name);
                let saved_elem = self.locals.remove(elem_name);

                let acc_param = self.builder.alloc_local(acc_name.clone(), init_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: acc_param,
                    rvalue: MirRValue::Use(MirOperand::Local(acc)),
                });
                self.locals.insert(acc_name.clone(), (acc_param, init_ty.clone()));

                let elem_param = self.builder.alloc_local(elem_name.clone(), final_ty.clone());
                self.builder.push_stmt(MirStmt::Assign {
                    dst: elem_param,
                    rvalue: MirRValue::Use(final_op),
                });
                self.locals.insert(elem_name.clone(), (elem_param, final_ty));

                let (result_op, _) = self.lower_expr(body)?;
                self.builder.push_stmt(MirStmt::Assign {
                    dst: acc,
                    rvalue: MirRValue::Use(result_op),
                });

                self.locals.remove(acc_name);
                self.locals.remove(elem_name);
                if let Some(prev) = saved_acc { self.locals.insert(acc_name.clone(), prev); }
                if let Some(prev) = saved_elem { self.locals.insert(elem_name.clone(), prev); }
            }
        }

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(acc), init_ty))
    }

    /// .any(|x| pred) — fused loop, short-circuit on first true.
    fn lower_iter_any(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        });

        // Found — set result true and exit
        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// .all(|x| pred) — fused loop, short-circuit on first false.
    fn lower_iter_all(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        let result = self.builder.alloc_temp(MirType::Bool);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op, final_ty)?;
        let fail_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: setup.inc_block,
            else_block: fail_block,
        });

        // Failed — set result false and exit
        self.builder.switch_to_block(fail_block);
        self.builder.push_stmt(MirStmt::Assign {
            dst: result,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Bool))
    }

    /// .count() — fused loop counting elements that pass filters.
    fn lower_iter_count(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let counter = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let _ = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        // Increment counter
        let incremented = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: incremented,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(counter),
                right: MirOperand::Constant(MirConst::Int(1)),
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: counter,
            rvalue: MirRValue::Use(MirOperand::Local(incremented)),
        });

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(counter), MirType::I64))
    }

    /// .sum() — fused loop accumulating with Add.
    fn lower_iter_sum(
        &mut self,
        chain: &super::IterChain<'_>,
    ) -> Result<TypedOperand, LoweringError> {
        let acc = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, _) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let sum = self.builder.alloc_temp(MirType::I64);
        self.builder.push_stmt(MirStmt::Assign {
            dst: sum,
            rvalue: MirRValue::BinaryOp {
                op: crate::operand::BinOp::Add,
                left: MirOperand::Local(acc),
                right: final_op,
            },
        });
        self.builder.push_stmt(MirStmt::Assign {
            dst: acc,
            rvalue: MirRValue::Use(MirOperand::Local(sum)),
        });

        self.builder.terminate(MirTerminator::Goto { target: setup.inc_block });
        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(acc), MirType::I64))
    }

    /// .find(|x| pred) — fused loop, return Some on first match, None otherwise.
    fn lower_iter_find(
        &mut self,
        chain: &super::IterChain<'_>,
        predicate: &Expr,
    ) -> Result<TypedOperand, LoweringError> {
        // Result: Option represented as Ptr (tag 0 = Some, tag 1 = None)
        let result = self.builder.alloc_temp(MirType::Ptr);
        // Start as None (tag = 1)
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(1)),
        });

        let setup = self.setup_iter_chain_loop(chain)?;
        let (final_op, final_ty) = self.apply_iter_adapters(
            chain, MirOperand::Local(setup.elem_local), setup.elem_ty,
            setup.inc_block, setup.idx,
        )?;

        let (pred_op, _) = self.inline_closure_body(predicate, final_op.clone(), final_ty)?;
        let found_block = self.builder.create_block();
        self.builder.terminate(MirTerminator::Branch {
            cond: pred_op,
            then_block: found_block,
            else_block: setup.inc_block,
        });

        // Found — set result to Some(elem)
        self.builder.switch_to_block(found_block);
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 0,
            value: MirOperand::Constant(MirConst::Int(0)), // Some tag
        });
        self.builder.push_stmt(MirStmt::Store {
            addr: result,
            offset: 8,
            value: final_op,
        });
        self.builder.terminate(MirTerminator::Goto { target: setup.exit_block });

        self.emit_iter_increment(setup.idx, setup.inc_block, setup.check_block);
        self.builder.switch_to_block(setup.exit_block);
        Ok((MirOperand::Local(result), MirType::Ptr))
    }
}

/// Internal state for iterator chain loop setup.
pub(super) struct IterLoopSetup {
    pub(super) idx: crate::LocalId,
    pub(super) elem_local: crate::LocalId,
    pub(super) elem_ty: MirType,
    pub(super) inc_block: crate::BlockId,
    pub(super) exit_block: crate::BlockId,
    pub(super) check_block: crate::BlockId,
}
