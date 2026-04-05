// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Expression lowering.

use super::{
    binop_result_type, is_type_constructor_name, lower_binop, lower_unaryop,
    operator_method_to_binop, operator_method_to_unaryop, LoopContext, LoweringError,
    MirLowerer, TypedOperand, HANDLE_NONE_SENTINEL,
};
use crate::{
    operand::MirConst, types::{EnumLayoutId, StructLayoutId},
    BlockId, FunctionRef, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType,
};
use rask_ast::{
    expr::{Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
    token::{FloatSuffix, IntSuffix},
};

/// Detect comparison patterns in assert conditions for smart failure messages.
///
/// Returns `Some((left_expr, right_expr, op_str, is_string))` if the condition
/// is a desugared comparison. After desugar: `a == b` → `a.eq(b)`,
/// `a != b` → `!(a.eq(b))`, `a < b` → `a.lt(b)`, etc.
fn extract_assert_comparison(condition: &Expr) -> Option<(&Expr, &Expr, &'static str, bool)> {
    match &condition.kind {
        // Desugared comparison: a.eq(b), a.lt(b), etc.
        ExprKind::MethodCall { object, method, args, .. } if args.len() == 1 => {
            let op_str = match method.as_str() {
                "eq" => "==",
                "lt" => "<",
                "gt" => ">",
                "le" => "<=",
                "ge" => ">=",
                _ => return None,
            };
            // Conservative: assume i64 unless one side is obviously a string literal
            let is_string = matches!(&object.kind, ExprKind::String(_))
                || matches!(&args[0].expr.kind, ExprKind::String(_));
            Some((object.as_ref(), &args[0].expr, op_str, is_string))
        }
        // Desugared !=: !(a.eq(b))
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            if let ExprKind::MethodCall { object, method, args, .. } = &operand.kind {
                if method == "eq" && args.len() == 1 {
                    let is_string = matches!(&object.kind, ExprKind::String(_))
                        || matches!(&args[0].expr.kind, ExprKind::String(_));
                    return Some((object.as_ref(), &args[0].expr, "!=", is_string));
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract pattern name from an `is` pattern in an assert condition.
/// Returns the pattern name as a string for the failure message.
fn extract_assert_is_pattern(condition: &Expr) -> Option<String> {
    use rask_ast::expr::Pattern;
    match &condition.kind {
        ExprKind::IsPattern { pattern, .. } => {
            let name = match pattern {
                Pattern::Constructor { name, .. } => name.clone(),
                Pattern::Ident(n) => n.clone(),
                _ => return None,
            };
            Some(name)
        }
        _ => None,
    }
}

/// Resolve primitive type associated constants (e.g. i64.MAX, i32.MIN).
fn primitive_type_constant(type_name: &str, field: &str) -> Option<TypedOperand> {
    let (val, ty) = match (type_name, field) {
        ("i8", "MAX") => (i8::MAX as i64, MirType::I8),
        ("i8", "MIN") => (i8::MIN as i64, MirType::I8),
        ("i16", "MAX") => (i16::MAX as i64, MirType::I16),
        ("i16", "MIN") => (i16::MIN as i64, MirType::I16),
        ("i32", "MAX") => (i32::MAX as i64, MirType::I32),
        ("i32", "MIN") => (i32::MIN as i64, MirType::I32),
        ("i64", "MAX") => (i64::MAX, MirType::I64),
        ("i64", "MIN") => (i64::MIN, MirType::I64),
        ("u8", "MAX") => (u8::MAX as i64, MirType::U8),
        ("u16", "MAX") => (u16::MAX as i64, MirType::U16),
        ("u32", "MAX") => (u32::MAX as i64, MirType::U32),
        ("u64", "MAX") => (u64::MAX as i64, MirType::U64),
        ("u8" | "u16" | "u32" | "u64", "MIN") => (0, MirType::U64),
        _ => return None,
    };
    Some((MirOperand::Constant(MirConst::Int(val)), ty))
}

impl<'a> MirLowerer<'a> {
    /// Resolve a MirType to its named type prefix using struct/enum layouts.
    pub(super) fn mir_type_name(&self, ty: &MirType) -> Option<String> {
        match ty {
            MirType::Struct(crate::types::StructLayoutId { id, .. }) => {
                self.ctx.struct_layouts.get(*id as usize).map(|l| l.name.clone())
            }
            MirType::Enum(crate::types::EnumLayoutId { id, .. }) => {
                self.ctx.enum_layouts.get(*id as usize).map(|l| l.name.clone())
            }
            MirType::String => Some("string".to_string()),
            MirType::F64 | MirType::F32 => Some("f64".to_string()),
            MirType::Bool => Some("bool".to_string()),
            MirType::Char => Some("char".to_string()),
            _ => None,
        }
    }

    /// Emit a TraitBox instruction: heap-allocate `value` and produce a trait object.
    /// Used for both explicit `as any Trait` casts and implicit TR5 coercions.
    fn emit_trait_box(
        &mut self,
        val: MirOperand,
        concrete_mir_ty: &MirType,
        trait_name: &str,
    ) -> (MirOperand, MirType) {
        let concrete_type = self.mir_type_name(concrete_mir_ty)
            .unwrap_or_else(|| "unknown".to_string());
        let concrete_size = self.elem_size_for_type(concrete_mir_ty) as u32;
        let vtable_name = format!(".vtable.{}__{}", concrete_type, trait_name);
        let trait_obj_ty = MirType::TraitObject { trait_name: trait_name.to_string() };
        let result_local = self.builder.alloc_temp(trait_obj_ty.clone());

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::TraitBox {
            dst: result_local,
            value: val,
            concrete_type,
            trait_name: trait_name.to_string(),
            concrete_size,
            vtable_name,
        }));

        (MirOperand::Local(result_local), trait_obj_ty)
    }

    /// Derive a tracking key for Vec element type inference.
    /// Returns `"v"` for `v.push(x)` and `"self.field"` for `self.field.push(x)`.
    fn vec_tracking_key(object: &Expr) -> Option<String> {
        match &object.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object: inner, field } => {
                if let ExprKind::Ident(name) = &inner.kind {
                    Some(format!("{}.{}", name, field))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Resolve a numeric field name on a tuple type.
    /// Returns (field_index, element_type, byte_offset, field_size).
    pub(super) fn resolve_tuple_field(
        ty: &MirType,
        field: &str,
    ) -> Option<(u32, MirType, Option<u32>, Option<u32>)> {
        let fields = match ty {
            MirType::Tuple(fields) => fields,
            _ => return None,
        };
        let idx: usize = field.parse().ok()?;
        if idx >= fields.len() {
            return None;
        }
        let elem_ty = fields[idx].clone();
        let mut offset = 0u32;
        for (i, f) in fields.iter().enumerate() {
            let align = f.align();
            offset = (offset + align - 1) & !(align - 1);
            if i == idx {
                break;
            }
            offset += f.size();
        }
        let size = elem_ty.size();
        Some((idx as u32, elem_ty, Some(offset), Some(size)))
    }

    pub(super) fn lower_expr(&mut self, expr: &Expr) -> Result<TypedOperand, LoweringError> {
        self.builder.set_span(expr.span);
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
                        // Allocate a proper tagged union with tag=1 (None)
                        let option_ty = self.lookup_expr_type(expr)
                            .filter(|t| matches!(t, MirType::Option(_)))
                            .unwrap_or_else(|| MirType::Option(Box::new(MirType::I64)));
                        let result_local = self.builder.alloc_temp(option_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(1)), // tag = None
                            store_size: None,
                        }));
                        Ok((MirOperand::Local(result_local), option_ty))
                    }
                } else if let Some(meta) = self.ctx.comptime_globals.get(name) {
                    // Module-level comptime global reference
                    let global_local = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                        dst: global_local,
                        name: name.clone(),
                    }));

                    if meta.type_prefix == "Vec" {
                        // Array global: wrap raw data into a Vec
                        let vec_local = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(vec_local),
                            func: FunctionRef::internal("rask_vec_from_static".to_string()),
                            args: vec![
                                MirOperand::Local(global_local),
                                MirOperand::Constant(MirConst::Int(meta.elem_count as i64)),
                            ],
                        }));
                        self.meta_mut(&name).type_prefix = Some("Vec".to_string());
                        Ok((MirOperand::Local(vec_local), MirType::I64))
                    } else {
                        // Scalar global: load value from the data pointer
                        let mir_ty = match meta.type_prefix.as_str() {
                            "bool" => MirType::Bool,
                            "i32" => MirType::I32,
                            "i64" => MirType::I64,
                            "f32" => MirType::F32,
                            "f64" => MirType::F64,
                            _ => MirType::I64,
                        };
                        let result_local = self.builder.alloc_temp(mir_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Deref(MirOperand::Local(global_local)),
                        }));
                        Ok((MirOperand::Local(result_local), mir_ty))
                    }
                } else {
                    Err(LoweringError::UnresolvedVariable(name.clone()))
                }
            }

            ExprKind::Binary { op, left, right } => {
                let (left_op, left_ty) = self.lower_expr(left)?;
                let (right_op, _) = self.lower_expr(right)?;
                let mir_op = lower_binop(*op);
                let result_ty = binop_result_type(&mir_op, &left_ty);
                let result_local = self.builder.alloc_temp(result_ty.clone());

                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::BinaryOp {
                        op: mir_op,
                        left: left_op,
                        right: right_op,
                    },
                }));

                Ok((MirOperand::Local(result_local), result_ty))
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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue,
                }));

                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Function call — direct or through closure
            ExprKind::Call { func, args } => {
                let mut arg_operands = Vec::new();
                let mut arg_mir_types = Vec::new();
                for a in args {
                    let (op, mir_ty) = self.lower_expr(&a.expr)?;
                    // TR5: implicit trait coercion — emit TraitBox if type checker flagged this arg
                    if let Some(trait_name) = self.ctx.trait_coercions.get(&a.expr.id) {
                        let (boxed_op, _) = self.emit_trait_box(op, &mir_ty, trait_name);
                        arg_operands.push(boxed_op);
                    } else {
                        arg_operands.push(op);
                    }
                    arg_mir_types.push(mir_ty);
                }

                // Non-ident callees: field access, returned functions, etc.
                // Lower the callee expression and emit an indirect ClosureCall.
                let func_name = match &func.kind {
                    ExprKind::Ident(name) => {
                        // Check for monomorphized generic call rewrite
                        if let Some(mangled) = self.ctx.call_rewrites.get(&expr.id) {
                            mangled.clone()
                        } else {
                            name.clone()
                        }
                    }
                    _ => {
                        let (callee_op, _callee_ty) = self.lower_expr(func)?;
                        let callee_local = match callee_op {
                            MirOperand::Local(id) => id,
                            _ => {
                                let tmp = self.builder.alloc_temp(MirType::Ptr);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: tmp,
                                    rvalue: MirRValue::Use(callee_op),
                                }));
                                tmp
                            }
                        };
                        let ret_ty = self.lookup_expr_type(expr).unwrap_or(MirType::I64);
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
                            dst: Some(result_local),
                            closure: callee_local,
                            args: arg_operands,
                        }));
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
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ClosureCall {
                            dst: Some(result_local),
                            closure: closure_local,
                            args: arg_operands,
                        }));
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                }

                // transmute(val) — identity at MIR level (all values are i64)
                if func_name == "transmute" {
                    let val = arg_operands.into_iter().next()
                        .unwrap_or(MirOperand::Constant(MirConst::Int(0)));
                    return Ok((val, MirType::I64));
                }

                // todo()/unreachable() — desugar to panic() with descriptive message
                if func_name == "todo" || func_name == "unreachable" {
                    let prefix = if func_name == "todo" {
                        "not yet implemented"
                    } else {
                        "entered unreachable code"
                    };
                    let msg = if let Some(MirOperand::Constant(MirConst::String(s))) = arg_operands.first() {
                        format!("{}: {}", prefix, s)
                    } else {
                        prefix.to_string()
                    };
                    let msg_op = MirOperand::Constant(MirConst::String(msg));
                    let result_local = self.builder.alloc_temp(MirType::I64);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("panic".to_string()),
                        args: vec![msg_op],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                    let cont = self.builder.create_block();
                    self.builder.switch_to_block(cont);
                    return Ok((MirOperand::Local(result_local), MirType::I64));
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
                        // Derive the result MirType from type checker info if available.
                        // Fallback uses the payload's actual type so aggregate payloads
                        // get a correctly-sized stack slot.
                        let payload_ty = arg_mir_types.first().cloned().unwrap_or(MirType::I64);
                        let fallback_ty = if func_name == "Some" {
                            MirType::Option(Box::new(payload_ty.clone()))
                        } else if func_name == "Ok" {
                            MirType::Result {
                                ok: Box::new(payload_ty.clone()),
                                err: Box::new(MirType::I64),
                            }
                        } else {
                            // Err
                            MirType::Result {
                                ok: Box::new(MirType::I64),
                                err: Box::new(payload_ty.clone()),
                            }
                        };
                        let result_ty = self.lookup_expr_type(expr)
                            .filter(|t| match t {
                                MirType::Result { .. } => true,
                                MirType::Option(_) => true,
                                _ => false,
                            })
                            .unwrap_or(fallback_ty);
                        let result_local = self.builder.alloc_temp(result_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(tag)),
                            store_size: None,
                        }));
                        if let Some(payload) = arg_operands.first() {
                            let payload_offset = if matches!(result_ty, MirType::Result { .. }) {
                                crate::types::RESULT_PAYLOAD_OFFSET
                            } else {
                                8 // Option payload offset
                            };
                            // Result: zero origin fields
                            if matches!(result_ty, MirType::Result { .. }) {
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: crate::types::RESULT_ORIGIN_FILE_OFFSET,
                                    value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                                    store_size: None,
                                }));
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: crate::types::RESULT_ORIGIN_LINE_OFFSET,
                                    value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                                    store_size: None,
                                }));
                            }
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: payload_offset,
                                value: payload.clone(),
                                store_size: None,
                            }));
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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: func_ref,
                    args: arg_operands,
                }));

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

                // E9: .discriminant() on enum values — extract tag via EnumTag
                if method == "discriminant" && args.is_empty() {
                    let (obj_op, obj_ty) = self.lower_expr(object)?;
                    if matches!(obj_ty, MirType::Enum(_)) {
                        let result_local = self.builder.alloc_temp(MirType::U16);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::EnumTag { value: obj_op },
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::U16));
                    }
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
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                // When the object is a type name (not a local variable), intercept
                // before lowering it as a value expression.
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        // Cross-package call: pkg.func() → direct call to func
                        // Skip builtin stdlib modules — they use prefixed names
                        // (e.g. net.tcp_listen → net_tcp_listen) handled by
                        // the is_known_type path below.
                        if self.ctx.package_modules.contains(name)
                            && !super::is_type_constructor_name(name)
                        {
                            let func_name = method.clone();
                            let mut arg_operands = Vec::new();
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
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }

                        // Comptime global: TABLE.get(0) → GlobalRef + Vec_get
                        if let Some(meta) = self.ctx.comptime_globals.get(name) {
                            let type_prefix = meta.type_prefix.clone();
                            let elem_count = meta.elem_count;

                            // Load the comptime global data pointer
                            let global_local = self.builder.alloc_temp(MirType::Ptr);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
                                dst: global_local,
                                name: name.clone(),
                            }));

                            // Wrap raw data into a Vec: rask_vec_from_static(ptr, count)
                            let vec_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(vec_local),
                                func: FunctionRef::internal("rask_vec_from_static".to_string()),
                                args: vec![
                                    MirOperand::Local(global_local),
                                    MirOperand::Constant(MirConst::Int(elem_count as i64)),
                                ],
                            }));

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
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }

                        // Enum variant constructor: Shape.Circle(r)
                        // Extract layout data before mutable borrows in lower_expr
                        let enum_variant = self.ctx.find_enum(name).and_then(|(idx, layout)| {
                            let variant = layout.variants.iter().find(|v| v.name == *method)?;
                            Some((
                                idx,
                                layout.size,
                                layout.align,
                                layout.tag_offset,
                                variant.tag,
                                variant.payload_offset,
                                variant.fields.clone(),
                            ))
                        });

                        if let Some((idx, enum_size, enum_align, tag_offset, tag_val, payload_offset, fields)) =
                            enum_variant
                        {
                            let enum_ty = MirType::Enum(EnumLayoutId::new(idx, enum_size, enum_align));
                            let result_local = self.builder.alloc_temp(enum_ty.clone());

                            // Store discriminant tag
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: tag_offset,
                                value: MirOperand::Constant(MirConst::Int(tag_val as i64)),
                                store_size: None,
                            }));

                            // Store payload fields
                            for (i, arg) in args.iter().enumerate() {
                                let (val, _) = self.lower_expr(&arg.expr)?;
                                let offset = if i < fields.len() {
                                    payload_offset + fields[i].offset
                                } else {
                                    payload_offset + (i as u32 * 8)
                                };
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset,
                                    value: val,
                                    store_size: None,
                                }));
                            }

                            return Ok((MirOperand::Local(result_local), enum_ty));
                        }

                        // .variants() on enum types: build a Vec of tag values
                        if method == "variants" && args.is_empty() {
                            if let Some((_idx, layout)) = self.ctx.find_enum(name) {
                                // Create a new Vec
                                let vec_local = self.builder.alloc_temp(MirType::I64);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                    dst: Some(vec_local),
                                    func: FunctionRef::internal("Vec_new".to_string()),
                                    args: vec![],
                                }));
                                // Push each variant's tag value
                                for variant in &layout.variants {
                                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                        dst: None,
                                        func: FunctionRef::internal("Vec_push".to_string()),
                                        args: vec![
                                            MirOperand::Local(vec_local),
                                            MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                        ],
                                    }));
                                }
                                return Ok((MirOperand::Local(vec_local), MirType::I64));
                            }
                        }

                        // json.encode — expand struct/vec/primitive serialization at MIR level
                        if name == "json" && method == "encode" && args.len() == 1 {
                            let (arg_op, arg_ty) = self.lower_expr(&args[0].expr)?;
                            if let MirType::Struct(StructLayoutId { id, .. }) = &arg_ty {
                                if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                                    return self.lower_json_encode_struct(arg_op, layout.clone());
                                }
                            }

                            // Vec<T>: generate loop that encodes each element.
                            // Detection: check type checker first, fall back to local_meta type_prefix.
                            let raw_ty = self.ctx.lookup_raw_type(args[0].expr.id);
                            let is_vec_from_type = raw_ty.map_or(false, |ty| {
                                matches!(ty,
                                    rask_types::Type::UnresolvedGeneric { name, .. } if name == "Vec"
                                ) || matches!(ty, rask_types::Type::UnresolvedNamed(n) if n == "Vec")
                            });
                            let is_vec_from_prefix = if !is_vec_from_type {
                                if let ExprKind::Ident(var_name) = &args[0].expr.kind {
                                    self.meta(var_name)
                                        .and_then(|m| m.type_prefix.as_deref())
                                        .map(|p| p == "Vec")
                                        .unwrap_or(false)
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            if is_vec_from_type || is_vec_from_prefix {
                                // Extract element type from generic args when available
                                let elem_ty = raw_ty.and_then(|ty| match ty {
                                    rask_types::Type::UnresolvedGeneric { args: ga, .. } => {
                                        ga.first().and_then(|a| match a {
                                            rask_types::GenericArg::Type(t) => Some(t.as_ref().clone()),
                                            _ => None,
                                        })
                                    }
                                    _ => None,
                                });
                                return self.lower_json_encode_vec(arg_op, elem_ty);
                            }

                            // Non-struct: string or integer
                            let helper = if matches!(arg_ty, MirType::String) {
                                "json_encode_string"
                            } else {
                                "json_encode_i64"
                            };
                            let result_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(helper.to_string()),
                                args: vec![arg_op],
                            }));
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
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal("json_decode".to_string()),
                                args: vec![str_op],
                            }));
                            return Ok((MirOperand::Local(result_local), MirType::I64));
                        }

                        // Vec.from([...]) → stack array + rask_vec_from_static(ptr, count)
                        // Map.from([("k", "v"), ...]) → Map.new() + Map.insert() per pair
                        {
                            let base = name.split('<').next().unwrap_or(name);
                            if base == "Vec" && method == "from" && args.len() == 1 {
                                if let ExprKind::Array(elems) = &args[0].expr.kind {
                                    return self.lower_vec_from_array(elems);
                                }
                            }
                            if base == "Map" && method == "from" && args.len() == 1 {
                                if let ExprKind::Array(elems) = &args[0].expr.kind {
                                    return self.lower_map_from_pairs(elems);
                                }
                            }
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

                            // Inject elem_size/data_size for generic constructors.
                            // The C runtime needs actual sizes for struct types;
                            // the dispatch table expects these as extra arguments.
                            if (base_name == "Channel" && (method == "buffered" || method == "unbuffered"))
                                || ((base_name == "Shared" || base_name == "Mutex") && method == "new")
                            {
                                let elem_size = self.generic_inner_struct_size(name);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                if base_name == "Channel" {
                                    // Channel: elem_size goes first → (elem_size, capacity)
                                    arg_operands.insert(0, size_op);
                                } else {
                                    // Shared: data_size goes last → (data_ptr, data_size)
                                    arg_operands.push(size_op);
                                }
                            }
                            // Pool.new(): inject elem_size so pool allocates
                            // correctly-sized slots for struct elements.
                            if base_name == "Pool" && method == "new" {
                                let elem_size = self.generic_inner_struct_size(name);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                arg_operands.insert(0, size_op);
                            }

                            // Vec.new(): inject elem_size so runtime allocates correct slots.
                            // string elements need 16 bytes; structs use layout size; default 8.
                            if base_name == "Vec" && method == "new" {
                                let elem_size = self.generic_type_param_size(name, 0);
                                let size_op = MirOperand::Constant(MirConst::Int(elem_size));
                                arg_operands.insert(0, size_op);
                            }
                            // Map.new(): inject key_size, val_size
                            if (base_name == "Map") && method == "new" {
                                let key_size = self.generic_type_param_size(name, 0);
                                let val_size = self.generic_type_param_size(name, 1);
                                arg_operands.insert(0, MirOperand::Constant(MirConst::Int(key_size)));
                                arg_operands.insert(1, MirOperand::Constant(MirConst::Int(val_size)));
                            }

                            // Map.new() with string keys → use string hash/eq
                            let func_name = if func_name == "Map_new" {
                                let has_string_keys = self.ctx.lookup_raw_type(expr.id)
                                    .map(|ty| {
                                        let s = format!("{:?}", ty);
                                        s.contains("Map") && s.contains("String")
                                    })
                                    .unwrap_or(false);
                                if has_string_keys {
                                    "Map_new_string_keys".to_string()
                                } else {
                                    func_name
                                }
                            } else {
                                func_name
                            };

                            let ret_ty = self
                                .func_sigs
                                .get(&func_name)
                                .map(|s| s.ret_ty.clone())
                                .unwrap_or_else(|| super::stdlib_return_mir_type(&func_name));
                            // Channel.buffered()/unbuffered() C runtime returns a
                            // single i64 (raw channel pair pointer), not a tuple.
                            // Override the Tuple return type from stubs to I64 so the
                            // codegen allocates a register, not a stack slot. The
                            // tuple destructure emits channel_tx/channel_rx calls.
                            let ret_ty = if base_name == "Channel"
                                && (method == "buffered" || method == "unbuffered")
                            {
                                MirType::I64
                            } else {
                                ret_ty
                            };
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(result_local),
                                func: FunctionRef::internal(func_name),
                                args: arg_operands,
                            }));

                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;

                // Raw pointer methods: dispatch directly to RawPtr_* C functions.
                // Skip for smart pointer types (Shared, Channel, etc.) that also use MirType::Ptr.
                let is_smart_ptr = self.ctx.lookup_raw_type(object.id)
                    .and_then(|ty| super::MirContext::stdlib_type_prefix(ty))
                    .map(|prefix| matches!(prefix, "Shared" | "Mutex" | "Channel" | "Sender" | "Receiver"))
                    .unwrap_or(false)
                    || if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name)
                            .and_then(|m| m.type_prefix.as_deref())
                            .map(|p| matches!(p, "Shared" | "Mutex" | "Channel" | "Sender" | "Receiver"))
                            .unwrap_or(false)
                    } else {
                        false
                    };
                if matches!(obj_ty, MirType::Ptr) && !is_smart_ptr {
                    let ptr_method = match method.as_str() {
                        "read" | "write" | "add" | "sub" | "offset"
                        | "is_null" | "is_aligned" | "is_aligned_to" | "align_offset" => {
                            Some(format!("RawPtr_{}", method))
                        }
                        "cast" => None, // cast is type-only, no runtime call
                        _ => None,
                    };
                    if method == "cast" {
                        // Cast is a no-op at runtime — pointer value unchanged
                        return Ok((obj_op, MirType::Ptr));
                    }
                    if let Some(func_name) = ptr_method {
                        // Determine element size from the pointer's type (*u8 → 1, *i64 → 8)
                        let elem_size: i64 = self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| match ty {
                                rask_types::Type::RawPtr(inner) => Some(match inner.as_ref() {
                                    rask_types::Type::U8 | rask_types::Type::I8 | rask_types::Type::Bool => 1,
                                    rask_types::Type::U16 | rask_types::Type::I16 => 2,
                                    rask_types::Type::U32 | rask_types::Type::I32 | rask_types::Type::F32 => 4,
                                    _ => 8,
                                }),
                                _ => None,
                            })
                            .unwrap_or(8);

                        let mut all_args = vec![obj_op];
                        for arg in args {
                            let (op, _) = self.lower_expr(&arg.expr)?;
                            all_args.push(op);
                        }
                        // Inject element size for read/write/add/sub/offset
                        if matches!(method.as_str(), "read" | "write" | "add" | "sub" | "offset") {
                            all_args.push(MirOperand::Constant(crate::operand::MirConst::Int(elem_size)));
                        }
                        let ret_ty = match method.as_str() {
                            "read" => MirType::I64,
                            "write" => MirType::Void,
                            "add" | "sub" | "offset" => MirType::Ptr,
                            "is_null" | "is_aligned" | "is_aligned_to" => MirType::Bool,
                            "align_offset" => MirType::I64,
                            _ => MirType::I64,
                        };
                        let result_local = self.builder.alloc_temp(ret_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal(func_name),
                            args: all_args,
                        }));
                        return Ok((MirOperand::Local(result_local), ret_ty));
                    }
                }

                // Skip native binop for types that need C runtime calls (strings,
                // SIMD vectors) or special method dispatch (raw pointers:
                // ptr.add != arithmetic add).
                // When obj_ty is Ptr (type info lost), check the type checker to
                // see if the actual type is numeric — if so, use native binop.
                let raw_type_is_numeric = self.ctx.lookup_raw_type(object.id)
                    .map(|ty| matches!(ty,
                        rask_types::Type::I8 | rask_types::Type::I16 | rask_types::Type::I32 | rask_types::Type::I64
                        | rask_types::Type::U8 | rask_types::Type::U16 | rask_types::Type::U32 | rask_types::Type::U64
                        | rask_types::Type::F32 | rask_types::Type::F64 | rask_types::Type::Bool
                    ))
                    .unwrap_or(false);
                let skip_binop = if raw_type_is_numeric {
                    false
                } else {
                    matches!(obj_ty, MirType::String)
                    || if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name)
                            .and_then(|m| m.type_prefix.as_deref())
                            .map(|p| matches!(p, "string" | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8" | "Ptr"))
                            .unwrap_or(false)
                    } else {
                        // Unknown type from complex expression — default to native
                        // binop. The common case is numeric field access chains
                        // (e.g. self.entries.len() / 2) where Ptr means lost type info.
                        false
                    }
                };

                // Detect binary operator methods (desugared from a + b → a.add(b))
                // Skip for SIMD types and raw pointers — they use method dispatch.
                if !skip_binop {
                if let Some(mir_binop) = operator_method_to_binop(method) {
                    if args.len() == 1 {
                        let (rhs, _) = self.lower_expr(&args[0].expr)?;
                        let result_ty = binop_result_type(&mir_binop, &obj_ty);
                        let result_local = self.builder.alloc_temp(result_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::BinaryOp {
                                op: mir_binop,
                                left: obj_op,
                                right: rhs,
                            },
                        }));
                        return Ok((MirOperand::Local(result_local), result_ty));
                    }
                }

                // Detect unary operator methods (desugared from -a → a.neg())
                if let Some(mir_unop) = operator_method_to_unaryop(method) {
                    if args.is_empty() {
                        let result_local = self.builder.alloc_temp(obj_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::UnaryOp {
                                op: mir_unop,
                                operand: obj_op,
                            },
                        }));
                        return Ok((MirOperand::Local(result_local), obj_ty));
                    }
                }
                } // end if !skip_binop

                // String comparison operators: route to string_lt, string_ge, etc.
                let is_string_obj = matches!(obj_ty, MirType::String) || self.ctx.lookup_raw_type(object.id)
                    .map(|ty| matches!(ty, rask_types::Type::String))
                    .unwrap_or(false);
                if is_string_obj && args.len() == 1 {
                    let string_cmp_fn = match method.as_str() {
                        "eq" => Some("string_eq"),
                        "lt" => Some("string_lt"),
                        "gt" => Some("string_gt"),
                        "le" => Some("string_le"),
                        "ge" => Some("string_ge"),
                        "compare" => Some("string_compare"),
                        _ => None,
                    };
                    if let Some(func_name) = string_cmp_fn {
                        let (rhs, _) = self.lower_expr(&args[0].expr)?;
                        let result_local = self.builder.alloc_temp(MirType::Bool);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal(func_name.to_string()),
                            args: vec![obj_op, rhs],
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::Bool));
                    }
                }

                // concat(): string concatenation from interpolation
                if method == "concat" && args.len() == 1 && matches!(obj_ty, MirType::String) {
                    let (arg_op, _) = self.lower_expr(&args[0].expr)?;
                    let result_local = self.builder.alloc_temp(MirType::String);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("concat".to_string()),
                        args: vec![obj_op, arg_op],
                    }));
                    return Ok((MirOperand::Local(result_local), MirType::String));
                }

                // to_string(): route to type-specific runtime function.
                // Types with their own to_string in stdlib dispatch (Path, etc.)
                // fall through to normal method dispatch.
                if method == "to_string" && args.is_empty() {
                    // Check if the type checker knows this is a type with its own to_string
                    let has_own_to_string = self.ctx.lookup_raw_type(object.id)
                        .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
                        .map(|prefix| {
                            let qualified = format!("{}_to_string", prefix);
                            rask_stdlib::mir_metadata::lookup(&qualified).is_some()
                        })
                        .unwrap_or(false);

                    if !has_own_to_string {
                        let func_name = match &obj_ty {
                            MirType::String => {
                                return Ok((obj_op, MirType::String));
                            }
                            MirType::I64 | MirType::I32 | MirType::I16 | MirType::I8
                            | MirType::U64 | MirType::U32 | MirType::U16 | MirType::U8 => "i64_to_string",
                            MirType::F64 | MirType::F32 => "f64_to_string",
                            MirType::Bool => "bool_to_string",
                            MirType::Char => "char_to_string",
                            _ => "i64_to_string",
                        };
                        let result_local = self.builder.alloc_temp(MirType::String);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal(func_name.to_string()),
                            args: vec![obj_op],
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::String));
                    }
                }

                // map_err: inline expansion — branch on tag, transform error payload
                if method == "map_err" && args.len() == 1 {
                    if matches!(&args[0].expr.kind, ExprKind::Closure { params, .. } if params.len() == 1) {
                        return self.lower_map_err(obj_op, &obj_ty, &args[0].expr);
                    }
                    // Variant constructor: result.map_err(MyError) or
                    // result.map_err(ConfigError.Io)
                    if let ExprKind::Ident(name) = &args[0].expr.kind {
                        return self.lower_map_err_constructor(obj_op, &obj_ty, name);
                    }
                    // Qualified variant: EnumName.Variant
                    if let ExprKind::Field { object, field } = &args[0].expr.kind {
                        if matches!(&object.kind, ExprKind::Ident(_)) {
                            return self.lower_map_err_constructor(obj_op, &obj_ty, field);
                        }
                    }
                }

                // .ok() / .to_option(): Result<T,E> → Option<T>
                // Pass through as-is — runtime uses the same tagged-union layout
                // (tag 0 = Ok/Some, tag 1 = Err/None).
                if (method == "ok" || method == "to_option") && args.is_empty() {
                    return Ok((obj_op, obj_ty));
                }

                // .unwrap(): Option<T>/Result<T,E> → T — panic on None/Err
                // Special case: .get(i).unwrap() on collections.
                // Vec_get panics on OOB → unwrap is a no-op.
                // Map_get returns NULL on missing key → rewrite to Map_get_unwrap.
                if method == "unwrap" && args.is_empty() {
                    if let ExprKind::MethodCall { method: inner_method, object: inner_obj, .. } = &object.kind {
                        if inner_method == "get" {
                            // Only rewrite Map_get → Map_get_unwrap, not Pool_get
                            let is_map = if let ExprKind::Ident(name) = &inner_obj.kind {
                                self.meta(name.as_str())
                                    .and_then(|m| m.type_prefix.as_deref())
                                    .map_or(false, |p| p == "Map")
                            } else { false };
                            if is_map {
                                self.builder.rewrite_last_call("Map_get", "Map_get_unwrap");
                                return Ok((obj_op, obj_ty));
                            }
                        }
                    }
                }
                if method == "unwrap" && args.is_empty() {
                    let is_niche = self.is_niche_option_expr(object);
                    let tag_local = self.emit_option_tag(&obj_op, is_niche);

                    let ok_block = self.builder.create_block();
                    let panic_block = self.builder.create_block();
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(tag_local),
                        then_block: panic_block,
                        else_block: ok_block,
                    }));

                    self.builder.switch_to_block(panic_block);

                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("panic_unwrap".to_string()),
                        args: vec![],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    let payload_ty = self.extract_payload_type(object)
                        .unwrap_or(MirType::I64);
                    let result_local = self.emit_option_payload(obj_op, payload_ty.clone(), is_niche);
                    return Ok((MirOperand::Local(result_local), payload_ty));
                }

                // .clone(): dispatch to type-specific clone (Vec_clone, string_clone, etc.)
                // Value types (integers, bools) fall through to generic rask_clone.
                // Heap types (Vec, Map, string) need deep copy via their runtime functions.

                // Array.len() → compile-time constant (no runtime call)
                if method == "len" && args.is_empty() {
                    if let MirType::Array { len, .. } = &obj_ty {
                        return Ok((
                            MirOperand::Constant(MirConst::Int(*len as i64)),
                            MirType::I64,
                        ));
                    }
                }

                // Trait object dispatch: method call on `any Trait`
                if let MirType::TraitObject { ref trait_name } = obj_ty {
                    if let Some(methods) = self.ctx.trait_methods.get(trait_name) {
                        if let Some(idx) = methods.iter().position(|m| m == method) {
                            let vtable_offset = 24 + (idx as u32) * 8;
                            let mut arg_operands = Vec::new();
                            for arg in args {
                                let (op, _) = self.lower_expr(&arg.expr)?;
                                arg_operands.push(op);
                            }
                            // Resolve return type from type checker or fall back to i64
                            let ret_ty = self.ctx.lookup_raw_type(expr.id)
                                .map(|t| self.ctx.type_to_mir(t))
                                .unwrap_or(MirType::I64);
                            let result_local = self.builder.alloc_temp(ret_ty.clone());
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::TraitCall {
                                dst: Some(result_local),
                                trait_object: match &obj_op {
                                    MirOperand::Local(id) => *id,
                                    _ => return Err(LoweringError::InvalidConstruct(
                                        "trait object must be a local variable".to_string()
                                    )),
                                },
                                method_name: method.clone(),
                                vtable_offset,
                                args: arg_operands,
                            }));
                            return Ok((MirOperand::Local(result_local), ret_ty));
                        }
                    }
                }

                // Generic method: append type arg to name (e.g. parse<i32> → parse_i32)
                let method = if let Some(ta) = type_args {
                    if let Some(ty_name) = ta.first() {
                        format!("{}_{}", method, ty_name)
                    } else {
                        method.clone()
                    }
                } else {
                    method.clone()
                };

                // Regular method call
                let mut all_args = vec![obj_op];
                let mut arg_types = Vec::new();
                for arg in args {
                    let (op, ty) = self.lower_expr(&arg.expr)?;
                    all_args.push(op);
                    arg_types.push(ty);
                }

                // Qualify method name with receiver type to avoid dispatch
                // ambiguity (e.g. Vec.get vs Map.get vs Pool.get).
                // Check local_meta type_prefix first (tracks actual codegen types),
                // then fall back to type-checker info (handles both stdlib
                // and user-defined types from extend blocks).
                let qualified_name = if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name).and_then(|m| m.type_prefix.clone())
                    } else {
                        None
                    }
                    // Field access on struct: resolve field type from struct layout
                    .or_else(|| {
                        if let ExprKind::Field { object: inner_obj, field: field_name } = &object.kind {
                            if let ExprKind::Ident(var_name) = &inner_obj.kind {
                                if let Some((local_id, _)) = self.locals.get(var_name) {
                                    let local_ty = self.builder.local_type(*local_id);
                                    if let Some(MirType::Struct(StructLayoutId { id, .. })) = local_ty {
                                        if let Some(layout) = self.ctx.struct_layouts.get(id as usize) {
                                            if let Some(fl) = layout.fields.iter().find(|f| f.name == *field_name) {
                                                return super::MirContext::type_prefix(&fl.ty, self.ctx.type_names);
                                            }
                                        }
                                    }
                                }
                            }
                            None
                        } else {
                            None
                        }
                    })
                    // Unambiguous method names — each belongs to exactly one type.
                    // Checked early because type-checker info can be wrong for
                    // unresolved types (e.g. tuple destructures from stdlib methods).
                    // Only methods unique to a single type belong here.
                    .or_else(|| match method.as_str() {
                        // String (unique to string — Vec/Map don't have these)
                        "contains" | "starts_with" | "ends_with" | "trim"
                        | "to_lowercase" | "to_uppercase" | "replace"
                        | "substr" | "substring" | "repeat" | "reverse"
                        | "lines" | "split" | "split_whitespace"
                        | "char_at" | "index_of"
                        | "chars" | "push_str" | "push_char"
                        | "compare"
                        | "as_c_str" | "as_ptr" => Some("string".to_string()),
                        // Vec / iterator (no other Rask type has these)
                        "push" | "remove_at" | "to_vec" | "chunks" | "skip"
                        | "map" | "filter" | "collect"
                        | "enumerate" | "any" | "all" | "find" | "fold"
                        | "for_each" | "flat_map" | "take" | "zip"
                        | "sort_by" | "sort" | "dedup"
                        | "first" | "last" => Some("Vec".to_string()),
                        "join" if all_args.len() == 2 => Some("Vec".to_string()),
                        // Map (Vec uses index syntax for element access, so .get()/.insert()/.remove()
                        // as method calls are effectively Map-only in practice)
                        "values" | "keys" | "contains_key"
                        | "get" | "insert" | "remove" => Some("Map".to_string()),
                        // Path (unique to Path — Vec/Map/string don't have these)
                        "parent" | "file_name" | "extension" | "stem"
                        | "components" | "is_absolute" | "is_relative"
                        | "has_extension" | "with_extension" | "with_file_name"
                            => Some("Path".to_string()),
                        // Time
                        "now" | "elapsed" | "duration_since" => Some("Instant".to_string()),
                        "as_secs_f64" | "as_secs_f32" | "as_secs" | "as_millis" | "as_micros" | "as_nanos"
                        | "seconds" | "millis" | "micros" | "nanos" | "from_secs_f64" => Some("Duration".to_string()),
                        "sleep" => Some("time".to_string()),
                        // Net/HTTP
                        "read_http_request" | "write_http_response"
                        | "read_all" | "write_all" | "remote_addr"
                            => Some("TcpConnection".to_string()),
                        "accept" => Some("TcpListener".to_string()),
                        "respond" => Some("Responder".to_string()),
                        // Metadata
                        "size" | "accessed" | "modified" => Some("Metadata".to_string()),
                        // Args
                        "flag" | "option" | "option_or" | "positional" | "program"
                            => Some("Args".to_string()),
                        // TaskHandle/ThreadHandle
                        "cancel" | "detach" => Some("TaskHandle".to_string()),
                        _ => None,
                    })
                    .or_else(|| {
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
                    })
                    // Fallback: derive prefix from MIR type (catches F64, String, etc.)
                    .or_else(|| super::mir_type_method_prefix(&obj_ty).map(|s| s.to_string()))
                    // parse<T> always belongs to string (structural, not type-prefix related)
                    .or_else(|| if method.starts_with("parse_") { Some("string".to_string()) } else { None })
                    .map(|prefix| format!("{}_{}", prefix, method))
                    .unwrap_or_else(|| {
                        eprintln!(
                            "[mir] method `{}` has no type prefix — type checker should have resolved this",
                            method
                        );
                        method.clone()
                    });

                // If still unqualified, search func_sigs for a matching *_method entry
                let qualified_name = if qualified_name == method {
                    let suffix = format!("_{}", method);
                    self.func_sigs.keys()
                        .find(|k| k.ends_with(&suffix))
                        .cloned()
                        .unwrap_or(qualified_name)
                } else {
                    qualified_name
                };

                // Last resort: unqualified operator methods on strings.
                // When qualification fails for lt/gt/le/ge/compare/push/push_str
                // and the obj_ty is String, prefix with "string_".
                let qualified_name = if qualified_name == method && matches!(obj_ty, MirType::String) {
                    format!("string_{}", method)
                } else {
                    qualified_name
                };

                // Track collection element types from push/insert so get returns the right type.
                // Handles both `v.push(x)` and `self.field.push(x)`.
                // Writes to both per-function and shared cross-function maps.
                if matches!(qualified_name.as_str(), "Vec_push" | "Vec_set" | "Pool_insert") {
                    if let Some(arg_ty) = arg_types.first() {
                        if !matches!(arg_ty, MirType::I64) {
                            if let Some(key) = Self::vec_tracking_key(object) {
                                self.meta_mut(&key).elem_type = Some(arg_ty.clone());
                                self.ctx.shared_elem_types.borrow_mut().insert(key, arg_ty.clone());
                            }
                        }
                    }
                }

                // Channel recv with struct elements: switch to struct variant
                // and inject elem_size so the builder can allocate the right buffer.
                let qualified_name = if qualified_name == "Receiver_recv" {
                    let elem_size = if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name).and_then(|m| m.channel_elem_size).unwrap_or(8)
                    } else {
                        8
                    };
                    if elem_size > 8 {
                        all_args.push(MirOperand::Constant(MirConst::Int(elem_size)));
                        "Receiver_recv_struct".to_string()
                    } else {
                        qualified_name
                    }
                } else {
                    qualified_name
                };

                // Use tracked element type for Vec_get return instead of default I64.
                // Checks per-function map first, then shared cross-function map.
                let ret_ty = if matches!(qualified_name.as_str(), "Vec_get" | "Vec_index") {
                    Self::vec_tracking_key(object)
                        .and_then(|key| {
                            self.meta(&key).and_then(|m| m.elem_type.clone())
                                .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
                        })
                } else if qualified_name == "Pool_get" {
                    // Pool.get returns Option<T> — extract T from tracked element type
                    let elem_ty = Self::vec_tracking_key(object)
                        .and_then(|key| {
                            self.meta(&key).and_then(|m| m.elem_type.clone())
                                .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
                        })
                        // Fallback: extract from Pool<T> generic parameter
                        .or_else(|| {
                            self.ctx.lookup_raw_type(object.id)
                                .and_then(|ty| match ty {
                                    rask_types::Type::UnresolvedGeneric { args, .. } => {
                                        args.first().and_then(|a| match a {
                                            rask_types::GenericArg::Type(t) => Some(t.as_ref()),
                                            _ => None,
                                        })
                                    }
                                    _ => None,
                                })
                                .map(|elem_ty| self.ctx.type_to_mir(elem_ty))
                                .filter(|t| !matches!(t, MirType::Ptr))
                        })
                        .unwrap_or(MirType::I64);
                    Some(MirType::Option(Box::new(elem_ty)))
                } else {
                    None
                }.unwrap_or_else(|| self
                    .func_sigs
                    .get(&method)
                    .or_else(|| self.func_sigs.get(&qualified_name))
                    .map(|s| s.ret_ty.clone())
                    .unwrap_or_else(|| super::stdlib_return_mir_type(&qualified_name)));

                // Struct clone: inline field-by-field copy with deep clone for
                // heap fields (string, Vec, Map). Avoids needing a generated
                // runtime clone function for every user struct.
                if method == "clone" {
                    if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                        if let Some(layout) = self.ctx.struct_layouts.get(*id as usize).cloned() {
                            let result_local = self.builder.alloc_temp(obj_ty.clone());
                            let src = all_args[0].clone();
                            for field in &layout.fields {
                                let field_val = self.builder.alloc_temp(MirType::I64);
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: field_val,
                                    rvalue: MirRValue::Field {
                                        base: src.clone(),
                                        field_index: field.offset,
                                        byte_offset: None,
                                        field_size: None,
                                    },
                                }));
                                // Deep clone heap types
                                let clone_fn = Self::clone_fn_for_type(&field.ty);
                                let store_val = if let Some(cfn) = clone_fn {
                                    let cloned = self.builder.alloc_temp(MirType::I64);
                                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                        dst: Some(cloned),
                                        func: FunctionRef::internal(cfn.to_string()),
                                        args: vec![MirOperand::Local(field_val)],
                                    }));
                                    MirOperand::Local(cloned)
                                } else {
                                    MirOperand::Local(field_val)
                                };
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: field.offset,
                                    value: store_val,
                                    store_size: None,
                                }));
                            }
                            return Ok((MirOperand::Local(result_local), obj_ty));
                        }
                    }
                    // Enum clone: copy tag, then switch on tag to deep-clone
                    // heap fields per variant.
                    if let MirType::Enum(EnumLayoutId { id, .. }) = &obj_ty {
                        if let Some(layout) = self.ctx.enum_layouts.get(*id as usize).cloned() {
                            return self.lower_enum_clone(&layout, &all_args[0], obj_ty);
                        }
                    }
                }

                // Pool.alloc(value) → Pool_insert(pool, elem_ptr)
                // Pool_alloc takes no element arg; codegen Pool_insert appends elem_size
                let (final_name, final_args) = if qualified_name == "Pool_alloc" && all_args.len() == 2 {
                    ("Pool_insert".to_string(), all_args)
                } else if qualified_name == "Vec_join" {
                    // Vec_join assumes Vec<string>; use Vec_join_i64 for non-string elements
                    let is_string = Self::vec_tracking_key(object)
                        .and_then(|key| self.meta(&key).and_then(|m| m.elem_type.clone())
                            .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned()))
                        .map_or(false, |ty| matches!(ty, MirType::String));
                    if is_string {
                        (qualified_name.clone(), all_args)
                    } else {
                        ("Vec_join_i64".to_string(), all_args)
                    }
                } else {
                    (qualified_name.clone(), all_args)
                };

                let result_local = self.builder.alloc_temp(ret_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(final_name.clone()),
                    args: final_args,
                }));

                // W2a/W2b: Re-resolve pool bindings after pool mutators inside `with` blocks
                if matches!(final_name.as_str(),
                    "Pool_insert" | "Pool_remove" | "Pool_clear" | "Pool_drain" | "Pool_alloc"
                ) {
                    if let ExprKind::Ident(pool_var) = &object.kind {
                        if let Some(bindings) = self.with_pool_bindings.get(pool_var) {
                            for &(handle_local, binding_local, pool_local) in bindings {
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
                                    dst: binding_local,
                                    pool: pool_local,
                                    handle: handle_local,
                                }));
                            }
                        }
                    }
                }

                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // Field access
            ExprKind::Field { object, field } => {
                // Primitive type constants: i64.MAX, i32.MIN, etc.
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(val) = primitive_type_constant(name, field) {
                        return Ok(val);
                    }
                }

                // Cross-package type access: pkg.Type → treat field as the type name.
                // Subsequent field access (pkg.DbError.NotFound) chains through
                // enum variant resolution on the resolved type.
                if let ExprKind::Ident(name) = &object.kind {
                    if self.ctx.package_modules.contains(name) {
                        // Look up the field as an enum type
                        if let Some((idx, layout)) = self.ctx.find_enum(field) {
                            let enum_ty = MirType::Enum(EnumLayoutId::new(idx, layout.size, layout.align));
                            let result_local = self.builder.alloc_temp(enum_ty.clone());
                            // Default-initialize (tag 0) — caller will likely access a variant
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                addr: result_local,
                                offset: layout.tag_offset,
                                value: MirOperand::Constant(MirConst::Int(0)),
                                store_size: None,
                            }));
                            return Ok((MirOperand::Local(result_local), enum_ty));
                        }
                        // Look up as a struct type
                        if let Some((idx, sl)) = self.ctx.find_struct(field) {
                            let struct_ty = MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align));
                            let result_local = self.builder.alloc_temp(struct_ty.clone());
                            return Ok((MirOperand::Local(result_local), struct_ty));
                        }
                        // Fallback: treat as an opaque type reference
                        let result_local = self.builder.alloc_temp(MirType::I64);
                        return Ok((MirOperand::Local(result_local), MirType::I64));
                    }
                }

                // Enum variant access: Color.Red (no parens, fieldless variant)
                if let ExprKind::Ident(name) = &object.kind {
                    if !self.locals.contains_key(name) {
                        if let Some((idx, layout)) = self.ctx.find_enum(name) {
                            if let Some(variant) = layout.variants.iter().find(|v| v.name == *field) {
                                let enum_ty = MirType::Enum(EnumLayoutId::new(idx, layout.size, layout.align));
                                let result_local = self.builder.alloc_temp(enum_ty.clone());
                                // Store discriminant tag
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: result_local,
                                    offset: layout.tag_offset,
                                    value: MirOperand::Constant(MirConst::Int(variant.tag as i64)),
                                    store_size: None,
                                }));
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

                // Resolve field index, type, and byte offset from struct layout.
                // byte_offset is passed to codegen so it doesn't need to re-derive
                // the offset (which would require knowing the struct type).
                let (field_index, result_ty, byte_offset, field_size) = if let MirType::Struct(StructLayoutId { id, .. }) = &obj_ty {
                    if let Some(layout) = self.ctx.struct_layouts.get(*id as usize) {
                        if let Some((idx, fl)) = layout.fields.iter().enumerate()
                            .find(|(_, f)| f.name == *field)
                        {
                            // Resolve field type from layout; if generic/unresolved,
                            // prefer the type checker's type for this expression.
                            let mut ft = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                            if matches!(ft, MirType::Ptr | MirType::I64) {
                                if let Some(raw) = self.ctx.lookup_raw_type(expr.id) {
                                    let tc_ty = self.ctx.type_to_mir(raw);
                                    if !matches!(tc_ty, MirType::Ptr) {
                                        ft = tc_ty;
                                    }
                                }
                            }
                            (idx as u32, ft, Some(fl.offset), Some(fl.size))
                        } else {
                            (0, MirType::I64, None, None)
                        }
                    } else {
                        (0, MirType::I64, None, None)
                    }
                } else if let Some(resolved) = Self::resolve_tuple_field(&obj_ty, field) {
                    resolved
                } else {
                    // Object isn't MirType::Struct — try the type checker to
                    // resolve struct info (e.g. pool[h] returns Ptr but the
                    // type checker knows it's a struct).
                    let mut resolved = false;
                    let mut fi = 0u32;
                    let mut rt = MirType::I64;
                    let mut bo: Option<u32> = None;
                    let mut fs: Option<u32> = None;

                    // Strategy 1: Check type checker's node_types for the object
                    if let Some(raw_ty) = self.ctx.lookup_raw_type(object.id) {
                        let obj_mir = self.ctx.type_to_mir(raw_ty);
                        if let MirType::Struct(StructLayoutId { id: sid, .. }) = &obj_mir {
                            if let Some(layout) = self.ctx.struct_layouts.get(*sid as usize) {
                                if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                    .find(|(_, f)| f.name == *field)
                                {
                                    fi = idx as u32;
                                    rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                    bo = Some(fl.offset);
                                    fs = Some(fl.size);
                                    resolved = true;
                                }
                            }
                        } else if let Some(tuple_resolved) = Self::resolve_tuple_field(&obj_mir, field) {
                            return {
                                let (ti, trt, tbo, tfs) = tuple_resolved;
                                let result_local = self.builder.alloc_temp(trt.clone());
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: result_local,
                                    rvalue: MirRValue::Field {
                                        base: obj_op,
                                        field_index: ti,
                                        byte_offset: tbo,
                                        field_size: tfs,
                                    },
                                }));
                                Ok((MirOperand::Local(result_local), trt))
                            };
                        }
                    }

                    // Strategy 2: If object is a variable, check its MIR local type
                    if !resolved {
                        if let ExprKind::Ident(var_name) = &object.kind {
                            if let Some((local_id, _)) = self.locals.get(var_name) {
                                let local_ty = self.builder.local_type(*local_id);
                                if let Some(MirType::Struct(StructLayoutId { id: sid, .. })) = local_ty {
                                    if let Some(layout) = self.ctx.struct_layouts.get(sid as usize) {
                                        if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                            .find(|(_, f)| f.name == *field)
                                        {
                                            fi = idx as u32;
                                            rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                            bo = Some(fl.offset);
                                            fs = Some(fl.size);
                                            resolved = true;
                                        }
                                    }
                                } else if let Some(MirType::Tuple(_)) = &local_ty {
                                    if let Some(tuple_resolved) = Self::resolve_tuple_field(&local_ty.unwrap(), field) {
                                        fi = tuple_resolved.0;
                                        rt = tuple_resolved.1;
                                        bo = tuple_resolved.2;
                                        fs = tuple_resolved.3;
                                        resolved = true;
                                    }
                                }
                            }
                        }
                    }

                    // Strategy 3: Search all struct layouts for the field name
                    if !resolved {
                        for layout in self.ctx.struct_layouts.iter() {
                            if let Some((idx, fl)) = layout.fields.iter().enumerate()
                                .find(|(_, f)| f.name == *field)
                            {
                                fi = idx as u32;
                                rt = self.ctx.resolve_type_str(&format!("{}", fl.ty));
                                bo = Some(fl.offset);
                                fs = Some(fl.size);
                                resolved = true;
                                break;
                            }
                        }
                    }

                    if !resolved {
                        rt = self.ctx.lookup_node_type(expr.id)
                            .filter(|t| !matches!(t, MirType::Ptr))
                            .unwrap_or_else(|| {
                                eprintln!(
                                    "[mir] unresolved field `{}` — defaulting to I64 (should be caught by type checker)",
                                    field
                                );
                                MirType::I64
                            });
                    }

                    (fi, rt, bo, fs)
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Field {
                        base: obj_op,
                        field_index,
                        byte_offset,
                        field_size,
                    },
                }));
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Index access
            ExprKind::Index { object, index } => {
                // Range index → slice operation: vec[start..end] or string[start..end]
                if let ExprKind::Range { start, end, .. } = &index.kind {
                    let (obj_op, obj_ty) = self.lower_expr(object)?;

                    // Determine if receiver is a string (MIR type, type checker, or local prefix)
                    let is_string = matches!(obj_ty, MirType::String)
                        || self.ctx.lookup_raw_type(object.id)
                            .map(|ty| matches!(ty, rask_types::Type::String))
                            .unwrap_or(false)
                        || if let ExprKind::Ident(var_name) = &object.kind {
                            self.meta(var_name)
                                .and_then(|m| m.type_prefix.as_deref())
                                .map(|p| p == "string")
                                .unwrap_or(false)
                        } else {
                            false
                        };

                    let start_op = if let Some(s) = start {
                        let (op, _) = self.lower_expr(s)?;
                        op
                    } else {
                        MirOperand::Constant(MirConst::Int(0))
                    };

                    if is_string {
                        // String slice: string_substr(s, start, end)
                        let end_op = if let Some(e) = end {
                            let (op, _) = self.lower_expr(e)?;
                            op
                        } else {
                            let len_local = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                                dst: Some(len_local),
                                func: FunctionRef::internal("string_len".to_string()),
                                args: vec![obj_op.clone()],
                            }));
                            MirOperand::Local(len_local)
                        };
                        let result_local = self.builder.alloc_temp(MirType::String);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(result_local),
                            func: FunctionRef::internal("string_substr".to_string()),
                            args: vec![obj_op, start_op, end_op],
                        }));
                        return Ok((MirOperand::Local(result_local), MirType::String));
                    }

                    // Vec slice: Vec_slice(v, start, end)
                    // end is None for open ranges (parts[2..]), use Vec_len
                    let end_op = if let Some(e) = end {
                        let (op, _) = self.lower_expr(e)?;
                        op
                    } else {
                        let len_local = self.builder.alloc_temp(MirType::I64);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(len_local),
                            func: FunctionRef::internal("Vec_len".to_string()),
                            args: vec![obj_op.clone()],
                        }));
                        MirOperand::Local(len_local)
                    };
                    let result_local = self.builder.alloc_temp(MirType::Ptr);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(result_local),
                        func: FunctionRef::internal("Vec_slice".to_string()),
                        args: vec![obj_op, start_op, end_op],
                    }));
                    return Ok((MirOperand::Local(result_local), MirType::Ptr));
                }

                let (obj_op, obj_ty) = self.lower_expr(object)?;
                let (idx_op, _) = self.lower_expr(index)?;

                // Fixed-size arrays: direct memory access (base + index * elem_size)
                if let MirType::Array { ref elem, .. } = obj_ty {
                    let elem_size = elem.size();
                    let result_ty = *elem.clone();
                    let result_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::ArrayIndex {
                            base: obj_op,
                            index: idx_op,
                            elem_size,
                        },
                    }));
                    return Ok((MirOperand::Local(result_local), result_ty));
                }

                // Vec/Map/etc: dispatch through runtime
                // Try to determine the element type from the type checker,
                // then from tracked push/set calls, then default to I64
                let result_ty = self.ctx.lookup_node_type(expr.id)
                    .filter(|t| !matches!(t, MirType::Ptr))
                    .or_else(|| {
                        Self::vec_tracking_key(object).and_then(|key| {
                            self.meta(&key).and_then(|m| m.elem_type.clone())
                                .or_else(|| self.ctx.shared_elem_types.borrow().get(&key).cloned())
                        })
                    })
                    .unwrap_or(MirType::I64);
                let type_prefix = if let ExprKind::Ident(var_name) = &object.kind {
                        self.meta(var_name).and_then(|m| m.type_prefix.clone())
                    } else {
                        None
                    }
                    .or_else(|| {
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| super::MirContext::type_prefix(ty, self.ctx.type_names))
                    });

                // Pool index: emit PoolCheckedAccess for generation checking
                if type_prefix.as_deref() == Some("Pool") {
                    // If result_ty is I64 (default), try to extract the element type
                    // from the pool's generic parameter (Pool<Entity> → Entity)
                    let result_ty = if matches!(result_ty, MirType::I64) {
                        // Extract element type from Pool<T> generic parameter
                        self.ctx.lookup_raw_type(object.id)
                            .and_then(|ty| match ty {
                                rask_types::Type::UnresolvedGeneric { args, .. } => {
                                    args.first().and_then(|a| match a {
                                        rask_types::GenericArg::Type(t) => Some(t.as_ref()),
                                        _ => None,
                                    })
                                }
                                _ => None,
                            })
                            .map(|elem_ty| self.ctx.type_to_mir(elem_ty))
                            .filter(|t| !matches!(t, MirType::Ptr | MirType::I64))
                            .unwrap_or(result_ty)
                    } else {
                        result_ty
                    };
                    let pool_local = match obj_op {
                        MirOperand::Local(id) => id,
                        _ => {
                            let tmp = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: tmp,
                                rvalue: MirRValue::Use(obj_op),
                            }));
                            tmp
                        }
                    };
                    let handle_local = match idx_op {
                        MirOperand::Local(id) => id,
                        _ => {
                            let tmp = self.builder.alloc_temp(MirType::I64);
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: tmp,
                                rvalue: MirRValue::Use(idx_op),
                            }));
                            tmp
                        }
                    };
                    let result_local = self.builder.alloc_temp(result_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
                        dst: result_local,
                        pool: pool_local,
                        handle: handle_local,
                    }));
                    return Ok((MirOperand::Local(result_local), result_ty));
                }

                let index_name = type_prefix
                    .map(|prefix| format!("{}_index", prefix))
                    .unwrap_or_else(|| "index".to_string());
                let result_local = self.builder.alloc_temp(result_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(index_name),
                    args: vec![obj_op, idx_op],
                }));
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
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: i as u32 * elem_size,
                        value: elem_op,
                        store_size: None,
                    }));
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
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset,
                        value: elem_op,
                        store_size: None,
                    }));
                    offset += elem_size;
                }
                Ok((MirOperand::Local(result_local), tuple_ty))
            }

            // Struct literal
            ExprKind::StructLit { name, fields, .. } => {
                // Check for enum variant constructor: "EnumName.VariantName { ... }"
                let (result_ty, layout, enum_variant_info) = if let Some(dot_pos) = name.find('.') {
                    let enum_name = &name[..dot_pos];
                    let variant_name = &name[dot_pos + 1..];
                    if let Some((idx, el)) = self.ctx.find_enum(enum_name) {
                        let variant_info = el.variants.iter().find(|v| v.name == variant_name)
                            .map(|v| (v.tag, v.payload_offset, v.fields.clone()));
                        (MirType::Enum(EnumLayoutId::new(idx, el.size, el.align)), None, variant_info)
                    } else if let Some((idx, sl)) = self.ctx.find_struct(name) {
                        (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                    } else {
                        (MirType::Ptr, None, None)
                    }
                } else if let Some((idx, sl)) = self.ctx.find_struct(name) {
                    (MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)), Some(sl), None)
                } else {
                    (MirType::Ptr, None, None)
                };

                let result_local = self.builder.alloc_temp(result_ty.clone());

                // For enum variants, store the tag first
                if let Some((tag, payload_offset, ref variant_fields)) = enum_variant_info {
                    // Store discriminant tag at offset 0
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset: 0,
                        value: MirOperand::Constant(MirConst::Int(tag as i64)),
                        store_size: None,
                    }));
                    // Store fields at their offsets within the payload
                    for field in fields.iter() {
                        let (val_op, _) = self.lower_expr(&field.value)?;
                        let vf = variant_fields.iter()
                            .find(|f| f.name == field.name);
                        let offset = vf.map(|f| payload_offset + f.offset)
                            .unwrap_or(payload_offset);
                        let store_size = vf.map(|f| f.size);
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset,
                            value: val_op,
                            store_size,
                        }));
                    }
                } else {
                for field in fields.iter() {
                    let (val_op, _) = self.lower_expr(&field.value)?;
                    // Look up field offset and size from layout
                    let field_layout = layout
                        .and_then(|sl| sl.fields.iter().find(|f| f.name == field.name));
                    let offset = field_layout.map(|f| f.offset).unwrap_or(0);
                    let store_size = field_layout.map(|f| f.size);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: result_local,
                        offset,
                        value: val_op,
                        store_size,
                    }));

                    // Propagate Vec element types from source var to struct field.
                    // If v has known elem type F64 and we're constructing State { data: v },
                    // record "self.data" so methods can look it up.
                    if let ExprKind::Ident(src_var) = &field.value.kind {
                        if let Some(elem_ty) = self.meta(src_var).and_then(|m| m.elem_type.clone())
                            .or_else(|| self.ctx.shared_elem_types.borrow().get(src_var).cloned())
                        {
                            let field_key = format!("self.{}", field.name);
                            self.meta_mut(&field_key).elem_type = Some(elem_ty.clone());
                            self.ctx.shared_elem_types.borrow_mut().insert(field_key, elem_ty);
                        }
                    }
                }
                } // end else (non-enum-variant struct literal)
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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));

                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block,
                    else_block,
                }));

                // Then block: bind payload, evaluate body
                self.builder.switch_to_block(then_block);
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload_niche(pattern, val, payload_ty, is_niche);
                let (then_val, then_ty) = self.lower_expr(then_branch)?;
                let result_local = self.builder.alloc_temp(then_ty.clone());
                if self.builder.current_block_unterminated() {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(then_val),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                // Else block: evaluate else branch or default to zero-value
                self.builder.switch_to_block(else_block);
                if let Some(else_expr) = else_branch {
                    let (else_val, _) = self.lower_expr(else_expr)?;
                    if self.builder.current_block_unterminated() {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: result_local,
                            rvalue: MirRValue::Use(else_val),
                        }));
                    }
                } else if self.builder.current_block_unterminated() {
                    // No else branch — initialize to default zero value
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                    }));
                }
                if self.builder.current_block_unterminated() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: matches,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));

                let ok_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let merge_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(matches),
                    then_block: ok_block,
                    else_block,
                }));

                // Else branch diverges (return, panic, etc.)
                self.builder.switch_to_block(else_block);
                self.lower_expr(else_branch)?;
                // Only add unreachable if the else branch didn't already terminate
                // (e.g. via return or break)
                if self.builder.current_block_unterminated() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
                }

                // Ok block: bind payload and continue
                self.builder.switch_to_block(ok_block);
                let payload_ty = self.extract_payload_type(expr)
                    .unwrap_or(MirType::I64);
                self.bind_pattern_payload_niche(pattern, val.clone(), payload_ty.clone(), is_niche);
                // Extract the payload value for the result
                let payload = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::operand::BinOp::Eq,
                        left: MirOperand::Local(tag),
                        right: MirOperand::Constant(MirConst::Int(expected)),
                    },
                }));
                Ok((MirOperand::Local(result), MirType::Bool))
            }

            // Try expression (spec L3)
            ExprKind::Try { expr: inner, ref else_clause } => {
                if let Some(try_else) = else_clause {
                    self.lower_try_else(inner, try_else)
                } else {
                    self.lower_try(inner)
                }
            }

            // Unwrap (postfix !) - panic on None/Err
            ExprKind::Unwrap { expr: inner, message: _ } => {
                let is_niche = self.is_niche_option_expr(inner);
                let (val, _inner_ty) = self.lower_expr(inner)?;
                let tag_local = self.emit_option_tag(&val, is_niche);

                let ok_block = self.builder.create_block();
                let panic_block = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: panic_block,
                    else_block: ok_block,
                }));

                self.builder.switch_to_block(panic_block);

                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("panic_unwrap".to_string()),
                    args: vec![],
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

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

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                }));

                self.builder.switch_to_block(some_block);
                let payload_ty = self.extract_payload_type(value)
                    .unwrap_or(MirType::I64);
                let result_local = self.emit_option_payload(val, payload_ty.clone(), is_niche);
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(none_block);
                let (default_val, _) = self.lower_expr(default)?;
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(default_val),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(func_name.to_string()),
                    args,
                }));
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
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                            addr: result_local,
                            offset: i * elem_size,
                            value: val.clone(),
                            store_size: None,
                        }));
                    }
                    return Ok((MirOperand::Local(result_local), array_ty));
                }

                // Dynamic count: keep existing Ptr-based fallback
                let (val, elem_ty) = self.lower_expr(value)?;
                let (cnt, _) = self.lower_expr(count)?;
                let result_ty = MirType::Ptr;
                let result_local = self.builder.alloc_temp(result_ty.clone());
                let elem_size = self.elem_size_for_type(&elem_ty);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal("array_repeat".to_string()),
                    args: vec![val, cnt, MirOperand::Constant(MirConst::Int(elem_size))],
                }));
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

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(tag_local),
                    then_block: none_block,
                    else_block: some_block,
                }));

                self.builder.switch_to_block(some_block);
                let rvalue = if is_niche {
                    MirRValue::Use(obj)
                } else {
                    MirRValue::Field { base: obj, field_index: 0, byte_offset: None, field_size: None }
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue,
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(none_block);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Closure — synthesize a separate MIR function and emit ClosureCreate
            ExprKind::Closure { params, ret_ty, body } => {
                self.lower_closure(params, ret_ty.as_deref(), body)
            }

            // Cast
            ExprKind::Cast { expr, ty } => {
                // Trait object boxing: `value as any Trait`
                if let Some(trait_name) = ty.strip_prefix("any ") {
                    let (val, concrete_mir_ty) = self.lower_expr(expr)?;
                    return Ok(self.emit_trait_box(val, &concrete_mir_ty, trait_name));
                }

                let (val, _) = self.lower_expr(expr)?;
                let target_ty = self.ctx.resolve_type_str(ty);
                let result_local = self.builder.alloc_temp(target_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Cast {
                        value: val,
                        target_ty: target_ty.clone(),
                    },
                }));
                Ok((MirOperand::Local(result_local), target_ty))
            }

            // Using block — emit runtime init/shutdown for Multitasking/ThreadPool
            ExprKind::UsingBlock { name, args, body } => {
                if name == "Multitasking" || name == "MultiTasking" || name == "multitasking"
                    || name == "ThreadPool" || name == "threadpool"
                {
                    // Extract worker count from args, default to 0 (auto-detect)
                    let worker_count = if let Some(arg) = args.first() {
                        let (op, _ty) = self.lower_expr(&arg.expr)?;
                        op
                    } else {
                        MirOperand::Constant(crate::operand::MirConst::Int(0))
                    };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("rask_runtime_init".to_string()),
                        args: vec![worker_count],
                    }));
                    let result = self.lower_block(body);
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("rask_runtime_shutdown".to_string()),
                        args: vec![],
                    }));
                    result
                } else {
                    self.lower_block(body)
                }
            }

            // With-as binding
            ExprKind::WithAs { bindings, body } => {
                // Detect Shared.read() / Shared.write() pattern:
                //   with shared.read() as d { body }
                // Synthesize a closure from the body and call Shared_read(handle, closure).
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    if let ExprKind::MethodCall { object, method, args: call_args, .. } = &binding.source.kind {
                        let is_shared_access = (method == "read" || method == "write") && call_args.is_empty();
                        if is_shared_access {
                            // Check if the object type is Shared
                            let obj_raw_type = self.ctx.lookup_raw_type(object.id);
                            let is_shared = obj_raw_type.map(|ty| {
                                matches!(ty,
                                    rask_types::Type::UnresolvedGeneric { name, .. }
                                    | rask_types::Type::UnresolvedNamed(name)
                                    if name == "Shared"
                                )
                            }).unwrap_or(false)
                            // Fallback: check local_meta type_prefix
                            || if let ExprKind::Ident(var_name) = &object.kind {
                                self.meta(var_name)
                                    .and_then(|m| m.type_prefix.as_deref())
                                    .map(|p| p == "Shared")
                                    .unwrap_or(false)
                            } else {
                                false
                            };
                            if is_shared {
                                return self.lower_shared_with_block(object, method, &binding.name, body);
                            }
                        }
                    }
                }

                // Detect Mutex pattern: with mutex as v { body }
                // Source is a plain Ident referring to a Mutex variable.
                if bindings.len() == 1 {
                    let binding = &bindings[0];
                    let is_mutex = if let ExprKind::Ident(var_name) = &binding.source.kind {
                        let from_type = self.ctx.lookup_raw_type(binding.source.id)
                            .map(|ty| matches!(ty,
                                rask_types::Type::UnresolvedGeneric { name, .. }
                                | rask_types::Type::UnresolvedNamed(name)
                                if name == "Mutex"
                            ))
                            .unwrap_or(false);
                        let from_prefix = self.meta(var_name)
                            .and_then(|m| m.type_prefix.as_deref())
                            .map(|p| p == "Mutex")
                            .unwrap_or(false);
                        from_type || from_prefix
                    } else {
                        false
                    };
                    if is_mutex {
                        return self.lower_mutex_with_block(&binding.source, &binding.name, body);
                    }
                }

                // Default: simple alias binding (Pool, Cell, etc.)
                // W2a/W2b: Track pool bindings for re-resolution after pool mutators
                let mut pool_binding_keys: Vec<String> = Vec::new();
                for binding in bindings {
                    // Before lowering, extract pool/handle info for re-resolution tracking
                    let pool_info = if let ExprKind::Index { object, index } = &binding.source.kind {
                        if let ExprKind::Ident(coll_name) = &object.kind {
                            let is_pool = self.meta(coll_name)
                                .and_then(|m| m.type_prefix.as_deref())
                                .map(|p| p == "Pool")
                                .unwrap_or(false);
                            if is_pool {
                                let pool_local = self.locals.get(coll_name).map(|(id, _)| *id);
                                let handle_local = if let ExprKind::Ident(h) = &index.kind {
                                    self.locals.get(h).map(|(id, _)| *id)
                                } else {
                                    None
                                };
                                pool_local.zip(handle_local).map(|(p, h)| (coll_name.clone(), p, h))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    let (val, val_ty) = self.lower_expr(&binding.source)?;
                    let local = self.builder.alloc_local(binding.name.clone(), val_ty.clone());
                    self.locals.insert(binding.name.clone(), (local, val_ty));
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: local,
                        rvalue: MirRValue::Use(val),
                    }));

                    // Register pool binding for re-resolution
                    if let Some((pool_name, pool_local, handle_local)) = pool_info {
                        self.with_pool_bindings.entry(pool_name.clone())
                            .or_default()
                            .push((handle_local, local, pool_local));
                        pool_binding_keys.push(pool_name);
                    }
                }
                let result = self.lower_block(body);
                // Clean up pool binding registrations
                for key in &pool_binding_keys {
                    if let Some(entries) = self.with_pool_bindings.get_mut(key) {
                        entries.pop();
                        if entries.is_empty() {
                            self.with_pool_bindings.remove(key);
                        }
                    }
                }
                result
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
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(result_local),
                    func: FunctionRef::internal(name.clone()),
                    args: vec![body_val],
                }));
                Ok((MirOperand::Local(result_local), ret_ty))
            }

            // Unsafe block
            ExprKind::Unsafe { body } => {
                self.lower_block(body)
            }

            // CF25: loop expression — allocate result slot for break-with-value
            ExprKind::Loop { body, label } => {
                let result_local = self.builder.alloc_local(
                    "__loop_result".to_string(),
                    MirType::I64,
                );
                let loop_block = self.builder.create_block();
                let exit_block = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: loop_block,
                }));
                self.builder.switch_to_block(loop_block);

                self.loop_stack.push(LoopContext {
                    label: label.as_ref().map(|s| s.to_string()),
                    continue_block: loop_block,
                    exit_block,
                    result_local: Some(result_local),
                });

                for stmt in body {
                    self.lower_stmt(stmt)?;
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                    target: loop_block,
                }));

                self.loop_stack.pop();
                self.builder.switch_to_block(exit_block);

                Ok((MirOperand::Local(result_local), MirType::I64))
            }

            // Comptime expression — try compile-time evaluation (CC1)
            ExprKind::Comptime { body } => {
                if let Some(ref interp_cell) = self.ctx.comptime_interp {
                    // Try evaluating the entire comptime block
                    let mut interp = interp_cell.borrow_mut();
                    if let Ok(val) = interp.eval_block_to_value(body) {
                        return Ok(match val {
                            rask_comptime::ComptimeValue::Bool(b) => {
                                (MirOperand::Constant(MirConst::Bool(b)), MirType::Bool)
                            }
                            rask_comptime::ComptimeValue::I64(n) => {
                                (MirOperand::Constant(MirConst::Int(n)), MirType::I64)
                            }
                            rask_comptime::ComptimeValue::String(s) => {
                                (MirOperand::Constant(MirConst::String(s)), MirType::String)
                            }
                            _ => {
                                // Complex value — fall through to normal lowering
                                drop(interp);
                                return self.lower_block(body);
                            }
                        });
                    }
                    drop(interp);
                }
                self.lower_block(body)
            }

            // Select (channel multiplexing)
            ExprKind::Select { arms, .. } => {
                let merge_block = self.builder.create_block();
                let arm_blocks: Vec<BlockId> = arms.iter().map(|_| self.builder.create_block()).collect();

                if let Some(&first) = arm_blocks.first() {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: first }));
                } else {
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                let mut result_ty = MirType::Void;
                let result_local = self.builder.alloc_temp(MirType::I32);
                for (i, arm) in arms.iter().enumerate() {
                    self.builder.switch_to_block(arm_blocks[i]);
                    let (arm_val, arm_ty) = self.lower_expr(&arm.body)?;
                    if i == 0 {
                        result_ty = arm_ty;
                    }
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: result_local,
                        rvalue: MirRValue::Use(arm_val),
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));
                }

                self.builder.switch_to_block(merge_block);
                Ok((MirOperand::Local(result_local), result_ty))
            }

            // Assert
            ExprKind::Assert { condition, message } => {
                // Detect comparison patterns for smart failure messages.
                // After desugaring, `a == b` → `a.eq(b)`, `a != b` → `!a.eq(b)`.
                let cmp_info = if message.is_none() {
                    extract_assert_comparison(condition)
                } else {
                    None
                };

                if let Some((left_expr, right_expr, op_str, is_string)) = cmp_info {
                    // Lower both sides first to capture their values
                    let (left_op, _) = self.lower_expr(left_expr)?;
                    let (right_op, _) = self.lower_expr(right_expr)?;

                    // Now lower the full condition
                    let (cond_op, _) = self.lower_expr(condition)?;
                    let ok_block = self.builder.create_block();
                    let fail_block = self.builder.create_block();

                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: cond_op,
                        then_block: ok_block,
                        else_block: fail_block,
                    }));

                    self.builder.switch_to_block(fail_block);
                    let op_const = MirOperand::Constant(MirConst::String(op_str.to_string()));
                    let fail_fn = if is_string { "assert_fail_cmp_str" } else { "assert_fail_cmp_i64" };
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal(fail_fn.to_string()),
                        args: vec![left_op, right_op, op_const],
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
                } else {
                    // Check for `is` pattern: assert x is Some
                    let is_msg = if message.is_none() {
                        extract_assert_is_pattern(condition)
                            .map(|pat| format!("assertion failed: expected {}", pat))
                    } else {
                        None
                    };

                    // Generic path: lower condition, pass optional message
                    let (cond_op, _) = self.lower_expr(condition)?;
                    let ok_block = self.builder.create_block();
                    let fail_block = self.builder.create_block();

                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: cond_op,
                        then_block: ok_block,
                        else_block: fail_block,
                    }));

                    self.builder.switch_to_block(fail_block);
                    let mut args = Vec::new();
                    if let Some(msg) = message {
                        let (msg_op, _) = self.lower_expr(msg)?;
                        args.push(msg_op);
                    } else if let Some(is_msg) = is_msg {
                        args.push(MirOperand::Constant(MirConst::String(is_msg)));
                    }
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("assert_fail".to_string()),
                        args,
                    }));
                    self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));

                    self.builder.switch_to_block(ok_block);
                    Ok((MirOperand::Constant(MirConst::Bool(true)), MirType::Bool))
                }
            }

            // Check (like assert but continues)
            ExprKind::Check { condition, message } => {
                let (cond_op, _) = self.lower_expr(condition)?;
                let ok_block = self.builder.create_block();
                let fail_block = self.builder.create_block();
                let merge_block = self.builder.create_block();
                let result_local = self.builder.alloc_temp(MirType::Bool);

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: cond_op,
                    then_block: ok_block,
                    else_block: fail_block,
                }));

                self.builder.switch_to_block(fail_block);
                let mut args = Vec::new();
                if let Some(msg) = message {
                    let (msg_op, _) = self.lower_expr(msg)?;
                    args.push(msg_op);
                }
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("check_fail".to_string()),
                    args,
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(false))),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

                self.builder.switch_to_block(ok_block);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
                }));
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge_block }));

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

        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: cond_op,
            then_block,
            else_block,
        }));

        // Then branch
        self.builder.switch_to_block(then_block);
        let (then_val, then_ty) = self.lower_expr(then_branch)?;
        let result_local = self.builder.alloc_temp(then_ty.clone());
        // Only add merge-goto if the branch didn't already terminate (e.g. return)
        if self.builder.current_block_unterminated() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result_local,
                rvalue: MirRValue::Use(then_val),
            }));
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        // Else branch
        self.builder.switch_to_block(else_block);
        if let Some(else_expr) = else_branch {
            let (else_val, _) = self.lower_expr(else_expr)?;
            if self.builder.current_block_unterminated() {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: result_local,
                    rvalue: MirRValue::Use(else_val),
                }));
            }
        }
        if self.builder.current_block_unterminated() {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
                target: merge_block,
            }));
        }

        self.builder.switch_to_block(merge_block);

        Ok((MirOperand::Local(result_local), then_ty))
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
} // end impl MirLowerer
